use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Uma tool = uma ação executável. `schema` é o JSON Schema enviado ao LLM.
#[derive(Debug, Clone, Serialize)]
pub struct Tool {
    pub action: &'static str,
    pub description: &'static str,
    pub schema: Value,
}

/// Chamada de tool já parseada, pronta para o dispatcher.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub action: String,
    pub params: Value,
}

fn obj(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required })
}

/// Catálogo de ações do Windows. Toda ação aqui precisa de scope em
/// `lilith-guard::scope_for` e de handler em `lilith-win::dispatch` — o teste
/// `catalog_is_wired` no app garante isso.
pub fn all_tools() -> Vec<Tool> {
    vec![
        // ── apps ────────────────────────────────────────────────────────
        Tool {
            action: "app.open",
            description: "Abre um aplicativo pelo nome (ex.: 'spotify', 'discord', 'bloco de notas'). \
                          Procura nos atalhos do Menu Iniciar.",
            schema: obj(json!({ "app": { "type": "string" } }), &["app"]),
        },
        Tool {
            action: "app.close",
            description: "Fecha um aplicativo em execução pelo nome do processo (ex.: 'chrome', 'spotify').",
            schema: obj(
                json!({
                    "app": { "type": "string" },
                    "force": { "type": "boolean", "description": "Forçar encerramento" }
                }),
                &["app"],
            ),
        },
        Tool {
            action: "browser.open",
            description: "Abre uma URL no navegador padrão.",
            schema: obj(json!({ "url": { "type": "string" } }), &["url"]),
        },
        // ── arquivos ────────────────────────────────────────────────────
        Tool {
            action: "file.open",
            description: "Abre um arquivo ou pasta com o programa padrão do Windows.",
            schema: obj(json!({ "path": { "type": "string" } }), &["path"]),
        },
        Tool {
            action: "file.find",
            description: "Procura arquivos/pastas pelo nome (busca parcial, sem diferenciar maiúsculas). \
                          `root` opcional; padrão: pasta do usuário.",
            schema: obj(
                json!({
                    "name": { "type": "string" },
                    "root": { "type": "string" },
                    "limit": { "type": "integer" }
                }),
                &["name"],
            ),
        },
        Tool {
            action: "file.list",
            description: "Lista o conteúdo de uma pasta.",
            schema: obj(json!({ "path": { "type": "string" } }), &["path"]),
        },
        Tool {
            action: "file.move",
            description: "Move ou renomeia um arquivo/pasta.",
            schema: obj(
                json!({ "source": { "type": "string" }, "destination": { "type": "string" } }),
                &["source", "destination"],
            ),
        },
        Tool {
            action: "file.copy",
            description: "Copia um arquivo.",
            schema: obj(
                json!({ "source": { "type": "string" }, "destination": { "type": "string" } }),
                &["source", "destination"],
            ),
        },
        Tool {
            action: "file.delete",
            description: "Manda um arquivo/pasta para a Lixeira.",
            schema: obj(json!({ "path": { "type": "string" } }), &["path"]),
        },
        Tool {
            action: "download.file",
            description: "Baixa um arquivo de uma URL para a pasta Downloads (ou `destination`).",
            schema: obj(
                json!({
                    "url": { "type": "string" },
                    "destination": { "type": "string", "description": "Pasta ou caminho final (opcional)" }
                }),
                &["url"],
            ),
        },
        // ── desktop / sistema ───────────────────────────────────────────
        Tool {
            action: "desktop.set_wallpaper",
            description: "Troca o papel de parede. `source` pode ser caminho de imagem, URL, \
                          ou um termo de busca (ex.: 'gato preto', 'cyberpunk rosa').",
            schema: obj(json!({ "source": { "type": "string" } }), &["source"]),
        },
        Tool {
            action: "audio.set_volume",
            description: "Define o volume geral (0 a 100).",
            schema: obj(json!({ "level": { "type": "integer", "minimum": 0, "maximum": 100 } }), &["level"]),
        },
        Tool {
            action: "audio.get_volume",
            description: "Lê o volume geral atual e se está mudo.",
            schema: obj(json!({}), &[]),
        },
        Tool {
            action: "audio.set_mute",
            description: "Liga/desliga o mudo.",
            schema: obj(json!({ "muted": { "type": "boolean" } }), &["muted"]),
        },
        Tool {
            action: "clipboard.set",
            description: "Copia um texto para a área de transferência.",
            schema: obj(json!({ "text": { "type": "string" } }), &["text"]),
        },
        Tool {
            action: "clipboard.get",
            description: "Lê o texto da área de transferência.",
            schema: obj(json!({}), &[]),
        },
        Tool {
            action: "system.power",
            description: "Bloquear, suspender, reiniciar ou desligar o PC.",
            schema: obj(
                json!({ "op": { "type": "string", "enum": ["lock", "sleep", "restart", "shutdown"] } }),
                &["op"],
            ),
        },
        // ── agentes de código ───────────────────────────────────────────
        Tool {
            action: "agent.delegate",
            description: "Delega uma tarefa de programação, terminal ou configuração a um agente de código                           que roda em segundo plano (ex.: 'claude' = Claude Code, 'codex' = Codex).                           Escreva em `task` um prompt completo e autossuficiente, como se fosse para um                           dev que não viu a conversa. `folder` é a pasta do projeto. Retorna o id do job;                           o usuário acompanha pelo painel da Lilith.",
            schema: obj(
                json!({
                    "agent": { "type": "string", "description": "Nome do agente: 'claude' ou 'codex'" },
                    "task": { "type": "string" },
                    "folder": { "type": "string", "description": "Pasta de trabalho (padrão: pasta do usuário)" }
                }),
                &["agent", "task"],
            ),
        },
        Tool {
            action: "agent.jobs",
            description: "Lista as tarefas delegadas a agentes de código e o status/resultado de cada uma.",
            schema: obj(json!({}), &[]),
        },
        // ── web ─────────────────────────────────────────────────────────
        Tool {
            action: "web.search",
            description: "Pesquisa na web. Retorna títulos, URLs e trechos.",
            schema: obj(
                json!({ "query": { "type": "string" }, "limit": { "type": "integer" } }),
                &["query"],
            ),
        },
        Tool {
            action: "web.fetch",
            description: "Lê o texto de uma página web (para resumir ou extrair informação).",
            schema: obj(json!({ "url": { "type": "string" } }), &["url"]),
        },
    ]
}

/// Formato do array `tools` do `/api/chat` do Ollama (compatível com OpenAI).
pub fn ollama_tools_payload(tools: &[Tool]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": { "name": t.action, "description": t.description, "parameters": t.schema }
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_are_unique() {
        let tools = all_tools();
        let mut names: Vec<_> = tools.iter().map(|t| t.action).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), tools.len());
    }
}
