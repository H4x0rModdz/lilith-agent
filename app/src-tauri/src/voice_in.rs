//! Voz da Lilith (entrada): o usuário fala, ela ouve — reconhecimento de fala
//! local, em português do Brasil, com push-to-talk.
//!
//! Motor: o mesmo sherpa-onnx da voz de saída (C API carregada no processo),
//! agora com o reconhecedor offline e o Whisper small (OpenAI, MIT) em int8,
//! com o idioma fixo em português (≈375 MB). Comparado com o Kokoro falando
//! frases conhecidas, ele acerta nomes de apps (Spotify, Discord, WhatsApp) e
//! frases curtas ("Oi, Lilith!") onde os modelos menores erravam a grafia ou
//! viravam inglês. Os arquivos são baixados no primeiro uso para
//! `%APPDATA%\Lilith\voice\asr\`, conferidos por SHA-256; o modelo fica na
//! memória só enquanto se usa e sai depois de um tempo parado.
//!
//! O código decide tudo: o microfone só grava enquanto o atalho está apertado
//! (ou entre um toque e outro), o silêncio das pontas é cortado por energia, e o
//! texto reconhecido vai para o chat pelo mesmo caminho de uma mensagem digitada
//! (`lilith://heard` → o chat chama `send_message`). Nada sai do PC.
//!
//! Atalho (padrão Ctrl+Alt+Espaço, configurável):
//! - segurar e soltar: grava enquanto segura e envia ao soltar;
//! - toque rápido: começa a ouvir e envia sozinha depois de uma pausa (ou no
//!   próximo toque);
//! - Esc (ou o kill switch) cancela.

use crate::config::data_dir;
use serde::Serialize;
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Shortcut, ShortcutState};

/// Whisper small exportado para o sherpa-onnx (os mesmos arquivos do pacote
/// `sherpa-onnx-whisper-small` do release `asr-models`), num commit fixo — só
/// as versões int8, sem baixar as fp32 (≈1 GB) que o pacote também traz.
const MODEL_BASE: &str =
    "https://huggingface.co/csukuangfj/sherpa-onnx-whisper-small/resolve/8f3c18b358db4d1f2fc1eae49d75cd20989e4309";
/// (arquivo, SHA-256, tamanho em bytes)
const MODEL_FILES: [(&str, &str, u64); 3] = [
    ("small-encoder.int8.onnx", "4cbe7b22fa9026b843b60a68640c747de05bafb1a11b57edc0e66c232d9f33a9", 112_442_483),
    ("small-decoder.int8.onnx", "acad50b5c782696e91b55914cc5ab4f756f1532f76e22aa6fc615f39fb69a8ee", 262_226_114),
    ("small-tokens.txt", "b34b360dbb493e781e479794586d661700670d65564001f23024971d1f2fa126", 816_730),
];
/// Idioma fixo: sem ele o Whisper às vezes "ouve" inglês em frases curtas.
const LANGUAGE: &str = "pt";

/// Taxa de captura (o modelo espera 16 kHz mono).
pub const SAMPLE_RATE: u32 = 16_000;
/// Segurando o atalho por mais que isto, soltar envia ("segurar para falar").
const HOLD_MS: u64 = 450;
/// No modo toque: depois de falar, este silêncio encerra e envia.
const END_SILENCE_MS: u64 = 1_300;
/// No modo toque: sem nenhuma fala neste tempo, desiste.
const NO_SPEECH_MS: u64 = 8_000;
/// Gravação máxima de uma vez (o microfone nunca fica aberto à toa).
const MAX_RECORD_MS: u64 = 45_000;
/// Depois deste tempo sem ouvir, o modelo sai da memória.
const IDLE_UNLOAD_SECS: u64 = 180;

/// Atalho padrão: não briga com atalhos comuns do Windows e dos apps.
pub const DEFAULT_HOTKEY: &str = "Ctrl+Alt+Space";
const SETTING_ENABLED: &str = "voice_in";
const SETTING_HOTKEY: &str = "voice_in_hotkey";

static INSTALLING: AtomicBool = AtomicBool::new(false);
static LAST_USED: AtomicU64 = AtomicU64::new(0);
/// Cada sessão de escuta nova (ou cancelada) invalida a anterior.
static GENERATION: AtomicU64 = AtomicU64::new(0);
static RECOGNIZER: Mutex<Option<Recognizer>> = Mutex::new(None);
static SESSION: Mutex<Option<Session>> = Mutex::new(None);
/// Reconhecendo o que foi gravado (entre soltar o atalho e o texto chegar).
static PROCESSING: AtomicBool = AtomicBool::new(false);
/// Atalho registrado agora (para trocar e para reconhecer no handler).
static HOTKEY: Mutex<Option<Shortcut>> = Mutex::new(None);
static ESC_REGISTERED: AtomicBool = AtomicBool::new(false);

fn esc_key() -> Shortcut {
    Shortcut::new(None, Code::Escape)
}

fn asr_dir() -> PathBuf {
    data_dir().join("voice").join("asr")
}

fn model_dir() -> PathBuf {
    asr_dir().join("model")
}

fn model_installed(dir: &Path) -> bool {
    MODEL_FILES.iter().all(|(f, _, _)| dir.join(f).is_file())
}

