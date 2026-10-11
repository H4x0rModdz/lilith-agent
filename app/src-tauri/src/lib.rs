mod config;
mod profile;
mod dispatcher;
pub mod elevated;
mod screen;
mod windows;

use dispatcher::{Answer, Approvals, EventSink, GuardedDispatcher};
use lilith_agents::{Job, JobEvent, Jobs};
use lilith_brain::{Agent, Ollama, OllamaClient};
use lilith_guard::{AuditLog, Policy, CHANNEL_LOCAL};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

type TurnHandle = tauri::async_runtime::JoinHandle<()>;

struct AppState {
    /// Um modelo (Ollama) compartilhado; uma memória por conversa (aba do chat).
    ollama: Arc<dyn Ollama>,
    conversations: Mutex<HashMap<String, Arc<Agent>>>,
    /// Memória de longo prazo (SQLite): conversas, fatos e configurações.
    store: Arc<lilith_memory::Memory>,
    policy: Arc<Policy>,
    audit: Arc<AuditLog>,
    audit_path: PathBuf,
    approvals: Arc<Approvals>,
    jobs: Arc<Jobs>,
    /// Turno em andamento de cada conversa — o kill switch aborta todos.
    running: Mutex<HashMap<String, TurnHandle>>,
}

impl AppState {
    /// Memória da conversa; reabrindo uma do histórico, recarrega as falas.
    fn agent_for(&self, conversation: &str) -> Arc<Agent> {
        self.conversations
            .lock()
            .unwrap()
            .entry(conversation.to_string())
            .or_insert_with(|| {
                let agent = Agent::new(self.ollama.clone());
                agent.seed(self.store.turns(conversation));
                Arc::new(agent)
            })
            .clone()
    }
}

/// Avisa todas as janelas (abas do chat, painel) do título novo.
fn emit_title(app: &AppHandle, conversation: &str, title: &str) {
    let _ = app.emit("lilith://conversation", json!({ "id": conversation, "title": title }));
}

fn emit_state(app: &AppHandle, conversation: &str, state: &str) {
    let _ = app.emit("lilith://state", json!({ "conversation": conversation, "state": state }));
}

#[tauri::command]
fn send_message(text: String, conversation: String, app: AppHandle, state: State<'_, AppState>) {
    let text = text.trim().to_string();
    if text.is_empty() || !lilith_memory::valid_id(&conversation) {
        return;
    }
    let dispatcher = GuardedDispatcher {
        conversation: conversation.clone(),
        store: state.store.clone(),
        jobs: state.jobs.clone(),
        channel: CHANNEL_LOCAL.into(),
        app: app.clone(),
        policy: state.policy.clone(),
        audit: state.audit.clone(),
        approvals: state.approvals.clone(),
        tainted: Default::default(),
        chat_intent: Default::default(),
    };
    let agent = state.agent_for(&conversation);
    if let Some(title) = state.store.append(&conversation, "user", &text) {
        emit_title(&app, &conversation, &title);
    }
    emit_state(&app, &conversation, "thinking");
    let conv = conversation.clone();
    let store = state.store.clone();
    let ollama = state.ollama.clone();
    let context = profile::context(&state.store);
    let handle = tauri::async_runtime::spawn(async move {
        let sink = Arc::new(EventSink { app: app.clone(), conversation: conv.clone() });
        let reply = agent.process_with_context(&text, Some(&context), &dispatcher, sink).await;
        store.append(&conv, "lilith", &reply.reply);
        // Aprende com o que o usuário DIGITOU (nunca com conteúdo lido).
        if profile::auto_learn(&store) {
            let (app, store, text) = (app.clone(), store.clone(), text.clone());
            tauri::async_runtime::spawn(async move {
                let facts = lilith_brain::learn::extract(ollama.as_ref(), &text).await;
                let mut learned = false;
                for f in facts {
                    learned |= store.remember(&f.key, &f.value, "learned");
                }
                if learned {
                    let _ = app.emit("lilith://facts", ());
                }
            });
        }
        let _ = app.emit(
            "lilith://reply",
            json!({ "conversation": conv, "reply": reply.reply, "action": reply.action, "emotion": reply.emotion }),
        );
        emit_state(&app, &conv, "idle");
    });
    // Um turno por conversa: um novo pedido na mesma aba substitui o anterior;
    // outras abas seguem rodando.
    if let Some(old) = state.running.lock().unwrap().insert(conversation, handle) {
        old.abort();
    }
}

