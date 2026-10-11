//! Conversa com o modelo real pelo terminal, com as ações apenas simuladas:
//! `cargo run -p lilith-brain --example ask -- "quanto está o volume?"`.
//! nada é executado no sistema.
use async_trait::async_trait;
use lilith_brain::{Agent, Dispatcher, OllamaClient, OllamaConfig, Sink, ToolCall};
use serde_json::{json, Value};
use std::sync::Arc;

struct ReadOnly;

#[async_trait]
impl Dispatcher for ReadOnly {
    async fn dispatch(&self, call: &ToolCall) -> Value {
        println!("  → {} {}", call.action, call.params);
        json!({ "status": "success", "result": { "simulated": true, "action": call.action } })
    }
}

struct Print;

#[async_trait]
impl Sink for Print {
    async fn partial(&self, _: u32, _: &str) {}
    async fn step(&self, _: u32, _: &str) {}
}

#[tokio::main]
async fn main() {
    let agent = Agent::new(Arc::new(OllamaClient::new(OllamaConfig::default())));
    for text in std::env::args().skip(1) {
        let started = std::time::Instant::now();
        println!("> {text}");
        let r = agent.process(&text, &ReadOnly, Arc::new(Print)).await;
        println!("< [{}] {} ({:.1}s)\n", r.emotion, r.reply, started.elapsed().as_secs_f32());
    }
}
