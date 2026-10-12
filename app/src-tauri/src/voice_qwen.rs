//! Voz "Serena" (Qwen3-TTS CustomVoice 1.7B) rodando na GPU NVIDIA.
//!
//! O Qwen é Python/PyTorch, então roda como processo auxiliar (`qwen_server.py`,
//! embutido no app) falando por linhas JSON. O app instala tudo sozinho, sob
//! demanda, em `%APPDATA%\Lilith\voice\qwen\` (Python gerenciado pelo `uv`,
//! PyTorch com CUDA e o modelo), nunca no instalador. O modelo ocupa ≈4 GB de
//! VRAM enquanto carregado e é descarregado depois de um tempo sem falar.

use crate::config::data_dir;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

const SERVER_PY: &str = include_str!("../voice/qwen_server.py");
const UV_URL: &str = "https://github.com/astral-sh/uv/releases/download/0.13.0/uv-x86_64-pc-windows-msvc.zip";
const UV_SHA256: &str = "088962f9e7b7bd9ea740c04c650b2a21c8928c345bd99ac24350dc924dba656c";
const MODEL_ID: &str = "Qwen/Qwen3-TTS-12Hz-1.7B-CustomVoice";
const SPEAKER: &str = "Serena";
const INSTRUCT: &str = "Speak in a cute, sweet and gentle young female voice, warm and friendly, with a soft smile.";
const PROTO: &str = "@@LILITH@@ ";
/// Descarrega o modelo (e devolve os ≈4 GB de VRAM) depois deste tempo parado.
const IDLE_UNLOAD_SECS: u64 = 20 * 60;
/// Tempo máximo carregando o modelo (a primeira vez inclui o download).
const LOAD_TIMEOUT_SECS: u64 = 900;
const REQUEST_TIMEOUT_SECS: u64 = 180;

static LOADING: AtomicBool = AtomicBool::new(false);
static INSTALLING: AtomicBool = AtomicBool::new(false);
static LAST_USED: AtomicU64 = AtomicU64::new(0);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static PROC: Mutex<Option<Proc>> = Mutex::const_new(None);

struct Proc {
    child: Child,
    stdin: ChildStdin,
    lines: tokio::io::Lines<BufReader<ChildStdout>>,
}

fn base_dir() -> PathBuf {
    data_dir().join("voice").join("qwen")
}

fn managed_python() -> PathBuf {
    base_dir().join("venv").join("Scripts").join("python.exe")
}

/// Ambiente de desenvolvimento (testes de voz) como alternativa à instalação do app.
fn dev_python() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(|p| PathBuf::from(p).join("Lilith").join("qwen-spike").join("venv").join("Scripts").join("python.exe"))
        .unwrap_or_default()
}

/// (python, HF_HOME opcional). O gerenciado pelo app tem prioridade.
fn runtime() -> Option<(PathBuf, Option<PathBuf>)> {
    if managed_python().is_file() {
        return Some((managed_python(), Some(base_dir().join("hf"))));
    }
    if dev_python().is_file() {
        return Some((dev_python(), None));
    }
    None
}

