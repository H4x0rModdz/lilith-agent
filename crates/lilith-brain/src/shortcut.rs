//! Atalhos sem IA: comandos curtos e inequívocos ("volume 30", "muta o pc",
//! "minimiza o discord", "abre o spotify") viram a chamada direta, sem passar
//! pelo modelo — instantâneo e sem chance de interpretar errado. As frases são
//! ancoradas (a frase INTEIRA precisa casar); qualquer dúvida cai no fluxo
//! normal do modelo. A execução continua passando pelo dispatcher/guard.

use crate::tools::ToolCall;
use regex::Regex;
use serde_json::json;
use std::sync::OnceLock;

/// Chamadas a executar, em ordem, e a resposta quando dão certo.
#[derive(Debug, Clone, PartialEq)]
pub struct Shortcut {
    pub calls: Vec<ToolCall>,
    pub reply: String,
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("regex de atalho válida"))
}

/// Minúsculas, sem pontuação final nem educação ("por favor", "pra mim"…).
fn normalize(text: &str) -> String {
    let mut t = text.trim().to_lowercase();
    t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    loop {
        let before = t.clone();
        t = t.trim_end_matches(['!', '.', '?', ',', ' ']).to_string();
        for suffix in [" por favor", " pfv", " pf", " pra mim", " para mim", " aí", " ai", " agora", " please"] {
            if let Some(rest) = t.strip_suffix(suffix) {
                t = rest.to_string();
            }
        }
        if t == before {
            return t;
        }
    }
}

/// Apelidos comuns → nome do programa.
fn alias(name: &str) -> String {
    match name {
        "zap" | "zapzap" | "whats" | "wpp" | "zap zap" => "whatsapp".into(),
        other => other.to_string(),
    }
}

/// Sites conhecidos → endereço. "abre o github" é navegador, não programa.
const SITES: &[(&str, &str)] = &[
    ("youtube", "https://www.youtube.com"),
    ("github", "https://github.com"),
    ("google", "https://www.google.com"),
    ("gmail", "https://mail.google.com"),
    ("wikipedia", "https://www.wikipedia.org"),
    ("twitter", "https://twitter.com"),
    ("facebook", "https://www.facebook.com"),
    ("instagram", "https://www.instagram.com"),
    ("reddit", "https://www.reddit.com"),
    ("netflix", "https://www.netflix.com"),
    ("twitch", "https://www.twitch.tv"),
    ("linkedin", "https://www.linkedin.com"),
    ("stackoverflow", "https://stackoverflow.com"),
    ("tiktok", "https://www.tiktok.com"),
    ("pinterest", "https://www.pinterest.com"),
    ("chatgpt", "https://chatgpt.com"),
];

/// Outros nomes que também não são programas.
const NOT_APPS: &[&str] = &["x", "maps", "drive"];

/// O texto cita um site conhecido?
pub fn mentions_site(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.split(|c: char| !c.is_alphanumeric()).any(|w| SITES.iter().any(|(s, _)| *s == w))
}

/// "abre o youtube", "vai pro gmail", "abre o github numa janela privada".
fn site_shortcut(t: &str) -> Option<Shortcut> {
    const FILLER: &[&str] = &[
        "o", "a", "no", "na", "pro", "pra", "para", "do", "da", "de", "site", "numa", "num", "uma", "um", "em", "in",
        "the", "janela", "window", "nova", "novo", "new", "privada", "private", "anônima", "anonima", "anônimo",
        "anonimo", "aba", "tab", "modo", "navegador", "to",
    ];
    let words: Vec<&str> = t.split_whitespace().collect();
    if !matches!(words.first().copied(), Some("abre" | "abra" | "entra" | "vai" | "open" | "go" | "navega")) {
        return None;
    }
    let mut site = None;
    for w in &words[1..] {
        if let Some((_, url)) = SITES.iter().find(|(s, _)| s == w) {
            if site.replace(*url).is_some() {
                return None;
            }
        } else if !FILLER.contains(w) {
            return None;
        }
    }
    let private = words.iter().any(|w| matches!(*w, "privada" | "private" | "anônima" | "anonima" | "anônimo" | "anonimo"));
    let window = words.iter().any(|w| matches!(*w, "janela" | "window")) && words.iter().any(|w| matches!(*w, "nova" | "novo" | "new"));
    let mode = if private { "private" } else if window { "window" } else { "tab" };
    Some(Shortcut {
        calls: vec![call("browser.open", json!({ "url": site?, "mode": mode }))],
        reply: "Abri!".into(),
    })
}

