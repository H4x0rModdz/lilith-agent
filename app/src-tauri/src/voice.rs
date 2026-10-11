//! Voz da Lilith (saída): TTS local, em português do Brasil, voz feminina.
//!
//! Motor: Kokoro (82M, Apache-2.0) rodando no sherpa-onnx, voz `pf_dora`. O
//! motor e o modelo (≈150 MB) são baixados no primeiro uso para
//! `%APPDATA%\Lilith\voice\` — nunca vão no instalador — e conferidos por
//! SHA-256. A biblioteca é carregada no processo (texto em UTF-8, modelo
//! mantido na memória enquanto se usa e liberado depois de um tempo parado).
//!
//! A fala é gerada por frases: a primeira já toca enquanto a próxima é
//! sintetizada. O volume da fala (envelope de amplitude) abre e fecha a boca do
//! modelo 3D pelo evento `lilith://mouth` (ideia reaproveitada do `jarvis-os`).

use crate::config::data_dir;
use futures_util::StreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::ffi::{c_char, c_void, CString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

const SHERPA_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.8/sherpa-onnx-v1.13.8-win-x64-shared-MD-MinSizeRel.tar.bz2";
const SHERPA_SHA256: &str = "416011eabb9a1e26fd4433b41871d67c7a00b1f0a11b75fe66ff8182b8224f95";
const MODEL_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-int8-multi-lang-v1_0.tar.bz2";
const MODEL_SHA256: &str = "4c3052abaa60943a341f193888cf6abd68787dae6ab8ae5c925a706caa247e4e";
/// Voz feminina em português do Brasil do Kokoro v1.0 (`pf_dora`).
const SPEAKER_ID: i32 = 42;
/// Velocidade da fala (1.0 = normal).
const SPEED: f32 = 1.05;

/// Tamanho máximo de texto falado de uma vez (o resto é cortado numa frase).
const MAX_SPOKEN_CHARS: usize = 500;
/// Tamanho máximo de cada pedaço sintetizado (a primeira frase toca logo).
const CHUNK_CHARS: usize = 200;
/// Janela do envelope de amplitude (um valor de boca por janela).
const FRAME_MS: u32 = 40;
/// Depois deste tempo sem falar, o modelo sai da memória.
const IDLE_UNLOAD_SECS: u64 = 300;

/// Cada fala nova (ou parada) invalida a anterior.
static GENERATION: AtomicU64 = AtomicU64::new(0);
static INSTALLING: AtomicBool = AtomicBool::new(false);
static LAST_USED: AtomicU64 = AtomicU64::new(0);
static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);

fn voice_dir() -> PathBuf {
    data_dir().join("voice").join("kokoro")
}

fn lib_dir() -> PathBuf {
    voice_dir().join("sherpa").join("lib")
}

fn model_dir() -> PathBuf {
    voice_dir().join("model")
}

pub fn is_installed() -> bool {
    let m = model_dir();
    lib_dir().join("sherpa-onnx-c-api.dll").is_file()
        && lib_dir().join("onnxruntime.dll").is_file()
        && m.join("model.int8.onnx").is_file()
        && m.join("voices.bin").is_file()
        && m.join("tokens.txt").is_file()
        && m.join("lexicon-us-en.txt").is_file()
        && m.join("espeak-ng-data").is_dir()
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

/// Extrai um `.tar.bz2` (tar do Windows) tirando a pasta de topo.
async fn extract(archive: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let tar = std::env::var_os("SystemRoot")
        .map(|r| PathBuf::from(r).join("System32").join("tar.exe"))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("tar"));
    let mut cmd = tokio::process::Command::new(tar);
    cmd.arg("-xjf").arg(archive).arg("-C").arg(dest).arg("--strip-components=1");
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // sem janela de console
    let status = cmd.status().await.map_err(|e| format!("não consegui extrair: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("falha ao extrair o arquivo baixado".into())
    }
}

/// Instala o motor e a voz (≈150 MB). Idempotente.
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
    let tmp = voice_dir().join("dl");
    if !lib_dir().join("sherpa-onnx-c-api.dll").is_file() {
        let archive = tmp.join("sherpa.tar.bz2");
        download(app, "Baixando o motor de voz", SHERPA_URL, &archive, SHERPA_SHA256).await?;
        progress(app, "Instalando o motor de voz", 100);
        extract(&archive, &voice_dir().join("sherpa")).await?;
    }
    if !model_dir().join("model.int8.onnx").is_file() {
        let archive = tmp.join("model.tar.bz2");
        download(app, "Baixando a voz", MODEL_URL, &archive, MODEL_SHA256).await?;
        progress(app, "Instalando a voz", 100);
        extract(&archive, &model_dir()).await?;
    }
    let _ = std::fs::remove_dir_all(&tmp);
    if is_installed() {
        Ok(())
    } else {
        Err("a instalação terminou, mas faltam arquivos".into())
    }
}

