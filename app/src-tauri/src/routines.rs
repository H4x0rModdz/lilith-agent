//! Modos e layouts de janelas, executados pelo app.
//!
//! - **Modo**: sequência salva de chamadas do catálogo. Criar (pelo chat ou
//!   pelo painel) passa cada passo pelo código (`parse_step` / `validate_step`)
//!   e mostra a prévia num cartão antes de salvar. Rodar executa os passos um
//!   por um pelo MESMO dispatcher guardado — cada passo passa pelo guard,
//!   aprovações continuam valendo — e devolve o resultado de cada um.
//! - **Layout**: fotografia das janelas (lilith-win `layout`); restaurar abre
//!   os apps que faltam (via `app.open`, guardado) e arrasta cada janela de
//!   volta com o cursor da Lilith. Janelas de administrador ficam como estão.

use crate::dispatcher::GuardedDispatcher;
use crate::AppState;
use lilith_brain::routines as plan;
use lilith_brain::{Dispatcher, ToolCall};
use lilith_memory::{Memory, Mode, ModeStep};
use lilith_win::layout::{self, SavedWindow};
use lilith_win::window;
use lilith_win::WinError;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};

/// Ações de rotina que o app executa.
pub const ROUTINE_ACTIONS: &[&str] = &[
    "mode.run",
    "mode.save",
    "mode.list",
    "mode.delete",
    "layout.save",
    "layout.restore",
    "layout.list",
    "layout.delete",
];

/// Conversa usada quando a ação vem do painel (cartões aparecem na aba ativa).
const PANEL_CONVERSATION: &str = "rotinas";

const YES_SAVE: &str = "Sim, salvar";
const YES_DELETE: &str = "Sim, apagar";
const YES_REPLACE: &str = "Sim, substituir";
const NO: &str = "Não";

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, WinError> + Send + 'static) -> Result<T, WinError> {
    tokio::task::spawn_blocking(f).await.map_err(|e| WinError::Failed(e.to_string()))?
}

fn fold(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' => 'a',
            'é' | 'ê' => 'e',
            'í' => 'i',
            'ó' | 'ô' | 'õ' => 'o',
            'ú' => 'u',
            'ç' => 'c',
            other => other,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Acha pelo nome exato; senão, um único nome parecido ("foc" → "foco").
fn pick_name<'a>(names: impl Iterator<Item = &'a str>, wanted: &str) -> Option<String> {
    let w = fold(wanted);
    if w.is_empty() {
        return None;
    }
    let names: Vec<&str> = names.collect();
    if let Some(n) = names.iter().find(|n| fold(n) == w) {
        return Some(n.to_string());
    }
    let close: Vec<&&str> = names.iter().filter(|n| fold(n).starts_with(&w) || (w.chars().count() >= 4 && fold(n).contains(&w))).collect();
    (close.len() == 1).then(|| close[0].to_string())
}

fn find_mode(store: &Memory, name: &str) -> Result<Mode, WinError> {
    let modes = store.modes();
    pick_name(modes.iter().map(|m| m.name.as_str()), name)
        .and_then(|n| store.mode(&n))
        .ok_or_else(|| {
            let known: Vec<&str> = modes.iter().map(|m| m.name.as_str()).collect();
            let hint = if known.is_empty() { "nenhum modo criado ainda".to_string() } else { format!("modos: {}", known.join(", ")) };
            WinError::NotFound(format!("modo '{name}' ({hint})"))
        })
}

fn find_layout(store: &Memory, name: &str) -> Result<lilith_memory::Layout, WinError> {
    let layouts = store.layouts();
    pick_name(layouts.iter().map(|l| l.name.as_str()), name)
        .and_then(|n| store.layout(&n))
        .ok_or_else(|| {
            let known: Vec<&str> = layouts.iter().map(|l| l.name.as_str()).collect();
            let hint = if known.is_empty() { "nenhum layout salvo ainda".to_string() } else { format!("layouts: {}", known.join(", ")) };
            WinError::NotFound(format!("layout '{name}' ({hint})"))
        })
}

