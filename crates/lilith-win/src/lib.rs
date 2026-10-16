//! Mãos da Lilith no Windows. Cada ação do catálogo do `lilith-brain` tem um
//! handler aqui. Nada neste crate checa permissão — isso é do `lilith-guard`,
//! aplicado pelo app antes de chamar [`dispatch`].

pub mod browsers;
pub mod dev;
pub mod discord;
mod error;
mod handlers;
mod paths;
pub mod input;
pub mod layout;
pub mod ocr;
pub mod phone;
pub mod vitals;
pub mod ui_map;
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
    "browser.tab",
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
    "screen.read",
    "system.status",
    "system.checkup",
    "screen.capture",
    "media.control",
    "media.now_playing",
    "audio.app_volume",
    "discord.voice",
    "discord.status",
    "discord.account",
    "discord.pending",
    "discord.friend",
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
    "dev.errors",
    "dev.git_status",
    "phone.status",
    "phone.notifications",
    "phone.messages",
    "phone.calls",
    "phone.send_sms",
    "settings.open",
    "screen.snip",
];

pub async fn dispatch(action: &str, params: Value) -> Result<Value, WinError> {
    use handlers::*;
    match action {
        "app.open" => apps::open(params).await,
        "app.close" => apps::close(params).await,
        "browser.open" => apps::browser_open(params).await,
        "browser.search" => apps::browser_search(params).await,
        "browser.tab" => apps::browser_tab(params).await,
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
        "screen.read" => screen::read(params).await,
        "system.status" => system::status(params).await,
        "system.checkup" => checkup::checkup(params).await,
        "screen.capture" => capture::capture(params).await,
        "media.control" => media::control(params).await,
        "media.now_playing" => media::now_playing(params).await,
        "audio.app_volume" => media::app_volume(params).await,
        "web.search" => web::search(params).await,
        "web.fetch" => web::fetch(params).await,
        "dev.errors" => handlers::blocking(move || dev::read_errors(&params)).await,
        "dev.git_status" => dev::git_status(&params).await,
        "settings.open" => native::settings_open(params).await,
        "screen.snip" => native::screen_snip(params).await,
        a if a.starts_with("phone.") => {
            let a = a.to_string();
            handlers::blocking(move || match a.as_str() {
                "phone.status" => phone::status(&params),
                "phone.notifications" => phone::notifications(&params),
                "phone.messages" => phone::messages(&params),
                "phone.calls" => phone::calls(&params),
                "phone.send_sms" => phone::send_sms(&params),
                other => Err(WinError::NotFound(format!("ação '{other}'"))),
            })
            .await
        }
        "ui.find" | "ui.click" | "ui.type" | "ui.open_chat" => {
            let a = action.to_string();
            handlers::blocking(move || ui_action(&a, &params)).await
        }
        a if a.starts_with("discord.") => {
            let a = a.to_string();
            handlers::blocking(move || match a.as_str() {
                "discord.voice" => discord::voice(&params),
                "discord.status" => discord::status(&params),
                "discord.account" => discord::account(&params),
                "discord.pending" => discord::pending(&params),
                "discord.friend" => discord::friend(&params),
                other => Err(WinError::NotFound(format!("ação '{other}'"))),
            })
            .await
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
    /// O título da janela traz o nome da conversa aberta: dá para CONFIRMAR
    /// que abrimos a certa (e tentar de novo, ou desistir, se não).
    pub titled: bool,
}

pub(crate) fn chat_search_for(process: &str) -> Option<ChatSearch> {
    let p = paths::fold(process);
    if p.contains("whatsapp") {
        // Desktop: Ctrl+F busca na lista de conversas.
        Some(ChatSearch { app: "whatsapp", shortcut: 'f', titled: false })
    } else if p.contains("discord") {
        // Seletor rápido: DMs, servidores e canais.
        Some(ChatSearch { app: "discord", shortcut: 'k', titled: true })
    } else if p.contains("teams") {
        // Teams: Ctrl+E é a busca; o título vira "Chat | <nome> | … | Microsoft Teams".
        Some(ChatSearch { app: "teams", shortcut: 'e', titled: true })
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

/// O nome procurado aparece em `haystack` (título da janela ou nome do campo
/// de mensagem)? Por palavras inteiras e sem acento: "Ana" não casa com
/// "Mariana", mas "José" casa com "para Jose".
pub(crate) fn names_match(haystack: &str, name: &str) -> bool {
    let words = |s: &str| -> Vec<String> {
        paths::fold(s)
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_string)
            .collect()
    };
    let hay = words(haystack);
    let wanted = words(name);
    !wanted.is_empty() && wanted.iter().all(|w| hay.contains(w))
}

/// Espera (até `max_ms`) o título da janela passar a conter o nome.
fn wait_for_title(id: isize, name: &str, max_ms: u64) -> bool {
    let started = std::time::Instant::now();
    loop {
        let title = window::find(&format!("id:{id}")).map(|f| f.title).unwrap_or_default();
        if names_match(&title, name) {
            return true;
        }
        if started.elapsed().as_millis() as u64 >= max_ms {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
}

/// Já estamos nessa conversa? Dois sinais baratos (sem varrer a árvore): o
/// título da janela (Discord: "@userx - Discord") e o nome do campo de
/// mensagem quando ele está com o foco. Se nenhum dos dois der certeza,
/// devolve false e a busca do app é usada.
fn already_in_chat(w: &window::WindowInfo, name: &str) -> bool {
    // O título mudou desde que a janela foi listada? Lê de novo, é barato.
    let title = window::find(&format!("id:{}", w.id)).map(|f| f.title).unwrap_or_else(|_| w.title.clone());
    if names_match(&title, name) {
        return true;
    }
    window::is_foreground(w.id) && uia::focused_name().is_some_and(|n| names_match(&n, name))
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
    // Já em primeiro plano? Não refaz o foco nem espera.
    let was_in_front = window::is_foreground(w.id);
    if !was_in_front {
        window::focus(w.id)?;
        pause(300);
    }
    // Já estamos nessa conversa? Então não abre a busca de novo.
    if already_in_chat(w, &name) {
        return Ok(serde_json::json!({
            "window": w.title, "opened": name, "already_open": true, "was_in_front": was_in_front,
        }));
    }
    // Tentativa 1: atalho, nome, Enter (abre o primeiro resultado).
    let search_for = |down: bool| -> Result<(), WinError> {
        input::press_ctrl(search.shortcut)?;
        pause(300);
        input::type_text(&name)?;
        // Os resultados chegam depois de uma pausa.
        pause(if down { 1000 } else { 800 });
        if down {
            // Variação: escolhe a primeira SUGESTÃO da lista (Enter direto
            // pode abrir só a página de resultados, como no Teams).
            input::press_down()?;
            pause(150);
        }
        input::press_enter()
    };
    search_for(false)?;
    pause(400);
    let mut verified = None;
    if search.titled {
        // O título da janela diz em que conversa estamos: confirma.
        verified = Some(wait_for_title(w.id, &name, 1800));
        if verified == Some(false) {
            search_for(true)?;
            verified = Some(wait_for_title(w.id, &name, 2200));
        }
        if verified == Some(false) {
            // Não digitar numa conversa errada: o plano para aqui.
            return Err(WinError::Failed(format!(
                "não consegui confirmar que abri a conversa '{name}' no {}",
                search.app
            )));
        }
    } else {
        pause(100);
    }
    Ok(serde_json::json!({
        "window": w.title, "opened": name, "via": format!("busca do {}", search.app),
        "was_in_front": was_in_front, "verified": verified,
    }))
}

/// Retângulo do campo já localizado pelo app (evita uma segunda busca).
fn rect_param(params: &Value) -> Option<window::Rect> {
    let r = &params["field_rect"];
    let (x, y, w, h) = (r["x"].as_i64()?, r["y"].as_i64()?, r["w"].as_i64()?, r["h"].as_i64()?);
    (w > 0 && h > 0).then_some(window::Rect { x: x as i32, y: y as i32, w: w as i32, h: h as i32 })
}

/// Digita no campo de texto chamado `element` (clica nele antes) e, com
/// `submit`, aperta Enter. Sempre depois de trazer a janela para frente.
fn type_into(w: &window::WindowInfo, element: &str, params: &Value) -> Result<Value, WinError> {
    let text = params["text"].as_str().unwrap_or("");
    if text.is_empty() || text.chars().count() > 2000 {
        return Err(WinError::InvalidParams("'text' obrigatório (até 2000 caracteres)".into()));
    }
    // Já está em primeiro plano (o comum: o chat acabou de abrir)? Não espera.
    if !window::is_foreground(w.id) {
        window::focus(w.id)?;
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    // O campo: o que o app já achou (rect no pedido), pelo nome, se o modelo acertou; senão (e é o comum — cada app
    // chama o campo de um jeito) o campo de mensagem da janela: o campo de
    // texto mais baixo e largo. O clique dá foco nele.
    let known = rect_param(params);
    let by_name = if element.is_empty() || known.is_some() {
        None
    } else {
        uia::click(w.id, element, 0).ok().map(|(f, _)| f)
    };
    let field = match by_name {
        Some(f) => f,
        None => {
            // O ponto lembrado (arquivo ui-map.json) ou o que o app já achou:
            // clica e confere quem ficou com o foco. Só se errou, varre de novo.
            let remembered = known.or_else(|| uia::message_input_for(w).ok().map(|f| f.rect));
            let mut done = None;
            if let Some(rect) = remembered {
                let (x, y) = rect.center();
                input::click_at(x, y)?;
                std::thread::sleep(std::time::Duration::from_millis(90));
                if uia::focused_is_text_input() {
                    uia::confirm_message_input(&w.app);
                    done = Some(uia::UiElement::field(rect));
                } else {
                    uia::forget_message_input(&w.app);
                }
            }
            match done {
                Some(f) => f,
                None => {
                    let f = uia::discover_message_input(w)?;
                    let (x, y) = f.rect.center();
                    input::click_at(x, y)?;
                    f
                }
            }
        }
    };
    std::thread::sleep(std::time::Duration::from_millis(120));
    // Como uma pessoa digitando (e @menções de verdade), nunca a mensagem
    // inteira de uma vez.
    input::reset_cancel();
    input::type_human(text, params["mentions"].as_bool().unwrap_or(true))?;
    // "Manda uma mensagem" é digitar E enviar: o padrão é enviar; só não
    // envia se `submit` for false (ex.: "escreve mas não envia").
    let submit = params["submit"].as_bool().unwrap_or(true);
    if submit {
        std::thread::sleep(std::time::Duration::from_millis(100));
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
        assert_eq!(chat_search_for("WhatsApp.Root"), Some(ChatSearch { app: "whatsapp", shortcut: 'f', titled: false }));
        assert_eq!(chat_search_for("Discord"), Some(ChatSearch { app: "discord", shortcut: 'k', titled: true }));
        // Teams novo (ms-teams.exe) e o clássico (Teams.exe).
        assert_eq!(chat_search_for("ms-teams"), Some(ChatSearch { app: "teams", shortcut: 'e', titled: true }));
        assert_eq!(chat_search_for("Teams"), Some(ChatSearch { app: "teams", shortcut: 'e', titled: true }));
        assert_eq!(chat_search_for("notepad"), None);
    }

    #[test]
    fn current_chat_is_matched_by_whole_words() {
        assert!(names_match("@userx - Discord", "userx"));
        // Título do Teams: "Chat | <nome> (convidado) | <org> | <conta> | Microsoft Teams".
        assert!(names_match("Chat | Jose Silva (Guest) | Org | a@b.com | Microsoft Teams", "jose silva"));
        assert!(!names_match("Chat | Jose Silva | Org | Microsoft Teams", "Jose Souza"));
        assert!(names_match("Conversar em @userx", "UserX"));
        assert!(names_match("Digite uma mensagem para José", "jose"));
        assert!(names_match("#chat-geral | Cantinho da are - Discord", "chat geral"));
        assert!(!names_match("Digite uma mensagem para Mariana", "Ana"));
        assert!(!names_match("WhatsApp", "User X"));
        assert!(!names_match("qualquer coisa", ""));
    }

    #[test]
    fn chat_names_are_one_clean_line() {
        assert_eq!(clean_chat_name("  Família \n 🎉  ").as_deref(), Some("Família 🎉"));
        assert_eq!(clean_chat_name("a\u{7}b").as_deref(), Some("ab"));
        assert_eq!(clean_chat_name("   "), None);
        assert_eq!(clean_chat_name(&"x".repeat(200)).unwrap().chars().count(), 60);
    }
}
