//! Vincular ao Celular (Phone Link) decidido por código: estado do celular
//! (conectado, bateria, sinal), notificações, conversas de SMS, chamadas
//! recentes e envio de SMS.
//!
//! Tudo pela acessibilidade do próprio app (processo `PhoneExperienceHost`,
//! janela "Vincular ao Celular"/"Phone Link"). Como no Discord: o app
//! minimizado não expõe a árvore, então ele é restaurado para ler e volta a
//! ser minimizado no fim (se fomos nós que o trouxemos). Os rótulos são
//! procurados por âncoras estáveis em pt-BR e inglês; a leitura é feita numa
//! foto da árvore (`Node`) e interpretada por funções puras, testáveis sem o
//! app aberto.
//!
//! Textos lidos (notificações, mensagens, nomes) são de terceiros: saem
//! curtos e o app marca o turno como contaminado (TAINT_SOURCES).

use crate::paths::fold;
use crate::window::{self, Rect, WindowInfo};
use crate::WinError;
use regex::Regex;
use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::Duration;
use windows::core::BSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern,
    IUIAutomationLegacyIAccessiblePattern, IUIAutomationSelectionItemPattern, IUIAutomationValuePattern,
    UIA_ButtonControlTypeId, UIA_CustomControlTypeId, UIA_EditControlTypeId, UIA_GroupControlTypeId,
    UIA_HyperlinkControlTypeId, UIA_ImageControlTypeId, UIA_InvokePatternId, UIA_LegacyIAccessiblePatternId,
    UIA_ListControlTypeId, UIA_ListItemControlTypeId, UIA_PaneControlTypeId, UIA_SelectionItemPatternId,
    UIA_TabControlTypeId, UIA_TabItemControlTypeId, UIA_TextControlTypeId, UIA_ValuePatternId,
    UIA_CONTROLTYPE_ID,
};

/// AppUserModelID do Vincular ao Celular (abre pelo Menu Iniciar).
const APP_ID: &str = r"shell:AppsFolder\Microsoft.YourPhone_8wekyb3d8bbwe!App";
/// Títulos conhecidos da janela (o app já se chamou "Seu Telefone").
const TITLES: &[&str] = &["vincular ao celular", "vinculo ao celular", "phone link", "seu telefone", "your phone"];
/// Processo do app.
const PROCESSES: &[&str] = &["phoneexperiencehost", "yourphone"];
/// Teto de nós por leitura (a árvore do app é pequena; isto só protege).
const MAX_NODES: usize = 5000;
/// Textos de terceiros saem curtos (não viram veículo de prompt injection).
const MAX_TEXT: usize = 200;
/// Itens por leitura (padrão e teto).
const DEFAULT_LIMIT: usize = 8;
const MAX_LIMIT: usize = 15;

fn pause(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

// ── foto da árvore ────────────────────────────────────────────────────────

/// Um nó da árvore de acessibilidade, já resumido.
#[derive(Debug, Clone)]
pub(crate) struct Node {
    /// Tipo curto: button, listitem, list, tabitem, text, edit, group, pane, image, link, custom, other.
    pub kind: &'static str,
    pub name: String,
    /// AutomationId (estável entre idiomas, quando o app define).
    pub aid: String,
    pub rect: Rect,
    /// Nomes e ids dos contêineres acima, sem acento, separados por " / ".
    pub path: String,
    /// Índice do item de lista que contém este nó (junta os textos do item).
    pub item: Option<usize>,
    el: Option<IUIAutomationElement>,
}

impl Node {
    #[cfg(test)]
    fn fake(kind: &'static str, name: &str, path: &str, item: Option<usize>) -> Self {
        Node { kind, name: name.into(), aid: String::new(), rect: Rect { x: 0, y: 0, w: 10, h: 10 }, path: fold(path), item, el: None }
    }
}

fn kind_of(id: UIA_CONTROLTYPE_ID) -> &'static str {
    match id {
        x if x == UIA_ButtonControlTypeId => "button",
        x if x == UIA_ListItemControlTypeId => "listitem",
        x if x == UIA_ListControlTypeId => "list",
        x if x == UIA_TabItemControlTypeId => "tabitem",
        x if x == UIA_TabControlTypeId => "tab",
        x if x == UIA_TextControlTypeId => "text",
        x if x == UIA_EditControlTypeId => "edit",
        x if x == UIA_GroupControlTypeId => "group",
        x if x == UIA_PaneControlTypeId => "pane",
        x if x == UIA_ImageControlTypeId => "image",
        x if x == UIA_HyperlinkControlTypeId => "link",
        x if x == UIA_CustomControlTypeId => "custom",
        _ => "other",
    }
}