// ── motor (sherpa-onnx C API) ────────────────────────────────────────────

#[repr(C)]
struct VitsCfg {
    model: *const c_char,
    lexicon: *const c_char,
    tokens: *const c_char,
    data_dir: *const c_char,
    noise_scale: f32,
    noise_scale_w: f32,
    length_scale: f32,
    dict_dir: *const c_char,
}
#[repr(C)]
struct MatchaCfg {
    acoustic_model: *const c_char,
    vocoder: *const c_char,
    lexicon: *const c_char,
    tokens: *const c_char,
    data_dir: *const c_char,
    noise_scale: f32,
    length_scale: f32,
    dict_dir: *const c_char,
}
#[repr(C)]
struct KokoroCfg {
    model: *const c_char,
    voices: *const c_char,
    tokens: *const c_char,
    data_dir: *const c_char,
    length_scale: f32,
    dict_dir: *const c_char,
    lexicon: *const c_char,
    lang: *const c_char,
}
#[repr(C)]
struct KittenCfg {
    model: *const c_char,
    voices: *const c_char,
    tokens: *const c_char,
    data_dir: *const c_char,
    length_scale: f32,
}
#[repr(C)]
struct ZipvoiceCfg {
    tokens: *const c_char,
    encoder: *const c_char,
    decoder: *const c_char,
    vocoder: *const c_char,
    data_dir: *const c_char,
    lexicon: *const c_char,
    feat_scale: f32,
    t_shift: f32,
    target_rms: f32,
    guidance_scale: f32,
}
#[repr(C)]
struct PocketCfg {
    lm_flow: *const c_char,
    lm_main: *const c_char,
    encoder: *const c_char,
    decoder: *const c_char,
    text_conditioner: *const c_char,
    vocab_json: *const c_char,
    token_scores_json: *const c_char,
    voice_embedding_cache_capacity: i32,
}
#[repr(C)]
struct SupertonicCfg {
    duration_predictor: *const c_char,
    text_encoder: *const c_char,
    vector_estimator: *const c_char,
    vocoder: *const c_char,
    tts_json: *const c_char,
    unicode_indexer: *const c_char,
    voice_style: *const c_char,
}
#[repr(C)]
struct ModelCfg {
    vits: VitsCfg,
    num_threads: i32,
    debug: i32,
    provider: *const c_char,
    matcha: MatchaCfg,
    kokoro: KokoroCfg,
    kitten: KittenCfg,
    zipvoice: ZipvoiceCfg,
    pocket: PocketCfg,
    supertonic: SupertonicCfg,
}
#[repr(C)]
struct TtsCfg {
    model: ModelCfg,
    rule_fsts: *const c_char,
    max_num_sentences: i32,
    rule_fars: *const c_char,
    silence_scale: f32,
}
#[repr(C)]
struct GeneratedAudio {
    samples: *const f32,
    n: i32,
    sample_rate: i32,
}

type CreateFn = unsafe extern "C" fn(*const TtsCfg) -> *const c_void;
type DestroyFn = unsafe extern "C" fn(*const c_void);
type GenerateFn = unsafe extern "C" fn(*const c_void, *const c_char, i32, f32) -> *const GeneratedAudio;
type DestroyAudioFn = unsafe extern "C" fn(*const GeneratedAudio);

struct Engine {
    tts: *const c_void,
    generate: GenerateFn,
    destroy_audio: DestroyAudioFn,
    destroy: DestroyFn,
    /// Mantém a DLL carregada enquanto o motor existir.
    _lib: libloading::Library,
}

// O ponteiro do motor só é usado sob o `Mutex` global.
unsafe impl Send for Engine {}

impl Drop for Engine {
    fn drop(&mut self) {
        unsafe { (self.destroy)(self.tts) };
    }
}

fn cstr(path: &Path) -> Result<CString, String> {
    CString::new(path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())
}

