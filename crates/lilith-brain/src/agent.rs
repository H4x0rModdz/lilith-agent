use crate::emotion;
use crate::error::LilithError;
use crate::memory::{SessionMemory, Turn};
use crate::ollama::{append_tool_step, build_initial_messages, Ollama};
use crate::tools::{all_tools, ToolCall};
use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;

/// Executa uma ação. O app implementa passando pelos guardrails; a resposta
/// é o envelope `{status: "success"|"error", result?, error?: {code, message}}`
/// que volta como contexto para o LLM.
#[async_trait]
pub trait Dispatcher: Send + Sync {
    async fn dispatch(&self, call: &ToolCall) -> Value;
}

/// Recebe o progresso do turno para a UI (tokens em streaming, tool em curso).
#[async_trait]
pub trait Sink: Send + Sync {
    async fn partial(&self, step: u32, chunk: &str);
    async fn step(&self, step: u32, action: &str);
}

#[derive(Debug, Clone, Serialize)]
pub struct Reply {
    pub reply: String,
    pub action: Option<String>,
    pub emotion: &'static str,
}

const HISTORY_TURNS: usize = 8;
const MAX_STEPS: usize = 8;

pub struct Agent {
    ollama: Arc<dyn Ollama>,
    memory: SessionMemory,
}

impl Agent {
    pub fn new(ollama: Arc<dyn Ollama>) -> Self {
        Self {
            ollama,
            memory: SessionMemory::new(32),
        }
    }

    pub fn reset(&self) {
        self.memory.reset();
    }

    /// Um turno completo: pergunta ao LLM, executa tools uma por vez (até
    /// `MAX_STEPS`) devolvendo cada resultado, e termina com texto.
    pub async fn process(&self, text: &str, dispatcher: &dyn Dispatcher, sink: Arc<dyn Sink>) -> Reply {
        let history = self.memory.recent(HISTORY_TURNS);
        let mut messages = build_initial_messages(text, &history);
        let tools = all_tools();
        let mut last_call: Option<ToolCall> = None;
        let mut last_step_reply = String::new();

        for step in 0..MAX_STEPS {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let fwd_sink = sink.clone();
            let step_idx = step as u32;
            // Agrupa tokens em ~20 eventos/s em vez de um por token — evita
            // re-layout da UI a cada token (mesma disciplina do jarvis-os).
            let forwarder = tokio::spawn(async move {
                let mut pending = String::new();
                let mut tick = tokio::time::interval(std::time::Duration::from_millis(50));
                loop {
                    tokio::select! {
                        chunk = rx.recv() => match chunk {
                            Some(c) => pending.push_str(&c),
                            None => {
                                if !pending.is_empty() {
                                    fwd_sink.partial(step_idx, &pending).await;
                                }
                                break;
                            }
                        },
                        _ = tick.tick() => {
                            if !pending.is_empty() {
                                fwd_sink.partial(step_idx, &pending).await;
                                pending.clear();
                            }
                        }
                    }
                }
            });

            let result = self.ollama.chat_messages(&messages, &tools, Some(tx)).await;
            let _ = forwarder.await;

            let reply = match result {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(step, "falha no Ollama: {e}");
                    let text_out = match e {
                        LilithError::OllamaUnreachable(_) => {
                            "Não consigo falar com o meu modelo de IA agora — o Ollama está aberto?"
                        }
                        LilithError::OllamaInvalid(_) => {
                            "O Ollama respondeu com erro. Confere se o modelo configurado está baixado."
                        }
                    };
                    return self.finish(text, last_call, text_out.to_string());
                }
            };

            if reply.tool_calls.is_empty() {
                let final_text = if !reply.text.trim().is_empty() {
                    reply.text.trim().to_string()
                } else if !last_step_reply.is_empty() {
                    last_step_reply
                } else {
                    "Hm, não entendi. Pode repetir de outro jeito?".into()
                };
                return self.finish(text, last_call, final_text);
            }

            // Uma tool por passo; se o modelo quiser as outras, re-emite depois.
            let call = reply.tool_calls.into_iter().next().expect("não vazio");
            tracing::info!(step, action = %call.action, "tool escolhida");
            sink.step(step_idx, &call.action).await;

            let response = dispatcher.dispatch(&call).await;
            last_step_reply = summarize_step(&call.action, &response);
            append_tool_step(&mut messages, &call, &response);
            last_call = Some(call);
        }

