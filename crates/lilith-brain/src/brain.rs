//! O cérebro escolhido nas Configurações: Ollama local (padrão, 100% no PC)
//! ou o Claude Code instalado. Troca na hora — os agentes de cada conversa
//! seguram este objeto, não um backend fixo.

use crate::claude_code::{ClaudeCode, ClaudeCodeConfig};
use crate::error::LilithError;
use crate::ollama::{Ollama, OllamaClient, OllamaReply};
use crate::tools::Tool;
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::UnboundedSender;

pub struct Brain {
    local: OllamaClient,
    claude_cfg: ClaudeCodeConfig,
    workdir: PathBuf,
    /// Criado na primeira vez que é usado (o Claude Code pode ser instalado
    /// com a Lilith aberta).
    claude: Mutex<Option<Arc<ClaudeCode>>>,
    use_claude: AtomicBool,
}

impl Brain {
    pub fn new(local: OllamaClient, claude_cfg: ClaudeCodeConfig, workdir: PathBuf, use_claude: bool) -> Self {
        Self { local, claude_cfg, workdir, claude: Mutex::new(None), use_claude: AtomicBool::new(use_claude) }
    }

    pub fn set_claude_code(&self, on: bool) {
        self.use_claude.store(on, Ordering::Relaxed);
    }

    pub fn uses_claude_code(&self) -> bool {
        self.use_claude.load(Ordering::Relaxed)
    }

    /// O Claude Code está instalado neste PC?
    pub fn claude_code_found(&self) -> bool {
        crate::claude_code::locate(&self.claude_cfg).is_some()
    }

    pub fn claude_cfg(&self) -> &ClaudeCodeConfig {
        &self.claude_cfg
    }

    fn claude(&self) -> Option<Arc<ClaudeCode>> {
        let mut slot = self.claude.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            *slot = ClaudeCode::new(self.claude_cfg.clone(), self.workdir.clone()).map(Arc::new);
        }
        slot.clone()
    }

    /// Claude Code escolhido mas fora do ar (não instalado, sem login, sem
    /// limite): tenta o local para não deixar o usuário sem resposta; se o
    /// local também falhar, o erro mostrado é o do Claude Code.
    async fn fallback<T, F, Fut>(&self, claude_err: LilithError, local: F) -> Result<T, LilithError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, LilithError>>,
    {
        tracing::warn!(error = %claude_err, "Claude Code indisponível — tentando o cérebro local");
        local().await.map_err(|_| claude_err)
    }
}

#[async_trait]
impl Ollama for Brain {
    async fn chat_messages(
        &self,
        messages: &[Value],
        tools: &[Tool],
        chunks: Option<UnboundedSender<String>>,
    ) -> Result<OllamaReply, LilithError> {
        if !self.uses_claude_code() {
            return self.local.chat_messages(messages, tools, chunks).await;
        }
        let Some(claude) = self.claude() else {
            let err = LilithError::OllamaUnreachable("o Claude Code não está instalado neste PC".into());
            return self.fallback(err, || self.local.chat_messages(messages, tools, chunks)).await;
        };
        match claude.chat_messages(messages, tools, chunks.clone()).await {
            Err(e @ LilithError::OllamaUnreachable(_)) => self.fallback(e, || self.local.chat_messages(messages, tools, chunks)).await,
            other => other,
        }
    }

    async fn structured(&self, messages: &[Value], schema: &Value) -> Result<Value, LilithError> {
        if !self.uses_claude_code() {
            return self.local.structured(messages, schema).await;
        }
        let Some(claude) = self.claude() else {
            let err = LilithError::OllamaUnreachable("o Claude Code não está instalado neste PC".into());
            return self.fallback(err, || self.local.structured(messages, schema)).await;
        };
        match claude.structured(messages, schema).await {
            Err(e @ LilithError::OllamaUnreachable(_)) => self.fallback(e, || self.local.structured(messages, schema)).await,
            other => other,
        }
    }
}