fn load_engine() -> Result<Engine, String> {
    let dll = lib_dir().join("sherpa-onnx-c-api.dll");
    // As dependências (onnxruntime.dll) são procuradas na mesma pasta.
    #[cfg(windows)]
    let lib = unsafe {
        use libloading::os::windows::{Library as WinLib, LOAD_WITH_ALTERED_SEARCH_PATH};
        WinLib::load_with_flags(&dll, LOAD_WITH_ALTERED_SEARCH_PATH).map(libloading::Library::from)
    }
    .map_err(|e| format!("não consegui carregar o motor de voz: {e}"))?;
    #[cfg(not(windows))]
    let lib = unsafe { libloading::Library::new(&dll) }.map_err(|e| e.to_string())?;

    let m = model_dir();
    let model = cstr(&m.join("model.int8.onnx"))?;
    let voices = cstr(&m.join("voices.bin"))?;
    let tokens = cstr(&m.join("tokens.txt"))?;
    let data = cstr(&m.join("espeak-ng-data"))?;
    let lexicon = CString::new(format!(
        "{},{}",
        m.join("lexicon-us-en.txt").display(),
        m.join("lexicon-zh.txt").display()
    ))
    .map_err(|e| e.to_string())?;
    let lang = CString::new("pt-br").unwrap();
    let provider = CString::new("cpu").unwrap();

    unsafe {
        let create: CreateFn = *lib.get::<CreateFn>(b"SherpaOnnxCreateOfflineTts\0").map_err(|e| e.to_string())?;
        let destroy: DestroyFn = *lib.get::<DestroyFn>(b"SherpaOnnxDestroyOfflineTts\0").map_err(|e| e.to_string())?;
        let generate: GenerateFn =
            *lib.get::<GenerateFn>(b"SherpaOnnxOfflineTtsGenerate\0").map_err(|e| e.to_string())?;
        let destroy_audio: DestroyAudioFn = *lib
            .get::<DestroyAudioFn>(b"SherpaOnnxDestroyOfflineTtsGeneratedAudio\0")
            .map_err(|e| e.to_string())?;

        // Tudo zerado (ponteiros nulos = vazio); só o Kokoro é preenchido.
        let mut cfg: TtsCfg = std::mem::zeroed();
        cfg.model.num_threads = 3;
        cfg.model.provider = provider.as_ptr();
        cfg.model.kokoro.model = model.as_ptr();
        cfg.model.kokoro.voices = voices.as_ptr();
        cfg.model.kokoro.tokens = tokens.as_ptr();
        cfg.model.kokoro.data_dir = data.as_ptr();
        cfg.model.kokoro.lexicon = lexicon.as_ptr();
        cfg.model.kokoro.lang = lang.as_ptr();
        cfg.model.kokoro.length_scale = 1.0;
        cfg.max_num_sentences = 1;
        cfg.silence_scale = 0.2;
        let tts = create(&cfg);
        if tts.is_null() {
            return Err("o motor de voz não iniciou (modelo inválido?)".into());
        }
        Ok(Engine { tts, generate, destroy_audio, destroy, _lib: lib })
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Sintetiza `text` (UTF-8) em amostras mono [-1, 1] e a taxa de amostragem.
fn synthesize(text: &str) -> Result<(Vec<f32>, u32), String> {
    let text = CString::new(text.replace('\0', " ")).map_err(|e| e.to_string())?;
    let mut guard = ENGINE.lock().map_err(|_| "motor de voz travado".to_string())?;
    if guard.is_none() {
        *guard = Some(load_engine()?);
    }
    LAST_USED.store(now_secs(), Ordering::Relaxed);
    let engine = guard.as_ref().expect("carregado acima");
    unsafe {
        let audio = (engine.generate)(engine.tts, text.as_ptr(), SPEAKER_ID, SPEED);
        if audio.is_null() {
            return Err("o motor de voz não gerou áudio".into());
        }
        let a = &*audio;
        let samples = std::slice::from_raw_parts(a.samples, a.n.max(0) as usize).to_vec();
        let rate = a.sample_rate.max(1) as u32;
        (engine.destroy_audio)(audio);
        Ok((samples, rate))
    }
}

/// Tira o modelo da memória se ninguém falou há um tempo.
fn schedule_unload() {
    tauri::async_runtime::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(IDLE_UNLOAD_SECS + 5)).await;
        if now_secs().saturating_sub(LAST_USED.load(Ordering::Relaxed)) >= IDLE_UNLOAD_SECS {
            if let Ok(mut g) = ENGINE.lock() {
                *g = None;
            }
        }
    });
}

// ── texto, áudio e boca ──────────────────────────────────────────────────

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
    let cleaned: String = words
        .join(" ")
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || ".,;:!?-–—'\"()%/".contains(*c))
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

