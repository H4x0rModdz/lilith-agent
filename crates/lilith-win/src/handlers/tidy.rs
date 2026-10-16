//! Arrumação de arquivos: organizar uma pasta (e desfazer), achar duplicados
//! (e mandar as cópias extras para a Lixeira) e os maiores arquivos.
//!
//! As ações que mexem em arquivos têm duas fases. [`prepare`] calcula o plano
//! (só lendo) e devolve a prévia para o cartão de aprovação; o app troca os
//! parâmetros do modelo pelo plano calculado, e o handler executa exatamente
//! esse plano — o modelo nunca escolhe quais arquivos mover ou apagar.

use super::blocking;
use crate::organize::{self, human_size, Plan};
use crate::paths::resolve;
use crate::scan::{self, Entry};
use crate::WinError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Plano pronto para aprovação: `params` (com o plano embutido) substituem os
/// do modelo; `preview` vai para o cartão do chat.
#[derive(Debug)]
pub enum Prepared {
    Ready { params: Value, preview: Value },
    /// Nada a fazer: vira a resposta direto, sem pedir aprovação.
    Nothing(Value),
}

/// Ações com plano calculado pelo código antes da aprovação.
pub const PLANNED_ACTIONS: &[&str] = &["file.organize", "file.organize_undo", "file.dedupe"];

