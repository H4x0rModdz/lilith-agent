//! Leitura do conteúdo de janelas abertas via UI Automation — a mesma API
//! que leitores de tela usam. Lê o texto do documento/campo principal
//! (Bloco de Notas, Word, página do navegador…) e, se não houver, junta os
//! textos visíveis da janela. Sem OCR e sem screenshot: barato e exato.

use crate::WinError;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationValuePattern,
    UIA_DocumentControlTypeId, UIA_EditControlTypeId, UIA_TextControlTypeId, UIA_TextPatternId, UIA_ValuePatternId,
};

/// Teto de nós visitados: navegadores têm árvores enormes.
const MAX_NODES: usize = 3000;

pub fn read_window_text(id: isize, max_chars: usize) -> Result<String, WinError> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
        let root = uia.ElementFromHandle(HWND(id as *mut _)).map_err(|_| {
            WinError::Unavailable("não consegui acessar essa janela (app de administrador?)".into())
        })?;
        let walker = uia.ControlViewWalker()?;

        let mut stack = vec![root];
        let mut visited = 0usize;
        let mut labels: Vec<String> = Vec::new();
        while let Some(el) = stack.pop() {
            visited += 1;
            if visited > MAX_NODES {
                break;
            }
            let control = el.CurrentControlType().unwrap_or_default();
            if control == UIA_DocumentControlTypeId || control == UIA_EditControlTypeId {
                if let Some(text) = document_text(&el, max_chars) {
                    return Ok(text);
                }
            }
            if control == UIA_TextControlTypeId {
                if let Ok(name) = el.CurrentName() {
                    let name = name.to_string();
                    if !name.trim().is_empty() && labels.last() != Some(&name) {
                        labels.push(name);
                    }
                }
            }
            // Filhos em ordem reversa na pilha = leitura na ordem da tela.
            let mut children = Vec::new();
            let mut next = walker.GetFirstChildElement(&el);
            while let Ok(child) = next {
                next = walker.GetNextSiblingElement(&child);
                children.push(child);
            }
            stack.extend(children.into_iter().rev());
        }

        let joined = labels.join("\n");
        if joined.trim().is_empty() {
            return Err(WinError::NotFound("não achei texto legível nessa janela".into()));
        }
        Ok(truncate(joined, max_chars))
    }
}

/// Texto do documento inteiro (TextPattern) ou o valor do campo (ValuePattern).
unsafe fn document_text(el: &IUIAutomationElement, max_chars: usize) -> Option<String> {
    if let Ok(pattern) = el.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) {
        if let Ok(text) = pattern.DocumentRange().and_then(|r| r.GetText(max_chars as i32)) {
            let text = text.to_string();
            if !text.trim().is_empty() {
                return Some(truncate(text, max_chars));
            }
        }
    }
    if let Ok(pattern) = el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) {
        if let Ok(value) = pattern.CurrentValue() {
            let value = value.to_string();
            if !value.trim().is_empty() {
                return Some(truncate(value, max_chars));
            }
        }
    }
    None
}

fn truncate(s: String, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s
    } else {
        s.chars().take(max_chars).collect::<String>() + "…"
    }
}

// ── navegar dentro de um programa: achar e "clicar" itens ─────────────────

use crate::paths::fold;
use crate::window::Rect;
use serde::Serialize;
use windows::Win32::UI::Accessibility::{
    IUIAutomationExpandCollapsePattern, IUIAutomationInvokePattern, IUIAutomationLegacyIAccessiblePattern,
    IUIAutomationSelectionItemPattern, IUIAutomationTogglePattern, UIA_ButtonControlTypeId,
    UIA_CheckBoxControlTypeId, UIA_ComboBoxControlTypeId, UIA_DataItemControlTypeId, UIA_ExpandCollapsePatternId,
    UIA_HyperlinkControlTypeId, UIA_InvokePatternId, UIA_LegacyIAccessiblePatternId, UIA_ListItemControlTypeId,
    UIA_MenuItemControlTypeId, UIA_RadioButtonControlTypeId, UIA_SelectionItemPatternId, UIA_TabItemControlTypeId,
    UIA_TogglePatternId, UIA_TreeItemControlTypeId, UIA_CONTROLTYPE_ID,
};