/// Parte o texto em frases de até `CHUNK_CHARS`: a primeira toca logo.
fn chunks(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        cur.push(ch);
        let end = matches!(ch, '.' | '!' | '?') && cur.chars().count() >= 12;
        if end || cur.chars().count() >= CHUNK_CHARS {
            out.push(cur.trim().to_string());
            cur.clear();
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out.retain(|c| c.chars().any(|ch| ch.is_alphanumeric()));
    out
}

/// Janelas de volume (0..1, a mais alta = 1) das amostras.
fn envelope(samples: &[f32], rate: u32) -> Vec<f32> {
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

fn write_wav(samples: &[f32], rate: u32) -> Result<PathBuf, String> {
    let path = std::env::temp_dir().join(format!(
        "lilith-voice-{}-{}.wav",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
    ));
    let spec = hound::WavSpec { channels: 1, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut writer = hound::WavWriter::create(&path, spec).map_err(|e| e.to_string())?;
    for &s in samples {
        writer.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).map_err(|e| e.to_string())?;
    }
    writer.finalize().map_err(|e| e.to_string())?;
    Ok(path)
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
    let parts = chunks(&clean_for_speech(text));
    if parts.is_empty() || !is_installed() {
        return;
    }
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    stop_playback();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let synth = |text: String| tokio::task::spawn_blocking(move || synthesize(&text));
        let mut next = Some(synth(parts[0].clone()));
        let _ = app.emit("lilith://speaking", true);
        for i in 0..parts.len() {
            let current = match next.take() {
                Some(handle) => handle.await,
                None => break,
            };
            let (samples, rate) = match current {
                Ok(Ok(x)) => x,
                Ok(Err(e)) => {
                    tracing::warn!("voz: {e}");
                    break;
                }
                Err(e) => {
                    tracing::warn!("voz: {e}");
                    break;
                }
            };
            if GENERATION.load(Ordering::SeqCst) != generation {
                break;
            }
            // Já começa a próxima frase enquanto esta toca.
            next = parts.get(i + 1).map(|p| synth(p.clone()));
            let wav = match write_wav(&samples, rate) {
                Ok(w) => w,
                Err(e) => {
                    tracing::warn!("voz: {e}");
                    break;
                }
            };
            let env = envelope(&samples, rate);
            let duration = std::time::Duration::from_millis(samples.len() as u64 * 1000 / rate as u64);
            play_async(&wav);
            let started = std::time::Instant::now();
            let tick = std::time::Duration::from_millis(FRAME_MS as u64);
            while started.elapsed() < duration {
                if GENERATION.load(Ordering::SeqCst) != generation {
                    break;
                }
                let idx = (started.elapsed().as_millis() / FRAME_MS as u128) as usize;
                // Suaviza: mistura a janela atual com a seguinte.
                let level = env.get(idx).copied().unwrap_or(0.0) * 0.7 + env.get(idx + 1).copied().unwrap_or(0.0) * 0.3;
                let _ = app.emit("lilith://mouth", level);
                tokio::time::sleep(tick).await;
            }
            if GENERATION.load(Ordering::SeqCst) == generation {
                // Deixa o áudio terminar de tocar antes de apagar o arquivo.
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
            let _ = std::fs::remove_file(&wav);
            if GENERATION.load(Ordering::SeqCst) != generation {
                break;
            }
        }
        if GENERATION.load(Ordering::SeqCst) == generation {
            let _ = app.emit("lilith://mouth", 0.0f32);
            let _ = app.emit("lilith://speaking", false);
        }
        schedule_unload();
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
    fn text_is_split_into_sentences() {
        let parts = chunks("Oi, tudo bem? Eu abri o Spotify pra você. Mais alguma coisa");
        assert_eq!(parts, vec!["Oi, tudo bem?", "Eu abri o Spotify pra você.", "Mais alguma coisa"]);
        assert!(chunks("...").is_empty());
        let long = chunks(&"palavra ".repeat(60));
        assert!(long.iter().all(|c| c.chars().count() <= CHUNK_CHARS + 1));
    }

    /// Valida o layout da config do motor e os acentos de ponta a ponta.
    /// Precisa da voz instalada: `cargo test -- --ignored engine_speaks_portuguese`.
    #[test]
    #[ignore]
    fn engine_speaks_portuguese() {
        assert!(is_installed(), "instale a voz antes");
        let (samples, rate) = synthesize("Oi, tudo bem com você?").expect("sintetizou");
        assert_eq!(rate, 24000);
        assert!(samples.len() > 24000, "pelo menos 1 s de áudio, veio {}", samples.len());
        assert!(samples.iter().any(|s| s.abs() > 0.05), "áudio não pode ser silêncio");
        let again = synthesize("Você").expect("segunda chamada reaproveita o motor");
        assert!(!again.0.is_empty());
    }

    #[test]
    fn envelope_is_normalized() {
        let rate = 1000u32; // 40 amostras por janela de 40 ms
        let mut samples = vec![0.0f32; 40];
        samples.extend(std::iter::repeat_n(0.5f32, 40));
        let env = envelope(&samples, rate);
        assert_eq!(env.len(), 2);
        assert!(env[0] < 0.01 && (env[1] - 1.0).abs() < 1e-3);
        assert!(envelope(&[0.0; 80], rate).iter().all(|v| *v == 0.0));
    }
}
