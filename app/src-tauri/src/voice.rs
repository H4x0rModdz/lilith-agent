//! Voz da Lilith (saída): TTS local com o Piper, fala em português.
//!
//! O Piper (executável) e a voz são baixados no primeiro uso para
//! `%APPDATA%\Lilith\voice\` — nunca vão no instalador — e conferidos por
//! SHA-256. A fala é sintetizada num WAV, tocada com o próprio Windows e, em
//! paralelo, o volume da fala (envelope de amplitude) abre e fecha a boca do
//! modelo 3D pelo evento `lilith://mouth`. Mesma ideia do `jarvis-os`
//! (Piper como subprocesso + envelope RMS), adaptada para Windows.

use crate::config::data_dir;
use futures_util::StreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tauri::{AppHandle, Emitter};

const PIPER_URL: &str = "https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_windows_amd64.zip";
const PIPER_SHA256: &str = "f3c58906402b24f3a96d92145f58acba6d86c9b5db896d207f78dc80811efcea";
const VOICE_NAME: &str = "pt_BR-faber-medium";
const VOICE_BASE: &str = "https://huggingface.co/rhasspy/piper-voices/resolve/main/pt/pt_BR/faber/medium/";
const VOICE_ONNX_SHA256: &str = "858555e3a064209c57088fe6bd70c4c3dc54d03eaa00c45d5ecaf43a33f95aa7";
const VOICE_JSON_SHA256: &str = "7e694de195ae3fc36dd732c445eb04fb49b649854893cb5506b978f0d50a1d6f";

/// Tamanho máximo de texto falado de uma vez (o resto é cortado numa frase).
const MAX_SPOKEN_CHARS: usize = 500;
/// Janela do envelope de amplitude (um valor de boca por janela).
const FRAME_MS: u32 = 40;

/// Cada fala nova (ou parada) invalida a anterior.
static GENERATION: AtomicU64 = AtomicU64::new(0);
static INSTALLING: AtomicBool = AtomicBool::new(false);

fn voice_dir() -> PathBuf {
    data_dir().join("voice")
}

fn piper_exe() -> PathBuf {
    voice_dir().join("piper").join("piper.exe")
}

fn model_path() -> PathBuf {
    voice_dir().join("models").join(format!("{VOICE_NAME}.onnx"))
}

pub fn is_installed() -> bool {
    piper_exe().is_file() && model_path().is_file() && model_path().with_extension("onnx.json").is_file()
}

#[derive(Serialize, Clone)]
pub struct InstallProgress {
    pub stage: String,
    /// 0..100 do que está sendo baixado.
    pub percent: u32,
}

fn progress(app: &AppHandle, stage: &str, percent: u32) {
    let _ = app.emit("lilith://voice-install", InstallProgress { stage: stage.into(), percent });
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

/// Baixa `url` para `dest` mostrando o progresso e confere o hash.
async fn download(app: &AppHandle, stage: &str, url: &str, dest: &Path, sha256: &str) -> Result<(), String> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let response = reqwest::get(url).await.map_err(|e| format!("sem rede: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("download falhou ({})", response.status()));
    }
    let total = response.content_length().unwrap_or(0);
    let part = dest.with_extension("part");
    let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut stream = response.bytes_stream();
    let (mut got, mut last_pct) = (0u64, 101u32);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        std::io::Write::write_all(&mut file, &chunk).map_err(|e| e.to_string())?;
        got += chunk.len() as u64;
        let pct = if total > 0 { (got * 100 / total) as u32 } else { 0 };
        if pct != last_pct {
            last_pct = pct;
            progress(app, stage, pct);
        }
    }
    drop(file);
    if sha256_file(&part)? != sha256 {
        let _ = std::fs::remove_file(&part);
        return Err("o arquivo baixado não confere (hash diferente) — descartado".into());
    }
    std::fs::rename(&part, dest).map_err(|e| e.to_string())
}