        tracing::warn!(steps = MAX_STEPS, "limite de passos");
        self.finish(
            text,
            last_call,
            format!("(parei após {MAX_STEPS} passos) {last_step_reply}"),
        )
    }

    fn finish(&self, user_text: &str, call: Option<ToolCall>, reply: String) -> Reply {
        let action = call.as_ref().map(|c| c.action.clone());
        self.memory.record(Turn {
            user_text: user_text.into(),
            tool_call: call,
            reply_text: reply.clone(),
        });
        Reply {
            emotion: emotion::classify(&reply),
            reply,
            action,
        }
    }
}

fn summarize_step(action: &str, response: &Value) -> String {
    match response["status"].as_str() {
        Some("success") => format!("Feito: {action}"),
        _ => format!(
            "Falhou ({action}): {}",
            response["error"]["message"].as_str().unwrap_or("erro desconhecido")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ollama::OllamaReply;
    use crate::tools::Tool;
    use serde_json::json;
    use std::sync::Mutex;
    use tokio::sync::mpsc::UnboundedSender;

    struct MockOllama(Mutex<Vec<Result<OllamaReply, LilithError>>>);

    #[async_trait]
    impl Ollama for MockOllama {
        async fn chat_messages(
            &self,
            _: &[Value],
            _: &[Tool],
            chunks: Option<UnboundedSender<String>>,
        ) -> Result<OllamaReply, LilithError> {
            let next = self.0.lock().unwrap().remove(0);
            if let (Ok(r), Some(tx)) = (&next, chunks) {
                let _ = tx.send(r.text.clone());
            }
            next
        }
    }

    #[derive(Default)]
    struct MockDispatcher(Mutex<Vec<String>>);

    #[async_trait]
    impl Dispatcher for MockDispatcher {
        async fn dispatch(&self, call: &ToolCall) -> Value {
            self.0.lock().unwrap().push(call.action.clone());
            json!({ "status": "success", "result": {} })
        }
    }

    struct NullSink;

    #[async_trait]
    impl Sink for NullSink {
        async fn partial(&self, _: u32, _: &str) {}
        async fn step(&self, _: u32, _: &str) {}
    }

    fn text(t: &str) -> Result<OllamaReply, LilithError> {
        Ok(OllamaReply { text: t.into(), tool_calls: vec![] })
    }

    fn tool(action: &str) -> Result<OllamaReply, LilithError> {
        Ok(OllamaReply {
            text: String::new(),
            tool_calls: vec![ToolCall { action: action.into(), params: json!({}) }],
        })
    }

    fn agent(replies: Vec<Result<OllamaReply, LilithError>>) -> Agent {
        Agent::new(Arc::new(MockOllama(Mutex::new(replies))))
    }

    #[tokio::test]
    async fn plain_chat_reply() {
        let a = agent(vec![text("Oi! Tudo bem?")]);
        let d = MockDispatcher::default();
        let r = a.process("oi", &d, Arc::new(NullSink)).await;
        assert_eq!(r.reply, "Oi! Tudo bem?");
        assert!(d.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn multi_step_chain_runs_tools_in_order() {
        let a = agent(vec![tool("web.search"), tool("download.file"), text("Pronto!")]);
        let d = MockDispatcher::default();
        let r = a.process("baixa o vlc", &d, Arc::new(NullSink)).await;
        assert_eq!(*d.0.lock().unwrap(), vec!["web.search", "download.file"]);
        assert_eq!(r.action.as_deref(), Some("download.file"));
        assert_eq!(r.emotion, emotion::HAPPY);
    }

    #[tokio::test]
    async fn step_cap_stops_runaway_model() {
        let a = agent((0..10).map(|_| tool("audio.get_volume")).collect());
        let d = MockDispatcher::default();
        let r = a.process("loop", &d, Arc::new(NullSink)).await;
        assert_eq!(d.0.lock().unwrap().len(), MAX_STEPS);
        assert!(r.reply.starts_with("(parei após"));
    }

    #[tokio::test]
    async fn ollama_offline_gives_clear_message() {
        let a = agent(vec![Err(LilithError::OllamaUnreachable("conn refused".into()))]);
        let r = a.process("oi", &MockDispatcher::default(), Arc::new(NullSink)).await;
        assert!(r.reply.contains("Ollama"));
        assert_eq!(r.emotion, emotion::CONCERNED);
    }
}
