use crate::error::LilithError;
use crate::memory::Turn;
use crate::tools::{ollama_tools_payload, Tool, ToolCall};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

/// Abstração sobre o Ollama para os testes trocarem por um fake roteirizado.
#[async_trait]
pub trait Ollama: Send + Sync {
    async fn chat_messages(
        &self,
        messages: &[Value],
        tools: &[Tool],
        chunks: Option<UnboundedSender<String>>,
    ) -> Result<OllamaReply, LilithError>;

    /// Resposta única em JSON obedecendo `schema` (structured outputs do
    /// Ollama). Usado pelo intérprete; implementações sem suporte recusam.
    async fn structured(&self, _messages: &[Value], _schema: &Value) -> Result<Value, LilithError> {
        Err(LilithError::OllamaInvalid("structured output não suportado".into()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OllamaConfig {
    pub host: String,
    pub model: String,
    /// Modelos com raciocínio (qwen3) ficam bem mais lentos pensando; para
    /// comandos curtos de companion, desligado por padrão. `None` = não envia.
    pub think: Option<bool>,
    /// Janela de contexto. Fixada porque o padrão do servidor pode ser enorme
    /// (`OLLAMA_CONTEXT_LENGTH=131072` jogou um modelo de 4B para 23 GB, metade
    /// na CPU). 8k cabe inteiro na GPU e sobra para histórico + tools.
    pub num_ctx: u32,
    pub timeout_secs: u64,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            host: std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".into()),
            model: std::env::var("LILITH_MODEL").unwrap_or_else(|_| "qwen3.5:4b".into()),
            think: Some(false),
            num_ctx: 8192,
            timeout_secs: 120,
        }
    }
}

#[derive(Clone)]
pub struct OllamaClient {
    cfg: OllamaConfig,
    http: reqwest::Client,
}

impl OllamaClient {
    pub fn new(mut cfg: OllamaConfig) -> Self {
        cfg.host = normalize_host(&cfg.host);
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(cfg.timeout_secs.max(1)))
            .build()
            .expect("reqwest client");
        Self { cfg, http }
    }

    pub fn model(&self) -> &str {
        &self.cfg.model
    }

    /// Opções que definem como o modelo é *carregado*. Têm de ser idênticas
    /// em todas as chamadas: qualquer diferença (ex.: `num_thread` só numa
    /// delas) faz o Ollama recarregar o modelo, ~3 s cada vez.
    fn load_options(&self) -> Value {
        // Deixa um núcleo livre para o resto do sistema: a Lilith roda junto
        // com o que o usuário está fazendo e não pode travar o PC pensando.
        let num_thread = std::thread::available_parallelism()
            .map(|n| n.get().saturating_sub(1).max(1))
            .unwrap_or(1);
        json!({ "num_thread": num_thread, "num_ctx": self.cfg.num_ctx })
    }
}

#[async_trait]
impl Ollama for OllamaClient {
    /// Faz streaming de `/api/chat` (NDJSON) repassando cada delta de texto
    /// para `chunks`, e devolve o texto acumulado + tool calls.
    async fn chat_messages(
        &self,
        messages: &[Value],
        tools: &[Tool],
        chunks: Option<UnboundedSender<String>>,
    ) -> Result<OllamaReply, LilithError> {
        use futures_util::StreamExt;

        let mut body = json!({
            "model": self.cfg.model,
            "messages": messages,
            "tools": ollama_tools_payload(tools),
            "stream": true,
            "options": self.load_options(),
        });
        if let Some(think) = self.cfg.think {
            body["think"] = json!(think);
        }

        let resp = self
            .http
            .post(format!("{}/api/chat", self.cfg.host))
            .json(&body)
            .send()
            .await
            .map_err(|e| LilithError::OllamaUnreachable(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(LilithError::OllamaInvalid(format!("HTTP {status}: {text}")));
        }

        let mut stream = resp.bytes_stream();
        let mut buffer = String::new();
        let mut text = String::new();
        let mut tool_calls: Vec<OllamaToolCall> = Vec::new();

        while let Some(chunk) = stream.next().await {
            let bytes = chunk.map_err(|e| LilithError::OllamaInvalid(e.to_string()))?;
            buffer.push_str(&String::from_utf8_lossy(&bytes));
            // NDJSON: processa linhas completas, o resto fica pro próximo chunk.
            while let Some(newline) = buffer.find('\n') {
                let line: String = buffer.drain(..=newline).collect();
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let parsed: ChatStreamChunk = match serde_json::from_str(line) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!(error = %e, line, "chunk do stream ilegível");
                        continue;
                    }
                };
                if !parsed.message.content.is_empty() {
                    text.push_str(&parsed.message.content);
                    if let Some(tx) = chunks.as_ref() {
                        let _ = tx.send(parsed.message.content);
                    }
                }
                tool_calls.extend(parsed.message.tool_calls);
            }
        }