/// Monta os passos de um modo a partir do que veio (chat, modelo ou painel).
/// Ordem de confiança: o código entende a frase (`parse_step`); a frase é
/// igual a um passo já salvo neste modo (edição no painel); a ação proposta
/// pelo modelo passa na validação do catálogo. O resto é recusado.
pub fn build_steps(items: &[Value], existing: Option<&Mode>) -> (Vec<ModeStep>, Vec<String>) {
    let mut steps = Vec::new();
    let mut rejected = Vec::new();
    for item in items {
        let say = item.as_str().or_else(|| item["say"].as_str()).unwrap_or("").trim().to_string();
        if !say.is_empty() {
            if let Some(call) = plan::parse_step(&say) {
                steps.push(ModeStep { action: call.action, params: call.params, say });
                continue;
            }
            // No editor, o passo aparece pela frase dita ou pela descrição.
            let same = |s: &&ModeStep| fold(&s.say) == fold(&say) || fold(&plan::describe(&s.action, &s.params)) == fold(&say);
            if let Some(kept) = existing.and_then(|m| m.steps.iter().find(same)) {
                steps.push(kept.clone());
                continue;
            }
        }
        let action = item["action"].as_str().unwrap_or("").trim();
        if !action.is_empty() {
            match plan::validate_step(action, &item["params"]) {
                Ok(params) => {
                    steps.push(ModeStep { action: action.into(), params, say });
                    continue;
                }
                Err(e) => {
                    rejected.push(if say.is_empty() { format!("{action} ({e})") } else { format!("{say} ({e})") });
                    continue;
                }
            }
        }
        if !say.is_empty() {
            rejected.push(say);
        }
    }
    (steps, rejected)
}

/// "Code", "chrome (2)" — os apps de um layout.
fn summarize_apps(saved: &[SavedWindow]) -> Vec<String> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for w in saved {
        match counts.iter_mut().find(|(a, _)| fold(a) == fold(&w.app)) {
            Some((_, n)) => *n += 1,
            None => counts.push((w.app.clone(), 1)),
        }
    }
    counts.into_iter().map(|(a, n)| if n > 1 { format!("{a} ({n})") } else { a }).collect()
}

fn saved_windows(l: &lilith_memory::Layout) -> Vec<SavedWindow> {
    serde_json::from_value(l.windows.clone()).unwrap_or_default()
}

fn mode_summary(m: &Mode) -> Value {
    json!({
        "name": m.name,
        "steps": m.steps.iter().map(|s| plan::describe(&s.action, &s.params)).collect::<Vec<_>>(),
    })
}

impl GuardedDispatcher {
    pub(crate) async fn routine_action(&self, action: &str, params: &Value) -> Result<Value, WinError> {
        let name = params["name"].as_str().unwrap_or("").trim().to_string();
        match action {
            "mode.run" => self.run_mode(&name).await,
            "mode.save" => self.save_mode(&name, params).await,
            "mode.list" => Ok(json!({ "modes": self.store.modes().iter().map(mode_summary).collect::<Vec<_>>() })),
            "mode.delete" => self.delete_mode(&name).await,
            "layout.save" => self.save_layout(&name).await,
            "layout.restore" => self.restore_layout(&name).await,
            "layout.list" => Ok(json!({ "layouts": layouts_json(&self.store) })),
            "layout.delete" => self.delete_layout(&name).await,
            other => Err(WinError::NotFound(format!("ação '{other}'"))),
        }
    }

    /// Enquanto a rotina roda, as ações dela contam como pedidas no chat (o
    /// usuário confirmou os passos ao salvar). Devolve a intenção anterior.
    fn allow_steps(&self, actions: impl Iterator<Item = String>) -> Option<Vec<String>> {
        let mut intent = self.chat_intent.lock().unwrap();
        let previous = intent.clone();
        let mut next = previous.clone().unwrap_or_default();
        next.extend(actions);
        *intent = Some(next);
        previous
    }

    fn restore_intent(&self, previous: Option<Vec<String>>) {
        *self.chat_intent.lock().unwrap() = previous;
    }

    async fn confirm(&self, title: String, yes: &str) -> bool {
        self.choices.ask(&self.app, &self.conversation, &title, vec![yes.to_string(), NO.to_string()]).await.as_deref() == Some(yes)
    }

