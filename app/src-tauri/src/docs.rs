//! Busca nos documentos do usuário. Ele escolhe as pastas (Configurações ou
//! "indexa a pasta Documentos"); a Lilith lê os txt/md/pdf/docx em segundo
//! plano, divide em trechos e guarda no SQLite — com vetores do Ollama quando
//! o modelo de embeddings está instalado, só com o índice de palavras quando
//! não está. Reindexa só o que mudou (data de modificação e tamanho).
//! Achar e ordenar é do código; o modelo só entra para resumir um documento
//! quando o usuário pede.

use lilith_brain::embed::{Embedder, DEFAULT_EMBED_MODEL};
use lilith_memory::Memory;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

const FOLDERS_KEY: &str = "docs_folders";
const MODEL_KEY: &str = "docs_embed_model";
/// Tamanho dos trechos (caracteres) e sobreposição entre eles.
const CHUNK: usize = 900;
const OVERLAP: usize = 150;
/// Trechos por documento, no máximo (um livro inteiro não trava o índice).
const MAX_CHUNKS: usize = 300;
const EMBED_BATCH: usize = 16;
/// Documentos olhados por pasta, no máximo.
const MAX_FILES: usize = 50_000;
/// De quanto em quanto tempo reindexa sozinha (só o que mudou).
const REINDEX_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

static INDEXING: AtomicBool = AtomicBool::new(false);
static PROGRESS: Mutex<Progress> = Mutex::new(Progress { running: false, done: 0, total: 0, semantic: false, failed: 0 });
static PULLING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub running: bool,
    pub done: usize,
    pub total: usize,
    /// Indexando com vetores (modelo de embeddings disponível).
    pub semantic: bool,
    /// Documentos que não deu para ler (protegidos, corrompidos…).
    pub failed: usize,
}

pub fn folders(store: &Memory) -> Vec<String> {
    store.setting(FOLDERS_KEY).and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default()
}

fn save_folders(store: &Memory, list: &[String]) {
    store.set_setting(FOLDERS_KEY, &serde_json::to_string(list).unwrap_or_else(|_| "[]".into()));
}

/// Acrescenta uma pasta ("Documentos", "C:\\…"). Devolve o caminho completo.
pub fn add_folder(store: &Memory, raw: &str) -> Result<String, String> {
    let path = lilith_win::resolve_path(raw);
    if !path.is_dir() {
        return Err(format!("não achei a pasta {}", path.display()));
    }
    let full = path.display().to_string();
    let mut list = folders(store);
    let key = full.to_lowercase();
    if !list.iter().any(|f| f.to_lowercase() == key) {
        list.push(full.clone());
        save_folders(store, &list);
    }
    Ok(full)
}

pub fn remove_folder(store: &Memory, folder: &str) {
    let list: Vec<String> = folders(store).into_iter().filter(|f| f != folder).collect();
    save_folders(store, &list);
    store.doc_remove_folder(folder);
}

pub fn model(store: &Memory) -> String {
    store.setting(MODEL_KEY).filter(|m| !m.trim().is_empty()).unwrap_or_else(|| DEFAULT_EMBED_MODEL.into())
}

pub fn set_model(store: &Memory, model: &str) {
    let m: String = model.trim().chars().filter(|c| c.is_ascii_alphanumeric() || "-_.:/".contains(*c)).take(80).collect();
    store.set_setting(MODEL_KEY, if m.is_empty() { DEFAULT_EMBED_MODEL } else { &m });
}

/// Endereço do Ollama (o mesmo do chat), lido uma vez do config.
fn host() -> &'static str {
    static HOST: OnceLock<String> = OnceLock::new();
    HOST.get_or_init(|| crate::config::Config::load().ollama.host)
}

pub fn embedder(store: &Memory) -> Embedder {
    Embedder::new(host(), &model(store))
}

fn progress() -> Progress {
    PROGRESS.lock().unwrap().clone()
}

fn emit_progress(app: &AppHandle) {
    let _ = app.emit("lilith://docs-index", progress());
}

