mod config;
mod dispatcher;
pub mod elevated;
mod screen;
mod windows;

use dispatcher::{Answer, Approvals, EventSink, GuardedDispatcher};
use lilith_agents::{Job, JobEvent, Jobs};
use lilith_brain::{Agent, OllamaClient};
use lilith_guard::{AuditLog, Policy, CHANNEL_LOCAL};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

struct AppState {
    agent: Arc<Agent>,
    policy: Arc<Policy>,
    audit: Arc<AuditLog>,
    audit_path: PathBuf,
    approvals: Arc<Approvals>,
    jobs: Arc<Jobs>,
    /// Turno em andamento — o kill switch aborta essa task.
    current: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

#[tauri::command]
fn send_message(text: String, app: AppHandle, state: State<'_, AppState>) {
    let text = text.trim().to_string();
    if text.is_empty() {
        return;
    }
    let dispatcher = GuardedDispatcher {
        jobs: state.jobs.clone(),
        channel: CHANNEL_LOCAL.into(),
        app: app.clone(),
        policy: state.policy.clone(),
        audit: state.audit.clone(),
        approvals: state.approvals.clone(),
        tainted: Default::default(),
    };
    let agent = state.agent.clone();
    let _ = app.emit("lilith://state", "thinking");
    let handle = tauri::async_runtime::spawn(async move {
        let reply = agent.process(&text, &dispatcher, Arc::new(EventSink(app.clone()))).await;
        let _ = app.emit("lilith://reply", &reply);
        let _ = app.emit("lilith://state", "idle");
    });
    // Um turno por vez: um novo pedido substitui o anterior.
    if let Some(old) = state.current.lock().unwrap().replace(handle) {
        old.abort();
    }
}

#[tauri::command]
fn answer_approval(id: u64, answer: Answer, state: State<'_, AppState>) -> bool {
    state.approvals.answer(id, answer)
}

#[tauri::command]
fn cancel(app: AppHandle, state: State<'_, AppState>) {
    kill_switch(&app, &state);
}

#[tauri::command]
fn reset_conversation(state: State<'_, AppState>) {
    state.agent.reset();
}

#[tauri::command]
fn toggle_chat(app: AppHandle) {
    windows::toggle_chat(&app);
}

#[tauri::command]
fn hide_window(label: String, app: AppHandle) {
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.hide();
    }
}

#[tauri::command]
fn lilith_avatar() -> Option<String> {
    config::avatar_data_url()
}

/// O frontend avisa quando o usuário arrasta o chat.
#[tauri::command]
fn chat_moved() {
    windows::CHAT_MOVED.store(true, std::sync::atomic::Ordering::Relaxed);
}

#[tauri::command]
fn list_jobs(state: State<'_, AppState>) -> Vec<Job> {
    state.jobs.list()
}

#[tauri::command]
async fn cancel_job(id: u64, state: State<'_, AppState>) -> Result<(), String> {
    state.jobs.cancel(id).await
}

