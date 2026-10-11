//! Aprendizado contínuo: depois de cada pedido, uma chamada curta extrai
//! fatos duradouros que o usuário disse sobre si (nome, preferências,
//! rotina, como quer ser tratado).
//!
//! Só olha o que o usuário DIGITOU no chat — nunca conteúdo lido de janelas,
//! arquivos ou web —, então um texto injetado não vira "memória" permanente.

use crate::ollama::Ollama;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LearnedFact {
    pub key: String,
    pub value: String,
}

#[derive(Deserialize)]
struct Extraction {
    #[serde(default)]
    facts: Vec<LearnedFact>,
}

const PROMPT: &str = "\
Você extrai fatos DURADOUROS que o usuário disse sobre ele mesmo, para a assistente \
lembrar no futuro: nome ou apelido, como quer ser chamado, gostos, rotina, trabalho, \
projetos, pessoas próximas, preferências de como ser tratado.
Ignore pedidos e comandos (\"abre o spotify\", \"lê o chat\"), perguntas e coisas \
passageiras (\"tô com fome\"). Na dúvida, não extraia: a lista quase sempre é vazia.
`key` curta em português, minúscula (ex.: \"apelido\", \"time\", \"linguagem favorita\"); \
`value` curto.";

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "facts": {
                "type": "array",
                "maxItems": 3,
                "items": {
                    "type": "object",
                    "properties": { "key": { "type": "string" }, "value": { "type": "string" } },
                    "required": ["key", "value"]
                }
            }
        },
        "required": ["facts"]
    })
}

/// Fatos novos na fala do usuário (normalmente nenhum). Best-effort.
pub async fn extract(ollama: &dyn Ollama, user_text: &str) -> Vec<LearnedFact> {
    // Comandos curtos não trazem fatos; poupa a GPU.
    if user_text.split_whitespace().count() < 4 {
        return Vec::new();
    }
    let messages = vec![
        json!({ "role": "system", "content": PROMPT }),
        json!({ "role": "user", "content": user_text }),
    ];
    let Ok(raw) = ollama.structured(&messages, &schema()).await else {
        return Vec::new();
    };
    serde_json::from_value::<Extraction>(raw)
        .map(|e| {
            e.facts
                .into_iter()
                .map(|f| LearnedFact { key: f.key.trim().to_lowercase(), value: f.value.trim().to_string() })
                .filter(|f| !f.key.is_empty() && !f.value.is_empty() && f.key.len() <= 40 && f.value.len() <= 200)
                .collect()
        })
        .unwrap_or_default()
}