/// Começa a indexar em segundo plano. `false` se já estava indexando.
pub fn start_index(app: AppHandle, store: Arc<Memory>) -> bool {
    if INDEXING.swap(true, Ordering::SeqCst) {
        return false;
    }
    tauri::async_runtime::spawn(async move {
        run_index(&app, &store).await;
        INDEXING.store(false, Ordering::SeqCst);
        PROGRESS.lock().unwrap().running = false;
        emit_progress(&app);
    });
    true
}

/// Reindexa sozinha de tempos em tempos (só o que mudou: barato).
pub fn schedule(app: AppHandle, store: Arc<Memory>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(90)).await;
        loop {
            if !folders(&store).is_empty() {
                start_index(app.clone(), store.clone());
            }
            tokio::time::sleep(REINDEX_EVERY).await;
        }
    });
}

fn mtime_secs(m: std::time::SystemTime) -> i64 {
    m.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

async fn run_index(app: &AppHandle, store: &Arc<Memory>) {
    let emb = embedder(store);
    let mut semantic = emb.available().await;
    *PROGRESS.lock().unwrap() = Progress { running: true, done: 0, total: 0, semantic, failed: 0 };
    emit_progress(app);
    let mut last_emit = Instant::now();
    for folder in folders(store) {
        let root = PathBuf::from(&folder);
        let walk = tokio::task::spawn_blocking(move || lilith_win::scan::walk(&root, MAX_FILES)).await;
        let Ok(Ok(walk)) = walk else {
            tracing::warn!("pasta de documentos indisponível");
            continue;
        };
        let docs: Vec<lilith_win::scan::Entry> = walk
            .files
            .into_iter()
            .filter(|f| {
                lilith_win::docs::is_document(&f.path)
                    && f.size <= lilith_win::docs::MAX_DOC_BYTES
                    && !lilith_win::looks_sensitive(&f.path)
            })
            .collect();
        PROGRESS.lock().unwrap().total += docs.len();
        let mut seen: HashSet<String> = HashSet::new();
        for doc in docs {
            let path = doc.path.display().to_string();
            seen.insert(path.clone());
            let mtime = mtime_secs(doc.modified);
            let size = doc.size as i64;
            let fresh = store
                .doc_file(&path)
                .is_some_and(|k| k.mtime == mtime && k.size == size && (k.embedded || !semantic));
            if !fresh {
                let p = doc.path.clone();
                let text = tokio::task::spawn_blocking(move || lilith_win::docs::extract_text(&p)).await;
                match text {
                    Ok(Ok(text)) => {
                        let chunks = lilith_win::docs::chunks(&text, CHUNK, OVERLAP, MAX_CHUNKS);
                        let vectors = if semantic && !chunks.is_empty() {
                            match embed_all(&emb, &chunks).await {
                                Some(v) => Some(v),
                                None => {
                                    // O Ollama parou de responder: segue só com palavras.
                                    semantic = false;
                                    PROGRESS.lock().unwrap().semantic = false;
                                    None
                                }
                            }
                        } else if chunks.is_empty() {
                            Some(Vec::new())
                        } else {
                            None
                        };
                        store.doc_store(&path, &folder, mtime, size, &chunks, vectors.as_deref());
                    }
                    _ => {
                        // Ilegível: registra vazio para não tentar de novo até mudar.
                        store.doc_store(&path, &folder, mtime, size, &[], Some(&[]));
                        PROGRESS.lock().unwrap().failed += 1;
                    }
                }
            }
            PROGRESS.lock().unwrap().done += 1;
            if last_emit.elapsed() > Duration::from_millis(400) {
                emit_progress(app);
                last_emit = Instant::now();
            }
        }
        // Documentos apagados ou movidos saem do índice.
        for old in store.doc_paths_in(&folder) {
            if !seen.contains(&old) {
                store.doc_remove(&old);
            }
        }
    }
}

async fn embed_all(emb: &Embedder, chunks: &[String]) -> Option<Vec<Vec<f32>>> {
    let mut out = Vec::with_capacity(chunks.len());
    for batch in chunks.chunks(EMBED_BATCH) {
        let inputs: Vec<String> = batch.iter().map(|c| emb.document_input(c)).collect();
        out.extend(emb.embed(&inputs).await.ok()?);
    }
    Some(out)
}

/// Minúscula e sem acento, um caractere por caractere (mantém as posições).
fn fold_char(c: char) -> char {
    let c = c.to_lowercase().next().unwrap_or(c);
    match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        other => other,
    }
}