/// Instala o Piper e a voz (≈85 MB). Idempotente.
pub async fn install(app: AppHandle) -> Result<(), String> {
    if is_installed() {
        return Ok(());
    }
    if INSTALLING.swap(true, Ordering::SeqCst) {
        return Err("a instalação já está em andamento".into());
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
    let dir = voice_dir();
    let tmp = dir.join("dl");
    if !piper_exe().is_file() {
        let zip = tmp.join("piper.zip");
        download(app, "Baixando o Piper", PIPER_URL, &zip, PIPER_SHA256).await?;
        progress(app, "Instalando o Piper", 100);
        let status = tokio::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command"])
            .arg(format!(
                "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
                zip.display(),
                dir.display()
            ))
            .creation_flags_hidden()
            .status()
            .await
            .map_err(|e| e.to_string())?;
        if !status.success() || !piper_exe().is_file() {
            return Err("não consegui extrair o Piper".into());
        }
        let _ = std::fs::remove_file(&zip);
    }
    let onnx = model_path();
    if !onnx.is_file() {
        download(app, "Baixando a voz", &format!("{VOICE_BASE}{VOICE_NAME}.onnx"), &onnx, VOICE_ONNX_SHA256).await?;
    }
    let json = onnx.with_extension("onnx.json");
    if !json.is_file() {
        download(app, "Baixando a voz", &format!("{VOICE_BASE}{VOICE_NAME}.onnx.json"), &json, VOICE_JSON_SHA256).await?;
    }
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}

/// Tira o que não deve ser lido em voz alta: marcação, emojis, links, blocos
/// de código e a linha "[ações nesse pedido…]". Corta numa frase.
pub fn clean_for_speech(text: &str) -> String {
    let mut out = String::new();
    let mut in_code = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code || l.is_empty() || l.starts_with("[ações") {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(l);
    }
    let words: Vec<String> = out
        .split_whitespace()
        .map(|w| if w.starts_with("http://") || w.starts_with("https://") { "link".to_string() } else { w.to_string() })
        .collect();
    let joined = words.join(" ");
    let cleaned: String = joined
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || ".,;:!?-–—'\"()%/".contains(*c) || "áéíóúâêôãõçàÁÉÍÓÚÂÊÔÃÕÇÀ".contains(*c))
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.chars().count() <= MAX_SPOKEN_CHARS {
        return cleaned;
    }
    let cut: String = cleaned.chars().take(MAX_SPOKEN_CHARS).collect();
    match cut.rfind(['.', '!', '?']) {
        Some(i) if i > 80 => cut[..=i].to_string(),
        _ => cut,
    }
}

/// Janelas de volume (0..1, a mais alta = 1) de um WAV PCM de 16 bits.
fn envelope(samples: &[i16], rate: u32) -> Vec<f32> {
    let frame = ((rate as u64 * FRAME_MS as u64) / 1000).max(1) as usize;
    let mut rms: Vec<f32> = samples
        .chunks(frame)
        .map(|c| {
            let sum: f64 = c.iter().map(|&s| (s as f64) * (s as f64)).sum();
            (sum / c.len() as f64).sqrt() as f32
        })
        .collect();
    let max = rms.iter().cloned().fold(0.0f32, f32::max);
    if max > 0.0 {
        for v in &mut rms {
            *v = (*v / max).clamp(0.0, 1.0);
        }
    }
    rms
}

fn read_wav(path: &Path) -> Result<(Vec<i16>, u32), String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let samples: Vec<i16> = reader.samples::<i16>().filter_map(|s| s.ok()).collect();
    Ok((samples, spec.sample_rate))
}

