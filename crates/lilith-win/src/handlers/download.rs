use super::web::http_client;
use crate::paths::{resolve, unique};
use crate::{req_str, WinError};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

/// Baixa `url` para Downloads (ou `destination`). Grava em `.part` e renomeia
/// no fim, para nunca deixar arquivo pela metade com o nome final.
pub async fn file(params: Value) -> Result<Value, WinError> {
    let url = req_str(&params, "url")?;
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(WinError::InvalidParams("url precisa ser http(s)".into()));
    }
    let resp = http_client()?
        .get(url)
        .send()
        .await
        .map_err(|e| WinError::Unavailable(format!("download: {e}")))?
        .error_for_status()
        .map_err(|e| WinError::Failed(format!("download: {e}")))?;

    let name = filename_from(resp.headers().get(reqwest::header::CONTENT_DISPOSITION), resp.url().path());
    let dest = target_path(params["destination"].as_str(), &name)?;
    if let Some(dir) = dest.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }

    let part = dest.with_extension(format!(
        "{}part",
        dest.extension().map(|e| format!("{}.", e.to_string_lossy())).unwrap_or_default()
    ));
    let mut out = tokio::fs::File::create(&part).await?;
    let mut bytes: u64 = 0;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                drop(out);
                let _ = tokio::fs::remove_file(&part).await;
                return Err(WinError::Failed(format!("download interrompido: {e}")));
            }
        };
        out.write_all(&chunk).await?;
        bytes += chunk.len() as u64;
    }
    out.flush().await?;
    drop(out);
    tokio::fs::rename(&part, &dest).await?;
    Ok(json!({ "path": dest, "bytes": bytes }))
}

fn target_path(destination: Option<&str>, name: &str) -> Result<PathBuf, WinError> {
    let base = match destination.filter(|d| !d.trim().is_empty()) {
        Some(d) => resolve(d),
        None => dirs::download_dir().ok_or_else(|| WinError::Unavailable("pasta Downloads".into()))?,
    };
    let path = if base.is_dir() || base.extension().is_none() { base.join(name) } else { base };
    Ok(unique(&path))
}

/// Nome do arquivo: `Content-Disposition` > último segmento da URL > "download".
fn filename_from(disposition: Option<&reqwest::header::HeaderValue>, url_path: &str) -> String {
    let from_header = disposition
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').find_map(|p| p.trim().strip_prefix("filename=")))
        .map(|s| s.trim_matches('"').to_string());
    let raw = from_header
        .or_else(|| url_path.rsplit('/').next().filter(|s| !s.is_empty()).map(str::to_string))
        .unwrap_or_else(|| "download".into());
    sanitize(&raw)
}

/// Remove separadores e caracteres proibidos no Windows (evita path traversal
/// vindo de um header malicioso).
fn sanitize(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { '_' } else { c })
        .collect();
    let clean = clean.trim_matches(|c| c == '.' || c == ' ').to_string();
    if clean.is_empty() || Path::new(&clean).components().count() != 1 {
        "download".into()
    } else {
        clean
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    #[test]
    fn filename_sources() {
        let h = HeaderValue::from_static("attachment; filename=\"setup.exe\"");
        assert_eq!(filename_from(Some(&h), "/x/y"), "setup.exe");
        assert_eq!(filename_from(None, "/files/vlc-3.0.exe"), "vlc-3.0.exe");
        assert_eq!(filename_from(None, "/"), "download");
    }

    #[test]
    fn sanitize_blocks_traversal() {
        assert_eq!(sanitize("..\\..\\Windows\\evil.dll"), "_.._Windows_evil.dll");
        assert_eq!(sanitize(".."), "download");
    }
}