/// Trecho curto em volta da primeira palavra da busca que aparece no texto
/// (ou o começo do trecho, se nenhuma aparece — achado pelo sentido).
pub fn snippet(text: &str, query: &str) -> String {
    const BEFORE: usize = 80;
    const AFTER: usize = 160;
    let chars: Vec<char> = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
    let folded: Vec<char> = chars.iter().map(|c| fold_char(*c)).collect();
    let words: Vec<Vec<char>> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3)
        .map(|w| w.chars().map(fold_char).collect())
        .collect();
    let hit = words
        .iter()
        .filter_map(|w| folded.windows(w.len()).position(|win| win == w.as_slice()))
        .min()
        .unwrap_or(0);
    let start = hit.saturating_sub(BEFORE);
    let end = (hit + AFTER).min(chars.len());
    let mut out: String = chars[start..end].iter().collect();
    out = out.trim().to_string();
    if start > 0 {
        out = format!("…{out}");
    }
    if end < chars.len() {
        out.push('…');
    }
    out
}

/// Os documentos mais parecidos com a pergunta.
pub async fn search(store: &Memory, query: &str, limit: usize) -> Value {
    let stats = store.doc_stats();
    let mut qvec = None;
    if stats.embedded_chunks > 0 {
        let emb = embedder(store);
        if let Ok(mut v) = emb.embed(&[emb.query_input(query)]).await {
            qvec = v.pop();
        }
    }
    let hits = store.doc_search(query, qvec.as_deref(), limit);
    let results: Vec<Value> = hits
        .iter()
        .map(|h| {
            let path = Path::new(&h.path);
            json!({
                "path": h.path,
                "name": path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                "snippet": snippet(&h.text, query),
                "semantic": h.semantic,
            })
        })
        .collect();
    json!({
        "query": query,
        "results": results,
        "indexed_files": stats.files,
        "mode": if qvec.is_some() { "semantic" } else { "keyword" },
    })
}

/// `docs.index` / `docs.search` pedidos no chat.
pub async fn action(app: &AppHandle, store: &Arc<Memory>, action: &str, params: &Value) -> Result<Value, lilith_win::WinError> {
    use lilith_win::WinError;
    match action {
        "docs.search" => {
            let query = params["query"].as_str().map(str::trim).filter(|q| !q.is_empty());
            let query = query.ok_or_else(|| WinError::InvalidParams("parâmetro 'query' obrigatório".into()))?;
            let limit = params["limit"].as_u64().unwrap_or(5).clamp(1, 10) as usize;
            Ok(search(store, query, limit).await)
        }
        "docs.index" => {
            if let Some(f) = params["folder"].as_str().map(str::trim).filter(|f| !f.is_empty()) {
                add_folder(store, f).map_err(WinError::NotFound)?;
            }
            let list = folders(store);
            if list.is_empty() {
                return Err(WinError::InvalidParams("nenhuma pasta escolhida: passe `folder`".into()));
            }
            let started = start_index(app.clone(), store.clone());
            let semantic = embedder(store).available().await;
            Ok(json!({ "folders": list, "started": started, "already_running": !started, "semantic": semantic }))
        }
        other => Err(WinError::NotFound(format!("ação '{other}'"))),
    }
}

// ── Configurações ──────────────────────────────────────────────────────

