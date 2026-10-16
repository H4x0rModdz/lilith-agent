//! Rotinas no cérebro: as ferramentas de modos e layouts, os atalhos sem IA
//! ("modo foco", "cria o modo foco: minimiza o monitor 1, muta e abre o vs
//! code", "salva esse layout como trabalho", "me lembra todo dia às 9h de
//! tomar remédio"), a validação de cada passo contra o catálogo e as
//! respostas montadas pelo código.
//!
//! O modelo só entra quando a frase não casa com nada daqui; mesmo assim, o
//! app confere cada passo (`validate_step`) e mostra a prévia antes de salvar.

use crate::shortcut::{self, Shortcut};
use crate::tools::{all_tools, Tool, ToolCall};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::sync::OnceLock;

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("regex de rotina válida"))
}

fn call(action: &str, params: Value) -> ToolCall {
    ToolCall { action: action.into(), params }
}

fn one(action: &str, params: Value) -> Shortcut {
    Shortcut { calls: vec![call(action, params)], reply: "Pronto!".into() }
}

fn obj(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required })
}

/// Ferramentas de rotinas (entram no catálogo de `all_tools`).
pub fn tools() -> Vec<Tool> {
    let name = || obj(json!({ "name": { "type": "string" } }), &["name"]);
    vec![
        Tool {
            action: "mode.run",
            description: "Ativa um MODO do usuário: uma rotina salva com várias ações ('modo foco', 'ativa o modo \
                          stream'). `name`: o nome do modo. O código executa os passos salvos, um por um, e diz o \
                          resultado de cada um — não chame as ações do modo você mesma.",
            schema: name(),
        },
        Tool {
            action: "mode.save",
            description: "Cria (ou substitui) um MODO: uma rotina com várias ações que roda quando o usuário disser \
                          'modo <nome>'. NÃO execute as ações agora — só salve. `steps`: os passos na ordem; em \
                          cada um, `say` = o passo como o usuário disse ('minimiza o monitor 1', 'muta o pc', 'abre \
                          o vs code') e, se souber, `action` + `params` do catálogo. O código confere cada passo e \
                          mostra uma prévia para o usuário confirmar.",
            schema: obj(
                json!({
                    "name": { "type": "string" },
                    "steps": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "say": { "type": "string" },
                                "action": { "type": "string" },
                                "params": { "type": "object" }
                            },
                            "required": ["say"]
                        }
                    }
                }),
                &["name", "steps"],
            ),
        },
        Tool {
            action: "mode.list",
            description: "Lista os MODOS que o usuário criou e os passos de cada um.",
            schema: obj(json!({}), &[]),
        },
        Tool {
            action: "mode.delete",
            description: "Apaga um MODO do usuário (pede confirmação). `name`: o nome do modo.",
            schema: name(),
        },
        Tool {
            action: "layout.save",
            description: "Salva o LAYOUT das janelas abertas agora (que app está em qual monitor e posição) com um \
                          nome: 'salva esse layout como trabalho'.",
            schema: name(),
        },
        Tool {
            action: "layout.restore",
            description: "Restaura um LAYOUT de janelas salvo: abre os apps que faltam e põe cada janela no lugar \
                          salvo ('restaura o layout trabalho').",
            schema: name(),
        },
        Tool {
            action: "layout.list",
            description: "Lista os LAYOUTS de janelas salvos.",
            schema: obj(json!({}), &[]),
        },
        Tool {
            action: "layout.delete",
            description: "Apaga um LAYOUT de janelas salvo (pede confirmação). `name`: o nome do layout.",
            schema: name(),
        },
    ]
}

/// Ações que não podem ser passo de um modo: rotinas dentro de rotinas
/// (laço) e leituras, cuja resposta ninguém leria (e que trariam texto de
/// fora para o meio do modo).
pub const NOT_IN_MODES: &[&str] = &[
    "mode.run",
    "mode.save",
    "mode.list",
    "mode.delete",
    "layout.save",
    "layout.list",
    "layout.delete",
    "window.read",
    "screen.read",
    "file.read",
    "web.fetch",
    "web.search",
    "clipboard.get",
    "memory.search",
    "discord.pending",
    "agent.jobs",
    "ui.find",
    "window.list",
    "media.now_playing",
    "audio.get_volume",
    "system.status",
];

// ── atalhos ───────────────────────────────────────────────────────────────

/// Atalhos de rotinas. `None` = não é uma frase de rotina.
pub fn parse(text: &str) -> Option<Shortcut> {
    if let Some(sc) = parse_reminder(text) {
        return Some(sc);
    }
    let t = shortcut::normalize(text);
    if t.is_empty() || t.chars().count() > 400 {
        return None;
    }
    parse_mode_save(&t).or_else(|| parse_mode_command(&t)).or_else(|| parse_layout(&t))
}