/// Programas onde "clica em X" navega pela interface.
const UI_APPS: &str = "discord|spotify|steam|teams|whatsapp|telegram|slack";

/// "clica em histórico no discord", "vai na aba downloads do spotify".
fn ui_shortcut(t: &str) -> Option<Shortcut> {
    static CLICK: OnceLock<Regex> = OnceLock::new();
    static TAB: OnceLock<Regex> = OnceLock::new();
    let click = re(&CLICK, &format!(r"^(?:clica|clique|aperta|aperte)\s+(.+?)\s+(?:no|na|do|da)\s+({UI_APPS})$"));
    let tab = re(&TAB, &format!(r"^vai\s+(?:na|pra|para\s+a)\s+aba\s+(.+?)\s+(?:no|na|do|da)\s+({UI_APPS})$"));
    let c = click.captures(t).or_else(|| tab.captures(t))?;
    let mut element = c[1].trim();
    for lead in ["em ", "no ", "na ", "nos ", "nas ", "aba ", "botão ", "o ", "a "] {
        while let Some(rest) = element.strip_prefix(lead) {
            element = rest;
        }
    }
    if element.is_empty() || element.split_whitespace().count() > 3 {
        return None;
    }
    Some(Shortcut {
        calls: vec![call("ui.click", json!({ "target": &c[2], "element": element }))],
        reply: "Pronto!".into(),
    })
}

/// Palavras que indicam um pedido mais complexo que "abrir um programa".
const COMPLEX: &[&str] = &[
    " e ", ",", " depois", " também", " tambem", " pra ", " para ", " no ", " na ", " em ", " com ", " aba", "aba ",
    "pasta", "arquivo", "site", "navegador", "janela", "outra", "outro", "mais um", "nova", "novo", "pesquis",
    "procura", "link", "http", "www", "/", "\\", ":",
];

fn plain_target(t: &str) -> Option<String> {
    let t = t.trim();
    let padded = format!(" {t} ");
    if t.is_empty() || t.chars().count() > 30 || COMPLEX.iter().any(|w| padded.contains(w)) {
        return None;
    }
    let name = alias(t);
    if NOT_APPS.contains(&name.as_str()) || SITES.iter().any(|(s, _)| *s == name) || name.starts_with("o ") || name.starts_with("a ") {
        return None;
    }
    Some(name)
}

fn call(action: &str, params: serde_json::Value) -> ToolCall {
    ToolCall { action: action.into(), params }
}