fn clip(s: &str, max: usize) -> String {
    let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > max {
        s.chars().take(max).collect::<String>() + "…"
    } else {
        s
    }
}

/// Foto da janela: os nós visíveis, na ordem da tela.
pub(crate) fn snapshot(id: isize) -> Result<Vec<Node>, WinError> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
        let root = uia
            .ElementFromHandle(HWND(id as *mut _))
            .map_err(|_| WinError::Unavailable("não consegui acessar a janela do Vincular ao Celular".into()))?;
        let walker = uia.ControlViewWalker()?;
        let mut out: Vec<Node> = Vec::new();
        let mut stack: Vec<(IUIAutomationElement, Vec<String>, Option<usize>)> = vec![(root, Vec::new(), None)];
        let mut visited = 0usize;
        while let Some((el, path, item)) = stack.pop() {
            visited += 1;
            if visited > MAX_NODES {
                break;
            }
            if el.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false) {
                continue;
            }
            let name = el.CurrentName().map(|b| b.to_string()).unwrap_or_default();
            let aid = el.CurrentAutomationId().map(|b| b.to_string()).unwrap_or_default();
            let kind = kind_of(el.CurrentControlType().unwrap_or_default());
            let r = el.CurrentBoundingRectangle().unwrap_or_default();
            let rect = Rect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top };
            let index = out.len();
            out.push(Node {
                kind,
                name: clip(&name, 400),
                aid: aid.clone(),
                rect,
                path: path.join(" / "),
                item,
                el: Some(el.clone()),
            });
            let mut child_path = path;
            let label = fold(&format!("{} {}", clip(&name, 60), aid));
            if !label.trim().is_empty() {
                child_path.push(label.trim().to_string());
                if child_path.len() > 8 {
                    child_path.remove(0);
                }
            }
            let child_item = if kind == "listitem" { Some(index) } else { item };
            let mut children = Vec::new();
            let mut next = walker.GetFirstChildElement(&el);
            while let Ok(child) = next {
                next = walker.GetNextSiblingElement(&child);
                children.push(child);
            }
            stack.extend(children.into_iter().rev().map(|c| (c, child_path.clone(), child_item)));
        }
        Ok(out)
    }
}

/// Aciona o nó: Invoke, Select ou a ação padrão. Sem mouse.
fn invoke(node: &Node) -> bool {
    let Some(el) = &node.el else { return false };
    unsafe {
        if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId) {
            if p.Invoke().is_ok() {
                return true;
            }
        }
        if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId) {
            if p.Select().is_ok() {
                return true;
            }
        }
        if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(UIA_LegacyIAccessiblePatternId) {
            if p.DoDefaultAction().is_ok() {
                return true;
            }
        }
    }
    false
}

/// Valor atual de um campo de texto.
fn value_of(node: &Node) -> Option<String> {
    let el = node.el.as_ref()?;
    unsafe {
        let p = el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId).ok()?;
        p.CurrentValue().ok().map(|b| b.to_string())
    }
}

/// Escreve no campo pela acessibilidade (sem teclado). `false` se o app recusar.
fn set_value(node: &Node, text: &str) -> bool {
    let Some(el) = &node.el else { return false };
    unsafe {
        match el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) {
            Ok(p) => p.SetValue(&BSTR::from(text)).is_ok(),
            Err(_) => false,
        }
    }
}

// ── a janela do app ───────────────────────────────────────────────────────

/// A janela do Vincular ao Celular, se estiver aberta (mesmo minimizada).
fn phone_window() -> Option<WindowInfo> {
    window::list().into_iter().find(|w| is_phone_window(&w.app, &w.title))
}

pub(crate) fn is_phone_window(app: &str, title: &str) -> bool {
    let (a, t) = (fold(app), fold(title));
    PROCESSES.iter().any(|p| a == *p) || TITLES.iter().any(|x| t == *x || t.starts_with(&format!("{x} ")))
}

/// Deixa a janela como estava: se fomos nós que a abrimos/restauramos, ela
/// volta a ficar minimizada quando a leitura termina (inclusive com erro).
struct Leave {
    minimize: Option<isize>,
}

impl Drop for Leave {
    fn drop(&mut self) {
        if let Some(id) = self.minimize {
            let _ = window::set_state(id, "minimize");
        }
    }
}

