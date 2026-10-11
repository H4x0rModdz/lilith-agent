//! Mãos da Lilith no Windows. Cada ação do catálogo do `lilith-brain` tem um
//! handler aqui. Nada neste crate checa permissão — isso é do `lilith-guard`,
//! aplicado pelo app antes de chamar [`dispatch`].

mod error;
mod handlers;
mod paths;
pub mod window;

pub use error::WinError;
pub use paths::resolve as resolve_path;
use serde_json::Value;

/// Ações que este crate sabe executar.
pub const ACTIONS: &[&str] = &[
    "app.open",
    "app.close",
    "browser.open",
    "file.open",
    "file.run",
    "file.find",
    "file.list",
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
    "window.focus",
    "window.minimize",
    "window.maximize",
    "window.restore",
    "window.close",
    "window.move",
    "window.tile",
];

pub async fn dispatch(action: &str, params: Value) -> Result<Value, WinError> {
    use handlers::*;
    match action {
        "app.open" => apps::open(params).await,
        "app.close" => apps::close(params).await,
        "browser.open" => apps::browser_open(params).await,
        "file.open" => apps::file_open(params).await,
        "file.run" => apps::file_run(params).await,
        "file.find" => files::find(params).await,
        "file.list" => files::list(params).await,
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
        a if a.starts_with("window.") => {
            let a = a.to_string();
            handlers::blocking(move || window::handle(&a, &params)).await
        }
        other => Err(WinError::NotFound(format!("ação '{other}'"))),
    }
}

/// Lê um parâmetro string obrigatório e não vazio.
pub(crate) fn req_str<'a>(params: &'a Value, key: &str) -> Result<&'a str, WinError> {
    params[key]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| WinError::InvalidParams(format!("parâmetro '{key}' obrigatório")))
}