pub fn is_installed() -> bool {
    crate::voice::engine_lib_installed() && model_installed(&model_dir())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ── instalação ───────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
pub struct InstallProgress {
    pub stage: String,
    pub percent: u32,
}

fn progress(app: &AppHandle, stage: &str, percent: u32) {
    let _ = app.emit("lilith://voice-in-install", InstallProgress { stage: stage.into(), percent });
}

/// Instala o motor (se a voz de saída ainda não trouxe) e o modelo (≈375 MB).
/// Idempotente. O progresso vai em `lilith://voice-in-install`.
pub async fn install(app: AppHandle) -> Result<(), String> {
    if is_installed() {
        progress(&app, "pronto", 100);
        return Ok(());
    }
    if INSTALLING.swap(true, Ordering::SeqCst) {
        return Err("a instalação já está em andamento".into());
    }
    let on = |stage: &str, pct: u32| progress(&app, stage, pct);
    let result = install_with(&on).await;
    INSTALLING.store(false, Ordering::SeqCst);
    match &result {
        Ok(()) => progress(&app, "pronto", 100),
        Err(e) => progress(&app, &format!("erro: {e}"), 0),
    }
    result
}

async fn install_with(on: &(dyn Fn(&str, u32) + Send + Sync)) -> Result<(), String> {
    let tmp = asr_dir().join("dl");
    crate::voice::ensure_engine_lib(&tmp, on).await?;
    // Um percentual só para os três arquivos (pesado pelo tamanho).
    let total: u64 = MODEL_FILES.iter().map(|(_, _, size)| size).sum();
    let mut done = 0u64;
    for (name, sha256, size) in MODEL_FILES {
        let dest = model_dir().join(name);
        if !dest.is_file() {
            let url = format!("{MODEL_BASE}/{name}");
            let overall =
                |pct: u32| on("Baixando o reconhecimento de voz", ((done + size * pct as u64 / 100) * 100 / total) as u32);
            crate::voice::download_with(&url, &dest, sha256, &overall).await?;
        }
        done += size;
    }
    let _ = std::fs::remove_dir_all(&tmp);
    if is_installed() {
        Ok(())
    } else {
        Err("a instalação terminou, mas faltam arquivos".into())
    }
}

// ── reconhecedor (sherpa-onnx C API, espelha `c-api.h` da v1.13.8) ───────

type P = *const c_char;

#[repr(C)]
struct FeatureCfg {
    sample_rate: i32,
    feature_dim: i32,
}
#[repr(C)]
struct TransducerCfg {
    encoder: P,
    decoder: P,
    joiner: P,
}
#[repr(C)]
struct WhisperCfg {
    encoder: P,
    decoder: P,
    language: P,
    task: P,
    tail_paddings: i32,
    enable_token_timestamps: i32,
    enable_segment_timestamps: i32,
}
#[repr(C)]
struct SenseVoiceCfg {
    model: P,
    language: P,
    use_itn: i32,
}
#[repr(C)]
struct CanaryCfg {
    paths: [P; 4], // encoder, decoder, src_lang, tgt_lang
    use_pnc: i32,
}
#[repr(C)]
struct FunAsrNanoCfg {
    paths: [P; 6], // encoder_adaptor, llm, embedding, tokenizer, system_prompt, user_prompt
    max_new_tokens: i32,
    temperature: f32,
    top_p: f32,
    seed: i32,
    language: P,
    itn: i32,
    hotwords: P,
}
#[repr(C)]
struct Qwen3AsrCfg {
    paths: [P; 4], // conv_frontend, encoder, decoder, tokenizer
    max_total_len: i32,
    max_new_tokens: i32,
    temperature: f32,
    top_p: f32,
    seed: i32,
    hotwords: P,
}
#[repr(C)]
struct CohereCfg {
    paths: [P; 3], // encoder, decoder, language
    use_punct: i32,
    use_itn: i32,
}
#[repr(C)]
struct AsrModelCfg {
    transducer: TransducerCfg,
    paraformer: [P; 1],
    nemo_ctc: [P; 1],
    whisper: WhisperCfg,
    tdnn: [P; 1],
    tokens: P,
    num_threads: i32,
    debug: i32,
    provider: P,
    model_type: P,
    modeling_unit: P,
    bpe_vocab: P,
    telespeech_ctc: P,
    sense_voice: SenseVoiceCfg,
    moonshine: [P; 5],
    fire_red_asr: [P; 2],
    dolphin: [P; 1],
    zipformer_ctc: [P; 1],
    canary: CanaryCfg,
    wenet_ctc: [P; 1],
    omnilingual: [P; 1],
    medasr: [P; 1],
    funasr_nano: FunAsrNanoCfg,
    fire_red_asr_ctc: [P; 1],
    qwen3_asr: Qwen3AsrCfg,
    cohere_transcribe: CohereCfg,
}
#[repr(C)]
struct LmCfg {
    model: P,
    scale: f32,
}
#[repr(C)]
struct HomophoneCfg {
    dict_dir: P,
    lexicon: P,
    rule_fsts: P,
}
#[repr(C)]
struct RecognizerCfg {
    feat_config: FeatureCfg,
    model_config: AsrModelCfg,
    lm_config: LmCfg,
    decoding_method: P,
    max_active_paths: i32,
    hotwords_file: P,
    hotwords_score: f32,
    rule_fsts: P,
    rule_fars: P,
    blank_penalty: f32,
    hr: HomophoneCfg,
}

type CreateFn = unsafe extern "C" fn(*const RecognizerCfg) -> *const c_void;
type DestroyFn = unsafe extern "C" fn(*const c_void);
type CreateStreamFn = unsafe extern "C" fn(*const c_void) -> *const c_void;
type AcceptFn = unsafe extern "C" fn(*const c_void, i32, *const f32, i32);
type DecodeFn = unsafe extern "C" fn(*const c_void, *const c_void);
/// Devolve `SherpaOnnxOfflineRecognizerResult*`; o primeiro campo é o texto.
type ResultFn = unsafe extern "C" fn(*const c_void) -> *const *const c_char;
type DestroyResultFn = unsafe extern "C" fn(*const *const c_char);

struct Recognizer {
    rec: *const c_void,
    destroy: DestroyFn,
    create_stream: CreateStreamFn,
    destroy_stream: DestroyFn,
    accept: AcceptFn,
    decode: DecodeFn,
    result: ResultFn,
    destroy_result: DestroyResultFn,
    /// Mantém a DLL carregada enquanto o reconhecedor existir.
    _lib: libloading::Library,
}

// O ponteiro só é usado sob o `Mutex` global (ou por um dono só, nos testes).
unsafe impl Send for Recognizer {}

impl Drop for Recognizer {
    fn drop(&mut self) {
        unsafe { (self.destroy)(self.rec) };
    }
}

fn cstr(path: &Path) -> Result<CString, String> {
    CString::new(path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())
}

fn load_recognizer(dir: &Path) -> Result<Recognizer, String> {
    let lib = crate::voice::load_engine_lib()?;
    let [encoder, decoder, tokens] = MODEL_FILES.map(|(f, _, _)| cstr(&dir.join(f)));
    let (encoder, decoder, tokens) = (encoder?, decoder?, tokens?);
    let language = CString::new(LANGUAGE).unwrap();
    let task = CString::new("transcribe").unwrap();
    let provider = CString::new("cpu").unwrap();
    let method = CString::new("greedy_search").unwrap();
    // Metade dos núcleos (2..4): rápido sem travar o resto do PC.
    let threads = std::thread::available_parallelism().map(|n| (n.get() / 2).clamp(2, 4)).unwrap_or(2) as i32;

    unsafe fn sym<T: Copy>(lib: &libloading::Library, name: &[u8]) -> Result<T, String> {
        lib.get::<T>(name).map(|s| *s).map_err(|e| e.to_string())
    }
    unsafe {
        let create: CreateFn = sym(&lib, b"SherpaOnnxCreateOfflineRecognizer\0")?;
        let destroy: DestroyFn = sym(&lib, b"SherpaOnnxDestroyOfflineRecognizer\0")?;
        let create_stream: CreateStreamFn = sym(&lib, b"SherpaOnnxCreateOfflineStream\0")?;
        let destroy_stream: DestroyFn = sym(&lib, b"SherpaOnnxDestroyOfflineStream\0")?;
        let accept: AcceptFn = sym(&lib, b"SherpaOnnxAcceptWaveformOffline\0")?;
        let decode: DecodeFn = sym(&lib, b"SherpaOnnxDecodeOfflineStream\0")?;
        let result: ResultFn = sym(&lib, b"SherpaOnnxGetOfflineStreamResult\0")?;
        let destroy_result: DestroyResultFn = sym(&lib, b"SherpaOnnxDestroyOfflineRecognizerResult\0")?;

        // Tudo zerado (ponteiros nulos = vazio/padrão); só o Whisper é preenchido.
        let mut cfg: RecognizerCfg = std::mem::zeroed();
        cfg.feat_config.sample_rate = SAMPLE_RATE as i32;
        cfg.feat_config.feature_dim = 80;
        cfg.model_config.whisper.encoder = encoder.as_ptr();
        cfg.model_config.whisper.decoder = decoder.as_ptr();
        cfg.model_config.whisper.language = language.as_ptr();
        cfg.model_config.whisper.task = task.as_ptr();
        cfg.model_config.tokens = tokens.as_ptr();
        cfg.model_config.num_threads = threads;
        cfg.model_config.provider = provider.as_ptr();
        cfg.decoding_method = method.as_ptr();
        let rec = create(&cfg);
        if rec.is_null() {
            return Err("o reconhecimento de voz não iniciou (modelo inválido?)".into());
        }
        Ok(Recognizer { rec, destroy, create_stream, destroy_stream, accept, decode, result, destroy_result, _lib: lib })
    }
}

impl Recognizer {
    /// Reconhece amostras mono [-1, 1] (qualquer taxa; o motor reamostra).
    fn recognize(&self, samples: &[f32], rate: u32) -> Result<String, String> {
        unsafe {
            let stream = (self.create_stream)(self.rec);
            if stream.is_null() {
                return Err("o reconhecimento de voz não abriu".into());
            }
            (self.accept)(stream, rate as i32, samples.as_ptr(), samples.len() as i32);
            (self.decode)(self.rec, stream);
            let r = (self.result)(stream);
            let text = if r.is_null() || (*r).is_null() {
                String::new()
            } else {
                CStr::from_ptr(*r).to_string_lossy().into_owned()
            };
            if !r.is_null() {
                (self.destroy_result)(r);
            }
            (self.destroy_stream)(stream);
            Ok(text.split_whitespace().collect::<Vec<_>>().join(" "))
        }
    }
}

/// Carrega o modelo (se preciso) — chamado ao começar a ouvir, para o
/// reconhecimento já estar pronto quando o usuário terminar de falar.
fn warm() -> Result<(), String> {
    let mut guard = RECOGNIZER.lock().map_err(|_| "reconhecimento travado".to_string())?;
    if guard.is_none() {
        *guard = Some(load_recognizer(&model_dir())?);
    }
    LAST_USED.store(now_secs(), Ordering::Relaxed);
    Ok(())
}

/// Texto do que foi dito (amostras mono, taxa `rate`).
pub fn transcribe(samples: &[f32], rate: u32) -> Result<String, String> {
    let mut guard = RECOGNIZER.lock().map_err(|_| "reconhecimento travado".to_string())?;
    if guard.is_none() {
        *guard = Some(load_recognizer(&model_dir())?);
    }
    LAST_USED.store(now_secs(), Ordering::Relaxed);
    let text = guard.as_ref().expect("carregado acima").recognize(samples, rate);
    LAST_USED.store(now_secs(), Ordering::Relaxed);
    text
}

/// Tira o modelo da memória se ninguém falou com ela há um tempo.
fn schedule_unload() {
    tauri::async_runtime::spawn(async {
        tokio::time::sleep(Duration::from_secs(IDLE_UNLOAD_SECS + 5)).await;
        let busy = SESSION.lock().map(|s| s.is_some()).unwrap_or(true) || PROCESSING.load(Ordering::SeqCst);
        if !busy && now_secs().saturating_sub(LAST_USED.load(Ordering::Relaxed)) >= IDLE_UNLOAD_SECS {
            if let Ok(mut g) = RECOGNIZER.lock() {
                *g = None;
            }
        }
    });
}

// ── áudio: energia, corte do silêncio ────────────────────────────────────

/// Volume (RMS) de um trecho.
fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

/// Abaixo disto é silêncio, mesmo num microfone baixo.
const MIN_SPEECH_RMS: f32 = 0.006;
/// Fala de verdade tem pelo menos isto de trechos com voz.
const MIN_SPEECH_MS: usize = 200;
/// Margem mantida antes e depois da fala (o modelo gosta de um respiro).
const PAD_MS: usize = 250;

/// Corta o silêncio das pontas por energia (janelas de 20 ms) e normaliza o
/// volume. `None` = não tinha fala.
pub fn trim_silence(samples: &[f32], rate: u32) -> Option<Vec<f32>> {
    let frame = (rate as usize * 20 / 1000).max(1);
    let levels: Vec<f32> = samples.chunks(frame).map(rms).collect();
    let peak = levels.iter().cloned().fold(0.0f32, f32::max);
    if peak < MIN_SPEECH_RMS * 2.0 {
        return None;
    }
    // Ruído de fundo ≈ as janelas mais baixas; a fala fica bem acima dele.
    let mut sorted = levels.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let noise = sorted[sorted.len() / 5];
    let threshold = MIN_SPEECH_RMS.max(noise * 2.5).max(peak * 0.06);
    let voiced: Vec<usize> = levels.iter().enumerate().filter(|(_, l)| **l > threshold).map(|(i, _)| i).collect();
    if voiced.len() * 20 < MIN_SPEECH_MS {
        return None;
    }
    let pad = PAD_MS / 20;
    let first = voiced[0].saturating_sub(pad);
    let last = (voiced[voiced.len() - 1] + pad + 1).min(levels.len());
    let mut out = samples[first * frame..(last * frame).min(samples.len())].to_vec();
    // Microfone baixo: aumenta até o pico ficar em ~0,7 (no máximo 8x).
    let max = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if max > 0.0 && max < 0.7 {
        let gain = (0.7 / max).min(8.0);
        for s in &mut out {
            *s *= gain;
        }
    }
    Some(out)
}

/// Frases que o Whisper "inventa" em ruído ou silêncio (legendas de vídeo do
/// treino). Se a transcrição inteira for uma delas, não houve fala.
const HALLUCINATIONS: [&str; 5] = [
    "legendas pela comunidade amara org",
    "obrigado por assistir",
    "obrigada por assistir",
    "inscreva se no canal",
    "legenda adriana zanotto",
];

/// Limpa o texto reconhecido: tira marcações como "[Música]" e "(risos)" e
/// descarta alucinações conhecidas. `None` = nada aproveitável.
pub fn clean_transcript(raw: &str) -> Option<String> {
    let mut out = String::new();
    let mut depth = 0i32;
    for c in raw.chars() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let text = out.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = text.trim_matches(|c: char| c == '-' || c == '–' || c.is_whitespace()).to_string();
    let key: String = text
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if key.is_empty() || HALLUCINATIONS.contains(&key.as_str()) {
        return None;
    }
    Some(text)
}

/// Detector de fala ao vivo (por energia), para o modo toque saber quando a
/// pessoa terminou e para o nível do microfone na tela.
struct LiveVad {
    noise: f32,
    voiced_ms: u64,
    silence_ms: u64,
    elapsed_ms: u64,
}

impl LiveVad {
    fn new() -> Self {
        LiveVad { noise: 0.0, voiced_ms: 0, silence_ms: 0, elapsed_ms: 0 }
    }

    /// Consome um trecho e diz se tem voz.
    fn push(&mut self, chunk: &[f32], rate: u32) -> bool {
        let level = rms(chunk);
        let ms = (chunk.len() as u64 * 1000) / rate.max(1) as u64;
        self.elapsed_ms += ms;
        // O ruído acompanha devagar os trechos baixos e rápido as quedas.
        self.noise = if self.noise == 0.0 {
            level
        } else if level < self.noise {
            self.noise * 0.7 + level * 0.3
        } else {
            self.noise * 0.995 + level * 0.005
        };
        let voiced = level > (MIN_SPEECH_RMS * 2.0).max(self.noise * 3.0);
        if voiced {
            self.voiced_ms += ms;
            self.silence_ms = 0;
        } else {
            self.silence_ms += ms;
        }
        voiced
    }

    /// No modo toque: já falou e fez uma pausa — terminou.
    fn finished_speaking(&self) -> bool {
        self.voiced_ms >= 300 && self.silence_ms >= END_SILENCE_MS
    }

    fn gave_up(&self) -> bool {
        self.voiced_ms < 300 && self.elapsed_ms >= NO_SPEECH_MS
    }
}

// ── captura do microfone (WASAPI, 16 kHz mono float) ─────────────────────

#[cfg(windows)]
mod mic {
    use windows::Win32::Media::Audio::{
        eCapture, eConsole, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
        AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
        AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, WAVEFORMATEX,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED};

    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

    /// Grava o microfone padrão até `on_chunk` devolver `false`. O Windows
    /// converte para 16 kHz mono float (AUTOCONVERTPCM), sem reamostrar aqui.
    pub fn record(rate: u32, on_chunk: &mut dyn FnMut(&[f32]) -> bool) -> Result<Vec<f32>, String> {
        unsafe {
            let com = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
            let result = record_inner(rate, on_chunk);
            if com {
                CoUninitialize();
            }
            result
        }
    }

    unsafe fn record_inner(rate: u32, on_chunk: &mut dyn FnMut(&[f32]) -> bool) -> Result<Vec<f32>, String> {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| format!("áudio indisponível: {e}"))?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eCapture, eConsole)
            .map_err(|_| "não achei um microfone ligado".to_string())?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None).map_err(|e| format!("microfone indisponível: {e}"))?;
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
            nChannels: 1,
            nSamplesPerSec: rate,
            nAvgBytesPerSec: rate * 4,
            nBlockAlign: 4,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        // Buffer de 200 ms (em unidades de 100 ns).
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                2_000_000,
                0,
                &format,
                None,
            )
            .map_err(|e| format!("o microfone recusou o formato: {e}"))?;
        let capture: IAudioCaptureClient = client.GetService().map_err(|e| e.to_string())?;
        client.Start().map_err(|e| format!("o microfone não abriu: {e}"))?;
        let mut all: Vec<f32> = Vec::with_capacity(rate as usize * 10);
        let mut fresh: Vec<f32> = Vec::with_capacity(rate as usize / 10);
        'outer: loop {
            std::thread::sleep(std::time::Duration::from_millis(30));
            fresh.clear();
            loop {
                let next = match capture.GetNextPacketSize() {
                    Ok(n) => n,
                    Err(_) => break 'outer,
                };
                if next == 0 {
                    break;
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                if capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None).is_err() {
                    break 'outer;
                }
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    fresh.extend(std::iter::repeat_n(0.0f32, frames as usize));
                } else {
                    fresh.extend_from_slice(std::slice::from_raw_parts(data as *const f32, frames as usize));
                }
                let _ = capture.ReleaseBuffer(frames);
            }
            all.extend_from_slice(&fresh);
            if !on_chunk(&fresh) {
                break;
            }
        }
        let _ = client.Stop();
        Ok(all)
    }
}

