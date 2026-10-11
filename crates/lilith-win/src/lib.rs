//! Mãos da Lilith no Windows. Cada ação do catálogo do `lilith-brain` tem um
//! handler aqui. Nada neste crate checa permissão — isso é do `lilith-guard`,
//! aplicado pelo app antes de chamar [`dispatch`].

mod error;
mod handlers;
mod paths;
pub mod input;
pub mod uia;
pub mod window;

pub use error::WinError;
pub use paths::{is_personal_read, resolve as resolve_path};
use serde_json::Value;

/// Ações que este crate sabe executar.
pub const ACTIONS: &[&str] = &[
    "app.open",
    "app.close",
    "browser.open",
    "browser.search",
    "file.open",
    "file.run",
    "file.find",
    "file.list",
    "file.read",
    "file.move",
    "file.copy",
    "file.delete",
    "download.file",
    "desktop.set_wallpaper",
    "audio.set_volume",
    "audio.get_volume",
    "audio.set_mute",
    "clipboard.set",
    "clipboard.get",
    "system.power",
    "web.search",
    "web.fetch",
    "window.list",
    "window.read",
    "window.focus",
    "window.minimize",
    "window.maximize",
    "window.restore",
    "window.close",
    "window.move",
    "window.tile",
    "ui.find",
    "ui.click",
    "ui.type",
    "ui.open_chat",
];

pub async fn dispatch(action: &str, params: Value) -> Result<Value, WinError> {
    use handlers::*;
    match action {
        "app.open" => apps::open(params).await,
        "app.close" => apps::close(params).await,
        "browser.open" => apps::browser_open(params).await,
        "browser.search" => apps::browser_search(params).await,
        "file.open" => apps::file_open(params).await,
        "file.run" => apps::file_run(params).await,
        "file.find" => files::find(params).await,
        "file.list" => files::list(params).await,
        "file.read" => files::read(params).await,
        "file.move" => files::move_(params).await,
        "file.copy" => files::copy(params).await,
        "file.delete" => files::delete(params).await,
        "download.file" => download::file(params).await,
        "desktop.set_wallpaper" => desktop::set_wallpaper(params).await,
        "audio.set_volume" => audio::set_volume(params).await,
        "audio.get_volume" => audio::get_volume().await,
        "audio.set_mute" => audio::set_mute(params).await,
        "clipboard.set" => clipboard::set(params).await,
        "clipboard.get" => clipboard::get().await,
        "system.power" => power::run(params).await,
        "web.search" => web::search(params).await,
        "web.fetch" => web::fetch(params).await,
        "ui.find" | "ui.click" | "ui.type" | "ui.open_chat" => {
            let a = action.to_string();
            handlers::blocking(move || ui_action(&a, &params)).await
        }
        a if a.starts_with("window.") => {
            let a = a.to_string();
            handlers::blocking(move || window::handle(&a, &params)).await
        }
        other => Err(WinError::NotFound(format!("ação '{other}'"))),
    }
}

/// ui.find / ui.click sem animação (o app anima o cursor por cima).
fn ui_action(action: &str, params: &Value) -> Result<Value, WinError> {
    let target = params["target"].as_str().unwrap_or("active");
    if action == "ui.open_chat" {
        let w = window::find(target)?;
        return open_chat(&w, req_str(params, "name")?);
    }
    let w = window::find(target)?;
    if action == "ui.type" {
        return type_into(&w, params["element"].as_str().unwrap_or("").trim(), params);
    }
    let element = req_str(params, "element")?;
    match action {
        "ui.find" => {
            let found = uia::find_elements(w.id, element, 8)?;
            Ok(serde_json::json!({ "window": w.title, "results": found }))
        }
        "ui.click" => {
            let index = params["index"].as_u64().unwrap_or(0) as usize;
            let (clicked, via) = uia::click(w.id, element, index)?;
            Ok(serde_json::json!({ "window": w.title, "clicked": clicked, "via": via }))
        }
        _ => unreachable!("ui.type tratado acima"),
    }
}

/// Como achar uma conversa pelo teclado em cada app: o atalho da busca do
/// próprio app. É o caminho confiável — acha qualquer conversa (pessoa ou
/// grupo) mesmo fora da lista visível, que em apps web é virtualizada.
#[derive(Debug, PartialEq)]
pub(crate) struct ChatSearch {
    pub app: &'static str,
    pub shortcut: char,
}

pub(crate) fn chat_search_for(process: &str) -> Option<ChatSearch> {
    let p = paths::fold(process);
    if p.contains("whatsapp") {
        // Desktop: Ctrl+F busca na lista de conversas.
        Some(ChatSearch { app: "whatsapp", shortcut: 'f' })
    } else if p.contains("discord") {
        // Seletor rápido: DMs, servidores e canais.
        Some(ChatSearch { app: "discord", shortcut: 'k' })
    } else {
        None
    }
}

