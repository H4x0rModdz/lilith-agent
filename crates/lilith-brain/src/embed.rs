//! Embeddings locais pelo Ollama (`/api/embed`) para a busca nos documentos.
//! O modelo de embeddings é opcional: sem ele, a busca cai para palavras
//! (FTS5). Ver se está instalado é só leitura (`/api/tags`, o mesmo que
//! `ollama list`); baixar (`/api/pull`) só quando o usuário pede nas
//! Configurações.

use crate::error::LilithError;
use crate::ollama::normalize_host;
use serde_json::{json, Value};

/// Modelo padrão: pequeno (≈270 MB), roda bem em CPU ou GPU modesta.
pub const DEFAULT_EMBED_MODEL: &str = "nomic-embed-text";

#[derive(Clone)]
pub struct Embedder {
    host: String,
    model: String,
    http: reqwest::Client,
}

/// O nome instalado corresponde ao pedido? ("nomic-embed-text" casa com
/// "nomic-embed-text:latest").
pub fn same_model(installed: &str, wanted: &str) -> bool {
    let base = |s: &str| {
        let s = s.trim().to_lowercase();
        match s.strip_suffix(":latest") {
            Some(b) => b.to_string(),
            None => s,
        }
    };
    base(installed) == base(wanted)
}

impl Embedder {
    pub fn new(host: &str, model: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("reqwest client");
        Self { host: normalize_host(host), model: model.trim().to_string(), http }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// Modelos instalados no Ollama (como `ollama list`).
    pub async fn installed_models(&self) -> Result<Vec<String>, LilithError> {
        let resp = self
            .http
            .get(format!("{}/api/tags", self.host))
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| LilithError::OllamaUnreachable(e.to_string()))?;
        let v: Value = resp.json().await.map_err(|e| LilithError::OllamaInvalid(e.to_string()))?;
        Ok(v["models"]
            .as_array()
            .map(|a| a.iter().filter_map(|m| m["name"].as_str().map(str::to_string)).collect())
            .unwrap_or_default())
    }

    /// O modelo de embeddings está instalado? (Ollama fora do ar = não.)
    pub async fn available(&self) -> bool {
        self.installed_models().await.is_ok_and(|m| m.iter().any(|n| same_model(n, &self.model)))
    }

    /// Um vetor por texto, na mesma ordem.
    pub async fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, LilithError> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let body = json!({ "model": self.model, "input": inputs, "truncate": true });
        let resp = self
            .http
            .post(format!("{}/api/embed", self.host))
            .json(&body)
            .send()
            .await
            .map_err(|e| LilithError::OllamaUnreachable(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(LilithError::OllamaInvalid(format!("HTTP {}", resp.status())));
        }
        let v: Value = resp.json().await.map_err(|e| LilithError::OllamaInvalid(e.to_string()))?;
        let out: Vec<Vec<f32>> = v["embeddings"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .map(|r| r.as_array().map(|x| x.iter().filter_map(|n| n.as_f64()).map(|n| n as f32).collect()).unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default();
        if out.len() != inputs.len() || out.iter().any(Vec::is_empty) {
            return Err(LilithError::OllamaInvalid("embeddings incompletos".into()));
        }
        Ok(out)
    }

    /// Texto de um trecho de documento, com o prefixo que o modelo espera.
    pub fn document_input(&self, text: &str) -> String {
        match self.family() {
            Family::Nomic => format!("search_document: {text}"),
            Family::Gemma => format!("title: none | text: {text}"),
            Family::Other => text.to_string(),
        }
    }

    /// Texto da pergunta, com o prefixo que o modelo espera.
    pub fn query_input(&self, text: &str) -> String {
        match self.family() {
            Family::Nomic => format!("search_query: {text}"),
            Family::Gemma => format!("task: search result | query: {text}"),
            Family::Other => text.to_string(),
        }
    }

    fn family(&self) -> Family {
        let m = self.model.to_lowercase();
        if m.starts_with("nomic-embed") {
            Family::Nomic
        } else if m.starts_with("embeddinggemma") {
            Family::Gemma
        } else {
            Family::Other
        }
    }

    /// Baixa o modelo (`ollama pull`), avisando o progresso (concluído,
    /// total, etapa). Só roda quando o usuário pede.
    pub async fn pull(&self, mut progress: impl FnMut(u64, u64, &str)) -> Result<(), LilithError> {
        use futures_util::StreamExt;
        let resp = self
            .http
            .post(format!("{}/api/pull", self.host))
            .timeout(std::time::Duration::from_secs(60 * 60))
            .json(&json!({ "model": self.model, "stream": true }))
            .send()
            .await
            .map_err(|e| LilithError::OllamaUnreachable(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(LilithError::OllamaInvalid(format!("HTTP {}", resp.status())));
        }
        let mut stream = resp.bytes_stream();
        let mut buffer = String::new();
        while let Some(chunk) = stream.next().await {
            let bytes = chunk.map_err(|e| LilithError::OllamaInvalid(e.to_string()))?;
            buffer.push_str(&String::from_utf8_lossy(&bytes));
            while let Some(nl) = buffer.find('\n') {
                let line: String = buffer.drain(..=nl).collect();
                let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { continue };
                if let Some(err) = v["error"].as_str() {
                    return Err(LilithError::OllamaInvalid(err.to_string()));
                }
                progress(v["completed"].as_u64().unwrap_or(0), v["total"].as_u64().unwrap_or(0), v["status"].as_str().unwrap_or(""));
            }
        }
        Ok(())
    }
}

enum Family {
    Nomic,
    Gemma,
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_names_and_prefixes() {
        assert!(same_model("nomic-embed-text:latest", "nomic-embed-text"));
        assert!(same_model("Nomic-Embed-Text", "nomic-embed-text:latest"));
        assert!(!same_model("nomic-embed-text:v1.5", "nomic-embed-text"));
        let e = Embedder::new("127.0.0.1:11434", "nomic-embed-text");
        assert_eq!(e.query_input("férias"), "search_query: férias");
        assert_eq!(e.document_input("x"), "search_document: x");
        assert_eq!(Embedder::new("localhost", "bge-m3").query_input("x"), "x");
    }
}