fn model_cached() -> bool {
    let hf_hub = |root: PathBuf| root.join("hub").join("models--Qwen--Qwen3-TTS-12Hz-1.7B-CustomVoice").join("snapshots").is_dir();
    if base_dir().join("hf").join("hub").join("models--Qwen--Qwen3-TTS-12Hz-1.7B-CustomVoice").join("snapshots").is_dir() {
        return true;
    }
    let default = std::env::var_os("HF_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join(".cache").join("huggingface")));
    default.is_some_and(hf_hub)
}

pub fn is_installed() -> bool {
    runtime().is_some() && model_cached()
}

pub fn is_loaded() -> bool {
    PROC.try_lock().map(|g| g.is_some()).unwrap_or(true)
}

pub fn is_loading() -> bool {
    LOADING.load(Ordering::SeqCst)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[derive(Serialize, Clone)]
pub struct EngineState {
    pub state: String,
    pub detail: String,
}

fn emit_state(app: &AppHandle, state: &str, detail: &str) {
    let _ = app.emit("lilith://voice-engine", EngineState { state: state.into(), detail: detail.into() });
}

fn hidden(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // sem janela de console
    cmd
}

/// Carrega o modelo na GPU (uma vez). Seguro de chamar várias vezes.
pub async fn warm(app: AppHandle) {
    if !is_installed() || is_loaded() || LOADING.swap(true, Ordering::SeqCst) {
        return;
    }
    emit_state(&app, "loading", "Carregando a voz na GPU…");
    let result = load().await;
    LOADING.store(false, Ordering::SeqCst);
    match result {
        Ok(detail) => {
            LAST_USED.store(now_secs(), Ordering::Relaxed);
            emit_state(&app, "ready", &detail);
            schedule_unload(app);
        }
        Err(e) => {
            tracing::warn!("voz Serena: {e}");
            emit_state(&app, "error", &e);
        }
    }
}

async fn load() -> Result<String, String> {
    let (python, hf_home) = runtime().ok_or("a voz Serena não está instalada")?;
    let dir = base_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let script = dir.join("qwen_server.py");
    // Sempre a versão embutida neste app.
    std::fs::write(&script, SERVER_PY).map_err(|e| e.to_string())?;

    let mut cmd = Command::new(python);
    cmd.arg("-u")
        .arg(&script)
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1")
        .env("HF_HUB_DISABLE_SYMLINKS_WARNING", "1")
        .env("LILITH_QWEN_MODEL", MODEL_ID)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(hf) = hf_home {
        cmd.env("HF_HOME", hf);
    }
    hidden(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("não consegui iniciar o Python: {e}"))?;
    let stdin = child.stdin.take().ok_or("sem stdin")?;
    let mut lines = BufReader::new(child.stdout.take().ok_or("sem stdout")?).lines();

    let wait = async {
        while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
            let Some(json) = line.strip_prefix(PROTO) else { continue };
            let v: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
            if let Some(err) = v["error"].as_str() {
                return Err(err.to_string());
            }
            if v["ready"] == true {
                return Ok(format!(
                    "Voz pronta ({} · {} GB de VRAM)",
                    v["gpu"].as_str().unwrap_or("GPU"),
                    v["vram_gb"].as_f64().unwrap_or(0.0)
                ));
            }
        }
        Err("o processo de voz terminou sem ficar pronto".to_string())
    };
    let detail = tokio::time::timeout(std::time::Duration::from_secs(LOAD_TIMEOUT_SECS), wait)
        .await
        .map_err(|_| "o modelo demorou demais para carregar".to_string())??;
    *PROC.lock().await = Some(Proc { child, stdin, lines });
    Ok(detail)
}

/// Descarrega o modelo se ninguém falou há um tempo.
fn schedule_unload(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            if !is_loaded() {
                return;
            }
            if now_secs().saturating_sub(LAST_USED.load(Ordering::Relaxed)) >= IDLE_UNLOAD_SECS {
                unload().await;
                emit_state(&app, "idle", "Voz descarregada da GPU (parada há um tempo)");
                return;
            }
        }
    });
}

pub async fn unload() {
    if let Some(mut p) = PROC.lock().await.take() {
        let _ = p.stdin.write_all(b"{\"quit\":true}\n").await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), p.child.wait()).await;
        let _ = p.child.kill().await;
    }
}