/// O app visível e pronto para ler. Fechado/em segundo plano: abre pelo Menu
/// Iniciar e espera a janela (até ~10 s). Minimizado: restaura.
fn open_phone() -> Result<(WindowInfo, Leave), WinError> {
    if let Some(w) = phone_window() {
        if w.minimized {
            window::set_state(w.id, "restore")?;
            pause(1200);
            return Ok((w.clone(), Leave { minimize: Some(w.id) }));
        }
        return Ok((w, Leave { minimize: None }));
    }
    // Sem janela: o app pode estar só em segundo plano (ou fechado). O
    // explorer abre pelo AppUserModelID, como clicar no Menu Iniciar.
    std::process::Command::new("explorer.exe")
        .arg(APP_ID)
        .spawn()
        .map_err(|e| WinError::Unavailable(format!("não consegui abrir o Vincular ao Celular: {e}")))?;
    for _ in 0..40 {
        pause(250);
        if let Some(w) = phone_window() {
            // Dá tempo do app montar a tela e falar com o celular.
            pause(1500);
            return Ok((w.clone(), Leave { minimize: Some(w.id) }));
        }
    }
    Err(WinError::Unavailable(
        "o Vincular ao Celular não abriu — ele está instalado e configurado com o seu celular?".into(),
    ))
}

// ── interpretação (funções puras) ─────────────────────────────────────────

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("regex válida"))
}

/// Algum texto da tela contém uma destas expressões (sem acento)?
fn screen_has(nodes: &[Node], words: &[&str]) -> bool {
    nodes.iter().any(|n| {
        let s = fold(&n.name);
        words.iter().any(|w| s.contains(w))
    })
}

/// Tela de configuração/pareamento (o celular ainda não foi vinculado).
const SETUP_WORDS: &[&str] = &[
    "emparelhar", "parear", "vincule seu celular", "vincular seu celular", "codigo qr", "qr code", "get started",
    "pair your", "pair with", "link your phone", "introducao", "comecar", "entrar com a conta da microsoft",
];
/// O celular está vinculado mas fora do ar.
const OFFLINE_WORDS: &[&str] = &[
    "desconectado", "nao conectado", "nao esta conectado", "reconectar", "nao e possivel conectar",
    "nao foi possivel conectar", "verifique seu celular", "disconnected", "not connected", "reconnect",
    "can't connect", "cannot connect", "couldn't connect", "offline",
];
/// Abas do app: aparecem quando o celular está conectado.
const NAV_LABELS: &[&str] = &[
    "mensagens", "messages", "chamadas", "calls", "fotos", "photos", "aplicativos", "apps", "notificacoes",
    "notifications", "configuracoes", "settings", "tela do celular", "phone screen",
];

/// Estado do celular a partir da foto da tela.
pub(crate) fn parse_status(nodes: &[Node]) -> Value {
    static BATTERY_A: OnceLock<Regex> = OnceLock::new();
    static BATTERY_B: OnceLock<Regex> = OnceLock::new();
    static FRACTION: OnceLock<Regex> = OnceLock::new();
    let battery_a = re(&BATTERY_A, r"(?i)(?:bateria|battery)[^0-9%]{0,40}(\d{1,3})\s*%");
    let battery_b = re(&BATTERY_B, r"(?i)(\d{1,3})\s*%[^0-9%]{0,25}(?:bateria|battery)");
    let fraction = re(&FRACTION, r"(\d)\s*(?:de|of|/)\s*(\d)");

    let mut battery: Option<u32> = None;
    let mut charging = false;
    let mut signal: Option<String> = None;
    let mut wifi: Option<String> = None;
    let mut bluetooth: Option<String> = None;
    // Notificações do celular ("bateria fraca 15%") não são o estado dele.
    for n in nodes.iter().filter(|n| !NOTIFICATION_ANCHORS.iter().any(|a| n.path.contains(a))) {
        let text = format!("{} {}", n.name, n.aid);
        let f = fold(&text);
        if battery.is_none() {
            let hit = battery_a.captures(&n.name).or_else(|| battery_b.captures(&n.name));
            if let Some(level) = hit.and_then(|c| c[1].parse::<u32>().ok()).filter(|l| *l <= 100) {
                battery = Some(level);
            }
            // Ícone da bateria com o valor só no nome ("80%") e "battery" no id.
            if battery.is_none() && fold(&n.aid).contains("battery") {
                static PCT: OnceLock<Regex> = OnceLock::new();
                if let Some(c) = re(&PCT, r"(\d{1,3})\s*%").captures(&n.name) {
                    battery = c[1].parse::<u32>().ok().filter(|l| *l <= 100);
                }
            }
        }
        if (f.contains("bateria") || f.contains("battery")) && (f.contains("carregando") || f.contains("charging")) {
            charging = true;
        }
        if signal.is_none() && (f.contains("sinal") || f.contains("signal") || f.contains("celular rede")) {
            signal = Some(match fraction.captures(&n.name) {
                Some(c) => format!("{} de {}", &c[1], &c[2]),
                None => clip(&n.name, 60),
            })
            .filter(|s| !s.trim().is_empty());
        }
        if wifi.is_none() && (f.contains("wi-fi") || f.contains("wifi")) && !n.name.trim().is_empty() {
            wifi = Some(clip(&n.name, 60));
        }
        if bluetooth.is_none() && f.contains("bluetooth") && !n.name.trim().is_empty() {
            bluetooth = Some(clip(&n.name, 60));
        }
    }
    let tabs = nodes
        .iter()
        .filter(|n| matches!(n.kind, "tabitem" | "listitem" | "button" | "link"))
        .filter(|n| NAV_LABELS.contains(&fold(&n.name).as_str()))
        .count();
    let setup = screen_has(nodes, SETUP_WORDS) && tabs < 2;
    let offline = screen_has(nodes, OFFLINE_WORDS);
    let connected = !setup && !offline && (battery.is_some() || tabs >= 2);
    let mut out = json!({
        "paired": !setup,
        "connected": connected,
        "battery": battery,
        "charging": charging,
    });
    if let Some(s) = signal {
        out["signal"] = json!(s);
    }
    if let Some(s) = wifi {
        out["wifi"] = json!(s);
    }
    if let Some(s) = bluetooth {
        out["bluetooth"] = json!(s);
    }
    out
}