/// Teto de nós na busca (a árvore do Discord/navegador é grande).
const FIND_MAX_NODES: usize = 8000;
/// Nomes são texto de terceiros (ex.: apelido de alguém): curtos, para não
/// virar veículo de prompt injection.
const MAX_NAME: usize = 80;

#[derive(Debug, Clone, Serialize)]
pub struct UiElement {
    /// Nome visível (como um leitor de tela leria).
    pub name: String,
    /// Tipo em pt-BR (botão, item de lista, link…).
    pub kind: String,
    /// Grupo onde está — ex.: "Mensagens diretas" x "Servidores".
    pub context: String,
    pub rect: Rect,
    pub clickable: bool,
}

fn kind_of(id: UIA_CONTROLTYPE_ID) -> (&'static str, bool) {
    match id {
        x if x == UIA_ButtonControlTypeId => ("botão", true),
        x if x == UIA_ListItemControlTypeId => ("item de lista", true),
        x if x == UIA_HyperlinkControlTypeId => ("link", true),
        x if x == UIA_TreeItemControlTypeId => ("item de árvore", true),
        x if x == UIA_TabItemControlTypeId => ("aba", true),
        x if x == UIA_MenuItemControlTypeId => ("item de menu", true),
        x if x == UIA_CheckBoxControlTypeId => ("caixa de seleção", true),
        x if x == UIA_RadioButtonControlTypeId => ("opção", true),
        x if x == UIA_ComboBoxControlTypeId => ("lista suspensa", true),
        x if x == UIA_DataItemControlTypeId => ("item", true),
        x if x == UIA_DocumentControlTypeId => ("documento", false),
        x if x == UIA_EditControlTypeId => ("campo de texto", false),
        x if x == UIA_TextControlTypeId => ("texto", false),
        _ => ("elemento", false),
    }
}

/// Quão bem `name` casa com a busca: 0 começa com ela, 1 contém, 2 contém
/// todas as palavras; None não casa. Sem acento/maiúscula.
pub(crate) fn match_rank(name: &str, query: &str) -> Option<u8> {
    let (n, q) = (fold(name), fold(query));
    if q.trim().is_empty() {
        return None;
    }
    if n.starts_with(&q) {
        Some(0)
    } else if n.contains(&q) {
        Some(1)
    } else if q.split_whitespace().all(|w| n.contains(w)) {
        Some(2)
    } else {
        None
    }
}

fn short(s: &str, max: usize) -> String {
    let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > max {
        s.chars().take(max).collect::<String>() + "…"
    } else {
        s
    }
}

/// Elementos visíveis da janela cujo nome casa com `query`, melhores
/// primeiro (clicáveis antes; depois começo do nome > contém > palavras).
pub fn find_elements(id: isize, query: &str, limit: usize) -> Result<Vec<UiElement>, WinError> {
    Ok(search(id, query)?.into_iter().take(limit).map(|(_, e, _)| e).collect())
}

/// Ordem: clicável, melhor nome, link/botão antes de item de lista (no
/// Discord o link é que abre a conversa), ordem na tela.
type Hit = ((bool, u8, u8, usize), UiElement, IUIAutomationElement);

/// Entre nomes iguais, quem costuma responder ao clique.
fn kind_priority(kind: &str) -> u8 {
    match kind {
        "link" | "botão" => 0,
        "aba" | "item de menu" | "caixa de seleção" | "opção" | "lista suspensa" => 1,
        _ => 2,
    }
}