async fn synthesize(text: &str) -> Result<PathBuf, String> {
    use tokio::io::AsyncWriteExt;
    let wav = std::env::temp_dir().join(format!(
        "lilith-voice-{}-{}.wav",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
    ));
    let mut child = tokio::process::Command::new(piper_exe())
        .arg("--model")
        .arg(model_path())
        .arg("--output_file")
        .arg(&wav)
        .arg("--quiet")
        .current_dir(voice_dir().join("piper"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags_hidden()
        .spawn()
        .map_err(|e| format!("não consegui iniciar o Piper: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes()).await.map_err(|e| e.to_string())?;
    }
    let status = child.wait().await.map_err(|e| e.to_string())?;
    if !status.success() || !wav.is_file() {
        return Err("o Piper não gerou áudio".into());
    }
    Ok(wav)
}

#[cfg(windows)]
fn play_async(wav: &Path) {
    use windows::core::HSTRING;
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_FILENAME};
    let path = HSTRING::from(wav.as_os_str());
    unsafe {
        let _ = PlaySoundW(&path, None, SND_FILENAME | SND_ASYNC);
    }
}

#[cfg(windows)]
fn stop_playback() {
    use windows::core::PCWSTR;
    use windows::Win32::Media::Audio::{PlaySoundW, SND_PURGE};
    unsafe {
        let _ = PlaySoundW(PCWSTR::null(), None, SND_PURGE);
    }
}

/// Interrompe a fala em curso (e fecha a boca).
pub fn stop(app: &AppHandle) {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    stop_playback();
    let _ = app.emit("lilith://mouth", 0.0f32);
}

/// Fala `text` (limpo) se a voz estiver instalada. Uma fala nova interrompe a
/// anterior. Erros só vão para o log.
pub fn speak(app: &AppHandle, text: &str) {
    let spoken = clean_for_speech(text);
    if spoken.is_empty() || !is_installed() {
        return;
    }
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    stop_playback();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let wav = match synthesize(&spoken).await {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!("voz: {e}");
                return;
            }
        };
        if GENERATION.load(Ordering::SeqCst) != generation {
            let _ = std::fs::remove_file(&wav);
            return;
        }
        let (samples, rate) = match read_wav(&wav) {
            Ok(x) => x,
            Err(e) => {
                tracing::warn!("voz: {e}");
                let _ = std::fs::remove_file(&wav);
                return;
            }
        };
        let env = envelope(&samples, rate);
        let duration = std::time::Duration::from_millis(samples.len() as u64 * 1000 / rate.max(1) as u64);
        play_async(&wav);
        let _ = app.emit("lilith://speaking", true);
        let started = std::time::Instant::now();
        let tick = std::time::Duration::from_millis(FRAME_MS as u64);
        while started.elapsed() < duration {
            if GENERATION.load(Ordering::SeqCst) != generation {
                break;
            }
            let i = (started.elapsed().as_millis() / FRAME_MS as u128) as usize;
            // Suaviza: mistura a janela atual com a seguinte.
            let level = env.get(i).copied().unwrap_or(0.0) * 0.7 + env.get(i + 1).copied().unwrap_or(0.0) * 0.3;
            let _ = app.emit("lilith://mouth", level);
            tokio::time::sleep(tick).await;
        }
        if GENERATION.load(Ordering::SeqCst) == generation {
            // Deixa o áudio terminar de tocar antes de apagar o arquivo.
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        let _ = app.emit("lilith://mouth", 0.0f32);
        let _ = app.emit("lilith://speaking", false);
        let _ = std::fs::remove_file(&wav);
    });
}

/// Estado para a tela de Configurações.
#[derive(Serialize)]
pub struct Status {
    pub installed: bool,
    pub installing: bool,
}

pub fn status() -> Status {
    Status { installed: is_installed(), installing: INSTALLING.load(Ordering::SeqCst) }
}

/// Sem janela de console piscando ao iniciar subprocessos.
trait HiddenWindow {
    fn creation_flags_hidden(&mut self) -> &mut Self;
}

impl HiddenWindow for tokio::process::Command {
    fn creation_flags_hidden(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            self.creation_flags(CREATE_NO_WINDOW);
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speech_text_is_cleaned() {
        let raw = "**Pronto!** Abri o 😊 spotify.\n[ações nesse pedido: app.open {}]\n```\ncódigo\n```\nVeja https://example.com/x agora.";
        assert_eq!(clean_for_speech(raw), "Pronto! Abri o spotify. Veja link agora.");
        assert_eq!(clean_for_speech("   "), "");
        let long = format!("{} Fim.", "Frase curta. ".repeat(80));
        let cut = clean_for_speech(&long);
        assert!(cut.chars().count() <= MAX_SPOKEN_CHARS && cut.ends_with('.'));
    }

    #[test]
    fn envelope_is_normalized() {
        let rate = 1000u32; // 40 amostras por janela de 40 ms
        let mut samples = vec![0i16; 40];
        samples.extend(std::iter::repeat_n(8000i16, 40));
        let env = envelope(&samples, rate);
        assert_eq!(env.len(), 2);
        assert!(env[0] < 0.01 && (env[1] - 1.0).abs() < 1e-3);
        assert!(envelope(&[0; 80], rate).iter().all(|v| *v == 0.0));
    }
}