/// Nome que é só o tipo do objeto (o WinUI usa o nome da classe quando o app
/// não dá nome acessível ao item): "PhoneExperienceHost.ViewModels.Item".
fn is_type_name(s: &str) -> bool {
    let s = s.trim();
    !s.contains(' ') && s.matches('.').count() >= 2
}

/// Texto de um item de lista: o nome dele ou, se vazio, os textos de dentro.
pub(crate) fn item_text(nodes: &[Node], index: usize) -> String {
    let own = nodes[index].name.trim();
    if !own.is_empty() && !is_type_name(own) {
        return clip(own, MAX_TEXT);
    }
    let mut parts: Vec<String> = Vec::new();
    for n in nodes.iter().filter(|n| n.item == Some(index) && n.kind == "text") {
        let t = n.name.trim();
        if !t.is_empty() && !is_type_name(t) && !parts.iter().any(|p| p == t) {
            parts.push(t.to_string());
        }
    }
    clip(&parts.join(" · "), MAX_TEXT)
}

/// Itens de lista visíveis dentro de uma região (âncoras no caminho). Sem
/// âncora casando, devolve vazio — quem chama decide o plano B.
pub(crate) fn items_under(nodes: &[Node], anchors: &[&str], exclude: &[&str], limit: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (i, n) in nodes.iter().enumerate() {
        if n.kind != "listitem" {
            continue;
        }
        let in_region = anchors.iter().any(|a| n.path.contains(a) || fold(&n.aid).contains(a));
        let excluded = exclude.iter().any(|a| n.path.contains(a));
        if !in_region || excluded {
            continue;
        }
        let text = item_text(nodes, i);
        if text.is_empty() || NAV_LABELS.contains(&fold(&text).as_str()) || out.contains(&text) {
            continue;
        }
        out.push(text);
        if out.len() >= limit {
            break;
        }
    }
    out
}

/// "Fulano, Oi tudo bem?, 10:32" → ("Fulano", "Oi tudo bem?, 10:32").
pub(crate) fn split_item(text: &str) -> (String, String) {
    let cut = [" · ", ", ", "\n", ": "].iter().filter_map(|sep| text.find(sep).map(|i| (i, sep.len()))).min_by_key(|(i, _)| *i);
    match cut {
        Some((i, len)) => (text[..i].trim().to_string(), text[i + len..].trim().to_string()),
        None => (text.trim().to_string(), String::new()),
    }
}

fn entries(items: Vec<String>, head: &str, rest: &str) -> Vec<Value> {
    items
        .into_iter()
        .map(|t| {
            let (h, r) = split_item(&t);
            json!({ head: h, rest: r })
        })
        .collect()
}

