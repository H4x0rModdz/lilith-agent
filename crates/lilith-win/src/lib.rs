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
        "ui.find" | "ui.click" | "ui.type" => {
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
    let element = req_str(params, "element")?;
    let w = window::find(target)?;
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
        _ => type_into(&w, element, params),
    }
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
    // O campo é achado pelo nome; o clique dá foco nele.
    let (field, _) = uia::click(w.id, element, 0)?;
    std::thread::sleep(std::time::Duration::from_millis(200));
    input::type_text(text)?;
    let submit = params["submit"].as_bool().unwrap_or(false);
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
