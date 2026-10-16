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

    // ── Discord: decidido por código ──────────────────────────────────────
    static D_MIC: OnceLock<Regex> = OnceLock::new();
    static D_AUDIO: OnceLock<Regex> = OnceLock::new();
    static D_STATUS: OnceLock<Regex> = OnceLock::new();
    static D_PENDING: OnceLock<Regex> = OnceLock::new();
    static D_FRIEND: OnceLock<Regex> = OnceLock::new();
    static D_ADD: OnceLock<Regex> = OnceLock::new();
    static D_ACCOUNT: OnceLock<Regex> = OnceLock::new();
    static D_DEVICE: OnceLock<Regex> = OnceLock::new();
    const IN_DISCORD: &str = r"(?:\s+(?:no|do|pelo)\s+discord)?";
    let discord = |action: &str, params: serde_json::Value| Shortcut { calls: vec![call(action, params)], reply: "Pronto!".into() };
    if let Some(c) = re(&D_MIC, &format!(r"^(muta|silencia|desativa|desliga|fecha|desmuta|ativa|liga|abre)\s+(?:o\s+|meu\s+)?(?:microfone|mic){IN_DISCORD}$")).captures(&t) {
        let on = matches!(&c[1], "desmuta" | "ativa" | "liga" | "abre");
        return Some(discord("discord.voice", json!({ "mic": if on { "unmute" } else { "mute" } })));
    }
    if let Some(c) = re(&D_AUDIO, r"^(silencia|desativa|desliga|muta|ensurdece|ativa|liga|desmuta|volta|desensurdece)\s+(?:o\s+|meu\s+)?(?:[aá]udio|som|fone)\s+(?:no|do)\s+discord$").captures(&t) {
        let on = matches!(&c[1], "ativa" | "liga" | "desmuta" | "volta" | "desensurdece");
        return Some(discord("discord.voice", json!({ "audio": if on { "undeafen" } else { "deafen" } })));
    }
    if let Some(c) = re(&D_STATUS, &format!(r"^(?:no\s+discord\s+)?(?:fica|me\s+(?:deixa|coloca|p[oõ]e)|coloca|deixa|p[oõ]e|muda\s+(?:o\s+|meu\s+)?status\s+(?:pra|para))\s+(?:o\s+status\s+)?(?:como\s+|em\s+)?(online|dispon[ií]vel|ausente|invis[ií]vel|n[aã]o\s+perturbe|ocupado){IN_DISCORD}$")).captures(&t) {
        let status = match fold(&c[1]).as_str() {
            "ausente" => "idle",
            "invisivel" => "invisible",
            s if s.starts_with("nao") || s == "ocupado" => "dnd",
            _ => "online",
        };
        return Some(discord("discord.status", json!({ "status": status })));
    }
    if re(&D_PENDING, &format!(r"^(?:quais\s+(?:s[aã]o\s+)?(?:os\s+)?|mostra\s+(?:os\s+)?|tem\s+|lista\s+(?:os\s+)?)?(?:pedidos?\s+(?:de\s+amizade\s+)?pendentes?|pedidos?\s+de\s+amizade|amizades?\s+pendentes?){IN_DISCORD}$")).is_match(&t) {
        return Some(discord("discord.pending", json!({})));
    }
    if let Some(c) = re(&D_FRIEND, &format!(r"^(aceita|recusa|ignora|rejeita)\s+(?:o\s+)?(?:pedido\s+(?:de\s+amizade\s+)?(?:d[oa]\s+)?)?(.+?){IN_DISCORD}$")).captures(&t) {
        let op = if &c[1] == "aceita" { "accept" } else { "decline" };
        return Some(discord("discord.friend", json!({ "op": op, "name": c[2].trim() })));
    }
    if let Some(c) = re(&D_ADD, r"^(?:adiciona\s+(?:o\s+|a\s+)?(.+?)\s+(?:no|ao)\s+discord|manda\s+(?:um\s+)?pedido\s+de\s+amizade\s+pr[oa]\s+(.+?)(?:\s+no\s+discord)?)$").captures(&t) {
        let name = c.get(1).or(c.get(2)).map(|m| m.as_str().trim()).unwrap_or("");
        return Some(discord("discord.friend", json!({ "op": "add", "name": name })));
    }
    if let Some(c) = re(&D_ACCOUNT, r"^(?:muda|troca|alterna)\s+(?:a\s+|de\s+)?conta\s+(?:do\s+discord\s+)?(?:pro|pra|para)\s+(?:a\s+)?(.+?)(?:\s+no\s+discord)?$").captures(&t) {
        return Some(discord("discord.account", json!({ "account": c[1].trim() })));
    }
    if let Some(c) = re(&D_DEVICE, r"^(?:muda|troca|coloca|p[oõ]e|escolhe)\s+(?:o\s+|a\s+)?(?:dispositivo\s+de\s+)?(microfone|entrada|sa[ií]da|fone|alto-falante|som)\s+(?:d[oe]\s+[aá]udio\s+)?(?:(?:do|no)\s+discord)(?:\s+(?:pro|pra|para|no|na)\s+(?:o\s+|a\s+)?(.+?))?$|^(?:muda|troca|coloca|p[oõ]e)\s+(?:o\s+|a\s+)?(microfone|entrada|sa[ií]da|fone|alto-falante|som)\s+(?:pro|pra|para|no|na)\s+(?:o\s+|a\s+)?(.+?)\s+no\s+discord$").captures(&t) {
        let kind = c.get(1).or(c.get(3)).map(|m| m.as_str()).unwrap_or("");
        let target = c.get(2).or(c.get(4)).map(|m| m.as_str().trim()).unwrap_or("");
        let key = if matches!(fold(kind).as_str(), "microfone" | "entrada") { "input_device" } else { "output_device" };
        // Sem dizer qual: o app mostra a lista para escolher.
        return Some(discord("discord.voice", json!({ key: if target.is_empty() { "ask" } else { target } })));
    }

    // ── mídia: decidido por código ────────────────────────────────────────
    static MEDIA_PAUSE: OnceLock<Regex> = OnceLock::new();
    static MEDIA_PLAY: OnceLock<Regex> = OnceLock::new();
    static MEDIA_NEXT: OnceLock<Regex> = OnceLock::new();
    static MEDIA_PREV: OnceLock<Regex> = OnceLock::new();
    static MEDIA_STOP: OnceLock<Regex> = OnceLock::new();
    static NOW_PLAYING: OnceLock<Regex> = OnceLock::new();
    const WHAT: &str = r"(?:\s+(?:a|o|essa|esta))?(?:\s+(?:m[uú]sica|v[ií]deo|som|faixa|playlist|spotify))?";
    let media = |action: &str| Shortcut {
        calls: vec![call("media.control", json!({ "action": action }))],
        reply: "Pronto!".into(),
    };
    if re(&MEDIA_PAUSE, &format!(r"^(?:pausa|pause){WHAT}$")).is_match(&t) {
        return Some(media("pause"));
    }
    if re(&MEDIA_PLAY, &format!(r"^(?:despausa|d[aá]\s+play|play|continua|retoma|volta\s+a\s+tocar|toca){WHAT}$")).is_match(&t) {
        return Some(media("play"));
    }
    if re(&MEDIA_NEXT, r"^(?:pr[oó]xima(?:\s+(?:m[uú]sica|faixa))?|(?:pula|passa|avan[cç]a)(?:\s+(?:a|essa|esta))?(?:\s+(?:m[uú]sica|faixa))?|next)$").is_match(&t) {
        return Some(media("next"));
    }
    if re(&MEDIA_PREV, r"^(?:(?:m[uú]sica|faixa)\s+anterior|volta\s+(?:a|uma|pra)\s+(?:m[uú]sica|faixa)(?:\s+anterior)?|previous)$").is_match(&t) {
        return Some(media("previous"));
    }
    if re(&MEDIA_STOP, r"^para\s+(?:a\s+|o\s+)?(?:m[uú]sica|v[ií]deo)$").is_match(&t) {
        return Some(media("stop"));
    }
    if re(
        &NOW_PLAYING,
        r"^(?:(?:o\s+)?que\s+(?:m[uú]sica\s+)?(?:t[aá]|est[aá])\s+tocando|que\s+m[uú]sica\s+[eé]\s+essa|qual\s+(?:[eé]\s+)?(?:a\s+)?m[uú]sica(?:\s+(?:t[aá]|est[aá])\s+tocando)?)$",
    )
    .is_match(&t)
    {
        return Some(Shortcut { calls: vec![call("media.now_playing", json!({}))], reply: "Pronto!".into() });
    }

    // ── autodiagnóstico: decidido por código ──────────────────────────────
    static SELF_CHECK: OnceLock<Regex> = OnceLock::new();
    if re(
        &SELF_CHECK,
        r"^(?:(?:voc[eê]|vc)\s+(?:t[aá]|est[aá])\s+(?:funcionando|ok|bem)(?:\s+direito)?|faz\s+(?:um\s+)?(?:checkup|diagn[oó]stico|autodiagn[oó]stico)(?:\s+(?:de|em)\s+(?:voc[eê]|vc|si\s+mesma))?|autodiagn[oó]stico|(?:o\s+)?que\s+(?:t[aá]|est[aá])\s+errado\s+com\s+(?:voc[eê]|vc)|diagnose)$",
    )
    .is_match(&t)
    {
        return Some(Shortcut { calls: vec![call("self.diagnose", json!({}))], reply: "Pronto!".into() });
    }

    // ── print: decidido por código ────────────────────────────────────────
    static PRINT: OnceLock<Regex> = OnceLock::new();
    if let Some(c) = re(
        &PRINT,
        r"^(?:tira|faz|bate|me\s+manda)\s+(?:um\s+|uma\s+)?(?:print(?:screen)?|screenshot|captura(?:\s+de\s+tela)?)(?:\s+(?:da|de|do|dos)\s+(.+))?$",
    )
    .captures(&t)
    {
        let what = c.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        static MONITOR_N: OnceLock<Regex> = OnceLock::new();
        let params = if what.is_empty() || matches!(what, "tela" | "tela toda" | "tela inteira" | "tudo" | "todos os monitores" | "todas as telas" | "monitores") {
            json!({})
        } else if let Some(m) = re(&MONITOR_N, r"^(?:monitor|tela)\s*(\d)$").captures(what) {
            json!({ "monitor": m[1].parse::<u32>().unwrap_or(1) })
        } else if matches!(what, "uma parte" | "uma area" | "uma área" | "parte da tela" | "uma parte da tela") {
            // Recorte com seleção de área: o modelo escolhe a ferramenta de recorte.
            return None;
        } else {
            let target = what.trim_start_matches("janela do ").trim_start_matches("janela da ").trim_start_matches("o ").trim_start_matches("a ");
            json!({ "target": alias(target) })
        };
        return Some(Shortcut { calls: vec![call("screen.capture", params)], reply: "Pronto!".into() });
    }

    // ── checkup completo do PC: decidido por código ───────────────────────
    static PC_CHECKUP: OnceLock<Regex> = OnceLock::new();
    let pcx = r"(?:o\s+|meu\s+|do\s+|no\s+)?(?:pc|computador|note|notebook)";
    if re(
        &PC_CHECKUP,
        &format!(
            r"^(?:(?:por\s*que|pq|porque)\s+{pcx}\s+(?:t[aá]|est[aá])\s+(?:t[aã]o\s+)?(?:lento|travando|pesado|esquentando|quente|reiniciando|desligando)|(?:faz\s+(?:um\s+)?)?(?:checkup|diagn[oó]stico)\s+completo(?:\s+{pcx})?|(?:faz\s+(?:um\s+)?)?checkup\s+{pcx}|diagn[oó]stico\s+(?:geral|detalhado)(?:\s+{pcx})?)$"
        ),
    )
    .is_match(&t)
    {
        return Some(Shortcut { calls: vec![call("system.checkup", json!({}))], reply: "Pronto!".into() });
    }

    // ── status do PC: decidido por código ─────────────────────────────────
    static PC_STATUS: OnceLock<Regex> = OnceLock::new();
    let pc = r"(?:o\s+|meu\s+)?(?:pc|computador|note|notebook)";
    if re(
        &PC_STATUS,
        &format!(
            r"^(?:(?:por\s*que|pq|porque)\s+{pc}\s+(?:t[aá]|est[aá])\s+(?:t[aã]o\s+)?(?:lento|travando|pesado|esquentando|quente)|como\s+(?:t[aá]|est[aá])\s+{pc}|(?:o\s+)?que\s+(?:t[aá]|est[aá])\s+(?:pesando|travando|consumindo)(?:\s+(?:o|no|meu)\s+(?:pc|computador))?|diagn[oó]stico(?:\s+do\s+(?:pc|computador))?|status\s+do\s+(?:pc|computador))$"
        ),
    )
    .is_match(&t)
    {
        return Some(Shortcut { calls: vec![call("system.status", json!({}))], reply: "Pronto!".into() });
    }

    // ── volume de um app: decidido por código ─────────────────────────────
    static APP_VOLUME: OnceLock<Regex> = OnceLock::new();
    static APP_MUTE: OnceLock<Regex> = OnceLock::new();
    if let Some(c) = re(
        &APP_VOLUME,
        r"^(?:(?:coloca|p[oõ]e|deixa|muda|abaixa|baixa|diminui|aumenta|sobe)\s+)?(?:o\s+volume\s+d[oa]\s+|o\s+)([\p{L}0-9][\p{L}0-9 ]*?)\s+(?:pra|para|em|a)\s+(\d{1,3})\s*(?:%|por\s*cento)?$",
    )
    .captures(&t)
    {
        let app = c[1].trim().to_string();
        let level: u32 = c[2].parse().ok()?;
        if level <= 100 && !matches!(app.as_str(), "volume" | "som" | "pc" | "computador") {
            return Some(Shortcut {
                calls: vec![call("audio.app_volume", json!({ "app": alias(&app), "level": level }))],
                reply: "Pronto!".into(),
            });
        }
    }
    if let Some(c) = re(&APP_MUTE, r"^(muta|silencia|desmuta)\s+o\s+([\p{L}0-9][\p{L}0-9 ]*)$").captures(&t) {
        let app = c[2].trim().to_string();
        if !matches!(app.as_str(), "pc" | "computador" | "som" | "áudio" | "audio" | "volume") {
            return Some(Shortcut {
                calls: vec![call("audio.app_volume", json!({ "app": alias(&app), "muted": &c[1] != "desmuta" }))],
                reply: "Pronto!".into(),
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

/// Minúsculas, sem acento e espaços normalizados (para comparar rótulos).
fn fold(s: &str) -> String {
    let plain: String = s
        .to_lowercase()
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
        .collect();
    plain.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Resposta montada pelo código a partir do resultado (sem modelo).
pub fn render(action: &str, result: &serde_json::Value) -> Option<String> {
    let text = |k: &str| result[k].as_str().unwrap_or("").trim().to_string();
    let track = || {
        let (title, artist, app) = (text("title"), text("artist"), text("app"));
        if title.is_empty() {
            return String::new();
        }
        let by = if artist.is_empty() { String::new() } else { format!(" — {artist}") };
        let on = if app.is_empty() { String::new() } else { format!(" (no {app})") };
        format!("{title}{by}{on}")
    };
    match action {
        "discord.voice" => {
            let mut parts = Vec::new();
            if let Some(m) = result["mic_muted"].as_bool() {
                parts.push(if m { "microfone mutado" } else { "microfone ligado" }.to_string());
            }
            if let Some(a) = result["audio_off"].as_bool() {
                parts.push(if a { "áudio desativado" } else { "áudio ligado" }.to_string());
            }
            for (k, label) in [("input_device", "entrada"), ("output_device", "saída")] {
                if result[k]["cancelled"] == true {
                    parts.push(format!("{label} sem mudança (você não escolheu)"));
                } else if let Some(sel) = result[k]["selected"].as_str() {
                    parts.push(format!("{label}: {sel}"));
                } else if let Some(list) = result[k]["devices"].as_array() {
                    let names: Vec<&str> = list.iter().filter_map(|v| v.as_str()).collect();
                    parts.push(format!("dispositivos de {label}: {}", names.join(", ")));
                }
            }
            if result["failed"] == true {
                let what = if result["mic_muted"].is_boolean() { "o microfone" } else { "o áudio" };
                return Some(format!("Tentei mudar {what} do Discord, mas ele não mudou de estado. Agora: {}.", parts.join("; ")));
            }
            if parts.is_empty() {
                return Some("Pronto!".into());
            }
            let mut s = parts.join("; ");
            s[..1].make_ascii_uppercase();
            Some(format!("No Discord: {s}."))
        }
        "discord.status" => Some(format!("Pronto, seu status no Discord agora é {}.", text("status").to_lowercase())),
        "discord.account" => Some(match result["accounts"].as_array() {
            Some(list) => format!("Contas no Discord: {}.", list.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", ")),
            None => format!("Troquei para a conta {} no Discord.", text("account")),
        }),
        "discord.pending" => {
            let list = |k: &str| result[k].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
            let (got, sent) = (list("received"), list("sent"));
            Some(match (got.is_empty(), sent.is_empty()) {
                (true, true) => "Nenhum pedido de amizade pendente no Discord.".into(),
                (false, true) => format!("Pedidos de amizade recebidos: {got}."),
                (true, false) => format!("Nenhum pedido recebido; você enviou para: {sent}."),
                (false, false) => format!("Recebidos: {got}. Enviados por você: {sent}."),
            })
        }
        "discord.friend" => Some(match text("op").as_str() {
            "accept" => format!("Aceitei o pedido de {}.", text("name")),
            "decline" => format!("Recusei o pedido de {}.", text("name")),
            _ if result["confirmed"] == true => format!("Pedido de amizade enviado para {}.", text("name")),
            _ => format!("Mandei o pedido para {}, mas não vi a confirmação do Discord — confere lá.", text("name")),
        }),
        "media.now_playing" => Some(if track().is_empty() {
            "Nada tocando agora.".into()
        } else if result["playing"] == true {
            format!("Tocando agora: {}.", track())
        } else {
            format!("Está pausado: {}.", track())
        }),
        "media.control" => Some(match result["done"].as_str().unwrap_or("") {
            "pause" => "Pausei.".into(),
            "stop" => "Parei a música.".into(),
            "next" | "previous" if !track().is_empty() => format!("Pronto! Agora: {}.", track()),
            "play" | "play_pause" if !track().is_empty() => format!("Tocando: {}.", track()),
            _ => "Pronto!".into(),
        }),
        "audio.app_volume" => {
            let apps = result["apps"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
            Some(match (result["muted"].as_bool(), result["level"].as_u64()) {
                (Some(true), _) => format!("Mutei o {apps}."),
                (Some(false), Some(l)) => format!("Som do {apps} de volta, em {l}%."),
                (Some(false), None) => format!("Som do {apps} de volta."),
                (None, Some(l)) => format!("Volume do {apps} em {l}%."),
                _ => "Pronto!".into(),
            })
        }
        "screen.capture" => Some(if result["copied"] == true {
            format!("Print tirado ({}): salvei em {} e copiei para a área de transferência — é só colar.", text("source"), text("path"))
        } else {
            format!("Print tirado ({}): salvei em {}.", text("source"), text("path"))
        }),
        "system.checkup" => {
            let findings = result["findings"].as_array().cloned().unwrap_or_default();
            let section = |level: &str, title: &str| {
                let items: Vec<String> = findings
                    .iter()
                    .filter(|x| x["level"] == level)
                    .map(|x| {
                        let fix = x["fix"].as_str().filter(|f| !f.is_empty()).map(|f| format!(" → {f}")).unwrap_or_default();
                        format!("- {}{fix}", x["text"].as_str().unwrap_or(""))
                    })
                    .collect();
                if items.is_empty() { String::new() } else { format!("\n{title}:\n{}", items.join("\n")) }
            };
            let (p, w, t) = (result["problems"].as_u64().unwrap_or(0), result["warnings"].as_u64().unwrap_or(0), result["tips"].as_u64().unwrap_or(0));
            let head = if p + w + t == 0 {
                "Checkup completo: seu PC está saudável, não achei nada para corrigir.".to_string()
            } else {
                format!("Checkup completo do PC: {p} problema(s), {w} ponto(s) de atenção e {t} dica(s).")
            };
            let overview: Vec<String> = result["overview"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| format!("- {s}"))).collect())
                .unwrap_or_default();
            Some(format!(
                "{head}{}{}{}\nVisão geral:\n{}",
                section("problem", "Problemas"),
                section("warn", "Atenção"),
                section("tip", "Dicas"),
                overview.join("\n")
            ))
        }
        "self.diagnose" => {
            let checks = result["checks"].as_array().cloned().unwrap_or_default();
            let mut lines = Vec::new();
            for c in &checks {
                let tag = match c["level"].as_str().unwrap_or("") {
                    "ok" => "OK",
                    "warn" => "Atenção",
                    _ => "Problema",
                };
                let mut line = format!("- {}: {} — {}", c["name"].as_str().unwrap_or(""), tag, c["detail"].as_str().unwrap_or(""));
                if let Some(fix) = c["fix"].as_str().filter(|f| !f.is_empty()) {
                    line.push_str(&format!(" → {fix}"));
                }
                lines.push(line);
            }
            let head = match result["overall"].as_str().unwrap_or("") {
                "ok" => "Fiz um checkup em mim: tudo funcionando.",
                "warn" => "Fiz um checkup em mim: funcionando, mas com pontos de atenção.",
                _ => "Fiz um checkup em mim e achei problemas:",
            };
            Some(format!("{head}
{}", lines.join("
")))
        }
        "system.status" => {
            let findings: Vec<String> =
                result["findings"].as_array().map(|f| f.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
            let s = &result["snapshot"];
            if findings.is_empty() {
                Some(format!(
                    "Tudo tranquilo no PC: CPU em {}%, memória {} de {} GB, nada pesando fora do normal.",
                    s["cpu"], s["memory_used_gb"], s["memory_total_gb"]
                ))
            } else {
                Some(format!("Olhei o PC e achei isto:\n- {}", findings.join("\n- ")))
            }
        }
        _ => None,
    }
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
    fn discord_controls_are_decided_by_code() {
        assert_eq!(one("muta meu microfone no discord"), Some(("discord.voice".into(), json!({ "mic": "mute" }))));
        assert_eq!(one("desmuta o mic").unwrap().1, json!({ "mic": "unmute" }));
        assert_eq!(one("desativa o áudio no discord").unwrap().1, json!({ "audio": "deafen" }));
        assert_eq!(one("fica invisível no discord").unwrap().1, json!({ "status": "invisible" }));
        assert_eq!(one("me deixa ausente no discord").unwrap().1, json!({ "status": "idle" }));
        assert_eq!(one("coloca não perturbe no discord").unwrap().1, json!({ "status": "dnd" }));
        assert_eq!(one("quais são os pedidos de amizade pendentes?").unwrap().0, "discord.pending");
        assert_eq!(one("aceita o pedido do user x").unwrap().1, json!({ "op": "accept", "name": "user x" }));
        assert_eq!(one("recusa o pedido de amizade da user y no discord").unwrap().1, json!({ "op": "decline", "name": "user y" }));
        assert_eq!(one("adiciona o user z no discord").unwrap().1, json!({ "op": "add", "name": "user z" }));
        assert_eq!(one("troca a conta do discord pra conta b").unwrap().1, json!({ "account": "conta b" }));
        assert_eq!(one("troca o microfone do discord pro jbl").unwrap().1, json!({ "input_device": "jbl" }));
        assert_eq!(one("muda a saída do discord pra nvidia").unwrap().1, json!({ "output_device": "nvidia" }));
        assert_eq!(one("troca o microfone do discord").unwrap().1, json!({ "input_device": "ask" }), "sem dizer qual: pergunta");
        assert_eq!(one("muda a saída de áudio do discord").unwrap().1, json!({ "output_device": "ask" }));
        assert_eq!(one("troca o microfone pro jbl no discord").unwrap().1, json!({ "input_device": "jbl" }));
        assert_eq!(one("muta o pc").unwrap().0, "audio.set_mute", "mudo geral continua igual");
    }

    #[test]
    fn discord_replies_are_rendered() {
        assert_eq!(render("discord.voice", &json!({ "mic_muted": true })).unwrap(), "No Discord: Microfone mutado.");
        assert_eq!(render("discord.status", &json!({ "status": "Invisível" })).unwrap(), "Pronto, seu status no Discord agora é invisível.");
        assert_eq!(render("discord.pending", &json!({ "received": [], "sent": [] })).unwrap(), "Nenhum pedido de amizade pendente no Discord.");
        assert_eq!(render("discord.friend", &json!({ "op": "accept", "name": "user x" })).unwrap(), "Aceitei o pedido de user x.");
        let devs = json!({ "input_device": { "devices": ["A", "B"] } });
        assert_eq!(render("discord.voice", &devs).unwrap(), "No Discord: Dispositivos de entrada: A, B.");
    }

    #[test]
    fn prints_are_decided_by_code() {
        assert_eq!(one("tira um print"), Some(("screen.capture".into(), json!({}))));
        assert_eq!(one("tira um print da tela toda").unwrap().1, json!({}));
        assert_eq!(one("tira um print do monitor 1").unwrap().1, json!({ "monitor": 1 }));
        assert_eq!(one("faz uma captura de tela do monitor 2").unwrap().1, json!({ "monitor": 2 }));
        assert_eq!(one("tira um print do discord").unwrap().1, json!({ "target": "discord" }));
        assert_eq!(one("tira um print da janela do google").unwrap().1, json!({ "target": "google" }));
        assert_eq!(one("tira um print de uma parte da tela"), None, "recorte com seleção vai para a ferramenta de recorte");
        let r = json!({ "source": "monitor 1", "path": "C:\\x\\a.png", "copied": true });
        assert!(render("screen.capture", &r).unwrap().contains("copiei para a área de transferência"));
    }

    #[test]
    fn checkup_report_is_rendered_by_code() {
        let r = json!({ "problems": 1, "warnings": 0, "tips": 1, "findings": [
            { "level": "problem", "text": "O disco X reporta saúde \"Warning\"", "fix": "Faça backup." },
            { "level": "tip", "text": "12 GB em arquivos temporários", "fix": "" }
        ], "overview": ["Windows build 1", "2 disco(s), 1 saudável(is)"] });
        let text = render("system.checkup", &r).unwrap();
        assert!(text.starts_with("Checkup completo do PC: 1 problema(s), 0 ponto(s) de atenção e 1 dica(s)."));
        assert!(text.contains("Problemas:\n- O disco X reporta saúde \"Warning\" → Faça backup."));
        assert!(text.contains("Dicas:\n- 12 GB em arquivos temporários"));
        assert!(!text.contains("Atenção:"), "seção vazia não aparece");
        assert!(text.ends_with("Visão geral:\n- Windows build 1\n- 2 disco(s), 1 saudável(is)"));
    }

    #[test]
    fn self_diagnosis_is_decided_by_code() {
        assert_eq!(one("você tá funcionando direito?").unwrap().0, "self.diagnose");
        assert_eq!(one("faz um checkup de você").unwrap().0, "self.diagnose");
        assert_eq!(one("o que tá errado com você").unwrap().0, "self.diagnose");
        let report = json!({ "overall": "problem", "checks": [
            { "name": "Cérebro (Ollama)", "level": "problem", "detail": "não respondeu", "fix": "Abra o Ollama." },
            { "name": "Voz", "level": "ok", "detail": "instalada", "fix": "" }
        ] });
        let text = render("self.diagnose", &report).unwrap();
        assert!(text.starts_with("Fiz um checkup em mim e achei problemas:"));
        assert!(text.contains("- Cérebro (Ollama): Problema — não respondeu → Abra o Ollama."));
        assert!(text.contains("- Voz: OK — instalada"));
    }

    #[test]
    fn media_status_and_app_volume_are_decided_by_code() {
        assert_eq!(one("pausa a música"), Some(("media.control".into(), json!({ "action": "pause" }))));
        assert_eq!(one("dá play").unwrap().1, json!({ "action": "play" }));
        assert_eq!(one("próxima música").unwrap().1, json!({ "action": "next" }));
        assert_eq!(one("pula essa faixa").unwrap().1, json!({ "action": "next" }));
        assert_eq!(one("música anterior").unwrap().1, json!({ "action": "previous" }));
        assert_eq!(one("o que tá tocando?").unwrap().0, "media.now_playing");
        assert_eq!(one("que música é essa").unwrap().0, "media.now_playing");
        assert_eq!(one("por que o pc tá tão lento?").unwrap().0, "system.checkup", "lento → checkup completo");
        assert_eq!(one("faz um diagnóstico completo do pc").unwrap().0, "system.checkup");
        assert_eq!(one("checkup do computador").unwrap().0, "system.checkup");
        assert_eq!(one("como está meu computador").unwrap().0, "system.status");
        assert_eq!(one("o que tá pesando no pc").unwrap().0, "system.status");
        assert_eq!(
            one("abaixa o volume do spotify pra 20"),
            Some(("audio.app_volume".into(), json!({ "app": "spotify", "level": 20 })))
        );
        assert_eq!(one("muta o discord").unwrap().1, json!({ "app": "discord", "muted": true }));
        assert_eq!(one("muta o pc").unwrap().0, "audio.set_mute", "mudo geral continua igual");
        assert_eq!(one("volta pra aba anterior"), None, "aba do navegador não é música");
    }

    #[test]
    fn replies_are_rendered_from_results() {
        let playing = json!({ "title": "Song", "artist": "Band", "app": "Spotify", "playing": true });
        assert_eq!(render("media.now_playing", &playing).unwrap(), "Tocando agora: Song — Band (no Spotify).");
        assert_eq!(render("media.now_playing", &json!({ "playing": false })).unwrap(), "Nada tocando agora.");
        let mut next = playing.clone();
        next["done"] = json!("next");
        assert_eq!(render("media.control", &next).unwrap(), "Pronto! Agora: Song — Band (no Spotify).");
        assert_eq!(render("audio.app_volume", &json!({ "apps": ["Spotify"], "level": 20 })).unwrap(), "Volume do Spotify em 20%.");
        let ok = json!({ "findings": [], "snapshot": { "cpu": 7.3, "memory_used_gb": 23.5, "memory_total_gb": 31.1 } });
        assert!(render("system.status", &ok).unwrap().starts_with("Tudo tranquilo no PC: CPU em 7.3%"));
        let bad = json!({ "findings": ["Disco C: quase cheio"], "snapshot": {} });
        assert_eq!(render("system.status", &bad).unwrap(), "Olhei o PC e achei isto:\n- Disco C: quase cheio");
        assert_eq!(render("app.open", &json!({})), None);
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