#[tauri::command]
fn answer_approval(id: u64, answer: Answer, app: AppHandle, state: State<'_, AppState>) -> bool {
    let ok = state.approvals.answer(id, answer);
    // A orb tira o pedido da bolinha de pendências.
    let _ = app.emit("lilith://approval-resolved", json!({ "id": id }));
    ok
}

/// Menu da orb: abre o chat numa conversa nova.
#[tauri::command]
fn new_chat(app: AppHandle) {
    windows::show_chat(&app);
    let _ = app.emit_to("chat", "lilith://new-chat", ());
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

#[tauri::command]
fn cancel(app: AppHandle, state: State<'_, AppState>) {
    kill_switch(&app, &state);
}

#[tauri::command]
fn reset_conversation(conversation: String, state: State<'_, AppState>) {
    if let Some(agent) = state.conversations.lock().unwrap().get(&conversation) {
        agent.reset();
    }
}

// ── configurações e o que ela sabe sobre o usuário ─────────────────────

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> profile::Settings {
    profile::Settings::load(&state.store)
}

#[tauri::command]
fn save_settings(settings: profile::Settings, state: State<'_, AppState>) {
    settings.save(&state.store);
}

#[tauri::command]
fn list_facts(state: State<'_, AppState>) -> Vec<lilith_memory::Fact> {
    state.store.facts()
}

/// Fato digitado pelo usuário no painel (fonte "user": o aprendizado não o sobrescreve).
#[tauri::command]
fn add_fact(key: String, value: String, app: AppHandle, state: State<'_, AppState>) -> bool {
    let ok = state.store.remember(&key, &value, "user");
    let _ = app.emit("lilith://facts", ());
    ok
}

#[tauri::command]
fn get_quick_prompts(state: State<'_, AppState>) -> Vec<String> {
    profile::quick_prompts(&state.store)
}

/// Salva e avisa as janelas (o chat atualiza as bolhas e o "/" na hora).
#[tauri::command]
fn save_quick_prompts(prompts: Vec<String>, app: AppHandle, state: State<'_, AppState>) -> Vec<String> {
    let saved = profile::save_quick_prompts(&state.store, &prompts);
    let _ = app.emit("lilith://quick-prompts", &saved);
    saved
}

#[tauri::command]
fn forget_fact(key: String, app: AppHandle, state: State<'_, AppState>) -> bool {
    let ok = state.store.forget(&key);
    let _ = app.emit("lilith://facts", ());
    ok
}

#[tauri::command]
fn list_conversations(state: State<'_, AppState>) -> Vec<lilith_memory::Summary> {
    state.store.list()
}

#[tauri::command]
fn load_conversation(conversation: String, state: State<'_, AppState>) -> Option<lilith_memory::Conversation> {
    state.store.get(&conversation)
}

#[tauri::command]
fn rename_conversation(conversation: String, title: String, app: AppHandle, state: State<'_, AppState>) -> Option<String> {
    if !lilith_memory::valid_id(&conversation) {
        return None;
    }
    let title = state.store.rename(&conversation, &title);
    emit_title(&app, &conversation, &title);
    Some(title)
}

/// Apaga do histórico (e fecha, se estiver aberta).
#[tauri::command]
fn delete_conversation(conversation: String, app: AppHandle, state: State<'_, AppState>) {
    close_conversation(conversation.clone(), app.clone(), state.clone());
    state.store.delete(&conversation);
    let _ = app.emit("lilith://conversation-deleted", json!({ "id": conversation }));
}

/// Fecha a aba: para o turno dela (se houver) e descarta a memória.
#[tauri::command]
fn close_conversation(conversation: String, app: AppHandle, state: State<'_, AppState>) {
    if let Some(task) = state.running.lock().unwrap().remove(&conversation) {
        task.abort();
        emit_state(&app, &conversation, "idle");
    }
    state.conversations.lock().unwrap().remove(&conversation);
}

#[tauri::command]
fn toggle_chat(app: AppHandle) {
    windows::toggle_chat(&app);
}

/// Abre o painel numa aba (comandos /painel, /tarefas, /config…).
#[tauri::command]
fn show_panel_tab(tab: String, app: AppHandle) {
    if !matches!(tab.as_str(), "jobs" | "activity" | "rules" | "settings") {
        return;
    }
    windows::show_panel(&app);
    let _ = app.emit_to("panel", "lilith://panel-tab", tab);
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

#[tauri::command]
fn list_rules(state: State<'_, AppState>) -> Vec<Value> {
    state
        .policy
        .rules()
        .into_iter()
        .map(|(channel, scope, rule)| json!({ "channel": channel, "scope": scope, "rule": rule }))
        .collect()
}

#[tauri::command]
fn revoke_rule(channel: String, scope: String, state: State<'_, AppState>) -> Result<bool, String> {
    state.policy.remove_rule(&channel, &scope).map_err(|e| e.to_string())
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
    // Interrompe uma digitação em andamento (ui.type) na próxima letra.
    lilith_win::input::cancel();
    let running: Vec<(String, TurnHandle)> = state.running.lock().unwrap().drain().collect();
    let mut stopped = 0;
    for (conversation, task) in running {
        if task.inner().is_finished() {
            continue;
        }
        task.abort();
        stopped += 1;
        let _ = app.emit("lilith://cancelled", json!({ "conversation": conversation }));
        emit_state(app, &conversation, "idle");
    }
    if stopped > 0 {
        tracing::warn!(stopped, "kill switch acionado");
        state.audit.record(CHANNEL_LOCAL, "kill_switch", &json!({ "turns": stopped }), "aborted", &json!({}));
        if let Some(screen) = app.try_state::<Arc<screen::Screen>>() {
            screen.hide_now();
        }
    }
}

/// "11/10/2026 12:39" (horário local) para resultados de memória.
pub(crate) fn profile_date(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|d| d.format("%d/%m/%Y %H:%M").to_string())
        .unwrap_or_default()
}

/// Abre `lilith.db` e migra (uma vez) as conversas da versão em JSON.
fn open_memory(data: &std::path::Path) -> lilith_memory::Memory {
    let memory = lilith_memory::Memory::open(&data.join("lilith.db")).unwrap_or_else(|e| {
        tracing::error!(error = %e, "lilith.db indisponível — memória só nesta sessão");
        lilith_memory::Memory::in_memory().expect("sqlite em memória")
    });
    let old = data.join("conversations");
    if old.is_dir() {
        let n = memory.import_json_dir(&old);
        tracing::info!(imported = n, "conversas em JSON migradas para o SQLite");
        let _ = std::fs::rename(&old, data.join("conversations.migrated"));
    }
    memory
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

    // O estado é registrado ANTES de qualquer janela existir: o chat carrega
    // rápido e chama comandos logo de cara — com o .manage() dentro do setup,
    // às vezes chegava antes ("state not managed") e o chat travava sem
    // conversa. A única peça que precisa do AppHandle (avisar o painel sobre
    // tarefas) recebe o handle quando o setup roda.
    let app_handle: Arc<std::sync::OnceLock<AppHandle>> = Arc::new(std::sync::OnceLock::new());
    let job_handle = app_handle.clone();
    let jobs = Jobs::new(cfg.agents, move |event| {
        if let Some(handle) = job_handle.get() {
            let _ = handle.emit("lilith://job", &event);
        }
        if let JobEvent::Updated { job } = &event {
            tracing::info!(id = job.id, agent = %job.agent, status = ?job.status, "job");
        }
    });
    let audit_path = data.join("logs").join("audit.jsonl");
    let state = AppState {
        ollama: Arc::new(OllamaClient::new(cfg.ollama)),
        conversations: Mutex::new(HashMap::new()),
        store: Arc::new(open_memory(&data)),
        policy: Arc::new(Policy::load(&data.join("permissions.json"))),
        audit: Arc::new(AuditLog::new(&audit_path)),
        audit_path,
        approvals: Arc::new(Approvals::default()),
        jobs,
        running: Mutex::new(HashMap::new()),
    };

    tauri::Builder::default()
        .manage(state)
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
            let _ = app_handle.set(app.handle().clone());
            app.manage(screen::Screen::new(app.handle().clone()));
            if let Some(cursor) = app.get_webview_window("cursor") {
                let _ = cursor.set_ignore_cursor_events(true);
            }

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
            close_conversation,
            list_conversations,
            load_conversation,
            rename_conversation,
            delete_conversation,
            get_settings,
            save_settings,
            list_facts,
            add_fact,
            forget_fact,
            get_quick_prompts,
            save_quick_prompts,
            toggle_chat,
            new_chat,
            quit_app,
            show_panel_tab,
            hide_window,
            chat_moved,
            lilith_avatar,
            list_jobs,
            cancel_job,
            recent_activity,
            list_rules,
            revoke_rule,
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