#[cfg(not(windows))]
mod mic {
    pub fn record(_rate: u32, _on_chunk: &mut dyn FnMut(&[f32]) -> bool) -> Result<Vec<f32>, String> {
        Err("microfone só no Windows".into())
    }
}

// ── sessão de escuta (push-to-talk) ──────────────────────────────────────

struct Session {
    generation: u64,
    /// Para a gravação (o fio de captura confere a cada ~30 ms).
    stop: Arc<AtomicBool>,
    /// Soltou rápido: modo toque (encerra sozinha numa pausa).
    tap: Arc<AtomicBool>,
    /// O aperto que abriu a sessão ainda não foi solto.
    held: bool,
    started: Instant,
    thread: std::thread::JoinHandle<Result<Vec<f32>, String>>,
}

#[derive(Serialize, Clone)]
struct Listening {
    /// "start" | "processing" | "stop" | "cancel" | "missing" | "error"
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

fn emit_listening(app: &AppHandle, state: &'static str, message: Option<String>) {
    let _ = app.emit("lilith://listening", Listening { state, message });
}

pub fn is_listening() -> bool {
    SESSION.lock().map(|s| s.is_some()).unwrap_or(false)
}

fn set_esc(app: &AppHandle, on: bool) {
    let gs = app.global_shortcut();
    if on && !ESC_REGISTERED.swap(true, Ordering::SeqCst) {
        // Só enquanto ouve: fora disso o Esc é dos outros programas.
        if gs.register(esc_key()).is_err() {
            ESC_REGISTERED.store(false, Ordering::SeqCst);
        }
    } else if !on && ESC_REGISTERED.swap(false, Ordering::SeqCst) {
        let _ = gs.unregister(esc_key());
    }
}

/// Apertou o atalho (ou o botão do microfone).
pub fn press(app: &AppHandle) {
    if is_listening() {
        // Segundo toque: terminou de falar.
        finish(app);
        return;
    }
    if PROCESSING.load(Ordering::SeqCst) {
        return;
    }
    if !is_installed() {
        crate::windows::show_chat(app);
        emit_listening(app, "missing", None);
        return;
    }
    start(app);
}

/// Soltou o atalho (ou o botão): se segurou, envia; se foi um toque, segue
/// ouvindo até a pausa.
pub fn release(app: &AppHandle) {
    let held_long = {
        let Ok(mut guard) = SESSION.lock() else { return };
        let Some(session) = guard.as_mut() else { return };
        if !session.held {
            return;
        }
        session.held = false;
        let long = session.started.elapsed() >= Duration::from_millis(HOLD_MS);
        if !long {
            session.tap.store(true, Ordering::SeqCst);
        }
        long
    };
    if held_long {
        finish(app);
    }
}

fn start(app: &AppHandle) {
    // Ela não se ouve: para de falar antes de abrir o microfone.
    crate::voice::stop(app);
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let stop = Arc::new(AtomicBool::new(false));
    let tap = Arc::new(AtomicBool::new(false));
    let thread = {
        let (app, stop, tap) = (app.clone(), stop.clone(), tap.clone());
        std::thread::Builder::new()
            .name("lilith-mic".into())
            .spawn(move || {
                let mut vad = LiveVad::new();
                let mut last_level = Instant::now();
                let mut on_chunk = |chunk: &[f32]| -> bool {
                    if stop.load(Ordering::SeqCst) {
                        return false;
                    }
                    vad.push(chunk, SAMPLE_RATE);
                    if last_level.elapsed() >= Duration::from_millis(60) {
                        last_level = Instant::now();
                        let level = (rms(chunk) * 8.0).clamp(0.0, 1.0);
                        let _ = app.emit("lilith://mic-level", level);
                    }
                    let auto_end = tap.load(Ordering::SeqCst) && (vad.finished_speaking() || vad.gave_up());
                    !(auto_end || vad.elapsed_ms >= MAX_RECORD_MS)
                };
                let result = mic::record(SAMPLE_RATE, &mut on_chunk);
                // Terminou sem ninguém mandar (pausa no modo toque, tempo máximo
                // ou microfone com erro): encerra pelo resto do app, fora deste fio.
                if !stop.load(Ordering::SeqCst) {
                    tauri::async_runtime::spawn(async move {
                        if GENERATION.load(Ordering::SeqCst) == generation {
                            finish(&app);
                        }
                    });
                }
                result
            })
    };
    let thread = match thread {
        Ok(t) => t,
        Err(e) => {
            emit_listening(app, "error", Some(format!("Não consegui abrir o microfone: {e}")));
            return;
        }
    };
    if let Ok(mut guard) = SESSION.lock() {
        *guard = Some(Session { generation, stop, tap, held: true, started: Instant::now(), thread });
    }
    set_esc(app, true);
    emit_listening(app, "start", None);
    // Já carrega o modelo enquanto a pessoa fala.
    tauri::async_runtime::spawn_blocking(|| {
        if let Err(e) = warm() {
            tracing::warn!("voz (entrada): {e}");
        }
    });
}

/// Para de gravar e reconhece. O texto vai para o chat em `lilith://heard`.
pub fn finish(app: &AppHandle) {
    let Some(session) = SESSION.lock().ok().and_then(|mut g| g.take()) else { return };
    session.stop.store(true, Ordering::SeqCst);
    PROCESSING.store(true, Ordering::SeqCst);
    emit_listening(app, "processing", None);
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let generation = session.generation;
        let outcome = match session.thread.join() {
            Ok(Ok(samples)) => match trim_silence(&samples, SAMPLE_RATE) {
                None => Err("Não ouvi nada. Fale mais perto do microfone.".to_string()),
                Some(speech) => match transcribe(&speech, SAMPLE_RATE) {
                    Ok(raw) => clean_transcript(&raw).ok_or_else(|| "Não entendi. Pode repetir?".to_string()),
                    Err(e) => Err(format!("Não consegui reconhecer: {e}")),
                },
            },
            Ok(Err(e)) => Err(format!("Microfone: {e}")),
            Err(_) => Err("A gravação falhou.".to_string()),
        };
        set_esc(&app, false);
        PROCESSING.store(false, Ordering::SeqCst);
        schedule_unload();
        // Cancelada no meio (Esc, kill switch): descarta.
        if GENERATION.load(Ordering::SeqCst) != generation {
            return;
        }
        match outcome {
            Ok(text) => {
                crate::windows::show_chat(&app);
                let _ = app.emit("lilith://heard", serde_json::json!({ "text": text }));
                emit_listening(&app, "stop", None);
            }
            Err(message) => emit_listening(&app, "stop", Some(message)),
        }
    });
}

