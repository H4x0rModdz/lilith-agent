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
            "cursor.point" => self.screen_action(action, &call.params).await,
            a if APP_ACTIONS.contains(&a) => self.agent_action(action, &call.params),
            a if a.starts_with("window.") && !matches!(a, "window.list" | "window.read") => {
                self.screen_action(action, &call.params).await
            }
            "window.read" => {
                // Gesto de "olhar" a janela antes de ler.
                use tauri::Manager;
                let screen = self.app.state::<Arc<crate::screen::Screen>>().inner().clone();
                let _presence = screen.begin();
                let target = call.params["target"].as_str().unwrap_or("active").to_string();
                if let Ok(w) = blocking(move || lilith_win::window::find(&target)).await {
                    if !w.minimized {
                        screen.point(w.frame.center()).await;
                    }
                }
                lilith_win::dispatch(action, call.params.clone()).await
            }
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
pub const APP_ACTIONS: &[&str] = &["agent.delegate", "agent.jobs", "cursor.point"];

/// Win32 bloqueante fora do runtime async.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, lilith_win::WinError> + Send + 'static,
) -> Result<T, lilith_win::WinError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| lilith_win::WinError::Failed(e.to_string()))?
}

impl GuardedDispatcher {
    /// Ações de tela feitas "à mão" pela Lilith: borda rosa + cursor dela
    /// indo até o lugar, gesto, e a ação real por API.
    async fn screen_action(&self, action: &str, params: &Value) -> Result<Value, lilith_win::WinError> {
        use crate::elevated::{self, Op};
        use crate::screen::{caption_button, Screen};
        use lilith_win::window;
        use tauri::Manager;

        let screen = self.app.state::<Arc<Screen>>().inner().clone();
        let _presence = screen.begin();
        let target = params["target"].as_str().unwrap_or("active").to_string();

        match action {
            "window.move" => {
                let p = params.clone();
                let plan = blocking(move || window::plan_move(&p)).await?;
                if plan.elevated {
                    screen.drag_gesture(&plan).await;
                    elevated::run(vec![Op::Frame { id: plan.id, rect: plan.to, maximize_after: plan.maximize_after }])
                        .await?;
                } else {
                    screen.drag(&plan).await?;
                }
                Ok(json!({
                    "moved": plan.title, "to": plan.to, "maximized": plan.maximize_after, "as_admin": plan.elevated,
                }))
            }
            "window.tile" => {
                let p = params.clone();
                let plans = blocking(move || window::plan_tile(&p)).await?;
                // Janelas de admin: só o gesto agora; todas vão juntas para o
                // ajudante no fim (um único prompt do UAC).
                let mut admin_ops = Vec::new();
                for plan in &plans {
                    if plan.elevated {
                        screen.drag_gesture(plan).await;
                        admin_ops.push(Op::Frame { id: plan.id, rect: plan.to, maximize_after: false });
                    } else {
                        screen.drag(plan).await?;
                    }
                }
                elevated::run(admin_ops).await?;
                Ok(json!({ "tiled": plans.iter().map(|p| &p.title).collect::<Vec<_>>() }))
            }
            "cursor.point" => {
                let spec = params["monitor"].clone();
                let at = if !spec.is_null() && params["target"].is_null() {
                    let mons = window::monitors();
                    let idx = spec.as_u64().map(|n| n as usize).unwrap_or(1);
                    let m = mons.iter().find(|m| m.index == idx).or(mons.first());
                    m.map(|m| m.work.center())
                        .ok_or_else(|| lilith_win::WinError::NotFound("monitor".into()))?
                } else {
                    let w = blocking(move || window::find(&target)).await?;
                    w.frame.center()
                };
                screen.point(at).await;
                Ok(json!({ "pointed_at": at }))
            }
            _ => {
                // focus / minimize / maximize / restore / close: vai até o
                // botão (ou a barra de título), "clica" e executa. Janela
                // minimizada não tem onde mirar — executa direto.
                let w = blocking(move || window::find(&target)).await?;
                if !w.minimized {
                    let spot = if action == "window.focus" {
                        (w.frame.x + w.frame.w / 3, w.frame.y + 14)
                    } else {
                        caption_button(w.frame, action)
                    };
                    screen.fly_to(spot).await;
                    screen.click().await;
                }
                if w.elevated {
                    let op = match action {
                        "window.close" => Op::Close { id: w.id },
                        "window.focus" => Op::Focus { id: w.id },
                        other => Op::State { id: w.id, state: other.trim_start_matches("window.").into() },
                    };
                    elevated::run(vec![op]).await?;
                    return Ok(json!({ "window": w.title, "done": action, "as_admin": true }));
                }
                let mut p = params.clone();
                p["target"] = json!(format!("id:{}", w.id));
                lilith_win::dispatch(action, p).await
            }
        }
    }

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