    async fn run_mode(&self, name: &str) -> Result<Value, WinError> {
        let mode = find_mode(&self.store, name)?;
        let previous = self.allow_steps(mode.steps.iter().map(|s| s.action.clone()));
        let mut results = Vec::new();
        for (i, s) in mode.steps.iter().enumerate() {
            let label = plan::describe(&s.action, &s.params);
            // Confere de novo: o catálogo pode ter mudado desde que o modo foi salvo.
            let outcome = match plan::validate_step(&s.action, &s.params) {
                Err(e) => json!({ "label": label, "ok": false, "error": e }),
                Ok(params) => {
                    if i > 0 {
                        // Um respiro entre passos (um app recém-aberto aparecer).
                        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    }
                    let response = Dispatcher::dispatch(self, &ToolCall { action: s.action.clone(), params }).await;
                    if response["status"] == "success" {
                        json!({ "label": label, "ok": true })
                    } else {
                        json!({ "label": label, "ok": false, "error": response["error"]["message"].as_str().unwrap_or("deu erro") })
                    }
                }
            };
            results.push(outcome);
        }
        self.restore_intent(previous);
        Ok(json!({ "name": mode.name, "steps": results }))
    }

    async fn save_mode(&self, name: &str, params: &Value) -> Result<Value, WinError> {
        let name = lilith_memory::clean_name(name)
            .ok_or_else(|| WinError::InvalidParams("nome do modo inválido (até 40 letras, números, espaço ou hífen)".into()))?;
        let items = params["steps"].as_array().cloned().unwrap_or_default();
        if items.is_empty() {
            return Err(WinError::InvalidParams("informe os passos do modo em `steps`".into()));
        }
        let existing = self.store.mode(&name);
        let (steps, rejected) = build_steps(&items, existing.as_ref());
        if steps.is_empty() {
            return Err(WinError::InvalidParams(format!("não entendi nenhum passo: {}", rejected.join("; "))));
        }
        if steps.len() > lilith_memory::MAX_MODE_STEPS {
            return Err(WinError::InvalidParams(format!("um modo tem no máximo {} passos", lilith_memory::MAX_MODE_STEPS)));
        }
        let lines: Vec<String> = steps.iter().enumerate().map(|(i, s)| format!("{}. {}", i + 1, plan::describe(&s.action, &s.params))).collect();
        let mut title = format!(
            "{} o modo «{name}» com estes passos?\n{}",
            if existing.is_some() { "Substituir" } else { "Criar" },
            lines.join("\n")
        );
        if !rejected.is_empty() {
            title.push_str(&format!("\nNão entendi (fica de fora): {}", rejected.join("; ")));
        }
        if !self.confirm(title, YES_SAVE).await {
            return Ok(json!({ "saved": false, "name": name }));
        }
        let ok = self.store.save_mode(&name, &steps);
        let _ = self.app.emit("lilith://routines", ());
        Ok(json!({ "saved": ok, "name": name, "steps": steps.len(), "replaced": existing.is_some(), "skipped": rejected }))
    }

    async fn delete_mode(&self, name: &str) -> Result<Value, WinError> {
        let mode = find_mode(&self.store, name)?;
        let steps: Vec<String> = mode.steps.iter().map(|s| plan::describe(&s.action, &s.params)).collect();
        if !self.confirm(format!("Apagar o modo «{}»? ({})", mode.name, steps.join(", ")), YES_DELETE).await {
            return Ok(json!({ "deleted": false, "name": mode.name }));
        }
        let ok = self.store.remove_mode(&mode.name);
        let _ = self.app.emit("lilith://routines", ());
        Ok(json!({ "deleted": ok, "name": mode.name }))
    }

    async fn save_layout(&self, name: &str) -> Result<Value, WinError> {
        let name = lilith_memory::clean_name(name)
            .ok_or_else(|| WinError::InvalidParams("nome do layout inválido (até 40 letras, números, espaço ou hífen)".into()))?;
        let saved = blocking(|| Ok(layout::capture(&window::list(), &layout::screens()))).await?;
        if saved.is_empty() {
            return Err(WinError::NotFound("nenhuma janela visível para salvar".into()));
        }
        let apps = summarize_apps(&saved);
        let replaced = self.store.layout(&name).is_some();
        if replaced {
            let title = format!("Já existe o layout «{name}». Substituir pelo arranjo de agora? ({})", apps.join(", "));
            if !self.confirm(title, YES_REPLACE).await {
                return Ok(json!({ "saved": false, "name": name }));
            }
        }
        let windows = serde_json::to_value(&saved).map_err(|e| WinError::Failed(e.to_string()))?;
        let ok = self.store.save_layout(&name, &windows);
        let _ = self.app.emit("lilith://routines", ());
        Ok(json!({ "saved": ok, "name": name, "count": saved.len(), "apps": apps, "replaced": replaced }))
    }