fn limit_param(params: &Value) -> usize {
    params["limit"].as_u64().map(|l| l as usize).unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

/// Para a leitura se o celular não está vinculado/conectado (resposta clara).
fn require_connected(nodes: &[Node]) -> Result<(), WinError> {
    let st = parse_status(nodes);
    if st["paired"] == false {
        return Err(WinError::Unavailable(
            "o Vincular ao Celular ainda não está ligado a um celular — abra o app e siga o pareamento".into(),
        ));
    }
    if screen_has(nodes, OFFLINE_WORDS) {
        return Err(WinError::Unavailable(
            "o celular está desconectado do Vincular ao Celular (confira Bluetooth/Wi-Fi e o app no celular)".into(),
        ));
    }
    Ok(())
}

/// Vai para uma aba do app (Mensagens, Chamadas…) pelo nome. `false` se não achou.
fn go_tab(id: isize, labels: &[&str]) -> Result<bool, WinError> {
    let nodes = snapshot(id)?;
    let tab = nodes.iter().find(|n| {
        matches!(n.kind, "tabitem" | "listitem" | "button" | "link") && labels.iter().any(|l| fold(&n.name) == *l)
    });
    match tab {
        Some(t) => {
            let ok = invoke(t);
            pause(900);
            Ok(ok)
        }
        None => Ok(false),
    }
}

// ── ações ─────────────────────────────────────────────────────────────────

/// `phone.status`: conectado?, bateria, sinal, Wi-Fi/Bluetooth (o que o app mostrar).
pub fn status(_params: &Value) -> Result<Value, WinError> {
    let (w, _leave) = open_phone()?;
    let nodes = snapshot(w.id)?;
    Ok(parse_status(&nodes))
}

const NOTIFICATION_ANCHORS: &[&str] = &["notifica", "notification"];

/// `phone.notifications`: as notificações recentes do celular (app + resumo).
pub fn notifications(params: &Value) -> Result<Value, WinError> {
    let limit = limit_param(params);
    let (w, _leave) = open_phone()?;
    let mut nodes = snapshot(w.id)?;
    require_connected(&nodes)?;
    let mut items = items_under(&nodes, NOTIFICATION_ANCHORS, &[], limit);
    if items.is_empty() {
        // O painel pode estar recolhido: abre pelo botão/aba "Notificações".
        if go_tab(w.id, &["notificacoes", "notifications"])? {
            nodes = snapshot(w.id)?;
            items = items_under(&nodes, NOTIFICATION_ANCHORS, &[], limit);
        }
    }
    let empty_msg = screen_has(&nodes, &["nenhuma notificacao", "sem notificacoes", "no new notifications", "no notifications"]);
    Ok(json!({
        "count": items.len(),
        "notifications": entries(items, "app", "text"),
        "empty": empty_msg,
    }))
}

const MESSAGE_TABS: &[&str] = &["mensagens", "messages"];
const MESSAGE_ANCHORS: &[&str] = &["conversa", "conversation", "mensage", "message", "sms"];

/// Conversas visíveis na aba Mensagens (fora do painel de notificações).
fn conversation_items(nodes: &[Node], limit: usize) -> Vec<String> {
    let anchored = items_under(nodes, MESSAGE_ANCHORS, NOTIFICATION_ANCHORS, limit);
    if !anchored.is_empty() {
        return anchored;
    }
    // Plano B: qualquer item de lista fora das notificações e da navegação.
    items_under(nodes, &[""], NOTIFICATION_ANCHORS, limit)
}

/// `phone.messages`: as conversas de SMS mais recentes (quem + prévia).
pub fn messages(params: &Value) -> Result<Value, WinError> {
    let limit = limit_param(params);
    let (w, _leave) = open_phone()?;
    require_connected(&snapshot(w.id)?)?;
    if !go_tab(w.id, MESSAGE_TABS)? {
        return Err(WinError::Unavailable("não achei a aba Mensagens no Vincular ao Celular".into()));
    }
    let nodes = snapshot(w.id)?;
    let items = conversation_items(&nodes, limit);
    Ok(json!({ "count": items.len(), "conversations": entries(items, "who", "preview") }))
}

const CALL_TABS: &[&str] = &["chamadas", "calls"];
const CALL_ANCHORS: &[&str] = &["chamada", "call", "historico", "history", "recente", "recent"];

/// `phone.calls`: as chamadas recentes (quem + quando/tipo).
pub fn calls(params: &Value) -> Result<Value, WinError> {
    let limit = limit_param(params);
    let (w, _leave) = open_phone()?;
    require_connected(&snapshot(w.id)?)?;
    if !go_tab(w.id, CALL_TABS)? {
        return Err(WinError::Unavailable("não achei a aba Chamadas no Vincular ao Celular".into()));
    }
    let nodes = snapshot(w.id)?;
    let mut items = items_under(&nodes, CALL_ANCHORS, NOTIFICATION_ANCHORS, limit);
    if items.is_empty() {
        items = items_under(&nodes, &[""], NOTIFICATION_ANCHORS, limit);
    }
    // O app pede permissão no celular para mostrar chamadas: diz isso claro.
    let needs_permission = items.is_empty() && screen_has(&nodes, &["permiss", "permission", "configurar chamadas", "set up calls"]);
    Ok(json!({ "count": items.len(), "calls": entries(items, "who", "detail"), "needs_permission": needs_permission }))
}

// ── envio de SMS ──────────────────────────────────────────────────────────

/// Parece número de telefone? (dígitos, +, espaços, traços, parênteses; 8+ dígitos)
pub(crate) fn looks_like_number(s: &str) -> bool {
    let digits = s.chars().filter(|c| c.is_ascii_digit()).count();
    digits >= 8 && s.chars().all(|c| c.is_ascii_digit() || matches!(c, '+' | ' ' | '-' | '(' | ')' | '.'))
}

/// Conversas cujo contato casa com o nome pedido (palavras inteiras, sem
/// acento; com `exact`, o nome inteiro). Devolve (índice do nó, contato).
pub(crate) fn matching_conversations(nodes: &[Node], wanted: &str, exact: bool) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    for (i, n) in nodes.iter().enumerate() {
        if n.kind != "listitem" || NOTIFICATION_ANCHORS.iter().any(|a| n.path.contains(a)) {
            continue;
        }
        let text = item_text(nodes, i);
        let (who, _) = split_item(&text);
        if who.is_empty() || NAV_LABELS.contains(&fold(&who).as_str()) {
            continue;
        }
        let hit = if exact { fold(&who) == fold(wanted) } else { crate::names_match(&who, wanted) };
        if hit && !out.iter().any(|(_, w)| fold(w) == fold(&who)) {
            out.push((i, who));
        }
    }
    out
}