/// Tudo o que a seção "Documentos" das Configurações mostra.
pub async fn status(store: &Memory) -> Value {
    let emb = embedder(store);
    let models = emb.installed_models().await;
    let installed = models.as_ref().is_ok_and(|m| m.iter().any(|n| lilith_brain::embed::same_model(n, emb.model())));
    json!({
        "folders": folders(store),
        "stats": store.doc_stats(),
        "model": emb.model(),
        "model_installed": installed,
        "ollama_ok": models.is_ok(),
        "pulling": PULLING.load(Ordering::SeqCst),
        "progress": progress(),
    })
}

/// Baixa o modelo de embeddings (só quando o usuário pede) e, no fim,
/// reindexa para preencher os vetores. Progresso em `lilith://docs-model`.
pub fn pull_model(app: AppHandle, store: Arc<Memory>) -> bool {
    if PULLING.swap(true, Ordering::SeqCst) {
        return false;
    }
    tauri::async_runtime::spawn(async move {
        let emb = embedder(&store);
        let mut last = Instant::now() - Duration::from_secs(1);
        let result = emb
            .pull(|done, total, status| {
                if last.elapsed() > Duration::from_millis(300) {
                    let _ = app.emit("lilith://docs-model", json!({ "completed": done, "total": total, "status": status }));
                    last = Instant::now();
                }
            })
            .await;
        PULLING.store(false, Ordering::SeqCst);
        let error = result.err().map(|e| e.to_string());
        let _ = app.emit("lilith://docs-model", json!({ "finished": true, "error": error }));
        if error.is_none() && !folders(&store).is_empty() {
            start_index(app.clone(), store.clone());
        }
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_center_on_the_query() {
        let text = format!("{} O contrato de aluguel vence em março. {}", "a ".repeat(100), "b ".repeat(200));
        let s = snippet(&text, "Contrato");
        assert!(s.starts_with('…') && s.ends_with('…'), "{s}");
        assert!(s.contains("O contrato de aluguel vence em março."), "{s}");
        assert_eq!(snippet("Férias na praia", "ferias"), "Férias na praia");
        assert_eq!(snippet("texto curto", "nada"), "texto curto", "sem palavra: o começo");
    }

    #[test]
    fn folders_are_saved_once_and_removed_with_their_index() {
        let store = Memory::in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().display().to_string();
        assert_eq!(add_folder(&store, &path).unwrap(), path);
        add_folder(&store, &path.to_uppercase()).ok();
        assert_eq!(folders(&store).len(), 1);
        assert!(add_folder(&store, "Z:\\nao\\existe").is_err());
        store.doc_store("x.txt", &path, 1, 1, &["oi".to_string()], None);
        remove_folder(&store, &path);
        assert!(folders(&store).is_empty());
        assert_eq!(store.doc_stats().files, 0);
        set_model(&store, " bge-m3; rm ");
        assert_eq!(model(&store), "bge-m3rm");
    }
}

// ── comandos das Configurações ─────────────────────────────────────────

pub mod commands {
    use crate::AppState;
    use serde_json::Value;
    use tauri::{AppHandle, State};

    #[tauri::command]
    pub async fn docs_status(state: State<'_, AppState>) -> Result<Value, String> {
        let store = state.store.clone();
        Ok(super::status(&store).await)
    }

    /// Acrescenta a pasta e já começa a indexar.
    #[tauri::command]
    pub fn docs_add_folder(folder: String, app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
        let full = super::add_folder(&state.store, &folder)?;
        super::start_index(app, state.store.clone());
        Ok(full)
    }

    #[tauri::command]
    pub fn docs_remove_folder(folder: String, state: State<'_, AppState>) {
        super::remove_folder(&state.store, &folder);
    }

    #[tauri::command]
    pub fn docs_reindex(app: AppHandle, state: State<'_, AppState>) -> bool {
        super::start_index(app, state.store.clone())
    }

    /// Baixa o modelo de embeddings no Ollama (só quando o usuário clica).
    #[tauri::command]
    pub fn docs_pull_model(app: AppHandle, state: State<'_, AppState>) -> bool {
        super::pull_model(app, state.store.clone())
    }

    #[tauri::command]
    pub fn docs_set_model(model: String, state: State<'_, AppState>) {
        super::set_model(&state.store, &model);
    }
}