pub fn parse(text: &str) -> Option<Shortcut> {
    let t = normalize(text);
    if t.is_empty() || t.chars().count() > 60 {
        return None;
    }

    // Volume com número: "volume 30", "coloca o volume em 25%", "set the volume to 40".
    static VOLUME: OnceLock<Regex> = OnceLock::new();
    let volume = re(
        &VOLUME,
        r"^(?:(?:coloca|põe|poe|deixa|muda|define|ajusta|abaixa|diminui|aumenta|sobe|set|put)\s+)?(?:o\s+|the\s+)?(?:volume|som|sound)\s*(?:pra|para|em|no|a|to|:)?\s*(\d{1,3})\s*(?:%|por\s*cento|percent)?$",
    );
    if let Some(c) = volume.captures(&t) {
        let level: u32 = c[1].parse().ok()?;
        if level <= 100 {
            return Some(Shortcut {
                calls: vec![call("audio.set_volume", json!({ "level": level }))],
                reply: format!("Volume em {level}%."),
            });
        }
    }

    // Mudo.
    static MUTE: OnceLock<Regex> = OnceLock::new();
    static UNMUTE: OnceLock<Regex> = OnceLock::new();
    let mute = re(
        &MUTE,
        r"^(?:(?:muta|mutar|mute|silencia|silenciar)(?:\s+(?:o|a|the))?(?:\s+(?:pc|computador|som|áudio|audio|volume|sound|computer))?|(?:coloca|põe|poe|deixa)(?:\s+o\s+(?:pc|som|computador|áudio))?\s+no\s+mudo|tira\s+o\s+som)$",
    );
    let unmute = re(
        &UNMUTE,
        r"^(?:(?:desmuta|desmutar|unmute)(?:\s+(?:o|a|the))?(?:\s+(?:pc|computador|som|áudio|audio|sound|computer))?|tira(?:\s+o\s+(?:pc|som|computador))?\s+do\s+mudo|tira\s+o\s+mudo|volta\s+o\s+som|liga\s+o\s+som)$",
    );
    if mute.is_match(&t) {
        return Some(Shortcut { calls: vec![call("audio.set_mute", json!({ "muted": true }))], reply: "Mutado.".into() });
    }
    if unmute.is_match(&t) {
        return Some(Shortcut { calls: vec![call("audio.set_mute", json!({ "muted": false }))], reply: "Som de volta.".into() });
    }

    // "Minimiza as janelas do monitor 1": uma chamada só, pelo número do
    // monitor (o modelo errava ao escolher janela por janela).
    static MONITOR_ALL: OnceLock<Regex> = OnceLock::new();
    let monitor_all = re(
        &MONITOR_ALL,
        r"^(?:minimiza|minimize|esconde|some com)\s+(?:(?:todas\s+)?(?:as\s+)?janelas|tudo|todas)\s+(?:d[oa]|n[oa])\s+(?:meu\s+|minha\s+)?(?:monitor|tela)\s*(\d)$",
    );
    if let Some(c) = monitor_all.captures(&t) {
        let monitor: u32 = c[1].parse().ok()?;
        if monitor >= 1 {
            return Some(Shortcut {
                calls: vec![call("window.minimize", json!({ "monitor": monitor }))],
                reply: format!("Pronto! Minimizei as janelas do monitor {monitor}."),
            });
        }
    }

    // Minimizar / maximizar / restaurar uma janela.
    static WINDOW: OnceLock<Regex> = OnceLock::new();
    let window = re(
        &WINDOW,
        r"^(minimiza|minimize|maximiza|maximize|restaura|restore)\s+(?:a\s+janela\s+d[oae]s?\s+|o\s+|a\s+|the\s+)?([\p{L}0-9][\p{L}0-9 \-]*)$",
    );
    if let Some(c) = window.captures(&t) {
        let target = plain_target(&c[2])?;
        let action = match &c[1] {
            "minimiza" | "minimize" => "window.minimize",
            "maximiza" | "maximize" => "window.maximize",
            _ => "window.restore",
        };
        return Some(Shortcut { calls: vec![call(action, json!({ "target": target }))], reply: "Pronto!".into() });
    }

    // Site conhecido e cliques dentro de programas.
    if let Some(sc) = site_shortcut(&t).or_else(|| ui_shortcut(&t)) {
        return Some(sc);
    }

    // Abrir um programa.
    static OPEN: OnceLock<Regex> = OnceLock::new();
    let open = re(
        &OPEN,
        r"^(?:abre|abra|abrir|inicia|iniciar|roda|open|launch|start)\s+(?:o\s+|a\s+|the\s+)?([\p{L}0-9][\p{L}0-9 \-]*)$",
    );
    if let Some(c) = open.captures(&t) {
        let app = plain_target(&c[1])?;
        return Some(Shortcut { calls: vec![call("app.open", json!({ "app": app }))], reply: "Abri!".into() });
    }
    None
}