/// Campo de digitar a mensagem: pelo nome/ID ou, sem isso, o campo mais baixo.
fn message_field(nodes: &[Node]) -> Option<&Node> {
    let edits: Vec<&Node> = nodes.iter().filter(|n| n.kind == "edit" && n.rect.w > 0).collect();
    let named = edits.iter().copied().find(|n| {
        let f = fold(&format!("{} {}", n.name, n.aid));
        (f.contains("mensagem") || f.contains("message") || f.contains("compose") || f.contains("escrever"))
            && !f.contains("pesquis")
            && !f.contains("search")
    });
    named.or_else(|| edits.into_iter().max_by_key(|n| (n.rect.y, n.rect.w)))
}

/// Campo de destinatário da nova mensagem ("Para", "Insira um nome ou número").
fn recipient_field(nodes: &[Node]) -> Option<&Node> {
    nodes.iter().find(|n| {
        let f = fold(&format!("{} {}", n.name, n.aid));
        n.kind == "edit"
            && (f.starts_with("para")
                || f.starts_with("to ")
                || f == "to"
                || f.contains("nome ou numero")
                || f.contains("name or number")
                || f.contains("destinat")
                || f.contains("recipient"))
    })
}

fn find_button<'a>(nodes: &'a [Node], labels: &[&str]) -> Option<&'a Node> {
    nodes
        .iter()
        .find(|n| matches!(n.kind, "button" | "link" | "listitem") && labels.iter().any(|l| fold(&n.name) == *l))
}

/// Coloca o texto no campo: pela acessibilidade; se o app recusar, clica no
/// campo e digita (precisa da janela em primeiro plano).
fn fill(w: &WindowInfo, field: &Node, text: &str) -> Result<(), WinError> {
    if set_value(field, text) && value_of(field).is_some_and(|v| v.trim() == text.trim()) {
        return Ok(());
    }
    window::focus(w.id)?;
    pause(200);
    let (x, y) = field.rect.center();
    crate::input::click_at(x, y)?;
    pause(150);
    crate::input::reset_cancel();
    crate::input::type_text(text)
}