fn search(id: isize, query: &str) -> Result<Vec<Hit>, WinError> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
        let root = uia.ElementFromHandle(HWND(id as *mut _)).map_err(|_| {
            WinError::Unavailable("não consegui acessar essa janela (app de administrador?)".into())
        })?;
        let walker = uia.ControlViewWalker()?;

        let mut hits: Vec<Hit> = Vec::new();
        let mut stack = vec![(root, String::new())];
        let mut visited = 0usize;
        while let Some((el, context)) = stack.pop() {
            visited += 1;
            if visited > FIND_MAX_NODES {
                break;
            }
            let name = el.CurrentName().map(|b| b.to_string()).unwrap_or_default();
            if let Some(rank) = match_rank(&name, query) {
                let offscreen = el.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false);
                let r = el.CurrentBoundingRectangle().unwrap_or_default();
                let rect = Rect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top };
                if !offscreen && rect.w > 0 && rect.h > 0 {
                    let (kind, clickable) = kind_of(el.CurrentControlType().unwrap_or_default());
                    let element = UiElement {
                        name: short(&name, MAX_NAME),
                        kind: kind.into(),
                        context: short(&context, 40),
                        rect,
                        clickable,
                    };
                    let key = (!clickable, rank, kind_priority(&element.kind), hits.len());
                    hits.push((key, element, el.clone()));
                }
            }
            // O grupo dos filhos é o nome mais próximo acima deles.
            let child_context = if name.trim().is_empty() { context } else { name };
            let mut children = Vec::new();
            let mut next = walker.GetFirstChildElement(&el);
            while let Ok(child) = next {
                next = walker.GetNextSiblingElement(&child);
                children.push((child, child_context.clone()));
            }
            stack.extend(children.into_iter().rev());
        }
        hits.sort_by_key(|(key, _, _)| *key);
        Ok(hits)
    }
}

/// Localiza o `index`-ésimo melhor elemento (para o cursor mirar antes).
pub fn locate(id: isize, query: &str, index: usize) -> Result<UiElement, WinError> {
    search(id, query)?
        .into_iter()
        .nth(index)
        .map(|(_, e, _)| e)
        .ok_or_else(|| WinError::NotFound(format!("nada chamado '{query}' nessa janela")))
}

/// Acha o `index`-ésimo melhor elemento e o aciona por acessibilidade
/// (Invoke, Select, Toggle, Expand ou a ação padrão) — sem mexer no mouse.
pub fn activate(id: isize, query: &str, index: usize) -> Result<UiElement, WinError> {
    let Some((_, info, el)) = search(id, query)?.into_iter().nth(index) else {
        return Err(WinError::NotFound(format!("nada chamado '{query}' nessa janela")));
    };
    unsafe {
        if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId) {
            if p.Invoke().is_ok() {
                return Ok(info);
            }
        }
        if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId) {
            if p.Select().is_ok() {
                return Ok(info);
            }
        }
        if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId) {
            if p.Toggle().is_ok() {
                return Ok(info);
            }
        }
        if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(UIA_ExpandCollapsePatternId) {
            if p.Expand().is_ok() {
                return Ok(info);
            }
        }
        if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(UIA_LegacyIAccessiblePatternId) {
            if p.DoDefaultAction().is_ok() {
                return Ok(info);
            }
        }
    }
    Err(WinError::Failed(format!("'{}' não aceita clique por acessibilidade", info.name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_rank_is_accent_and_case_insensitive() {
        assert_eq!(match_rank("Kuromi 2 meses pro natal", "kuromi"), Some(0));
        assert_eq!(match_rank("Conversa com Kuromi", "KUROMI"), Some(1));
        assert_eq!(match_rank("Kuromi 2 meses pro natal", "natal kuromi"), Some(2));
        assert_eq!(match_rank("Configurações", "configuracoes"), Some(0));
        assert_eq!(match_rank("gui", "kuromi"), None);
        assert_eq!(match_rank("qualquer", "  "), None);
    }

    #[test]
    fn names_are_shortened() {
        assert_eq!(short("  a   b ", 10), "a b");
        assert!(short(&"x".repeat(200), 80).ends_with('…'));
    }
}

// ── mensagens de chat com horário (Discord, etc.) ─────────────────────────

use chrono::{Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    /// Hora da mensagem (HH:MM, horário local).
    pub time: String,
    /// Texto como o app expõe para leitores de tela (no Discord:
    /// "autor , conteúdo , HH:MM").
    pub text: String,
    #[serde(skip)]
    pub ts: i64,
}

fn full_date_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(\d{1,2}) de (\p{L}+) de (\d{4}),? (\d{1,2}):(\d{2})").unwrap())
}

fn hhmm_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b(\d{1,2}):(\d{2})\b").unwrap())
}

fn month(name: &str) -> Option<u32> {
    let m = ["janeiro", "fevereiro", "marco", "abril", "maio", "junho", "julho", "agosto", "setembro", "outubro", "novembro", "dezembro"];
    m.iter().position(|x| *x == fold(name)).map(|i| i as u32 + 1)
}