/// Cancela a escuta (e um reconhecimento em andamento). Nada é enviado.
pub fn cancel(app: &AppHandle) {
    let session = SESSION.lock().ok().and_then(|mut g| g.take());
    let was_busy = session.is_some() || PROCESSING.load(Ordering::SeqCst);
    GENERATION.fetch_add(1, Ordering::SeqCst);
    if let Some(s) = session {
        s.stop.store(true, Ordering::SeqCst);
        // O fio termina sozinho em ~30 ms; o áudio é descartado.
        std::thread::spawn(move || drop(s.thread.join()));
    }
    set_esc(app, false);
    if was_busy {
        emit_listening(app, "cancel", None);
    }
}

/// Handler dos atalhos globais: devolve `true` se o atalho era da escuta.
pub fn on_shortcut(app: &AppHandle, shortcut: &Shortcut, state: ShortcutState) -> bool {
    if ESC_REGISTERED.load(Ordering::SeqCst) && shortcut == &esc_key() {
        if state == ShortcutState::Pressed {
            cancel(app);
        }
        return true;
    }
    let ours = HOTKEY.lock().ok().and_then(|g| *g).is_some_and(|k| &k == shortcut);
    if !ours {
        return false;
    }
    match state {
        ShortcutState::Pressed => press(app),
        ShortcutState::Released => release(app),
    }
    true
}