/// Pasta pedida ou a padrão (Downloads).
fn folder_param(params: &Value, default: fn() -> Option<PathBuf>) -> Result<PathBuf, WinError> {
    params["folder"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(resolve)
        .or_else(default)
        .ok_or_else(|| WinError::Unavailable("pasta padrão".into()))
}

fn log_path() -> PathBuf {
    organize::default_log()
}

/// Calcula o plano e a prévia de uma ação de [`PLANNED_ACTIONS`]. `None` para
/// as outras ações. Sempre ignora qualquer plano vindo nos parâmetros.
pub async fn prepare(action: &str, params: &Value) -> Result<Option<Prepared>, WinError> {
    match action {
        "file.organize" => {
            let folder = folder_param(params, dirs::download_dir)?;
            let plan = blocking(move || organize::plan(&folder)).await?;
            if plan.moves.is_empty() {
                return Ok(Some(Prepared::Nothing(json!({
                    "folder": plan.folder, "moved": 0, "nothing": true, "skipped": plan.skipped,
                }))));
            }
            let preview = organize::preview(&plan);
            let plan = serde_json::to_value(&plan).map_err(|e| WinError::Failed(e.to_string()))?;
            Ok(Some(Prepared::Ready { params: json!({ "plan": plan }), preview }))
        }
        "file.organize_undo" => {
            let log = log_path();
            let Some(last) = blocking(move || Ok(organize::last_batch_summary(&log))).await? else {
                return Ok(Some(Prepared::Nothing(json!({ "restored": 0, "nothing": true }))));
            };
            Ok(Some(Prepared::Ready { params: json!({}), preview: undo_preview(&last) }))
        }
        "file.dedupe" => {
            let folder = folder_param(params, dirs::download_dir)?;
            let plan = blocking(move || dedupe_plan(&folder)).await?;
            if plan.groups.is_empty() {
                return Ok(Some(Prepared::Nothing(json!({
                    "folder": plan.folder, "trashed": 0, "nothing": true,
                }))));
            }
            let preview = dedupe_preview(&plan);
            let plan = serde_json::to_value(&plan).map_err(|e| WinError::Failed(e.to_string()))?;
            Ok(Some(Prepared::Ready { params: json!({ "plan": plan }), preview }))
        }
        _ => Ok(None),
    }
}

fn undo_preview(last: &Value) -> Value {
    use chrono::TimeZone;
    let folder = PathBuf::from(last["folder"].as_str().unwrap_or(""));
    let when = chrono::Local
        .timestamp_millis_opt(last["ts_ms"].as_i64().unwrap_or(0))
        .single()
        .map(|d| d.format("%d/%m às %H:%M").to_string())
        .unwrap_or_default();
    let count = last["count"].as_u64().unwrap_or(0);
    let items: Vec<String> = last["moves"]
        .as_array()
        .map(|m| m.iter().take(4).filter_map(|p| p[0].as_str()).map(|p| file_name(Path::new(p))).collect())
        .unwrap_or_default();
    json!({
        "kind": "organize_undo",
        "title": format!("Desfazer a organização de {}", organize::folder_label(&folder)),
        "summary": format!("Vou devolver {count} arquivo(s) para onde estavam (organização de {when})."),
        "groups": [{ "label": "Voltam para o lugar", "items": items, "more": (count as usize).saturating_sub(items.len()) }],
        "notes": ["Subpastas que ficarem vazias são removidas."],
        "confirm": "Desfazer",
    })
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

pub async fn organize(params: Value) -> Result<Value, WinError> {
    let plan: Plan = match params.get("plan") {
        Some(p) => serde_json::from_value(p.clone()).map_err(|e| WinError::InvalidParams(format!("plano: {e}")))?,
        // Sem plano (chamada direta): calcula e executa na hora.
        None => {
            let folder = folder_param(&params, dirs::download_dir)?;
            blocking(move || organize::plan(&folder)).await?
        }
    };
    blocking(move || organize::execute(&plan, &log_path())).await
}

pub async fn organize_undo(_params: Value) -> Result<Value, WinError> {
    blocking(|| organize::undo(&log_path())).await
}

// ── duplicados ─────────────────────────────────────────────────────────

/// Menor arquivo considerado nos duplicados (ícones e arquivinhos de 1 KB
/// repetidos só fazem barulho).
const DUP_MIN_SIZE: u64 = 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DedupeGroup {
    pub keep: PathBuf,
    pub trash: Vec<PathBuf>,
    pub size: u64,
    /// Hash (hex) do conteúdo: conferido de novo antes de apagar.
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DedupePlan {
    pub folder: PathBuf,
    pub groups: Vec<DedupeGroup>,
}

fn hex(h: &[u8; 32]) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

fn dedupe_plan(folder: &Path) -> Result<DedupePlan, WinError> {
    let walk = scan::walk(folder, scan::MAX_FILES)?;
    let groups = scan::duplicates(walk.files, DUP_MIN_SIZE)
        .into_iter()
        .map(|g| {
            let keep = scan::keeper(&g.files);
            DedupeGroup {
                keep: g.files[keep].path.clone(),
                trash: g.files.iter().enumerate().filter(|(i, _)| *i != keep).map(|(_, f)| f.path.clone()).collect(),
                size: g.size,
                hash: hex(&g.hash),
            }
        })
        .collect();
    Ok(DedupePlan { folder: folder.to_path_buf(), groups })
}

fn dedupe_preview(plan: &DedupePlan) -> Value {
    const SHOW_GROUPS: usize = 8;
    let files: usize = plan.groups.iter().map(|g| g.trash.len()).sum();
    let total: u64 = plan.groups.iter().map(|g| g.size * g.trash.len() as u64).sum();
    let groups: Vec<Value> = plan
        .groups
        .iter()
        .take(SHOW_GROUPS)
        .map(|g| {
            json!({
                "label": format!("Fica: {} ({})", relative(&plan.folder, &g.keep), human_size(g.size)),
                "items": g.trash.iter().take(4).map(|p| format!("Lixeira: {}", relative(&plan.folder, p))).collect::<Vec<_>>(),
                "more": g.trash.len().saturating_sub(4),
            })
        })
        .collect();
    let mut notes = vec!["Só vai para a Lixeira o que tem conteúdo idêntico a outro arquivo (conferido byte a byte pelo hash); dá para restaurar de lá.".to_string()];
    if plan.groups.len() > SHOW_GROUPS {
        notes.insert(0, format!("E mais {} grupos de cópias.", plan.groups.len() - SHOW_GROUPS));
    }
    json!({
        "kind": "dedupe",
        "title": format!("Limpar duplicados em {}", organize::folder_label(&plan.folder)),
        "summary": format!("Vou mandar {files} cópia(s) para a Lixeira e liberar {}. Fica uma de cada.", human_size(total)),
        "groups": groups,
        "notes": notes,
        "confirm": "Mandar para a Lixeira",
    })
}

/// Caminho relativo à pasta varrida, para caber no cartão.
fn relative(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).map(|r| r.display().to_string()).unwrap_or_else(|_| p.display().to_string())
}

/// Executa um plano de limpeza já aprovado. Antes de cada exclusão confere de
/// novo: a cópia e a que fica ainda existem e têm o MESMO conteúdo (hash do
/// arquivo inteiro). Qualquer dúvida, não apaga.
pub async fn dedupe(params: Value) -> Result<Value, WinError> {
    let plan: DedupePlan = match params.get("plan") {
        Some(p) => serde_json::from_value(p.clone()).map_err(|e| WinError::InvalidParams(format!("plano: {e}")))?,
        None => {
            let folder = folder_param(&params, dirs::download_dir)?;
            blocking(move || dedupe_plan(&folder)).await?
        }
    };
    blocking(move || {
        let (mut trashed, mut freed, mut kept_back) = (0usize, 0u64, 0usize);
        for g in &plan.groups {
            let keep_ok = g.keep.starts_with(&plan.folder)
                && std::fs::metadata(&g.keep).is_ok_and(|m| m.len() == g.size)
                && scan::full_hash(&g.keep).is_ok_and(|h| hex(&h) == g.hash);
            if !keep_ok {
                kept_back += g.trash.len();
                continue;
            }
            let keep_real = std::fs::canonicalize(&g.keep).ok();
            for p in &g.trash {
                // Mesmo arquivo com outra grafia ("FOTO.JPG") nunca conta como cópia.
                let same = keep_real.is_some()
                    && std::fs::canonicalize(p).ok() != keep_real
                    && p.starts_with(&plan.folder)
                    && std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() == g.size)
                    && scan::full_hash(p).is_ok_and(|h| hex(&h) == g.hash);
                if same && trash::delete(p).is_ok() {
                    trashed += 1;
                    freed += g.size;
                } else {
                    kept_back += 1;
                }
            }
        }
        Ok(json!({
            "folder": plan.folder,
            "trashed": trashed,
            "freed": freed,
            "freed_text": human_size(freed),
            "kept_back": kept_back,
        }))
    })
    .await
}

pub async fn duplicates(params: Value) -> Result<Value, WinError> {
    let folder = folder_param(&params, dirs::download_dir)?;
    let min = params["min_size_kb"].as_u64().map(|k| k * 1024).unwrap_or(DUP_MIN_SIZE);
    blocking(move || {
        let walk = scan::walk(&folder, scan::MAX_FILES)?;
        let groups = scan::duplicates(walk.files, min);
        Ok(scan::duplicates_report(&folder, &groups, walk.truncated, 15))
    })
    .await
}

// ── maiores arquivos ───────────────────────────────────────────────────

pub async fn big(params: Value) -> Result<Value, WinError> {
    let folder = folder_param(&params, dirs::home_dir)?;
    let limit = params["limit"].as_u64().unwrap_or(10).clamp(1, 50) as usize;
    blocking(move || {
        let walk = scan::walk(&folder, scan::MAX_FILES)?;
        let scanned = walk.files.len();
        let total: u64 = walk.files.iter().map(|f| f.size).sum();
        let top: Vec<Entry> = scan::biggest(walk.files, limit);
        let files: Vec<Value> = top
            .iter()
            .map(|f| json!({ "path": f.path, "size": f.size, "size_text": human_size(f.size) }))
            .collect();
        Ok(json!({
            "folder": folder,
            "files": files,
            "scanned": scanned,
            "total_text": human_size(total),
            "truncated": walk.truncated,
        }))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn old_file(p: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
        let f = std::fs::File::options().write(true).open(p).unwrap();
        f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600)).unwrap();
    }

    #[tokio::test]
    async fn organize_runs_exactly_the_previewed_plan() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        old_file(&root.join("a.png"), b"x");
        let Some(Prepared::Ready { params, preview }) =
            prepare("file.organize", &json!({ "folder": root, "plan": { "bogus": true } })).await.unwrap()
        else {
            panic!("esperava um plano");
        };
        assert_eq!(preview["kind"], "organize");
        // Arquivo que chegou depois da prévia não entra.
        old_file(&root.join("b.png"), b"y");
        let plan: Plan = serde_json::from_value(params["plan"].clone()).unwrap();
        let r = organize::execute(&plan, &root.join("log.json")).unwrap();
        assert_eq!(r["moved"], 1);
        assert!(root.join("Imagens").join("a.png").exists());
        assert!(root.join("b.png").exists());

        let empty = tempfile::tempdir().unwrap();
        assert!(matches!(prepare("file.organize", &json!({ "folder": empty.path() })).await.unwrap(), Some(Prepared::Nothing(_))));
        assert!(prepare("file.list", &json!({})).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn dedupe_trashes_only_verified_copies() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let content = vec![7u8; 4096];
        old_file(&root.join("foto.jpg"), &content);
        old_file(&root.join("foto (1).jpg"), &content);
        old_file(&root.join("outra.jpg"), &[1u8; 4096]);
        let Some(Prepared::Ready { params, preview }) = prepare("file.dedupe", &json!({ "folder": root })).await.unwrap() else {
            panic!("esperava um plano");
        };
        assert!(preview["summary"].as_str().unwrap().starts_with("Vou mandar 1 cópia(s) para a Lixeira"));
        let plan: DedupePlan = serde_json::from_value(params["plan"].clone()).unwrap();
        assert_eq!(plan.groups.len(), 1);
        assert!(plan.groups[0].keep.ends_with("foto.jpg"));
        assert!(plan.groups[0].trash[0].ends_with("foto (1).jpg"));

        // Plano adulterado: um arquivo diferente no lugar da cópia não é apagado.
        let mut tampered = plan.clone();
        tampered.groups[0].trash = vec![root.join("outra.jpg")];
        let r = dedupe(json!({ "plan": tampered })).await.unwrap();
        assert_eq!(r["trashed"], 0);
        assert!(root.join("outra.jpg").exists());
        // Fora da pasta varrida também não.
        let outside = tempfile::tempdir().unwrap();
        old_file(&outside.path().join("foto.jpg"), &content);
        let mut escaped = plan.clone();
        escaped.groups[0].trash = vec![outside.path().join("foto.jpg")];
        assert_eq!(dedupe(json!({ "plan": escaped })).await.unwrap()["trashed"], 0);
        assert!(outside.path().join("foto.jpg").exists());
    }

    #[tokio::test]
    async fn reports_duplicates_and_big_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        old_file(&root.join("x.bin"), &[3u8; 2048]);
        old_file(&root.join("sub").join("x.bin"), &[3u8; 2048]);
        old_file(&root.join("pequeno.bin"), &[3u8; 10]);
        let d = duplicates(json!({ "folder": root })).await.unwrap();
        assert_eq!(d["group_count"], 1);
        assert_eq!(d["reclaimable_text"], "2,0 KB");
        let b = big(json!({ "folder": root, "limit": 1 })).await.unwrap();
        assert_eq!(b["files"].as_array().unwrap().len(), 1);
        assert_eq!(b["scanned"], 3);
    }
}