const NAME: &str = r"[«“'\x22]?([\p{L}0-9][\p{L}0-9 _\-]{0,39}?)[»”'\x22]?";

/// "cria o modo foco: minimiza o monitor 1, muta e abre o vs code". Só vira
/// atalho se TODOS os passos forem entendidos pelo código; senão, o modelo
/// interpreta (e o app confere do mesmo jeito).
fn parse_mode_save(t: &str) -> Option<Shortcut> {
    static SAVE: OnceLock<Regex> = OnceLock::new();
    let save = re(
        &SAVE,
        &format!(
            r"^(?:cria|crie|criar|faz|faça|faca|monta|monte|define|configura|cadastra|salva|grava)\s+(?:um\s+|o\s+|uma\s+)?(?:novo\s+)?modo\s+(?:chamado\s+|de\s+nome\s+)?{NAME}\s*(?::|\s=\s|\s-\s|\s—\s|\s+(?:que|com|pra|para)\s+)\s*(.+)$"
        ),
    );
    let c = save.captures(t)?;
    let name = c[1].trim().to_string();
    let parts = split_steps(&c[2]);
    if parts.is_empty() {
        return None;
    }
    let mut steps = Vec::new();
    for say in parts {
        let step = parse_step(&say)?;
        steps.push(json!({ "say": say, "action": step.action, "params": step.params }));
    }
    Some(one("mode.save", json!({ "name": name, "steps": steps })))
}

/// "modo foco", "ativa o modo stream", "apaga o modo foco", "quais modos eu tenho".
fn parse_mode_command(t: &str) -> Option<Shortcut> {
    static RUN: OnceLock<Regex> = OnceLock::new();
    static DELETE: OnceLock<Regex> = OnceLock::new();
    static LIST: OnceLock<Regex> = OnceLock::new();
    let list = re(
        &LIST,
        r"^(?:quais\s+(?:s[ãa]o\s+)?(?:os\s+)?(?:meus\s+)?modos(?:\s+(?:que\s+)?(?:eu\s+)?tenho)?|(?:lista|mostra)\s+(?:os\s+)?(?:meus\s+)?modos|meus\s+modos)$",
    );
    if list.is_match(t) {
        return Some(one("mode.list", json!({})));
    }
    let delete = re(
        &DELETE,
        &format!(r"^(?:apaga|apague|apagar|deleta|exclui|excluir|remove|remover|esquece)\s+(?:o\s+)?modo\s+{NAME}$"),
    );
    if let Some(c) = delete.captures(t) {
        return Some(one("mode.delete", json!({ "name": c[1].trim() })));
    }
    let run = re(
        &RUN,
        &format!(
            r"^(?:(?:ativa|ativar|ative|liga|ligar|inicia|iniciar|come[çc]a|roda|rodar|executa|executar|aplica|aplicar|bota|p[õo]e|coloca|entra\s+no|entrar\s+no|vamos\s+de|vamos\s+pro|bora\s+de|bora\s+pro)\s+(?:o\s+|no\s+)?)?modo\s+{NAME}$"
        ),
    );
    let c = run.captures(t)?;
    Some(one("mode.run", json!({ "name": c[1].trim() })))
}

/// "salva esse layout como trabalho", "restaura o layout trabalho".
fn parse_layout(t: &str) -> Option<Shortcut> {
    static SAVE: OnceLock<Regex> = OnceLock::new();
    static RESTORE: OnceLock<Regex> = OnceLock::new();
    static DELETE: OnceLock<Regex> = OnceLock::new();
    static LIST: OnceLock<Regex> = OnceLock::new();
    let list = re(
        &LIST,
        r"^(?:quais\s+(?:s[ãa]o\s+)?(?:os\s+)?(?:meus\s+)?layouts(?:\s+salvos)?(?:\s+(?:que\s+)?(?:eu\s+)?tenho)?|(?:lista|mostra)\s+(?:os\s+)?(?:meus\s+)?layouts|meus\s+layouts)$",
    );
    if list.is_match(t) {
        return Some(one("layout.list", json!({})));
    }
    let save = re(
        &SAVE,
        &format!(
            r"^(?:salva|salve|salvar|grava|guarda|memoriza)\s+(?:esse\s+|este\s+|o\s+|meu\s+|essa\s+|esta\s+|a\s+|as\s+|minhas\s+)?(?:layout|arranjo|organiza[çc][ãa]o|disposi[çc][ãa]o|janelas)(?:\s+(?:das|de)\s+janelas)?(?:\s+atual)?\s+(?:como|com\s+o\s+nome(?:\s+de)?|de\s+nome|chamad[oa])\s+{NAME}$"
        ),
    );
    if let Some(c) = save.captures(t) {
        return Some(one("layout.save", json!({ "name": c[1].trim() })));
    }
    let delete = re(
        &DELETE,
        &format!(r"^(?:apaga|apague|apagar|deleta|exclui|excluir|remove|remover|esquece)\s+(?:o\s+)?layout\s+{NAME}$"),
    );
    if let Some(c) = delete.captures(t) {
        return Some(one("layout.delete", json!({ "name": c[1].trim() })));
    }
    let restore = re(
        &RESTORE,
        &format!(
            r"^(?:(?:restaura|restaure|restaurar|volta|voltar|aplica|aplique|aplicar|carrega|carregar|recupera|monta|arruma|p[õo]e|coloca|bota|abre|ativa)\s+(?:(?:o|meu|pro|para\s+o|ao)\s+)?)?layout\s+(?:de\s+|do\s+|da\s+)?{NAME}$"
        ),
    );
    let c = restore.captures(t)?;
    Some(one("layout.restore", json!({ "name": c[1].trim() })))
}