// ── configurações ────────────────────────────────────────────────────────

/// Confere o texto do atalho ("Ctrl+Alt+Space") e devolve a forma normalizada.
pub fn parse_hotkey(text: &str) -> Result<Shortcut, String> {
    let shortcut: Shortcut = text.trim().parse().map_err(|_| format!("atalho inválido: {text}"))?;
    let fkey = format!("{:?}", shortcut.key).starts_with('F');
    if shortcut.mods.is_empty() && !fkey {
        return Err("use pelo menos um modificador (Ctrl, Alt, Shift ou Win), ou uma tecla F".into());
    }
    if shortcut.key == Code::Escape {
        return Err("Esc é reservado para cancelar (e Ctrl+Alt+Esc para parar tudo)".into());
    }
    Ok(shortcut)
}

#[derive(Serialize, Clone)]
pub struct Status {
    pub installed: bool,
    pub installing: bool,
    pub enabled: bool,
    pub hotkey: String,
    /// O atalho está registrado (falha se outro programa já usa).
    pub hotkey_active: bool,
    pub listening: bool,
    pub processing: bool,
}

fn settings(store: &lilith_memory::Memory) -> (bool, String) {
    let enabled = store.setting(SETTING_ENABLED).map(|v| v != "false").unwrap_or(true);
    let hotkey = store
        .setting(SETTING_HOTKEY)
        .filter(|h| parse_hotkey(h).is_ok())
        .unwrap_or_else(|| DEFAULT_HOTKEY.to_string());
    (enabled, hotkey)
}

