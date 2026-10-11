use async_trait::async_trait;
use lilith_brain::{Dispatcher, Sink, ToolCall};
use lilith_guard::{AuditLog, Decision, Policy, Rule};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

const APPROVAL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    Deny,
    Once,
    Always,
}

#[derive(Debug, Clone, Serialize)]
struct ApprovalRequest {
    id: u64,
    channel: String,
    action: String,
    scope: &'static str,
    params: Value,
}

/// Pedidos de aprovação aguardando resposta do usuário.
#[derive(Default)]
pub struct Approvals {
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<Answer>>>,
}

impl Approvals {
    pub fn answer(&self, id: u64, answer: Answer) -> bool {
        match self.pending.lock().unwrap().remove(&id) {
            Some(tx) => tx.send(answer).is_ok(),
            None => false,
        }
    }

    fn open(&self) -> (u64, oneshot::Receiver<Answer>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        (id, rx)
    }

    fn drop_pending(&self, id: u64) {
        self.pending.lock().unwrap().remove(&id);
    }
}

/// Dispatcher que passa toda ação pelo `lilith-guard` antes do `lilith-win`.
pub struct GuardedDispatcher {
    pub jobs: Arc<lilith_agents::Jobs>,
    pub channel: String,
    pub app: AppHandle,
    pub policy: Arc<Policy>,
    pub audit: Arc<AuditLog>,
    pub approvals: Arc<Approvals>,
}

#[async_trait]
impl Dispatcher for GuardedDispatcher {
    async fn dispatch(&self, call: &ToolCall) -> Value {
        let started = Instant::now();
        let action = call.action.as_str();

        let scope = match self.policy.decide(&self.channel, action) {
            Decision::Allow { scope } => scope,
            Decision::Deny { scope, reason } => {
                self.audit.record(&self.channel, action, &call.params, "denied", &json!({ "scope": scope }));
                return error_envelope(action, "PERMISSION_DENIED", &reason, started);
            }
            Decision::Ask { scope } => match self.ask(call, scope).await {
                Answer::Deny => {
                    self.audit.record(&self.channel, action, &call.params, "denied_by_user", &json!({ "scope": scope }));
                    return error_envelope(action, "PERMISSION_DENIED", "o usuário negou esta ação", started);
                }
                Answer::Once => scope,
                Answer::Always => {
                    if let Err(e) = self.policy.set_rule(&self.channel, scope, Rule::Allow) {
                        tracing::warn!(error = %e, "não consegui salvar 'permitir sempre'");
                    }
                    scope
                }
            },
        };

        let result = match action {
            a if APP_ACTIONS.contains(&a) => self.agent_action(action, &call.params),
            _ => lilith_win::dispatch(action, call.params.clone()).await,
        };
        let envelope = match result {
            Ok(v) => json!({
                "action": action,
                "status": "success",
                "result": v,
                "duration_ms": started.elapsed().as_millis() as u64,
            }),
            Err(e) => error_envelope(action, e.code(), &e.to_string(), started),
        };
        let outcome = envelope["status"].as_str().unwrap_or("error").to_string();
        self.audit.record(&self.channel, action, &call.params, &outcome, &json!({ "scope": scope, "response": envelope }));
        envelope
    }
}

/// Ações que o próprio app executa (não o `lilith-win`).
pub const APP_ACTIONS: &[&str] = &["agent.delegate", "agent.jobs"];

impl GuardedDispatcher {
    fn agent_action(&self, action: &str, params: &Value) -> Result<Value, lilith_win::WinError> {
        use lilith_win::WinError;
        if action == "agent.jobs" {
            let jobs: Vec<Value> = self
                .jobs
                .list()
                .into_iter()
                .take(10)
                .map(|j| json!({ "id": j.id, "agent": j.agent, "task": j.task, "status": j.status, "result": j.result }))
                .collect();
            return Ok(json!({ "jobs": jobs }));
        }
        let agent = params["agent"].as_str().unwrap_or("").trim().to_lowercase();
        let task = params["task"].as_str().unwrap_or("");
        let folder = params["folder"]
            .as_str()
            .filter(|f| !f.trim().is_empty())
            .map(lilith_win::resolve_path)
            .or_else(dirs::home_dir)
            .ok_or_else(|| WinError::Unavailable("pasta do usuário".into()))?;
        let job = self.jobs.start(&agent, task, folder).map_err(WinError::InvalidParams)?;
        crate::windows::show_panel(&self.app);
        Ok(json!({
            "job_id": job.id,
            "agent": job.agent,
            "folder": job.folder,
            "note": "rodando em segundo plano; o usuário acompanha pelo painel",
        }))
    }

    /// Emite o pedido para a UI e espera a resposta (30 s → negado).
    async fn ask(&self, call: &ToolCall, scope: &'static str) -> Answer {
        let (id, rx) = self.approvals.open();
        let req = ApprovalRequest {
            id,
            channel: self.channel.clone(),
            action: call.action.clone(),
            scope,
            params: call.params.clone(),
        };
        crate::windows::show_chat(&self.app);
        let _ = self.app.emit("lilith://approval", &req);
        match tokio::time::timeout(APPROVAL_TIMEOUT, rx).await {
            Ok(Ok(a)) => a,
            _ => {
                self.approvals.drop_pending(id);
                let _ = self.app.emit("lilith://approval-expired", id);
                Answer::Deny
            }
        }
    }
}

fn error_envelope(action: &str, code: &str, message: &str, started: Instant) -> Value {
    json!({
        "action": action,
        "status": "error",
        "error": { "code": code, "message": message },
        "duration_ms": started.elapsed().as_millis() as u64,
    })
}

/// Repassa o progresso do agente para a UI como eventos.
pub struct EventSink(pub AppHandle);

#[async_trait]
impl Sink for EventSink {
    async fn partial(&self, step: u32, chunk: &str) {
        let _ = self.0.emit("lilith://partial", json!({ "step": step, "chunk": chunk }));
    }

    async fn step(&self, step: u32, action: &str) {
        let _ = self.0.emit("lilith://step", json!({ "step": step, "action": action }));
    }
}