/// Sintetiza `text` com a Serena. `Err` se o modelo não está carregado.
pub async fn synthesize(text: &str) -> Result<(Vec<f32>, u32), String> {
    let mut guard = PROC.lock().await;
    let proc = guard.as_mut().ok_or("a voz ainda não carregou")?;
    LAST_USED.store(now_secs(), Ordering::Relaxed);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let req = serde_json::json!({ "id": id, "text": text, "speaker": SPEAKER, "instruct": INSTRUCT, "language": "Portuguese" });
    proc.stdin.write_all(format!("{req}\n").as_bytes()).await.map_err(|e| e.to_string())?;
    proc.stdin.flush().await.map_err(|e| e.to_string())?;

    let wait = async {
        while let Some(line) = proc.lines.next_line().await.map_err(|e| e.to_string())? {
            let Some(json) = line.strip_prefix(PROTO) else { continue };
            let v: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
            if v["id"].as_u64() != Some(id) {
                continue;
            }
            if let Some(err) = v["error"].as_str() {
                return Err(err.to_string());
            }
            return v["path"].as_str().map(PathBuf::from).ok_or_else(|| "resposta sem arquivo".to_string());
        }
        Err("o processo de voz terminou".to_string())
    };
    let result = tokio::time::timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS), wait).await;
    let path = match result {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            // O processo morreu ou travou: descarta para recarregar na próxima.
            *guard = None;
            return Err(e);
        }
        Err(_) => {
            *guard = None;
            return Err("a voz demorou demais".into());
        }
    };
    let read = read_wav(&path);
    let _ = std::fs::remove_file(&path);
    read
}

fn read_wav(path: &Path) -> Result<(Vec<f32>, u32), String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().filter_map(|s| s.ok()).collect(),
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().filter_map(|s| s.ok()).map(|s| s as f32 / max).collect()
        }
    };
    Ok((samples, spec.sample_rate))
}

// ── instalação (uv + PyTorch/CUDA + qwen-tts + modelo) ───────────────────

#[derive(Serialize, Clone)]
pub struct InstallProgress {
    pub stage: String,
    pub percent: u32,
}

fn progress(app: &AppHandle, stage: &str, percent: u32) {
    let _ = app.emit("lilith://voice-install", InstallProgress { stage: stage.into(), percent });
}

pub fn installing() -> bool {
    INSTALLING.load(Ordering::SeqCst)
}

/// Placa NVIDIA presente? (`nvidia-smi` responde.)
pub async fn has_nvidia_gpu() -> bool {
    let mut cmd = Command::new("nvidia-smi");
    cmd.arg("-L").stdout(Stdio::null()).stderr(Stdio::null());
    hidden(&mut cmd);
    cmd.status().await.map(|s| s.success()).unwrap_or(false)
}