pub fn status(store: &lilith_memory::Memory) -> Status {
    let (enabled, hotkey) = settings(store);
    Status {
        installed: is_installed(),
        installing: INSTALLING.load(Ordering::SeqCst),
        enabled,
        hotkey,
        hotkey_active: HOTKEY.lock().map(|g| g.is_some()).unwrap_or(false),
        listening: is_listening(),
        processing: PROCESSING.load(Ordering::SeqCst),
    }
}

/// (Re)registra o atalho conforme as configurações.
fn apply(app: &AppHandle, enabled: bool, hotkey: &str) -> Result<(), String> {
    let gs = app.global_shortcut();
    // Solta a trava antes de falar com o Windows (o handler também a usa).
    let old = HOTKEY.lock().map_err(|_| "atalho travado")?.take();
    if let Some(old) = old {
        let _ = gs.unregister(old);
    }
    if !enabled {
        cancel(app);
        return Ok(());
    }
    let shortcut = parse_hotkey(hotkey)?;
    gs.register(shortcut).map_err(|_| "esse atalho já está em uso por outro programa — escolha outro".to_string())?;
    *HOTKEY.lock().map_err(|_| "atalho travado")? = Some(shortcut);
    Ok(())
}

/// Na abertura do app: registra o atalho salvo.
pub fn init(app: &AppHandle, store: &lilith_memory::Memory) {
    let (enabled, hotkey) = settings(store);
    if let Err(e) = apply(app, enabled, &hotkey) {
        tracing::warn!(error = %e, hotkey, "não registrei o atalho de voz");
    }
}

