use super::blocking;
use super::web::{http_client, public_get};
use crate::paths::resolve;
use crate::{req_str, WinError};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use windows::core::HSTRING;
use windows::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SPI_SETDESKWALLPAPER,
};

/// Troca o wallpaper. `source` é URL (baixa), caminho local (copia) ou termo
/// de busca (primeiro resultado do Wallhaven). Lógica portada de
/// `jarvis-os/system/action-bus/src/handlers/desktop.rs`; só a aplicação
/// mudou (swaybg → `SystemParametersInfoW`).
pub async fn set_wallpaper(params: Value) -> Result<Value, WinError> {
    let source = req_str(&params, "source")?;
    let dir = wallpaper_dir()?;
    tokio::fs::create_dir_all(&dir).await?;

    let (via, src_url_or_path) = if source.contains("://") {
        ("url", source.to_string())
    } else if resolve(source).is_file() {
        ("file", source.to_string())
    } else {
        ("search", wallhaven_top(source).await?)
    };

    // Nome novo a cada troca: o Windows faz cache pelo caminho e às vezes não
    // atualiza se o arquivo for sobrescrito no mesmo lugar.
    let ext = Path::new(&src_url_or_path)
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| ["jpg", "jpeg", "png", "bmp", "webp"].contains(&e.to_lowercase().as_str()))
        .unwrap_or("jpg")
        .to_lowercase();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let dest = dir.join(format!("wallpaper-{stamp}.{ext}"));

    if via == "file" {
        tokio::fs::copy(resolve(&src_url_or_path), &dest).await?;
    } else {
        download_to(&src_url_or_path, &dest).await?;
    }

    let apply = dest.clone();
    blocking(move || unsafe {
        let wide = HSTRING::from(apply.as_os_str());
        SystemParametersInfoW(
            SPI_SETDESKWALLPAPER,
            0,
            Some(wide.as_ptr() as *mut _),
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
        )
        .map_err(WinError::from)
    })
    .await?;

    prune_old(&dir, &dest).await;
    Ok(json!({ "set": true, "via": via, "path": dest }))
}

async fn download_to(url: &str, dest: &Path) -> Result<(), WinError> {
    // Wallpaper é scope seguro: a URL do modelo não pode alcançar a rede local.
    let bytes = public_get(url)
        .await?
        .bytes()
        .await
        .map_err(|e| WinError::Failed(format!("leitura: {e}")))?;
    tokio::fs::write(dest, &bytes).await?;
    Ok(())
}

/// URL da imagem do primeiro resultado do Wallhaven (API JSON, sem chave, SFW).
async fn wallhaven_top(query: &str) -> Result<String, WinError> {
    let v: Value = http_client()?
        .get("https://wallhaven.cc/api/v1/search")
        .query(&[("q", query), ("sorting", "relevance"), ("order", "desc")])
        .send()
        .await
        .map_err(|e| WinError::Unavailable(format!("wallhaven: {e}")))?
        .error_for_status()
        .map_err(|e| WinError::Failed(format!("wallhaven: {e}")))?
        .json()
        .await
        .map_err(|e| WinError::Failed(format!("wallhaven json: {e}")))?;
    v["data"][0]["path"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| WinError::NotFound(format!("wallpaper para '{query}'")))
}

fn wallpaper_dir() -> Result<PathBuf, WinError> {
    dirs::data_dir()
        .map(|d| d.join("Lilith").join("wallpapers"))
        .ok_or_else(|| WinError::Unavailable("pasta AppData".into()))
}

/// Mantém só o wallpaper atual na pasta.
async fn prune_old(dir: &Path, keep: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else { return };
    while let Ok(Some(e)) = entries.next_entry().await {
        let p = e.path();
        if p != keep && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("wallpaper-")) {
            let _ = tokio::fs::remove_file(p).await;
        }
    }
}