    async fn delete_layout(&self, name: &str) -> Result<Value, WinError> {
        let l = find_layout(&self.store, name)?;
        let apps = summarize_apps(&saved_windows(&l));
        if !self.confirm(format!("Apagar o layout «{}»? ({})", l.name, apps.join(", ")), YES_DELETE).await {
            return Ok(json!({ "deleted": false, "name": l.name }));
        }
        let ok = self.store.remove_layout(&l.name);
        let _ = self.app.emit("lilith://routines", ());
        Ok(json!({ "deleted": ok, "name": l.name }))
    }

    async fn restore_layout(&self, name: &str) -> Result<Value, WinError> {
        let l = find_layout(&self.store, name)?;
        let saved = saved_windows(&l);
        if saved.is_empty() {
            return Err(WinError::Failed(format!("o layout '{}' está vazio ou corrompido", l.name)));
        }
        let snapshot = || blocking(|| Ok((window::list(), layout::screens())));
        let (windows, screens) = snapshot().await?;
        let mut restore = layout::plan_restore(&saved, &windows, &screens);

        // Abre os apps que faltam — pelo guard, como qualquer app.open.
        let previous = self.allow_steps(std::iter::once("app.open".to_string()));
        let (mut opened, mut failed_open, mut launched) = (Vec::new(), Vec::new(), Vec::new());
        for app in &restore.missing {
            let Some(launch) = layout::launch_name(app) else {
                failed_open.push(app.clone());
                continue;
            };
            let response = Dispatcher::dispatch(self, &ToolCall { action: "app.open".into(), params: json!({ "app": launch }) }).await;
            if response["status"] == "success" {
                opened.push(launch);
                launched.push(fold(app));
            } else {
                failed_open.push(format!("{launch} ({})", response["error"]["message"].as_str().unwrap_or("erro")));
            }
        }
        self.restore_intent(previous);

        // Espera as janelas novas (até 15 s) e planeja de novo com elas.
        if !launched.is_empty() {
            for _ in 0..30 {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let (now, _) = snapshot().await?;
                if launched.iter().all(|a| now.iter().any(|w| fold(&w.app) == *a)) {
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(600)).await;
            let (windows, screens) = snapshot().await?;
            restore = layout::plan_restore(&saved, &windows, &screens);
        }
        // Abriu, mas a janela não apareceu a tempo (ou é de outro processo).
        let not_found: Vec<String> = restore.missing.iter().filter(|a| launched.contains(&fold(a))).cloned().collect();

        // Arrasta cada janela de volta com o cursor da Lilith.
        let screen = self.app.state::<Arc<crate::screen::Screen>>().inner().clone();
        let _presence = screen.begin();
        let (mut moved, mut failed) = (Vec::new(), Vec::new());
        for mv in &restore.moves {
            let id = mv.window.id;
            if mv.window.maximized || mv.window.minimized {
                // Maximizada/minimizada não aceita posição: restaura antes.
                blocking(move || window::set_state(id, "restore")).await?;
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            }
            let from = blocking(move || window::find(&format!("id:{id}"))).await.map(|w| w.frame).unwrap_or(mv.window.frame);
            let plan = window::MovePlan {
                id,
                title: mv.window.title.clone(),
                from,
                to: mv.to,
                maximize_after: mv.maximize,
                elevated: false,
            };
            match screen.drag(&plan).await {
                Ok(()) => moved.push(mv.window.title.clone()),
                Err(e) => {
                    tracing::warn!(error = %e, "não consegui mover uma janela do layout");
                    failed.push(mv.window.app.clone());
                }
            }
        }
        Ok(json!({
            "name": l.name,
            "moved": moved,
            "in_place": restore.in_place.len(),
            "opened": opened,
            "failed_open": failed_open,
            "not_found": not_found,
            "failed": failed,
            "skipped_admin": restore.skipped_admin,
        }))
    }
}

fn layouts_json(store: &Memory) -> Vec<Value> {
    store
        .layouts()
        .iter()
        .map(|l| {
            let saved = saved_windows(l);
            json!({ "name": l.name, "count": saved.len(), "apps": summarize_apps(&saved), "updated_ms": l.updated_ms })
        })
        .collect()
}

// ── comandos do painel (Configurações) ────────────────────────────────────

/// Dispatcher de uma ação pedida no painel: o mesmo guard do chat.
fn panel_dispatcher(app: &AppHandle, state: &AppState, action: &str) -> GuardedDispatcher {
    GuardedDispatcher {
        conversation: PANEL_CONVERSATION.into(),
        store: state.store.clone(),
        jobs: state.jobs.clone(),
        channel: lilith_guard::CHANNEL_LOCAL.into(),
        app: app.clone(),
        policy: state.policy.clone(),
        audit: state.audit.clone(),
        approvals: state.approvals.clone(),
        choices: state.choices.clone(),
        tainted: Default::default(),
        // O clique no painel é o pedido do usuário.
        chat_intent: Mutex::new(Some(vec![action.to_string()])),
        opened_chats: Default::default(),
    }
}

/// Roda uma ação de rotina pelo painel: guard, auditoria, e a resposta
/// (montada pelo código) vai também para o chat.
async fn run_from_panel(app: AppHandle, d: GuardedDispatcher, action: &str, params: Value) -> Result<String, String> {
    let envelope = Dispatcher::dispatch(&d, &ToolCall { action: action.into(), params }).await;
    let text = if envelope["status"] == "success" {
        lilith_brain::shortcut::render(action, &envelope["result"]).unwrap_or_else(|| "Pronto!".into())
    } else {
        format!("Não consegui: {}", envelope["error"]["message"].as_str().unwrap_or("deu erro"))
    };
    crate::alerts::notice(&app, &text, "sparkles");
    if envelope["status"] == "success" {
        Ok(text)
    } else {
        Err(text)
    }
}

#[tauri::command]
pub fn list_modes(state: State<'_, AppState>) -> Vec<Value> {
    state
        .store
        .modes()
        .iter()
        .map(|m| {
            let steps: Vec<Value> = m
                .steps
                .iter()
                .map(|s| json!({ "say": s.say, "label": plan::describe(&s.action, &s.params), "action": s.action }))
                .collect();
            json!({ "name": m.name, "steps": steps, "updated_ms": m.updated_ms })
        })
        .collect()
}

/// Prévia do editor: o que o código entendeu de cada linha.
#[tauri::command]
pub fn preview_mode(lines: Vec<String>, name: Option<String>, state: State<'_, AppState>) -> Vec<Value> {
    let existing = name.as_deref().and_then(|n| state.store.mode(n));
    lines
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .take(lilith_memory::MAX_MODE_STEPS + 1)
        .map(|line| {
            let (steps, _) = build_steps(&[json!(line)], existing.as_ref());
            match steps.first() {
                Some(s) => json!({ "line": line, "ok": true, "label": plan::describe(&s.action, &s.params) }),
                None => json!({ "line": line, "ok": false }),
            }
        })
        .collect()
}

/// Salva um modo escrito no editor (uma frase por linha). Linhas que o
/// código não entende impedem o salvamento — a prévia já mostrou quais.
#[tauri::command]
pub fn save_mode_lines(name: String, lines: Vec<String>, previous: Option<String>, app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let clean = lilith_memory::clean_name(&name).ok_or("Nome inválido: use até 40 letras, números, espaço ou hífen.")?;
    let items: Vec<Value> = lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).map(|l| json!(l)).collect();
    let existing = previous.as_deref().or(Some(clean.as_str())).and_then(|n| state.store.mode(n));
    let (steps, rejected) = build_steps(&items, existing.as_ref());
    if !rejected.is_empty() {
        return Err(format!("Não entendi: {}", rejected.join("; ")));
    }
    if steps.is_empty() || steps.len() > lilith_memory::MAX_MODE_STEPS {
        return Err(format!("Um modo precisa de 1 a {} passos.", lilith_memory::MAX_MODE_STEPS));
    }
    if !state.store.save_mode(&clean, &steps) {
        return Err("Não consegui salvar.".into());
    }
    // Renomeado no editor: o nome antigo sai.
    if let Some(old) = previous.filter(|p| fold(p) != fold(&clean)) {
        state.store.remove_mode(&old);
    }
    let _ = app.emit("lilith://routines", ());
    Ok(clean)
}