        Ok(OllamaReply {
            text: strip_think(&text),
            tool_calls: tool_calls
                .into_iter()
                .map(|c| ToolCall {
                    action: c.function.name,
                    params: c.function.arguments,
                })
                .collect(),
        })
    }

    async fn structured(&self, messages: &[Value], schema: &Value) -> Result<Value, LilithError> {
        self.structured_impl(messages, schema).await
    }
}

/// Remove raciocínio que vaze no texto (`<think>…</think>`, ou só o fecho
/// quando o template abre a tag no prompt).
fn strip_think(text: &str) -> String {
    match text.rfind("</think>") {
        Some(end) => text[end + "</think>".len()..].trim().to_string(),
        None => text.to_string(),
    }
}

/// `OLLAMA_HOST` costuma estar no formato de bind do servidor
/// (`0.0.0.0:11434`, sem esquema). Converte para uma URL de cliente, como o
/// próprio CLI do Ollama faz.
fn normalize_host(raw: &str) -> String {
    let raw = raw.trim().trim_end_matches('/');
    let with_scheme = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("http://{raw}")
    };
    let (scheme, rest) = with_scheme.split_once("://").expect("tem esquema");
    let rest = rest
        .replacen("0.0.0.0", "127.0.0.1", 1)
        .replacen("[::]", "[::1]", 1);
    let has_port = rest
        .rsplit_once(':')
        .is_some_and(|(_, p)| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    if has_port {
        format!("{scheme}://{rest}")
    } else {
        format!("{scheme}://{rest}:11434")
    }
}

