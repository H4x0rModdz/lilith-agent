//! Extrai texto legível da saída JSONL de cada CLI. Best-effort: formatos
//! mudam entre versões, então evento desconhecido é ignorado (nunca quebra).

use serde_json::Value;

#[derive(Debug, Default, PartialEq)]
pub struct Parsed {
    /// Linha para o log do painel.
    pub display: Option<String>,
    /// Resposta final do agente, se esta linha a contém.
    pub result: Option<String>,
}

pub fn line(format: &str, raw: &str) -> Parsed {
    let raw = raw.trim();
    if raw.is_empty() {
        return Parsed::default();
    }
    match format {
        "claude" | "codex" => match serde_json::from_str::<Value>(raw) {
            Ok(v) if format == "claude" => claude(&v),
            Ok(v) => codex(&v),
            Err(_) => Parsed { display: Some(raw.to_string()), result: None },
        },
        _ => Parsed { display: Some(raw.to_string()), result: Some(raw.to_string()) },
    }
}

/// `claude -p --output-format stream-json --verbose`
fn claude(v: &Value) -> Parsed {
    match v["type"].as_str() {
        Some("assistant") => {
            let parts: Vec<String> = v["message"]["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| match c["type"].as_str() {
                    Some("text") => c["text"].as_str().map(|t| t.trim().to_string()),
                    Some("tool_use") => Some(format!("🔧 {}{}", c["name"].as_str().unwrap_or("tool"), tool_hint(&c["input"]))),
                    _ => None,
                })
                .filter(|s| !s.is_empty())
                .collect();
            Parsed { display: (!parts.is_empty()).then(|| parts.join("\n")), result: None }
        }
        Some("result") => Parsed {
            display: None,
            result: v["result"].as_str().map(str::to_string),
        },
        _ => Parsed::default(),
    }
}

/// `codex exec --json`
fn codex(v: &Value) -> Parsed {
    let item = &v["item"];
    match (v["type"].as_str(), item["type"].as_str()) {
        (Some("item.completed"), Some("agent_message")) => {
            let text = item["text"].as_str().map(|t| t.trim().to_string());
            Parsed { display: text.clone(), result: text }
        }
        (Some("item.started"), Some("command_execution")) => Parsed {
            display: item["command"].as_str().map(|c| format!("🔧 {c}")),
            result: None,
        },
        (Some("item.completed"), Some("file_change")) => Parsed {
            display: Some(format!(
                "📝 {}",
                item["changes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c["path"].as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            result: None,
        },
        (Some("error"), _) => Parsed {
            display: v["message"].as_str().map(|m| format!("⚠ {m}")),
            result: None,
        },
        _ => Parsed::default(),
    }
}

/// Dica curta do que a ferramenta do Claude está mexendo (arquivo/comando).
fn tool_hint(input: &Value) -> String {
    ["file_path", "command", "pattern", "path", "url"]
        .iter()
        .find_map(|k| input[*k].as_str())
        .map(|s| {
            let short: String = s.chars().take(80).collect();
            format!(": {short}")
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_stream() {
        let a = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Vou ler o arquivo."},{"type":"tool_use","name":"Read","input":{"file_path":"C:\\x\\main.rs"}}]}}"#;
        assert_eq!(line("claude", a).display.unwrap(), "Vou ler o arquivo.\n🔧 Read: C:\\x\\main.rs");
        let r = r#"{"type":"result","subtype":"success","result":"Pronto, corrigi o bug."}"#;
        assert_eq!(line("claude", r).result.unwrap(), "Pronto, corrigi o bug.");
        assert_eq!(line("claude", r#"{"type":"system","subtype":"init"}"#), Parsed::default());
    }

    #[test]
    fn codex_stream() {
        let m = r#"{"type":"item.completed","item":{"id":"1","type":"agent_message","text":"Feito."}}"#;
        let p = line("codex", m);
        assert_eq!(p.display.as_deref(), Some("Feito."));
        assert_eq!(p.result.as_deref(), Some("Feito."));
        let c = r#"{"type":"item.started","item":{"type":"command_execution","command":"cargo test"}}"#;
        assert_eq!(line("codex", c).display.unwrap(), "🔧 cargo test");
    }

    #[test]
    fn non_json_is_shown_raw() {
        assert_eq!(line("claude", "warning: algo").display.unwrap(), "warning: algo");
    }
}