// ── comandos ─────────────────────────────────────────────────────────────

#[tauri::command]
pub fn voice_in_status(app: AppHandle) -> Status {
    status(&app.state::<crate::AppState>().store)
}

/// Baixa e instala o reconhecimento de voz (≈375 MB).
#[tauri::command]
pub async fn voice_in_install(app: AppHandle) -> Result<(), String> {
    install(app).await
}

/// Botão do microfone: apertou.
#[tauri::command]
pub fn voice_in_press(app: AppHandle) {
    press(&app);
}

/// Botão do microfone: soltou.
#[tauri::command]
pub fn voice_in_release(app: AppHandle) {
    release(&app);
}

#[tauri::command]
pub fn voice_in_cancel(app: AppHandle) {
    cancel(&app);
}

/// Salva liga/desliga e o atalho. Só grava se o atalho registrar.
#[tauri::command]
pub fn voice_in_configure(enabled: bool, hotkey: String, app: AppHandle) -> Result<Status, String> {
    let store = app.state::<crate::AppState>().store.clone();
    let hotkey = hotkey.trim().to_string();
    if enabled {
        parse_hotkey(&hotkey)?;
    }
    let (old_enabled, old_hotkey) = settings(&store);
    if let Err(e) = apply(&app, enabled, &hotkey) {
        // Volta ao atalho que funcionava.
        let _ = apply(&app, old_enabled, &old_hotkey);
        return Err(e);
    }
    store.set_setting(SETTING_ENABLED, if enabled { "true" } else { "false" });
    if !hotkey.is_empty() {
        store.set_setting(SETTING_HOTKEY, &hotkey);
    }
    let st = status(&store);
    // O chat mostra/esconde o microfone e atualiza a dica do atalho.
    let _ = app.emit("lilith://voice-in-config", &st);
    Ok(st)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(ms: usize, amp: f32) -> Vec<f32> {
        (0..SAMPLE_RATE as usize * ms / 1000).map(|i| amp * (i as f32 * 0.07).sin()).collect()
    }

    #[test]
    fn silence_is_not_speech() {
        assert!(trim_silence(&vec![0.0; 16_000], SAMPLE_RATE).is_none());
        // Chiado baixinho também não.
        let hiss: Vec<f32> = (0..16_000).map(|i| ((i * 7919) % 13) as f32 * 0.0003 - 0.002).collect();
        assert!(trim_silence(&hiss, SAMPLE_RATE).is_none());
        // Estalo curto (< 200 ms) não é fala.
        let mut click = vec![0.0; 8_000];
        click.extend(tone(60, 0.5));
        click.extend(vec![0.0; 8_000]);
        assert!(trim_silence(&click, SAMPLE_RATE).is_none());
    }

    #[test]
    fn edges_are_trimmed_with_padding() {
        let mut audio = vec![0.0f32; 16_000]; // 1 s de silêncio
        audio.extend(tone(800, 0.05)); // fala baixa
        audio.extend(vec![0.0f32; 32_000]); // 2 s de silêncio
        let out = trim_silence(&audio, SAMPLE_RATE).expect("tinha fala");
        let ms = out.len() * 1000 / SAMPLE_RATE as usize;
        assert!((800..=800 + 2 * PAD_MS + 40).contains(&ms), "ficou com {ms} ms");
        // Volume normalizado.
        let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.3 && peak <= 0.71, "pico {peak}");
    }

    #[test]
    fn live_vad_ends_after_a_pause() {
        let mut vad = LiveVad::new();
        for _ in 0..10 {
            vad.push(&vec![0.001; 480], SAMPLE_RATE); // ruído de fundo
        }
        assert!(!vad.finished_speaking());
        for _ in 0..20 {
            assert!(vad.push(&tone(30, 0.2), SAMPLE_RATE)); // 600 ms falando
        }
        for _ in 0..30 {
            vad.push(&vec![0.001; 480], SAMPLE_RATE); // 900 ms de pausa
        }
        assert!(!vad.finished_speaking(), "pausa curta ainda não encerra");
        for _ in 0..20 {
            vad.push(&vec![0.001; 480], SAMPLE_RATE);
        }
        assert!(vad.finished_speaking());
        let mut quiet = LiveVad::new();
        for _ in 0..300 {
            quiet.push(&vec![0.001; 480], SAMPLE_RATE);
        }
        assert!(quiet.gave_up());
    }

    #[test]
    fn transcripts_are_cleaned() {
        assert_eq!(clean_transcript(" [Música] Abre o  Spotify. ").as_deref(), Some("Abre o Spotify."));
        assert_eq!(clean_transcript("(risos) Oi, Lilith!").as_deref(), Some("Oi, Lilith!"));
        assert_eq!(clean_transcript("Legendas pela comunidade Amara.org"), None);
        assert_eq!(clean_transcript("Obrigado por assistir!"), None);
        assert_eq!(clean_transcript("[Música]"), None);
        assert_eq!(clean_transcript("  ...  "), None);
        assert_eq!(clean_transcript("Obrigado por assistir o vídeo comigo").as_deref(), Some("Obrigado por assistir o vídeo comigo"));
    }

    #[test]
    fn hotkeys_are_validated() {
        assert!(parse_hotkey(DEFAULT_HOTKEY).is_ok());
        assert!(parse_hotkey("ctrl+shift+KeyM").is_ok());
        assert!(parse_hotkey("F9").is_ok());
        assert!(parse_hotkey("Space").is_err(), "sem modificador roubaria a tecla");
        assert!(parse_hotkey("Ctrl+Alt+Escape").is_err(), "é o kill switch");
        assert!(parse_hotkey("Ctrl+").is_err());
        assert!(parse_hotkey("banana").is_err());
    }

    /// Reamostragem linear (só para o teste: o microfone já entrega 16 kHz).
    fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
        let n = (samples.len() as u64 * to as u64 / from as u64) as usize;
        (0..n)
            .map(|i| {
                let pos = i as f64 * from as f64 / to as f64;
                let (j, frac) = (pos as usize, (pos.fract()) as f32);
                let a = samples.get(j).copied().unwrap_or(0.0);
                let b = samples.get(j + 1).copied().unwrap_or(a);
                a + (b - a) * frac
            })
            .collect()
    }

    /// Palavras normalizadas (minúsculas, sem pontuação) para comparar.
    fn words(s: &str) -> Vec<String> {
        s.to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { ' ' })
            .collect::<String>()
            .split_whitespace()
            .map(str::to_string)
            .collect()
    }

    /// Taxa de erro de palavras (distância de edição / palavras esperadas).
    fn wer(expected: &str, got: &str) -> f32 {
        let (a, b) = (words(expected), words(got));
        let mut prev: Vec<usize> = (0..=b.len()).collect();
        for i in 1..=a.len() {
            let mut cur = vec![i; b.len() + 1];
            for j in 1..=b.len() {
                let cost = usize::from(a[i - 1] != b[j - 1]);
                cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            }
            prev = cur;
        }
        prev[b.len()] as f32 / a.len().max(1) as f32
    }

    #[test]
    fn word_error_rate() {
        assert_eq!(wer("Abre o bloco de notas.", "abre o bloco de notas"), 0.0);
        assert!((wer("um dois três quatro", "um dois tres quatro") - 0.25).abs() < 1e-6);
    }

    /// Autoteste sem microfone: a voz de saída (Kokoro) fala frases conhecidas
    /// e o reconhecimento tem de devolver o mesmo texto. Baixa o modelo se
    /// faltar. Precisa da voz de saída instalada:
    /// `cargo test -- --ignored recognizes_lilith_speaking`.
    /// `LILITH_ASR_TEST_DIR` testa outra pasta com os mesmos arquivos.
    #[test]
    #[ignore]
    fn recognizes_lilith_speaking() {
        assert!(crate::voice::is_installed(), "instale a voz de saída antes");
        if !is_installed() {
            let on = |stage: &str, pct: u32| {
                if pct.is_multiple_of(25) {
                    eprintln!("{stage}… {pct}%");
                }
            };
            tauri::async_runtime::block_on(install_with(&on)).expect("instalou o modelo");
        }
        let dir = std::env::var_os("LILITH_ASR_TEST_DIR").filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(model_dir);
        let t0 = Instant::now();
        let rec = load_recognizer(&dir).expect("carregou o reconhecedor");
        eprintln!("modelo carregado em {:?}", t0.elapsed());
        // Frases típicas de uso (comandos com nomes de apps, lembretes,
        // perguntas) e respostas curtas a cartões de escolha.
        let sentences = [
            "Abre o bloco de notas e escreve bom dia.",
            "Qual é a previsão do tempo para amanhã em São Paulo?",
            "Me lembra de ligar para a minha mãe às oito horas.",
            "Toca uma música calma no Spotify.",
            "Abre o WhatsApp e manda um oi para o Pedro.",
            "Fecha o Google Chrome e abre o Discord.",
            "Aumenta o volume e pausa o vídeo do YouTube.",
            "Pesquisa receita de bolo de cenoura.",
            "Liga o modo não perturbe por uma hora.",
            "A segunda.",
            "Cancela.",
            "Oi, Lilith!",
            "Desliga o computador.",
            "Qual é o meu nome?",
        ];
        let mut total = 0.0;
        for s in sentences {
            let (samples, rate) = crate::voice::synthesize(s).expect("sintetizou");
            // Como no microfone: 16 kHz, silêncio antes e depois e um chiado leve.
            let mut audio = vec![0.0f32; SAMPLE_RATE as usize / 2];
            audio.extend(resample(&samples, rate, SAMPLE_RATE));
            audio.extend(vec![0.0f32; SAMPLE_RATE as usize]);
            for (i, x) in audio.iter_mut().enumerate() {
                *x += ((i * 7919 % 101) as f32 / 101.0 - 0.5) * 0.004;
            }
            let speech = trim_silence(&audio, SAMPLE_RATE).expect("achou a fala");
            let t = Instant::now();
            let raw = rec.recognize(&speech, SAMPLE_RATE).expect("reconheceu");
            let text = clean_transcript(&raw).unwrap_or_default();
            let secs = speech.len() as f32 / SAMPLE_RATE as f32;
            let err = wer(s, &text);
            total += err;
            eprintln!("[{:.1} s de áudio em {:?}] WER {:.2}\n  esperado: {s}\n  ouvido:   {text}", secs, t.elapsed(), err);
            assert!(err <= 0.34, "transcrição longe demais: {text:?}");
        }
        let mean = total / sentences.len() as f32;
        eprintln!("WER médio: {mean:.3}");
        assert!(mean <= 0.15, "WER médio {mean}");
    }
}