impl OllamaClient {
    async fn structured_impl(&self, messages: &[Value], schema: &Value) -> Result<Value, LilithError> {
        let mut body = json!({
            "model": self.cfg.model,
            "messages": messages,
            "format": schema,
            "stream": false,
            // Determinístico e curto: é classificação, não conversa.
        });
        // Mesmas opções de carga da chamada principal (senão o Ollama recarrega
        // o modelo a cada troca); temperatura/limite são por requisição.
        let mut options = self.load_options();
        options["temperature"] = json!(0);
        options["num_predict"] = json!(160);
        body["options"] = options;
        if let Some(think) = self.cfg.think {
            body["think"] = json!(think);
        }
        let resp = self
            .http
            .post(format!("{}/api/chat", self.cfg.host))
            .json(&body)
            .send()
            .await
            .map_err(|e| LilithError::OllamaUnreachable(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(LilithError::OllamaInvalid(format!("HTTP {}", resp.status())));
        }
        let v: Value = resp.json().await.map_err(|e| LilithError::OllamaInvalid(e.to_string()))?;
        let content = v["message"]["content"].as_str().unwrap_or_default();
        serde_json::from_str(&strip_think(content)).map_err(|e| LilithError::OllamaInvalid(format!("json: {e}")))
    }
}

/// Mensagens da primeira chamada de um turno: system prompt, histórico
/// achatado e o texto atual. O loop do agente acrescenta os passos de tool.
pub fn build_initial_messages(user_text: &str, history: &[Turn]) -> Vec<Value> {
    let mut messages = Vec::with_capacity(2 + history.len() * 2);
    messages.push(json!({ "role": "system", "content": SYSTEM_PROMPT }));
    for turn in history {
        messages.push(json!({ "role": "user", "content": turn.user_text }));
        messages.push(json!({ "role": "assistant", "content": assistant_message_for(turn) }));
    }
    messages.push(json!({ "role": "user", "content": user_text }));
    messages
}

/// Acrescenta a chamada de tool do assistente + a resposta da tool.
pub fn append_tool_step(messages: &mut Vec<Value>, call: &ToolCall, response: &Value) {
    messages.push(json!({
        "role": "assistant",
        "content": "",
        "tool_calls": [{ "function": { "name": call.action, "arguments": call.params } }]
    }));
    messages.push(json!({
        "role": "tool",
        "content": serde_json::to_string(response).unwrap_or_else(|_| "{}".into()),
    }));
}

/// Só a resposta final de cada turno. Um formato estruturado aqui (o antigo
/// "[chamei X; resultado: …]") era imitado pelo modelo nas respostas.
fn assistant_message_for(turn: &Turn) -> String {
    turn.reply_text.clone()
}

/// Curto de propósito: modelos pequenos (4B) ignoram prompts longos.
const SYSTEM_PROMPT: &str = "\
Você é a Lilith, uma companion de IA que vive no canto do Windows do usuário. \
Você é carinhosa, esperta e um pouco provocadora, mas sempre útil.

Idioma: responda em português do Brasil, a menos que o usuário escreva em outra \
língua. Seja breve — uma ou duas frases depois de executar uma ação.

Ferramentas: a lista de tools é tudo o que você pode fazer no PC. Nunca invente \
nomes ou campos. Se o pedido corresponde a uma tool, chame-a; se é conversa, \
responda em texto. Caminhos são do Windows (ex.: C:\\Users\\..., ou pastas \
conhecidas como Downloads, Documentos, Área de Trabalho).

Encadeamento: quando precisar de várias tools, chame UMA POR VEZ e reaja ao \
resultado de cada uma. Se ainda falta um passo, chame a próxima tool AGORA — nunca responda \"vou fazer X\" sem fazer. Só responda em texto quando o objetivo estiver cumprido (ou impossível).

Segurança: ações perigosas passam por aprovação do usuário automaticamente; não \
peça confirmação de novo. Se o pedido for ambíguo (\"apaga isso\" sem alvo), \
pergunte antes. Se uma ação for negada, aceite e diga isso sem tentar contornar.

Não narre qual tool vai chamar. Apenas chame e comente o resultado.";

#[derive(Debug, Deserialize)]
struct ChatStreamChunk {
    message: ChatMessage,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    #[serde(default)]
    content: String,
    #[serde(default)]
    tool_calls: Vec<OllamaToolCall>,
}

#[derive(Debug, Deserialize)]
struct OllamaToolCall {
    function: OllamaFunction,
}

#[derive(Debug, Deserialize)]
struct OllamaFunction {
    name: String,
    /// O Ollama manda os argumentos como objeto JSON (não string).
    arguments: Value,
}

#[derive(Debug)]
pub struct OllamaReply {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn think_blocks_are_stripped() {
        assert_eq!(strip_think("<think>hmm</think>

Oi!"), "Oi!");
        assert_eq!(strip_think("raciocínio solto
</think>
Pronto!"), "Pronto!");
        assert_eq!(strip_think("Oi!"), "Oi!");
    }

    #[test]
    fn host_normalization() {
        assert_eq!(normalize_host("0.0.0.0:11434"), "http://127.0.0.1:11434");
        assert_eq!(normalize_host("http://localhost:11434/"), "http://localhost:11434");
        assert_eq!(normalize_host("192.168.0.5"), "http://192.168.0.5:11434");
        assert_eq!(normalize_host("[::]:8080"), "http://[::1]:8080");
        assert_eq!(normalize_host("https://ollama.lan"), "https://ollama.lan:11434");
    }

    #[test]
    fn initial_messages_are_system_history_user() {
        let history = vec![Turn {
            user_text: "abre o spotify".into(),
            tool_call: Some(ToolCall {
                action: "app.open".into(),
                params: json!({ "app": "spotify" }),
            }),
            reply_text: "Pronto!".into(),
        }];
        let m = build_initial_messages("e agora o discord", &history);
        assert_eq!(m.len(), 4);
        assert_eq!(m[0]["role"], "system");
        assert_eq!(m[2]["content"], "Pronto!");
        assert_eq!(m[3]["content"], "e agora o discord");
    }
}