/// Abre a conversa certa (existente ou nova). `Err(Ok(opções))` = ambíguo.
fn open_conversation(w: &WindowInfo, contact: &str, exact: bool) -> Result<Result<String, Vec<String>>, WinError> {
    let nodes = snapshot(w.id)?;
    let found = matching_conversations(&nodes, contact, exact);
    match found.len() {
        1 => {
            let (i, who) = &found[0];
            if !invoke(&nodes[*i]) {
                return Err(WinError::Failed(format!("não consegui abrir a conversa com {who}")));
            }
            pause(900);
            return Ok(Ok(who.clone()));
        }
        n if n > 1 => return Ok(Err(found.into_iter().map(|(_, w)| w).collect())),
        _ => {}
    }
    // Nenhuma conversa com esse nome: nova mensagem, procurando o contato.
    let Some(new) = find_button(&nodes, &["nova mensagem", "new message", "escrever mensagem", "compose", "nova conversa"]) else {
        return Err(WinError::NotFound(format!("não achei uma conversa com '{contact}' nem o botão Nova mensagem")));
    };
    invoke(new);
    pause(900);
    let nodes = snapshot(w.id)?;
    let Some(to) = recipient_field(&nodes) else {
        return Err(WinError::NotFound("não achei o campo do destinatário da nova mensagem".into()));
    };
    let before: Vec<String> = (0..nodes.len()).filter(|i| nodes[*i].kind == "listitem").map(|i| item_text(&nodes, i)).collect();
    // A busca de contatos só reage a digitação de verdade.
    window::focus(w.id)?;
    pause(200);
    let (x, y) = to.rect.center();
    crate::input::click_at(x, y)?;
    pause(150);
    crate::input::reset_cancel();
    crate::input::type_text(contact)?;
    pause(1500);
    let nodes = snapshot(w.id)?;
    let suggestions: Vec<(usize, String)> = (0..nodes.len())
        .filter(|i| nodes[*i].kind == "listitem")
        .map(|i| (i, item_text(&nodes, i)))
        .filter(|(_, t)| !t.is_empty() && !before.contains(t))
        .filter(|(_, t)| crate::names_match(t, contact) || looks_like_number(contact))
        .collect();
    match suggestions.len() {
        1 => {
            invoke(&nodes[suggestions[0].0]);
            pause(700);
            Ok(Ok(split_item(&suggestions[0].1).0))
        }
        0 if looks_like_number(contact) => {
            crate::input::press_enter()?;
            pause(700);
            Ok(Ok(contact.to_string()))
        }
        0 => {
            let _ = crate::input::press_escape();
            Err(WinError::NotFound(format!("não achei o contato '{contact}' no celular")))
        }
        _ => {
            let _ = crate::input::press_escape();
            let mut names: Vec<String> = suggestions.into_iter().map(|(_, t)| split_item(&t).0).collect();
            names.dedup();
            Ok(Err(names))
        }
    }
}

