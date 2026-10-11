use super::blocking;
use crate::paths::{fold, resolve, unique};
use crate::{req_str, WinError};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::PathBuf;

const MAX_DEPTH: usize = 6;
const MAX_VISITED: usize = 200_000;
/// Pastas grandes e sem interesse para o usuário — deixam a busca lenta.
const SKIP_DIRS: &[&str] = &["appdata", "node_modules", "$recycle.bin", "windows", "target", ".git"];

/// Busca em largura por nome (parcial, sem acento/maiúscula), com limites de
/// profundidade e de entradas visitadas para nunca travar o disco.
pub async fn find(params: Value) -> Result<Value, WinError> {
    let name = fold(req_str(&params, "name")?);
    let root = params["root"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map(resolve)
        .or_else(dirs::home_dir)
        .ok_or_else(|| WinError::Unavailable("pasta do usuário".into()))?;
    let limit = params["limit"].as_u64().unwrap_or(20).clamp(1, 100) as usize;

    blocking(move || {
        let mut found = Vec::new();
        let mut queue = VecDeque::from([(root.clone(), 0usize)]);
        let mut visited = 0usize;
        while let Some((dir, depth)) = queue.pop_front() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                visited += 1;
                let file_name = entry.file_name().to_string_lossy().into_owned();
                let folded = fold(&file_name);
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                if folded.contains(&name) {
                    found.push(json!({ "path": entry.path(), "dir": is_dir }));
                    if found.len() >= limit {
                        return Ok(json!({ "root": root, "results": found, "truncated": true }));
                    }
                }
                if is_dir && depth < MAX_DEPTH && !folded.starts_with('.') && !SKIP_DIRS.contains(&folded.as_str()) {
                    queue.push_back((entry.path(), depth + 1));
                }
            }
            if visited >= MAX_VISITED {
                break;
            }
        }
        Ok(json!({ "root": root, "results": found, "truncated": visited >= MAX_VISITED }))
    })
    .await
}

pub async fn list(params: Value) -> Result<Value, WinError> {
    let path = resolve(req_str(&params, "path")?);
    blocking(move || {
        let mut items: Vec<Value> = std::fs::read_dir(&path)?
            .flatten()
            .take(500)
            .map(|e| {
                let meta = e.metadata().ok();
                json!({
                    "name": e.file_name().to_string_lossy(),
                    "dir": meta.as_ref().map(|m| m.is_dir()).unwrap_or(false),
                    "size": meta.as_ref().filter(|m| m.is_file()).map(|m| m.len()),
                })
            })
            .collect();
        items.sort_by_key(|v| (!v["dir"].as_bool().unwrap_or(false), v["name"].as_str().unwrap_or("").to_lowercase()));
        Ok(json!({ "path": path, "items": items }))
    })
    .await
}

pub async fn move_(params: Value) -> Result<Value, WinError> {
    let (src, dest) = source_dest(&params)?;
    tokio::fs::rename(&src, &dest).await?;
    Ok(json!({ "moved": src, "to": dest }))
}

pub async fn copy(params: Value) -> Result<Value, WinError> {
    let (src, dest) = source_dest(&params)?;
    if src.is_dir() {
        return Err(WinError::InvalidParams("copiar pastas ainda não é suportado".into()));
    }
    let bytes = tokio::fs::copy(&src, &dest).await?;
    Ok(json!({ "copied": src, "to": dest, "bytes": bytes }))
}

/// Sempre para a Lixeira — apagar permanente não é exposto ao LLM.
pub async fn delete(params: Value) -> Result<Value, WinError> {
    let path = resolve(req_str(&params, "path")?);
    if !path.exists() {
        return Err(WinError::NotFound(path.display().to_string()));
    }
    let p = path.clone();
    blocking(move || trash::delete(&p).map_err(|e| WinError::Failed(format!("lixeira: {e}")))).await?;
    Ok(json!({ "trashed": path }))
}

/// Destino que é pasta existente recebe o nome do arquivo de origem; nunca
/// sobrescreve — colisão vira "nome (1).ext".
fn source_dest(params: &Value) -> Result<(PathBuf, PathBuf), WinError> {
    let src = resolve(req_str(params, "source")?);
    if !src.exists() {
        return Err(WinError::NotFound(src.display().to_string()));
    }
    let mut dest = resolve(req_str(params, "destination")?);
    if dest.is_dir() {
        if let Some(name) = src.file_name() {
            dest = dest.join(name);
        }
    }
    Ok((src, unique(&dest)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn find_list_copy_move() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_string_lossy().into_owned();
        std::fs::create_dir(dir.path().join("Relatórios")).unwrap();
        std::fs::write(dir.path().join("Relatórios").join("relatorio-final.pdf"), "x").unwrap();

        let r = find(json!({ "name": "RELATORIO-final", "root": root })).await.unwrap();
        assert_eq!(r["results"].as_array().unwrap().len(), 1);

        let src = dir.path().join("Relatórios").join("relatorio-final.pdf");
        copy(json!({ "source": src, "destination": root })).await.unwrap();
        copy(json!({ "source": src, "destination": root })).await.unwrap();
        assert!(dir.path().join("relatorio-final (1).pdf").exists());

        move_(json!({ "source": dir.path().join("relatorio-final (1).pdf"), "destination": dir.path().join("b.pdf") }))
            .await
            .unwrap();
        let l = list(json!({ "path": root })).await.unwrap();
        let names: Vec<_> = l["items"].as_array().unwrap().iter().map(|i| i["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(names, vec!["Relatórios", "b.pdf", "relatorio-final.pdf"]);
    }
}