/// "xpto quer dizer abrir o spotify" → ("xpto", "abrir o spotify"). Só
/// definições explícitas, curtas, de uma palavra.
pub fn parse_definition(text: &str) -> Option<(String, String)> {
    static DEF: OnceLock<Regex> = OnceLock::new();
    let def = re(
        &DEF,
        r#"^(?:lembra\s+que\s+|anota\s+que\s+|saiba\s+que\s+)?["'“‘]?([\p{L}0-9]{2,20})["'”’]?\s+(?:quer\s+dizer|significa|é\s+o\s+mesmo\s+que|=)\s+["'“‘]?(.{2,100}?)["'”’]?[.!]?$"#,
    );
    let t = text.trim();
    let c = def.captures(t)?;
    let (term, meaning) = (c[1].to_lowercase(), c[2].trim().to_string());
    // "isso quer dizer que…" é conversa, não glossário.
    if matches!(term.as_str(), "isso" | "aquilo" | "isto" | "ele" | "ela" | "que" | "tudo" | "nada") {
        return None;
    }
    Some((term, meaning))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str) -> Option<(String, serde_json::Value)> {
        parse(text).map(|s| (s.calls[0].action.clone(), s.calls[0].params.clone()))
    }

    #[test]
    fn volume_and_mute() {
        assert_eq!(one("volume 30"), Some(("audio.set_volume".into(), json!({ "level": 30 }))));
        assert_eq!(one("Coloca o volume em 25%!"), Some(("audio.set_volume".into(), json!({ "level": 25 }))));
        assert_eq!(one("set the volume to 40"), Some(("audio.set_volume".into(), json!({ "level": 40 }))));
        assert_eq!(one("volume 300"), None);
        assert_eq!(one("muta o pc").unwrap().0, "audio.set_mute");
        assert_eq!(one("tira o mudo").unwrap().1, json!({ "muted": false }));
        assert_eq!(one("aumenta o volume"), None, "sem número vai pro modelo");
    }

    #[test]
    fn windows_and_apps() {
        assert_eq!(one("minimiza o discord"), Some(("window.minimize".into(), json!({ "target": "discord" }))));
        assert_eq!(one("maximiza a janela do spotify por favor").unwrap().0, "window.maximize");
        assert_eq!(
            one("abre o github numa janela privada"),
            Some(("browser.open".into(), json!({ "url": "https://github.com", "mode": "private" })))
        );
        assert_eq!(one("vai pro gmail").unwrap().0, "browser.open");
        assert_eq!(
            one("clica em histórico no discord"),
            Some(("ui.click".into(), json!({ "target": "discord", "element": "histórico" })))
        );
        assert_eq!(one("vai na aba downloads do spotify").unwrap().1["element"], "downloads");
        assert_eq!(
            one("minimiza todas as janelas do monitor 1"),
            Some(("window.minimize".into(), json!({ "monitor": 1 })))
        );
        assert_eq!(one("esconde tudo da tela 2!").unwrap().1, json!({ "monitor": 2 }));
        assert_eq!(one("minimiza o discord").unwrap().1, json!({ "target": "discord" }), "uma janela só continua igual");
        assert_eq!(one("abre o zap"), Some(("app.open".into(), json!({ "app": "whatsapp" }))));
        assert_eq!(one("abre o bloco de notas pra mim").unwrap().1, json!({ "app": "bloco de notas" }));
        // Casos que o modelo resolve melhor:
        for t in [
            "abre o github e pesquisa rust",
            "abre uma aba e pesquisa gatos",
            "abre mais um bloco de notas",
            "minimiza o discord e abre o spotify",
            "abre o spotify e toca algo",
            "joga o discord pro lado",
            "abre o youtube e o github",
            "abre a pasta downloads",
        ] {
            assert_eq!(one(t), None, "{t}");
        }
    }

    #[test]
    fn explicit_definitions_only() {
        assert_eq!(parse_definition("fdp quer dizer amigo"), Some(("fdp".into(), "amigo".into())));
        assert_eq!(
            parse_definition("lembra que 'lanche' significa abrir o spotify."),
            Some(("lanche".into(), "abrir o spotify".into()))
        );
        assert_eq!(parse_definition("isso quer dizer que não deu certo"), None);
        assert_eq!(parse_definition("abre o spotify"), None);
    }
}