/// "a, b e c" / "a; depois b" → ["a", "b", "c"].
pub fn split_steps(text: &str) -> Vec<String> {
    let mut s = format!(" {} ", text.trim());
    for sep in [" e depois ", " depois disso ", " em seguida ", " depois ", " e então ", " e entao ", " então ", ";", " e "] {
        s = s.replace(sep, ",");
    }
    s.split(',')
        .map(|p| p.trim().trim_end_matches(['.', '!']).trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Infinitivo → imperativo no primeiro verbo ("minimizar" → "minimiza",
/// "abrir" → "abre"), para os passos ditos como lista ("mutar, abrir o vs code").
fn imperative(t: &str) -> Option<String> {
    let (verb, rest) = t.split_once(' ').unwrap_or((t, ""));
    let stem = verb.strip_suffix("ar").map(|s| format!("{s}a")).or_else(|| verb.strip_suffix("er").map(|s| format!("{s}e")))
        .or_else(|| verb.strip_suffix("ir").map(|s| format!("{s}e")))?;
    (verb.chars().count() > 3).then(|| format!("{stem} {rest}").trim().to_string())
}

/// Um passo de modo dito em português → a chamada, decidida pelo código.
/// `None` = o código não entendeu (o modelo pode propor, e o app confere).
pub fn parse_step(text: &str) -> Option<ToolCall> {
    let t = shortcut::normalize(text);
    let t = t.trim_start_matches("e ").trim_start_matches("depois ").trim().to_string();
    if t.is_empty() {
        return None;
    }
    let mut candidates = vec![t.clone()];
    candidates.extend(imperative(&t));
    for c in candidates {
        // "minimiza o monitor 1" = todas as janelas daquele monitor.
        static MONITOR: OnceLock<Regex> = OnceLock::new();
        let monitor = re(&MONITOR, r"^(?:minimiza|esconde)\s+(?:o\s+|a\s+)?(?:monitor|tela)\s*(\d)$");
        if let Some(m) = monitor.captures(&c) {
            let n: u32 = m[1].parse().ok()?;
            return (n >= 1).then(|| call("window.minimize", json!({ "monitor": n })));
        }
        let parsed = parse_layout(&c).or_else(|| shortcut::parse(&c));
        if let Some(sc) = parsed.filter(|sc| sc.calls.len() == 1) {
            let step = sc.calls.into_iter().next()?;
            if NOT_IN_MODES.contains(&step.action.as_str()) {
                return None;
            }
            return Some(step);
        }
    }
    None
}

// ── validação e descrição dos passos ──────────────────────────────────────

/// Confere um passo contra o catálogo: ação existente e permitida em modos,
/// parâmetros obrigatórios presentes e com o tipo certo. Devolve os
/// parâmetros limpos (chaves desconhecidas saem; "30" vira 30).
pub fn validate_step(action: &str, params: &Value) -> Result<Value, String> {
    let tool = all_tools().into_iter().find(|t| t.action == action).ok_or_else(|| format!("não existe a ação '{action}'"))?;
    if NOT_IN_MODES.contains(&action) {
        return Err(format!("'{action}' não pode ser passo de um modo"));
    }
    let given = match params {
        Value::Null => Map::new(),
        Value::Object(m) => m.clone(),
        _ => return Err("parâmetros inválidos".into()),
    };
    let props = tool.schema["properties"].as_object().cloned().unwrap_or_default();
    let mut clean = Map::new();
    for (key, value) in given {
        let Some(spec) = props.get(&key) else { continue };
        if value.is_null() {
            continue;
        }
        let v = coerce(spec, value).ok_or_else(|| format!("parâmetro '{key}' com valor inválido"))?;
        clean.insert(key, v);
    }
    for req in tool.schema["required"].as_array().into_iter().flatten().filter_map(|r| r.as_str()) {
        let missing = match clean.get(req) {
            None => true,
            Some(Value::String(s)) => s.trim().is_empty(),
            Some(_) => false,
        };
        if missing {
            return Err(format!("falta '{req}'"));
        }
    }
    Ok(Value::Object(clean))
}

/// O valor no tipo do schema (aceita "30" para inteiro e "true" para booleano).
fn coerce(spec: &Value, v: Value) -> Option<Value> {
    let types: Vec<&str> = match &spec["type"] {
        Value::String(s) => vec![s.as_str()],
        Value::Array(a) => a.iter().filter_map(|t| t.as_str()).collect(),
        _ => vec![],
    };
    let fits = |t: &str, v: &Value| match t {
        "string" => v.is_string(),
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "array" => v.is_array(),
        "object" => v.is_object(),
        _ => true,
    };
    let v = if types.is_empty() || types.iter().any(|t| fits(t, &v)) {
        v
    } else {
        let s = v.as_str()?.trim().to_lowercase();
        if types.contains(&"integer") {
            json!(s.parse::<i64>().ok()?)
        } else if types.contains(&"boolean") {
            json!(match s.as_str() {
                "true" | "sim" => true,
                "false" | "não" | "nao" => false,
                _ => return None,
            })
        } else {
            return None;
        }
    };
    if let (Some(allowed), Some(s)) = (spec["enum"].as_array(), v.as_str()) {
        if !allowed.iter().any(|a| a.as_str() == Some(s)) {
            return None;
        }
    }
    if let Some(n) = v.as_i64() {
        if spec["minimum"].as_i64().is_some_and(|min| n < min) || spec["maximum"].as_i64().is_some_and(|max| n > max) {
            return None;
        }
    }
    Some(v)
}

/// O que o passo faz, em português (prévia, Configurações e resultado).
pub fn describe(action: &str, params: &Value) -> String {
    let s = |k: &str| params[k].as_str().unwrap_or("").trim().to_string();
    match action {
        "app.open" => format!("abrir {}", s("app")),
        "app.close" => format!("fechar o {}", s("app")),
        "browser.open" => format!("abrir {}", s("url").trim_start_matches("https://").trim_start_matches("www.")),
        "browser.search" => format!("pesquisar \"{}\"", s("query")),
        "audio.set_volume" => format!("volume em {}%", params["level"]),
        "audio.set_mute" if params["muted"] == true => "mutar o som".into(),
        "audio.set_mute" => "tirar o som do mudo".into(),
        "audio.app_volume" => match (params["muted"].as_bool(), params["level"].as_u64()) {
            (Some(true), _) => format!("mutar o {}", s("app")),
            (Some(false), _) => format!("tirar o {} do mudo", s("app")),
            (None, Some(l)) => format!("volume do {} em {l}%", s("app")),
            _ => format!("volume do {}", s("app")),
        },
        "window.minimize" if params["monitor"].is_u64() => format!("minimizar as janelas do monitor {}", params["monitor"]),
        "window.minimize" => format!("minimizar o {}", s("target")),
        "window.maximize" => format!("maximizar o {}", s("target")),
        "window.restore" => format!("restaurar o {}", s("target")),
        "window.focus" => format!("trazer o {} pra frente", s("target")),
        "window.close" => format!("fechar a janela do {}", s("target")),
        "media.control" => match s("action").as_str() {
            "pause" => "pausar a música".into(),
            "play" | "play_pause" => "tocar a música".into(),
            "next" => "próxima música".into(),
            "previous" => "música anterior".into(),
            _ => "parar a música".into(),
        },
        "discord.status" => format!("status do Discord: {}", match s("status").as_str() {
            "idle" => "ausente",
            "dnd" => "não perturbe",
            "invisible" => "invisível",
            _ => "online",
        }),
        "discord.voice" if params["mic"].is_string() => {
            if s("mic") == "unmute" { "ligar o microfone do Discord".into() } else { "mutar o microfone do Discord".into() }
        }
        "discord.voice" => "áudio do Discord".into(),
        "layout.restore" => format!("restaurar o layout {}", s("name")),
        "desktop.set_wallpaper" => format!("trocar o wallpaper: {}", s("source")),
        "system.power" => format!("energia: {}", s("op")),
        other => other.to_string(),
    }
}

// ── lembretes repetidos ───────────────────────────────────────────────────

const WEEKDAY: &str = r"(?:segunda|ter[çc]a|quarta|quinta|sexta|s[áa]bado|domingo)s?(?:[\s-]feiras?)?";

/// "me lembra todo dia às 9h de tomar remédio", "me avisa de segunda a
/// sexta às 18h pra fechar o ponto". Só frases com repetição (um lembrete
/// comum segue o caminho de sempre). O texto mantém as maiúsculas.
fn parse_reminder(text: &str) -> Option<Shortcut> {
    static ASK: OnceLock<Regex> = OnceLock::new();
    static REPEAT: OnceLock<Regex> = OnceLock::new();
    static TIME_H: OnceLock<Regex> = OnceLock::new();
    static TIME_AS: OnceLock<Regex> = OnceLock::new();
    static NOON: OnceLock<Regex> = OnceLock::new();
    let t = text.trim().trim_end_matches(['.', '!']);
    if t.chars().count() > 300 {
        return None;
    }
    let ask = re(&ASK, r"(?i)^(?:me\s+(?:lembra|lembre|avisa|avise)|lembra-me|cria\s+um\s+lembrete|crie\s+um\s+lembrete)\s+(.+)$");
    let rest = ask.captures(t)?.get(1)?.as_str().to_string();
    let repeat = re(
        &REPEAT,
        &format!(
            r"(?i)\b(todo\s+dia\s+\d{{1,2}}(?:\s+de\s+cada\s+m[êe]s)?|todos\s+os\s+dias|todo\s+dia|diariamente|de\s+segunda\s+a\s+sexta(?:[\s-]feira)?|(?:em\s+|nos\s+|todos\s+os\s+)?dias\s+[úu]teis|toda\s+semana|semanalmente|todo\s+m[êe]s|mensalmente|tod[ao]s?\s+(?:as\s+|os\s+)?{WEEKDAY}(?:(?:\s*,\s*|\s+e\s+)(?:as?\s+|os?\s+)?{WEEKDAY})*)"
        ),
    );
    // O horário sai primeiro: "todo dia 9h" é diário às 9h, não "dia 9".
    let mut rest = rest;
    let mut time = String::new();
    let time_h = re(&TIME_H, r"(?i)(?:\b[àa]s\s+)?\b(\d{1,2})\s*(?:h|:)\s*(\d{2})?(?:\s*min)?\b");
    let time_as = re(&TIME_AS, r"(?i)\b[àa]s\s+(\d{1,2})(?:\s+horas)?\b");
    let noon = re(&NOON, r"(?i)\b(?:ao\s+|[àa]\s+)?meio[\s-]dia\b");
    if let Some(m) = time_h.captures(&rest).or_else(|| time_as.captures(&rest)) {
        let h: u32 = m[1].parse().ok()?;
        let min: u32 = m.get(2).map(|x| x.as_str().parse().unwrap_or(0)).unwrap_or(0);
        if h > 23 || min > 59 {
            return None;
        }
        time = format!("{h:02}:{min:02}");
        let span = m.get(0)?.range();
        rest.replace_range(span, " ");
    } else if let Some(m) = noon.find(&rest) {
        time = "12:00".into();
        rest.replace_range(m.range(), " ");
    }
    let m = repeat.find(&rest)?;
    let phrase = m.as_str().to_lowercase();
    rest.replace_range(m.range(), " ");
    let mut what: String = rest.split_whitespace().collect::<Vec<_>>().join(" ");
    loop {
        let before = what.clone();
        for lead in ["de ", "que ", "pra ", "para ", "e ", ", "] {
            if what.to_lowercase().starts_with(lead) {
                what = what[lead.len()..].trim().to_string();
            }
        }
        what = what.trim_end_matches([',', ' ']).to_string();
        for tail in [" de", " e", " que"] {
            if what.to_lowercase().ends_with(tail) {
                what.truncate(what.len() - tail.len());
            }
        }
        if what == before {
            break;
        }
    }
    if what.chars().count() < 2 {
        return None;
    }
    Some(one("reminder.add", json!({ "text": what, "repeat": phrase, "time": time, "date": "" })))
}

// ── correções do plano do intérprete ──────────────────────────────────────

/// Pedidos de rotina têm UMA ferramenta só. `t` em minúsculas.
pub fn refine(t: &str) -> Option<Vec<String>> {
    static CREATE: OnceLock<Regex> = OnceLock::new();
    let t = t.trim();
    let only = |a: &str| Some(vec![a.to_string()]);
    let create = re(&CREATE, r"^(?:cria|crie|criar|faz|faça|faca|monta|monte|define|configura|cadastra|salva|grava)\b.*\bmodo\b");
    if create.is_match(t) {
        return only("mode.save");
    }
    if t.contains("layout") {
        if ["salva", "grava", "guarda", "memoriza"].iter().any(|v| t.starts_with(v)) {
            return only("layout.save");
        }
        if ["restaura", "volta", "aplica", "carrega", "recupera", "arruma", "põe", "poe", "coloca", "bota", "layout "]
            .iter()
            .any(|v| t.starts_with(v))
        {
            return only("layout.restore");
        }
    }
    // "Ativa o modo X" com um modo que existe já foi resolvido pelo atalho;
    // aqui só chega modo que não existe ("modo avião", "modo escuro"), e aí
    // o modelo decide livremente.
    let asks = ["me lembra", "me lembre", "me avisa", "me avise", "lembra-me"].iter().any(|p| t.starts_with(p));
    let repeats = ["todo dia", "todos os dias", "diariamente", "segunda a sexta", "dias úteis", "dias uteis", "toda ", "todas as ", "todo mês", "todo mes", "mensalmente", "semanalmente"]
        .iter()
        .any(|w| t.contains(w));
    (asks && repeats).then(|| vec!["reminder.add".to_string()])
}

// ── respostas montadas pelo código ────────────────────────────────────────

fn quoted(s: &str) -> String {
    format!("«{s}»")
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// Resposta a partir do resultado de uma ação de rotina (sem modelo).
pub fn render(action: &str, r: &Value) -> Option<String> {
    let name = r["name"].as_str().unwrap_or("").to_string();
    match action {
        "mode.run" => {
            let steps = r["steps"].as_array()?;
            let lines: Vec<String> = steps
                .iter()
                .map(|s| {
                    let label = s["label"].as_str().unwrap_or("passo");
                    if s["ok"] == true {
                        format!("✓ {label}")
                    } else {
                        format!("✗ {label} — {}", s["error"].as_str().unwrap_or("deu erro"))
                    }
                })
                .collect();
            let ok = steps.iter().filter(|s| s["ok"] == true).count();
            let head = if ok == steps.len() {
                format!("Modo {} ativado!", quoted(&name))
            } else if ok == 0 {
                format!("Modo {}: nenhum passo deu certo.", quoted(&name))
            } else {
                format!("Modo {}: {ok} de {} passos deram certo.", quoted(&name), steps.len())
            };
            Some(format!("{head}\n{}", lines.join("\n")))
        }
        "mode.save" if r["saved"] == true => {
            let n = r["steps"].as_u64().unwrap_or(0);
            let verb = if r["replaced"] == true { "atualizado" } else { "criado" };
            Some(format!("Modo {} {verb} com {n} passo{}. É só dizer \"modo {name}\".", quoted(&name), if n == 1 { "" } else { "s" }))
        }
        "mode.save" => Some("Ok, não salvei o modo.".into()),
        "mode.delete" if r["deleted"] == true => Some(format!("Apaguei o modo {}.", quoted(&name))),
        "mode.delete" => Some("Ok, o modo continua lá.".into()),
        "mode.list" => {
            let modes = r["modes"].as_array()?;
            if modes.is_empty() {
                return Some("Você ainda não tem modos. Experimente: \"cria o modo foco: minimiza o monitor 1, muta e abre o vs code\".".into());
            }
            let lines: Vec<String> = modes
                .iter()
                .map(|m| format!("- {}: {}", m["name"].as_str().unwrap_or(""), strs(&m["steps"]).join(", ")))
                .collect();
            Some(format!("Seus modos:\n{}", lines.join("\n")))
        }
        "layout.save" if r["saved"] == true => {
            let n = r["count"].as_u64().unwrap_or(0);
            Some(format!(
                "Layout {} salvo com {n} janela{}: {}. Para voltar a ele: \"restaura o layout {name}\".",
                quoted(&name),
                if n == 1 { "" } else { "s" },
                strs(&r["apps"]).join(", ")
            ))
        }
        "layout.save" => Some("Ok, deixei o layout como estava.".into()),
        "layout.restore" => {
            let mut parts = Vec::new();
            let moved = strs(&r["moved"]).len();
            let placed = r["in_place"].as_u64().unwrap_or(0) as usize;
            let opened = strs(&r["opened"]);
            if moved + placed > 0 {
                parts.push(format!("{} janela{} no lugar", moved + placed, if moved + placed == 1 { "" } else { "s" }));
            }
            if !opened.is_empty() {
                parts.push(format!("abri {}", opened.join(", ")));
            }
            let mut out = if parts.is_empty() {
                format!("Não consegui arrumar nada do layout {}.", quoted(&name))
            } else {
                format!("Layout {} restaurado: {}.", quoted(&name), parts.join("; "))
            };
            for (key, label) in [
                ("failed_open", "Não consegui abrir"),
                ("not_found", "Não achei a janela de"),
                ("failed", "Não consegui mover"),
                ("skipped_admin", "Janelas de administrador ficaram como estavam"),
            ] {
                let list = strs(&r[key]);
                if !list.is_empty() {
                    out.push_str(&format!("\n{label}: {}.", list.join(", ")));
                }
            }
            Some(out)
        }
        "layout.list" => {
            let layouts = r["layouts"].as_array()?;
            if layouts.is_empty() {
                return Some("Nenhum layout salvo ainda. Arrume as janelas e diga: \"salva esse layout como trabalho\".".into());
            }
            let lines: Vec<String> = layouts
                .iter()
                .map(|l| format!("- {} ({} janelas: {})", l["name"].as_str().unwrap_or(""), l["count"], strs(&l["apps"]).join(", ")))
                .collect();
            Some(format!("Seus layouts:\n{}", lines.join("\n")))
        }
        "layout.delete" if r["deleted"] == true => Some(format!("Apaguei o layout {}.", quoted(&name))),
        "layout.delete" => Some("Ok, o layout continua lá.".into()),
        "reminder.add" if r["reminder"] == true => {
            let text = r["text"].as_str().unwrap_or("").trim();
            let at = r["time"].as_str().map(|t| format!(" às {t}")).unwrap_or_default();
            Some(match r["repeat"].as_str() {
                Some(label) => format!("Combinado! Vou te lembrar {label}{at}: {text}. A primeira vez é {}.", r["date"].as_str().unwrap_or("")),
                None => format!("Lembrete criado para {}{at}: {text}.", r["date"].as_str().unwrap_or("")),
            })
        }
        "reminder.add" => Some(format!("Não consegui criar o lembrete: {}", r["error"].as_str().unwrap_or("dados inválidos"))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sc(text: &str) -> Option<(String, Value)> {
        shortcut::parse(text).map(|s| (s.calls[0].action.clone(), s.calls[0].params.clone()))
    }

    #[test]
    fn mode_commands_are_decided_by_code() {
        assert_eq!(sc("modo foco"), Some(("mode.run".into(), json!({ "name": "foco" }))));
        assert_eq!(sc("Ativa o modo stream!"), Some(("mode.run".into(), json!({ "name": "stream" }))));
        assert_eq!(sc("entra no modo pós-almoço").unwrap().1, json!({ "name": "pós-almoço" }));
        assert_eq!(sc("apaga o modo foco").unwrap().0, "mode.delete");
        assert_eq!(sc("quais modos eu tenho?").unwrap().0, "mode.list");
        assert_eq!(sc("muta o pc").unwrap().0, "audio.set_mute", "os atalhos de sempre continuam");
    }

    #[test]
    fn creating_a_mode_parses_every_step_in_code() {
        let (action, params) = sc("cria o modo foco: minimiza o monitor 1, muta e abre o vs code").unwrap();
        assert_eq!(action, "mode.save");
        assert_eq!(params["name"], "foco");
        let steps = params["steps"].as_array().unwrap();
        let actions: Vec<&str> = steps.iter().map(|s| s["action"].as_str().unwrap()).collect();
        assert_eq!(actions, vec!["window.minimize", "audio.set_mute", "app.open"]);
        assert_eq!(steps[0]["params"], json!({ "monitor": 1 }));
        assert_eq!(steps[2]["params"], json!({ "app": "vs code" }));
        assert_eq!(steps[1]["say"], "muta");
        // No infinitivo, como uma lista.
        let (_, p) = sc("cria um modo stream: mutar, colocar o volume em 30, abrir o obs").unwrap();
        let steps = p["steps"].as_array().unwrap();
        assert_eq!(steps[1]["params"], json!({ "level": 30 }));
        // Um passo que o código não entende: o modelo interpreta.
        assert_eq!(sc("cria o modo foco: minimiza o monitor 1 e me conta uma piada"), None);
    }

    #[test]
    fn steps_never_nest_modes_or_reads() {
        assert!(parse_step("modo foco").is_none());
        assert!(parse_step("o que tá tocando").is_none(), "leitura não é passo");
        assert_eq!(parse_step("restaura o layout trabalho").unwrap().action, "layout.restore");
        assert_eq!(parse_step("minimizar a tela 2").unwrap().params, json!({ "monitor": 2 }));
        assert_eq!(parse_step("pausar a música").unwrap().params, json!({ "action": "pause" }));
    }

    #[test]
    fn layout_phrases() {
        assert_eq!(sc("salva esse layout como trabalho"), Some(("layout.save".into(), json!({ "name": "trabalho" }))));
        assert_eq!(sc("salva as janelas como \"jogo\"").unwrap().1, json!({ "name": "jogo" }));
        assert_eq!(sc("restaura o layout trabalho"), Some(("layout.restore".into(), json!({ "name": "trabalho" }))));
        assert_eq!(sc("volta pro layout de trabalho").unwrap().1, json!({ "name": "trabalho" }));
        assert_eq!(sc("layout trabalho").unwrap().0, "layout.restore");
        assert_eq!(sc("apaga o layout trabalho").unwrap().0, "layout.delete");
        assert_eq!(sc("quais layouts eu tenho").unwrap().0, "layout.list");
    }

    #[test]
    fn recurring_reminders_are_parsed_in_code() {
        let (a, p) = sc("me lembra todo dia às 9h de tomar o Remédio").unwrap();
        assert_eq!(a, "reminder.add");
        assert_eq!(p, json!({ "text": "tomar o Remédio", "repeat": "todo dia", "time": "09:00", "date": "" }));
        let (_, p) = sc("me avisa de segunda a sexta às 18h30 pra fechar o ponto").unwrap();
        assert_eq!((p["repeat"].as_str(), p["time"].as_str(), p["text"].as_str()), (Some("de segunda a sexta"), Some("18:30"), Some("fechar o ponto")));
        let (_, p) = sc("me lembra de pagar a academia toda segunda e quarta").unwrap();
        assert_eq!((p["repeat"].as_str(), p["time"].as_str(), p["text"].as_str()), (Some("toda segunda e quarta"), Some(""), Some("pagar a academia")));
        let (_, p) = sc("me lembra todo dia 5 de pagar o aluguel").unwrap();
        assert_eq!(p["repeat"], "todo dia 5");
        let (_, p) = sc("me lembra todo dia 9h de beber água").unwrap();
        assert_eq!((p["repeat"].as_str(), p["time"].as_str()), (Some("todo dia"), Some("09:00")), "9h é horário, não dia 9");
        assert_eq!(sc("me lembra amanhã às 9h de ligar pro dentista"), None, "sem repetição: caminho de sempre");
    }

    #[test]
    fn validation_against_the_catalog() {
        assert_eq!(validate_step("audio.set_volume", &json!({ "level": "30", "lixo": 1 })).unwrap(), json!({ "level": 30 }));
        assert!(validate_step("audio.set_volume", &json!({ "level": 300 })).is_err());
        assert!(validate_step("app.open", &json!({})).is_err(), "falta app");
        assert!(validate_step("media.control", &json!({ "action": "explode" })).is_err(), "fora do enum");
        assert!(validate_step("formata.disco", &json!({})).is_err());
        assert!(validate_step("mode.run", &json!({ "name": "x" })).is_err(), "modo dentro de modo");
        assert!(validate_step("window.read", &json!({ "target": "x" })).is_err());
        assert_eq!(validate_step("audio.set_mute", &json!({ "muted": "true" })).unwrap(), json!({ "muted": true }));
    }

    #[test]
    fn plans_for_routines_have_one_tool() {
        let one = |a: &str| Some(vec![a.to_string()]);
        assert_eq!(refine("cria o modo foco: minimiza tudo e abre o spotify"), one("mode.save"));
        assert_eq!(refine("ativa o modo avião"), None, "modo que não existe: o modelo decide");
        assert_eq!(refine("salva o layout atual com o nome trabalho"), one("layout.save"));
        assert_eq!(refine("restaura o layout trabalho"), one("layout.restore"));
        assert_eq!(refine("me lembra toda sexta de pagar a conta"), one("reminder.add"));
        assert_eq!(refine("abre o spotify"), None);
    }

    #[test]
    fn replies_come_from_the_result() {
        let run = json!({ "name": "foco", "steps": [
            { "label": "mutar o som", "ok": true },
            { "label": "abrir vs code", "ok": false, "error": "não achei o app" }
        ] });
        assert_eq!(render("mode.run", &run).unwrap(), "Modo «foco»: 1 de 2 passos deram certo.\n✓ mutar o som\n✗ abrir vs code — não achei o app");
        assert!(render("mode.save", &json!({ "saved": true, "name": "foco", "steps": 3 })).unwrap().contains("criado com 3 passos"));
        assert_eq!(render("mode.save", &json!({ "saved": false })).unwrap(), "Ok, não salvei o modo.");
        let restored = json!({ "name": "trabalho", "moved": ["a", "b"], "in_place": 1, "opened": ["Spotify"], "failed_open": [], "not_found": ["Discord"] });
        assert_eq!(
            render("layout.restore", &restored).unwrap(),
            "Layout «trabalho» restaurado: 3 janelas no lugar; abri Spotify.\nNão achei a janela de: Discord."
        );
        let rem = json!({ "reminder": true, "text": "tomar remédio", "repeat": "todo dia", "time": "09:00", "date": "17/10/2026" });
        assert_eq!(render("reminder.add", &rem).unwrap(), "Combinado! Vou te lembrar todo dia às 09:00: tomar remédio. A primeira vez é 17/10/2026.");
    }

    #[test]
    fn steps_split_on_commas_and_conjunctions() {
        assert_eq!(split_steps("a, b e c; depois d"), vec!["a", "b", "c", "d"]);
    }
}
