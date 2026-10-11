use super::blocking;
use crate::{req_str, WinError};
use serde_json::{json, Value};

fn board() -> Result<arboard::Clipboard, WinError> {
    arboard::Clipboard::new().map_err(|e| WinError::Unavailable(format!("clipboard: {e}")))
}

pub async fn set(params: Value) -> Result<Value, WinError> {
    let text = req_str(&params, "text")?.to_string();
    let len = text.chars().count();
    blocking(move || {
        board()?
            .set_text(text)
            .map_err(|e| WinError::Failed(format!("clipboard: {e}")))
    })
    .await?;
    Ok(json!({ "copied_chars": len }))
}

pub async fn get() -> Result<Value, WinError> {
    let text = blocking(|| {
        board()?
            .get_text()
            .map_err(|e| WinError::NotFound(format!("texto no clipboard: {e}")))
    })
    .await?;
    Ok(json!({ "text": text }))
}