/// Últimas `limit` linhas do log de auditoria, mais recentes primeiro.
#[tauri::command]
fn recent_activity(limit: usize, state: State<'_, AppState>) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(&state.audit_path) else { return Vec::new() };
    text.lines()
        .rev()
        .take(limit.min(500))
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Aborta o turno em curso (e com ele qualquer aprovação pendente — o
/// receiver morre junto e a ação nunca roda). Jobs de agentes de código
/// continuam; eles são cancelados individualmente pelo painel.
fn kill_switch(app: &AppHandle, state: &AppState) {
    let running = state.current.lock().unwrap().take();
    if let Some(task) = running.filter(|t| !t.inner().is_finished()) {
        task.abort();
        tracing::warn!("kill switch acionado");
        state.audit.record(CHANNEL_LOCAL, "kill_switch", &json!({}), "aborted", &json!({}));
        if let Some(screen) = app.try_state::<Arc<screen::Screen>>() {
            screen.hide_now();
        }
        let _ = app.emit("lilith://cancelled", ());
        let _ = app.emit("lilith://state", "idle");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg = config::Config::load();
    let data = config::data_dir();
    tracing::info!(model = %cfg.ollama.model, dir = %data.display(), "Lilith iniciando");

    // Ctrl+Alt+Esc: kill switch global, funciona com qualquer janela em foco.
    let panic_key = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Escape);

    tauri::Builder::default()
        // Lembra onde o usuário deixou a orb e o painel. Só posição: tamanho
        // é fixo e visibilidade é controlada pela Lilith.
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(tauri_plugin_window_state::StateFlags::POSITION)
                .with_denylist(&["chat"])
                .build(),
        )
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    if shortcut == &panic_key && event.state() == ShortcutState::Pressed {
                        if let Some(state) = app.try_state::<AppState>() {
                            kill_switch(app, &state);
                        }
                    }
                })
                .build(),
        )
        .setup(move |app| {
            let handle = app.handle().clone();
            let jobs = Jobs::new(cfg.agents, move |event| {
                let _ = handle.emit("lilith://job", &event);
                if let JobEvent::Updated { job } = &event {
                    tracing::info!(id = job.id, agent = %job.agent, status = ?job.status, "job");
                }
            });
            app.manage(screen::Screen::new(app.handle().clone()));
            if let Some(cursor) = app.get_webview_window("cursor") {
                let _ = cursor.set_ignore_cursor_events(true);
            }
            let audit_path = data.join("logs").join("audit.jsonl");
            app.manage(AppState {
                agent: Arc::new(Agent::new(Arc::new(OllamaClient::new(cfg.ollama)))),
                policy: Arc::new(Policy::load(&data.join("permissions.json"))),
                audit: Arc::new(AuditLog::new(&audit_path)),
                audit_path,
                approvals: Arc::new(Approvals::default()),
                jobs,
                current: Mutex::new(None),
            });

            if let Err(e) = app.global_shortcut().register(panic_key) {
                tracing::warn!(error = %e, "não registrei o kill switch Ctrl+Alt+Esc");
            }
            // Primeira execução: sem layout salvo, posiciona orb e painel nos
            // cantos padrão. Depois o window-state restaura onde o usuário deixou.
            let saved_layout = app
                .path()
                .app_config_dir()
                .map(|d| d.join(tauri_plugin_window_state::DEFAULT_FILENAME).exists())
                .unwrap_or(false);
            if saved_layout {
                windows::PANEL_PLACED.store(true, std::sync::atomic::Ordering::Relaxed);
            } else if let Some(orb) = app.get_webview_window("orb") {
                windows::place_orb(&orb);
            }

            // Ícone na área de notificação: clique esquerdo abre o painel,
            // direito mostra o menu.
            let panel = MenuItem::with_id(app, "panel", "Painel da Lilith", true, None::<&str>)?;
            let chat = MenuItem::with_id(app, "chat", "Conversar", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Sair", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&panel, &chat, &quit])?;
            TrayIconBuilder::with_id("lilith")
                .icon(app.default_window_icon().cloned().expect("ícone padrão"))
                .tooltip("Lilith")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "panel" => windows::show_panel(app),
                    "chat" => windows::show_chat(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        windows::toggle_panel(tray.app_handle());
                    }
                })
                .build(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            send_message,
            answer_approval,
            cancel,
            reset_conversation,
            toggle_chat,
            hide_window,
            chat_moved,
            lilith_avatar,
            list_jobs,
            cancel_job,
            recent_activity,
        ])
        .run(tauri::generate_context!())
        .expect("erro ao iniciar a Lilith");
}

#[cfg(test)]
mod tests {
    /// Toda tool que o LLM enxerga precisa de scope no guard e de quem a
    /// execute (lilith-win ou o próprio app) — senão falha em runtime.
    #[test]
    fn catalog_is_wired() {
        let tools = lilith_brain::all_tools();
        for tool in &tools {
            let scope = lilith_guard::scope_for(tool.action);
            assert_ne!(scope, "unknown", "{} sem scope no lilith-guard", tool.action);
            assert!(
                lilith_win::ACTIONS.contains(&tool.action) || crate::dispatcher::APP_ACTIONS.contains(&tool.action),
                "{} sem handler",
                tool.action
            );
        }
        assert_eq!(
            lilith_win::ACTIONS.len() + crate::dispatcher::APP_ACTIONS.len(),
            tools.len()
        );
    }
}