/// Nome seguro para digitar na busca: uma linha, sem controle, até 60.
pub(crate) fn clean_chat_name(raw: &str) -> Option<String> {
    let name: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let name: String = name.chars().filter(|c| !c.is_control()).take(60).collect();
    (!name.trim().is_empty()).then_some(name)
}

/// Abre uma conversa pelo nome usando a busca do app (atalho, nome, Enter).
fn open_chat(w: &window::WindowInfo, raw_name: &str) -> Result<Value, WinError> {
    let name = clean_chat_name(raw_name).ok_or_else(|| WinError::InvalidParams("'name' vazio".into()))?;
    let search = chat_search_for(&w.app).ok_or_else(|| {
        WinError::Unavailable(format!(
            "não sei buscar conversas em '{}'; use ui.find + ui.click",
            w.app
        ))
    })?;
    let pause = |ms: u64| std::thread::sleep(std::time::Duration::from_millis(ms));
    window::focus(w.id)?;
    pause(350);
    input::press_ctrl(search.shortcut)?;
    pause(300);
    input::type_text(&name)?;
    // Os resultados chegam depois de uma pausa; Enter abre o primeiro.
    pause(800);
    input::press_enter()?;
    pause(500);
    Ok(serde_json::json!({ "window": w.title, "opened": name, "via": format!("busca do {}", search.app) }))
}

/// Digita no campo de texto chamado `element` (clica nele antes) e, com
/// `submit`, aperta Enter. Sempre depois de trazer a janela para frente.
fn type_into(w: &window::WindowInfo, element: &str, params: &Value) -> Result<Value, WinError> {
    let text = params["text"].as_str().unwrap_or("");
    if text.is_empty() || text.chars().count() > 2000 {
        return Err(WinError::InvalidParams("'text' obrigatório (até 2000 caracteres)".into()));
    }
    window::focus(w.id)?;
    std::thread::sleep(std::time::Duration::from_millis(250));
    // O campo: pelo nome, se o modelo acertou; senão (e é o comum — cada app
    // chama o campo de um jeito) o campo de mensagem da janela: o campo de
    // texto mais baixo e largo. O clique dá foco nele.
    let by_name = if element.is_empty() { None } else { uia::click(w.id, element, 0).ok().map(|(f, _)| f) };
    let field = match by_name {
        Some(f) => f,
        None => {
            let f = uia::message_input(w.id)?;
            let (x, y) = f.rect.center();
            input::click_at(x, y)?;
            f
        }
    };
    std::thread::sleep(std::time::Duration::from_millis(200));
    // Como uma pessoa digitando (e @menções de verdade), nunca a mensagem
    // inteira de uma vez.
    input::reset_cancel();
    input::type_human(text, params["mentions"].as_bool().unwrap_or(true))?;
    // "Manda uma mensagem" é digitar E enviar: o padrão é enviar; só não
    // envia se `submit` for false (ex.: "escreve mas não envia").
    let submit = params["submit"].as_bool().unwrap_or(true);
    if submit {
        std::thread::sleep(std::time::Duration::from_millis(150));
        input::press_enter()?;
    }
    Ok(serde_json::json!({ "window": w.title, "field": field.name, "typed_chars": text.chars().count(), "sent": submit }))
}

/// Lê um parâmetro string obrigatório e não vazio.
pub(crate) fn req_str<'a>(params: &'a Value, key: &str) -> Result<&'a str, WinError> {
    params[key]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| WinError::InvalidParams(format!("parâmetro '{key}' obrigatório")))
}

#[cfg(test)]
mod open_chat_tests {
    use super::*;

    #[test]
    fn search_shortcuts_per_app() {
        assert_eq!(chat_search_for("WhatsApp.Root"), Some(ChatSearch { app: "whatsapp", shortcut: 'f' }));
        assert_eq!(chat_search_for("Discord"), Some(ChatSearch { app: "discord", shortcut: 'k' }));
        assert_eq!(chat_search_for("notepad"), None);
    }

    #[test]
    fn chat_names_are_one_clean_line() {
        assert_eq!(clean_chat_name("  Família \n 🎉  ").as_deref(), Some("Família 🎉"));
        assert_eq!(clean_chat_name("a\u{7}b").as_deref(), Some("ab"));
        assert_eq!(clean_chat_name("   "), None);
        assert_eq!(clean_chat_name(&"x".repeat(200)).unwrap().chars().count(), 60);
    }
}