#[tauri::command]
pub fn remove_mode(name: String, app: AppHandle, state: State<'_, AppState>) -> bool {
    let ok = state.store.remove_mode(&name);
    let _ = app.emit("lilith://routines", ());
    ok
}

#[tauri::command]
pub async fn run_mode(name: String, app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let d = panel_dispatcher(&app, &state, "mode.run");
    run_from_panel(app, d, "mode.run", json!({ "name": name })).await
}

#[tauri::command]
pub fn list_layouts(state: State<'_, AppState>) -> Vec<Value> {
    layouts_json(&state.store)
}

#[tauri::command]
pub async fn save_layout_now(name: String, app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let d = panel_dispatcher(&app, &state, "layout.save");
    run_from_panel(app, d, "layout.save", json!({ "name": name })).await
}

#[tauri::command]
pub async fn restore_layout(name: String, app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let d = panel_dispatcher(&app, &state, "layout.restore");
    run_from_panel(app, d, "layout.restore", json!({ "name": name })).await
}

#[tauri::command]
pub fn remove_layout(name: String, app: AppHandle, state: State<'_, AppState>) -> bool {
    let ok = state.store.remove_layout(&name);
    let _ = app.emit("lilith://routines", ());
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_are_decided_by_code_first() {
        let items = vec![
            json!({ "say": "muta o pc", "action": "app.close", "params": { "app": "x" } }),
            json!({ "say": "abre o projeto no editor", "action": "app.open", "params": { "app": "code", "lixo": 1 } }),
            json!({ "say": "faz um café", "action": "coffee.make" }),
            json!("volume 20"),
            json!({ "say": "lê o chat", "action": "window.read", "params": { "target": "discord" } }),
        ];
        let (steps, rejected) = build_steps(&items, None);
        let got: Vec<(&str, &Value)> = steps.iter().map(|s| (s.action.as_str(), &s.params)).collect();
        assert_eq!(
            got,
            vec![
                ("audio.set_mute", &json!({ "muted": true })),
                ("app.open", &json!({ "app": "code" })),
                ("audio.set_volume", &json!({ "level": 20 })),
            ],
            "a frase entendida pelo código vale mais que o palpite do modelo"
        );
        assert_eq!(rejected.len(), 2);
        assert!(rejected[0].starts_with("faz um café"));
        assert!(rejected[1].contains("não pode ser passo"));
    }

    #[test]
    fn editing_keeps_steps_the_code_cannot_reparse() {
        let existing = Mode {
            name: "foco".into(),
            steps: vec![ModeStep { action: "app.open".into(), params: json!({ "app": "code" }), say: "abre o projeto no editor".into() }],
            updated_ms: 0,
        };
        let (steps, rejected) = build_steps(&[json!("Abre o projeto no editor"), json!("muta")], Some(&existing));
        assert!(rejected.is_empty());
        assert_eq!(steps[0].params, json!({ "app": "code" }));
        assert_eq!(steps[1].action, "audio.set_mute");
    }

    #[test]
    fn names_are_found_exactly_or_by_a_unique_prefix() {
        let names = ["foco", "stream", "fotos"];
        assert_eq!(pick_name(names.iter().copied(), "Foco").as_deref(), Some("foco"));
        assert_eq!(pick_name(names.iter().copied(), "str").as_deref(), Some("stream"));
        assert_eq!(pick_name(names.iter().copied(), "fo"), None, "ambíguo");
        assert_eq!(pick_name(names.iter().copied(), "jogo"), None);
    }

    #[test]
    fn layout_apps_are_summarized() {
        let w = |app: &str| SavedWindow {
            app: app.into(),
            title: String::new(),
            monitor: 1,
            rel: window::Rect { x: 0, y: 0, w: 1, h: 1 },
            work_w: 1,
            work_h: 1,
            maximized: false,
        };
        assert_eq!(summarize_apps(&[w("chrome"), w("Code"), w("Chrome")]), vec!["chrome (2)", "Code"]);
    }
}