/// `phone.send_sms`: `contact` (nome ou número) e `text`. Contato ambíguo (ou
/// vazio) não envia nada: devolve `ambiguous` com as opções para o app perguntar.
/// `exact: true` = o nome veio de uma escolha do usuário (casa o nome inteiro).
pub fn send_sms(params: &Value) -> Result<Value, WinError> {
    let text = params["text"].as_str().unwrap_or("").trim().to_string();
    if text.is_empty() || text.chars().count() > 1000 {
        return Err(WinError::InvalidParams("'text' obrigatório (até 1000 caracteres)".into()));
    }
    let contact = crate::clean_chat_name(params["contact"].as_str().unwrap_or("")).unwrap_or_default();
    let exact = params["exact"].as_bool().unwrap_or(false);
    let (w, _leave) = open_phone()?;
    require_connected(&snapshot(w.id)?)?;
    if !go_tab(w.id, MESSAGE_TABS)? {
        return Err(WinError::Unavailable("não achei a aba Mensagens no Vincular ao Celular".into()));
    }
    if contact.is_empty() {
        // Sem destinatário: as conversas recentes viram as opções.
        let nodes = snapshot(w.id)?;
        let options: Vec<String> = conversation_items(&nodes, 10).iter().map(|t| split_item(t).0).collect();
        return Ok(json!({ "sent": false, "ambiguous": true, "options": options }));
    }
    let who = match open_conversation(&w, &contact, exact)? {
        Ok(who) => who,
        Err(options) => return Ok(json!({ "sent": false, "ambiguous": true, "options": options, "contact": contact })),
    };
    let nodes = snapshot(w.id)?;
    let field = message_field(&nodes).ok_or_else(|| WinError::NotFound("não achei o campo de mensagem".into()))?;
    fill(&w, field, &text)?;
    pause(300);
    let nodes = snapshot(w.id)?;
    let send = find_button(&nodes, &["enviar", "send", "enviar mensagem", "send message"])
        .ok_or_else(|| WinError::NotFound("não achei o botão Enviar".into()))?;
    if !invoke(send) {
        let (x, y) = send.rect.center();
        crate::input::click_at(x, y)?;
    }
    // Confere: o campo esvazia e/ou a mensagem aparece na conversa.
    let probe = fold(&text.chars().take(30).collect::<String>());
    let mut verified = false;
    for _ in 0..8 {
        pause(500);
        let nodes = snapshot(w.id)?;
        let cleared = message_field(&nodes).and_then(value_of).is_some_and(|v| v.trim().is_empty());
        let shown = nodes.iter().any(|n| n.kind != "edit" && fold(&n.name).contains(&probe));
        if cleared && shown {
            verified = true;
            break;
        }
    }
    Ok(json!({ "sent": true, "to": who, "verified": verified }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(kind: &'static str, name: &str, path: &str) -> Node {
        Node::fake(kind, name, path, None)
    }

    #[test]
    fn phone_window_by_process_or_title() {
        assert!(is_phone_window("PhoneExperienceHost", "qualquer"));
        assert!(is_phone_window("ApplicationFrameHost", "Vincular ao Celular"));
        assert!(is_phone_window("x", "Phone Link"));
        assert!(!is_phone_window("chrome", "Como usar o Phone Link - Google"));
        assert!(!is_phone_window("notepad", "celular.txt"));
    }

    #[test]
    fn status_reads_battery_signal_and_connection() {
        let tree = vec![
            n("text", "Galaxy X", ""),
            n("image", "Bateria: 76%", ""),
            n("image", "Intensidade do sinal: 3 de 4", ""),
            n("tabitem", "Mensagens", ""),
            n("tabitem", "Chamadas", ""),
        ];
        let st = parse_status(&tree);
        assert_eq!(st["connected"], true);
        assert_eq!(st["battery"], 76);
        assert_eq!(st["signal"], "3 de 4");
        assert_eq!(st["charging"], false);

        let noisy = vec![n("listitem", "App A, Bateria do fone: 5%", "notificacoes"), n("image", "Bateria: 60%", "")];
        assert_eq!(parse_status(&noisy)["battery"], 60, "notificação não é a bateria do celular");

        let en = vec![n("text", "Battery 15% charging", ""), n("tabitem", "Messages", ""), n("tabitem", "Calls", "")];
        let st = parse_status(&en);
        assert_eq!(st["battery"], 15);
        assert_eq!(st["charging"], true);

        let off = vec![n("text", "Celular desconectado", ""), n("button", "Reconectar", ""), n("tabitem", "Mensagens", "")];
        assert_eq!(parse_status(&off)["connected"], false);
        let setup = vec![n("text", "Vincule seu celular ao PC", ""), n("button", "Emparelhar com código QR", "")];
        let st = parse_status(&setup);
        assert_eq!(st["paired"], false);
        assert_eq!(st["connected"], false);
        assert!(st["battery"].is_null());
    }

    #[test]
    fn items_come_from_their_region_and_type_names_are_ignored() {
        let mut tree = vec![
            n("list", "Notificações", ""),
            n("listitem", "App A, Título 1, agora", "notificacoes"),
            n("listitem", "PhoneExperienceHost.ViewModels.NotificationItem", "notificacoes"),
            Node::fake("text", "App B", "notificacoes / item", Some(2)),
            Node::fake("text", "Lembrete", "notificacoes / item", Some(2)),
            n("listitem", "user x, oi!, 10:30", "conversas"),
            n("listitem", "Mensagens", "navegacao"),
        ];
        let got = items_under(&tree, NOTIFICATION_ANCHORS, &[], 10);
        assert_eq!(got, vec!["App A, Título 1, agora".to_string(), "App B · Lembrete".to_string()]);
        assert_eq!(conversation_items(&tree, 10), vec!["user x, oi!, 10:30".to_string()]);
        tree.truncate(2);
        assert_eq!(items_under(&tree, NOTIFICATION_ANCHORS, &[], 1).len(), 1, "respeita o limite");
        let long = "x".repeat(500);
        let t = vec![n("listitem", &long, "notificacoes")];
        assert!(items_under(&t, NOTIFICATION_ANCHORS, &[], 5)[0].chars().count() <= MAX_TEXT + 1);
    }

    #[test]
    fn item_text_splits_into_who_and_rest() {
        assert_eq!(split_item("user x, oi tudo bem?, 10:30"), ("user x".into(), "oi tudo bem?, 10:30".into()));
        assert_eq!(split_item("App B · Lembrete"), ("App B".into(), "Lembrete".into()));
        assert_eq!(split_item("sozinho"), ("sozinho".into(), String::new()));
    }

    #[test]
    fn contacts_match_by_whole_words_and_ambiguity_is_kept() {
        let tree = vec![
            n("listitem", "User X, oi, 10:30", "conversas"),
            n("listitem", "User X Trabalho, reunião, ontem", "conversas"),
            n("listitem", "User Y, ok, ontem", "conversas"),
            n("listitem", "User X, notificação", "notificacoes"),
        ];
        let many = matching_conversations(&tree, "user x", false);
        assert_eq!(many.iter().map(|(_, w)| w.as_str()).collect::<Vec<_>>(), vec!["User X", "User X Trabalho"]);
        let one = matching_conversations(&tree, "User X", true);
        assert_eq!(one.len(), 1, "escolha do usuário casa o nome inteiro");
        assert_eq!(matching_conversations(&tree, "user y", false).len(), 1);
        assert_eq!(matching_conversations(&tree, "user", false).len(), 3);
        assert!(matching_conversations(&tree, "user z", false).is_empty());
    }

    #[test]
    fn phone_numbers_are_recognized() {
        assert!(looks_like_number("+55 11 90000-0000"));
        assert!(looks_like_number("(11) 90000-0000"));
        assert!(!looks_like_number("user x"));
        assert!(!looks_like_number("1234"));
    }
}
