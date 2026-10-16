//! `screen.read`: lê o texto de uma janela ou de um monitor por OCR.

use super::blocking;
use crate::{ocr, paths, window, WinError};
use serde_json::{json, Value};

pub async fn read(params: Value) -> Result<Value, WinError> {
    blocking(move || {
        let max = params["max_chars"].as_u64().unwrap_or(6000).clamp(200, 20000) as usize;
        let (source, cap) = match params["target"].as_str().filter(|t| !t.trim().is_empty()) {
            Some(target) => {
                let w = window::find(target)?;
                if w.minimized {
                    return Err(WinError::Unavailable(format!("'{}' está minimizada; restaure antes de ler", w.title)));
                }
                (w.title.clone(), ocr::capture_window(w.id, w.frame)?)
            }
            None => {
                let n = params["monitor"].as_u64().unwrap_or(1) as usize;
                let mons = window::monitors();
                let m = mons
                    .iter()
                    .find(|m| m.index == n)
                    .ok_or_else(|| WinError::NotFound(format!("monitor {n} (há {})", mons.len())))?;
                (format!("monitor {n}"), ocr::capture_rect(m.work)?)
            }
        };
        let lines = ocr::recognize(&cap)?;
        Ok(shape(&source, &lines, params["query"].as_str(), max))
    })
    .await
}

/// Monta a resposta: texto (cortado) e, com busca, só as linhas que casam.
pub(crate) fn shape(source: &str, lines: &[String], query: Option<&str>, max: usize) -> Value {
    let mut text = lines.join("\n");
    if text.chars().count() > max {
        text = text.chars().take(max).collect::<String>() + "…";
    }
    let mut out = json!({ "source": source, "via": "ocr", "lines": lines.len(), "text": text });
    if let Some(q) = query.map(str::trim).filter(|q| !q.is_empty()) {
        let needle = paths::fold(q);
        let matches: Vec<&String> = lines.iter().filter(|l| paths::fold(l).contains(&needle)).take(30).collect();
        out["query"] = json!(q);
        out["matches"] = json!(matches);
    }
    if lines.is_empty() {
        out["note"] = json!("não reconheci nenhum texto nessa área");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_filters_by_query_and_cuts_long_text() {
        let lines = vec!["Arquivo salvo".to_string(), "Erro CS0103: nome não existe".to_string()];
        let v = shape("monitor 1", &lines, Some("erro"), 6000);
        assert_eq!(v["matches"].as_array().unwrap().len(), 1);
        assert_eq!(v["lines"], 2);
        let long = vec!["x".repeat(500)];
        assert!(shape("w", &long, None, 200)["text"].as_str().unwrap().ends_with('…'));
        assert!(shape("w", &[], None, 200)["note"].is_string());
    }
}