fn sha256_file(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

async fn run(app: &AppHandle, stage: &str, cmd: &mut Command) -> Result<(), String> {
    progress(app, stage, 0);
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    hidden(cmd);
    let output = cmd.output().await.map_err(|e| format!("{stage}: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&output.stderr);
    let tail: String = err.lines().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" | ");
    Err(format!("{stage} falhou: {tail}"))
}

/// Instala o ambiente da voz Serena (≈8 GB de download). Idempotente.
pub async fn install(app: AppHandle) -> Result<(), String> {
    if is_installed() {
        return Ok(());
    }
    if INSTALLING.swap(true, Ordering::SeqCst) {
        return Err("a instalação já está em andamento".into());
    }
    if !has_nvidia_gpu().await {
        INSTALLING.store(false, Ordering::SeqCst);
        let msg = "esta voz precisa de uma placa de vídeo NVIDIA".to_string();
        progress(&app, &format!("erro: {msg}"), 0);
        return Err(msg);
    }
    let result = install_inner(&app).await;
    INSTALLING.store(false, Ordering::SeqCst);
    match &result {
        Ok(()) => progress(&app, "pronto", 100),
        Err(e) => progress(&app, &format!("erro: {e}"), 0),
    }
    result
}

async fn install_inner(app: &AppHandle) -> Result<(), String> {
    use futures_util::StreamExt;
    let dir = base_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let uv = dir.join("uv").join("uv.exe");
    if !uv.is_file() {
        let zip = dir.join("uv.zip");
        let resp = reqwest::get(UV_URL).await.map_err(|e| format!("sem rede: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("download do uv falhou ({})", resp.status()));
        }
        let total = resp.content_length().unwrap_or(0);
        let mut file = std::fs::File::create(&zip).map_err(|e| e.to_string())?;
        let mut stream = resp.bytes_stream();
        let (mut got, mut last) = (0u64, 101u32);
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| e.to_string())?;
            std::io::Write::write_all(&mut file, &chunk).map_err(|e| e.to_string())?;
            got += chunk.len() as u64;
            let pct = if total > 0 { (got * 100 / total) as u32 } else { 0 };
            if pct != last {
                last = pct;
                progress(app, "Baixando o instalador do Python (uv)", pct);
            }
        }
        drop(file);
        if sha256_file(&zip)? != UV_SHA256 {
            let _ = std::fs::remove_file(&zip);
            return Err("o uv baixado não confere (hash diferente) — descartado".into());
        }
        let tar = std::env::var_os("SystemRoot")
            .map(|r| PathBuf::from(r).join("System32").join("tar.exe"))
            .filter(|p| p.is_file())
            .unwrap_or_else(|| PathBuf::from("tar"));
        std::fs::create_dir_all(dir.join("uv")).map_err(|e| e.to_string())?;
        // O tar do Windows (bsdtar) também abre .zip; o uv.exe vem dentro de uma pasta.
        run(app, "Extraindo o uv", Command::new(tar).arg("-xf").arg(&zip).arg("-C").arg(dir.join("uv")).arg("--strip-components=1")).await?;
        let _ = std::fs::remove_file(&zip);
        if !uv.is_file() {
            return Err("não achei o uv.exe depois de extrair".into());
        }
    }
    let uv_cmd = || {
        let mut c = Command::new(&uv);
        c.env("UV_PYTHON_INSTALL_DIR", dir.join("python")).env("UV_CACHE_DIR", dir.join("cache")).env("UV_NO_PROGRESS", "1");
        c
    };
    let python = managed_python();
    if !python.is_file() {
        run(app, "Preparando o Python 3.12", uv_cmd().args(["venv", "--python", "3.12"]).arg(dir.join("venv"))).await?;
    }
    run(
        app,
        "Baixando o PyTorch com CUDA (≈3 GB)",
        uv_cmd().args(["pip", "install", "--python"]).arg(&python).args(["torch", "--index-url", "https://download.pytorch.org/whl/cu128"]),
    )
    .await?;
    run(
        app,
        "Instalando o Qwen-TTS",
        uv_cmd().args(["pip", "install", "--python"]).arg(&python).args(["qwen-tts", "soundfile", "huggingface_hub"]),
    )
    .await?;
    let mut fetch = Command::new(&python);
    fetch
        .args(["-c", &format!("from huggingface_hub import snapshot_download; snapshot_download('{MODEL_ID}')")])
        .env("HF_HOME", dir.join("hf"))
        .env("HF_HUB_DISABLE_SYMLINKS_WARNING", "1")
        .env("PYTHONUTF8", "1");
    run(app, "Baixando a voz (≈4 GB)", &mut fetch).await?;
    if is_installed() {
        Ok(())
    } else {
        Err("a instalação terminou, mas faltam arquivos".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_script_is_embedded_and_speaks_the_protocol() {
        assert!(SERVER_PY.contains("@@LILITH@@") && SERVER_PY.contains("generate_custom_voice"));
        assert_eq!(PROTO, "@@LILITH@@ ");
    }

    /// Precisa da voz instalada e de GPU NVIDIA:
    /// `cargo test -- --ignored qwen_speaks_serena`.
    #[tokio::test]
    #[ignore]
    async fn qwen_speaks_serena() {
        assert!(is_installed(), "instale a voz Serena antes");
        // warm precisa de AppHandle; aqui carrega direto.
        let detail = load().await.expect("carregou");
        assert!(detail.contains("pronta"));
        let (samples, rate) = synthesize("Oi! Tudo bem com você?").await.expect("sintetizou");
        assert!(rate >= 16000 && samples.len() > rate as usize / 2, "áudio curto demais");
        assert!(samples.iter().any(|s| s.abs() > 0.02), "áudio não pode ser silêncio");
        unload().await;
    }
}