/// "11 de outubro de 2026 12:11" → timestamp local (ms).
pub(crate) fn parse_full_date(s: &str) -> Option<i64> {
    let c = full_date_re().captures(s)?;
    let date = NaiveDate::from_ymd_opt(c[3].parse().ok()?, month(&c[2])?, c[1].parse().ok()?)?;
    let time = NaiveTime::from_hms_opt(c[4].parse().ok()?, c[5].parse().ok()?, 0)?;
    to_ms(NaiveDateTime::new(date, time))
}

/// Só "HH:MM": hoje, ou ontem se ainda não chegou essa hora hoje.
pub(crate) fn parse_clock(s: &str, now: NaiveDateTime) -> Option<i64> {
    let c = hhmm_re().captures_iter(s).last()?;
    let time = NaiveTime::from_hms_opt(c[1].parse().ok()?, c[2].parse().ok()?, 0)?;
    let mut dt = NaiveDateTime::new(now.date(), time);
    if dt > now + chrono::Duration::minutes(1) {
        dt -= chrono::Duration::days(1);
    }
    to_ms(dt)
}

fn to_ms(dt: NaiveDateTime) -> Option<i64> {
    Local.from_local_datetime(&dt).earliest().map(|d| d.timestamp_millis())
}

/// Mensagens visíveis da janela, em ordem, com horário. Vazio se o app não
/// expõe mensagens com horário (aí quem chama lê o texto normal).
pub fn read_messages(id: isize) -> Result<Vec<ChatMessage>, WinError> {
    let now = Local::now().naive_local();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
        let root = uia.ElementFromHandle(HWND(id as *mut _)).map_err(|_| {
            WinError::Unavailable("não consegui acessar essa janela (app de administrador?)".into())
        })?;
        let walker = uia.ControlViewWalker()?;

        let mut out: Vec<ChatMessage> = Vec::new();
        // (elemento, data completa herdada do item de lista acima)
        let mut stack: Vec<(IUIAutomationElement, Option<i64>)> = vec![(root, None)];
        let mut visited = 0usize;
        while let Some((el, inherited)) = stack.pop() {
            visited += 1;
            if visited > FIND_MAX_NODES {
                break;
            }
            let name = el.CurrentName().map(|b| b.to_string()).unwrap_or_default();
            let kind = el.CurrentLocalizedControlType().map(|b| fold(&b.to_string())).unwrap_or_default();
            let ts_here = parse_full_date(&name).or(inherited);

            if (kind == "mensagem" || kind == "message") && !name.trim().is_empty() {
                if let Some(ts) = ts_here.or_else(|| parse_clock(&name, now)) {
                    let time = Local.timestamp_millis_opt(ts).single().map(|d| d.format("%H:%M").to_string());
                    out.push(ChatMessage {
                        time: time.unwrap_or_default(),
                        text: short(&name, 600),
                        ts,
                    });
                }
                continue; // o conteúdo já está no nome da mensagem
            }
            let mut children = Vec::new();
            let mut next = walker.GetFirstChildElement(&el);
            while let Ok(child) = next {
                next = walker.GetNextSiblingElement(&child);
                children.push((child, ts_here));
            }
            stack.extend(children.into_iter().rev());
        }
        Ok(out)
    }
}

#[cfg(test)]
mod message_tests {
    use super::*;

    #[test]
    fn parses_discord_full_dates() {
        let ts = parse_full_date("12:11Hoje, 11 de outubro de 2026 12:11texto").unwrap();
        let dt = Local.timestamp_millis_opt(ts).unwrap().naive_local();
        assert_eq!(dt.format("%Y-%m-%d %H:%M").to_string(), "2026-10-11 12:11");
        assert!(parse_full_date("sem data aqui").is_none());
        assert!(parse_full_date("3 de março de 2026 09:05").is_some());
    }

    #[test]
    fn clock_only_is_today_or_yesterday() {
        let now = NaiveDateTime::parse_from_str("2026-10-11 00:10", "%Y-%m-%d %H:%M").unwrap();
        let earlier = parse_clock("gui , oi , 00:05", now).unwrap();
        let late_yesterday = parse_clock("gui , oi , 23:50", now).unwrap();
        assert!(earlier > late_yesterday);
        let d = Local.timestamp_millis_opt(late_yesterday).unwrap().naive_local();
        assert_eq!(d.format("%Y-%m-%d").to_string(), "2026-10-10");
    }
}
