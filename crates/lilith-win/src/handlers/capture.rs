//! `screen.capture`: tira um print — de todos os monitores (padrão), de um
//! monitor ou de uma janela — salva em Imagens\Capturas de Tela e copia para a
//! área de transferência, pronto para colar. Tudo decidido por código.

use super::blocking;
use crate::ocr::{self, Capture};
use crate::window::{self, Rect};
use crate::WinError;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// A área de trabalho virtual inteira (todos os monitores).
fn virtual_screen() -> Rect {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    };
    unsafe {
        Rect {
            x: GetSystemMetrics(SM_XVIRTUALSCREEN),
            y: GetSystemMetrics(SM_YVIRTUALSCREEN),
            w: GetSystemMetrics(SM_CXVIRTUALSCREEN),
            h: GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

/// Pasta dos prints: a "Capturas de Tela" do Windows (Imagens\Screenshots).
fn screenshots_dir() -> PathBuf {
    dirs::picture_dir().unwrap_or_else(std::env::temp_dir).join("Screenshots")
}

/// Nome do arquivo: "Lilith 2026-10-16 14-30-05 (monitor 1).png".
pub(crate) fn file_name(source: &str, now: chrono::DateTime<chrono::Local>) -> String {
    let safe: String = source
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let safe: String = safe.chars().take(40).collect();
    format!("Lilith {} ({safe}).png", now.format("%Y-%m-%d %H-%M-%S"))
}

/// BGRA (de cima para baixo) → RGBA opaco.
pub(crate) fn to_rgba(c: &Capture) -> Vec<u8> {
    let mut out = Vec::with_capacity(c.bgra.len());
    for px in c.bgra.chunks_exact(4) {
        out.extend_from_slice(&[px[2], px[1], px[0], 255]);
    }
    out
}

fn save_png(path: &Path, c: &Capture, rgba: &[u8]) -> Result<(), WinError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| WinError::Failed(e.to_string()))?;
    }
    let file = std::fs::File::create(path).map_err(|e| WinError::Failed(e.to_string()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), c.width, c.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| WinError::Failed(e.to_string()))?;
    w.write_image_data(rgba).map_err(|e| WinError::Failed(e.to_string()))
}

fn copy_to_clipboard(c: &Capture, rgba: Vec<u8>) -> bool {
    arboard::Clipboard::new()
        .and_then(|mut cb| {
            cb.set_image(arboard::ImageData {
                width: c.width as usize,
                height: c.height as usize,
                bytes: std::borrow::Cow::Owned(rgba),
            })
        })
        .is_ok()
}

/// `target` (janela/app), ou `monitor` (1, 2…), ou nada = todos os monitores.
pub async fn capture(params: Value) -> Result<Value, WinError> {
    blocking(move || {
        let (source, cap) = if let Some(target) = params["target"].as_str().filter(|t| !t.trim().is_empty()) {
            let w = window::find(target)?;
            if w.minimized {
                return Err(WinError::Unavailable(format!("'{}' está minimizada; abra ela antes do print", w.title)));
            }
            (w.app.clone(), ocr::capture_window(w.id, w.frame)?)
        } else if let Some(n) = params["monitor"].as_u64() {
            let mons = window::monitors();
            let m = mons
                .iter()
                .find(|m| m.index == n as usize)
                .ok_or_else(|| WinError::NotFound(format!("monitor {n} (há {})", mons.len())))?;
            (format!("monitor {n}"), ocr::capture_rect(m.full)?)
        } else {
            ("todos os monitores".to_string(), ocr::capture_rect(virtual_screen())?)
        };
        let rgba = to_rgba(&cap);
        let path = screenshots_dir().join(file_name(&source, chrono::Local::now()));
        save_png(&path, &cap, &rgba)?;
        let copied = copy_to_clipboard(&cap, rgba);
        Ok(json!({
            "path": path.display().to_string(),
            "source": source,
            "width": cap.width,
            "height": cap.height,
            "copied": copied,
        }))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_safe_and_dated() {
        let now = chrono::Local::now();
        let name = file_name("Discord: #geral / chat", now);
        assert!(name.starts_with("Lilith ") && name.ends_with("(Discord geral chat).png"), "{name}");
        assert!(!name.contains(':') && !name.contains('/'));
    }

    #[test]
    fn converts_bgra_to_opaque_rgba() {
        let c = Capture { bgra: vec![10, 20, 30, 0, 1, 2, 3, 0], width: 2, height: 1 };
        assert_eq!(to_rgba(&c), vec![30, 20, 10, 255, 3, 2, 1, 255]);
    }

    /// Salva um PNG de verdade numa pasta temporária e confere o cabeçalho.
    #[test]
    fn writes_a_valid_png() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.png");
        let c = Capture { bgra: vec![255; 4 * 4 * 3], width: 4, height: 3 };
        save_png(&path, &c, &to_rgba(&c)).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    }
}
