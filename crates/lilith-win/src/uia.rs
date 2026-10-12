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

/// O que a janela mostra além do documento principal: avisos, barras de
/// status, listas (Lista de Erros, Gerenciador de Soluções…) e botões.
pub struct WindowRead {
    /// Texto do documento/campo principal (o editor, a página…), se houver.
    pub document: Option<String>,
    /// Demais textos visíveis, na ordem da tela (avisos, status, itens de lista…).
    pub texts: Vec<String>,
    /// Itens cujo nome contém a busca (`query`), com o tipo.
    pub matches: Vec<String>,
}

/// Teto de nós da visão geral (editores e navegadores têm árvores enormes).
const OVERVIEW_MAX_NODES: usize = 6000;
/// Cada texto de terceiros é curto: não vira veículo de prompt injection.
const OVERVIEW_MAX_TEXT: usize = 300;

/// Lê o documento principal E o resto da janela. `query` procura também por
/// nome em qualquer item visível (inclusive botões e linhas de lista) e no
/// texto do documento. Só o que está visível na tela.
pub fn read_window_overview(id: isize, max_chars: usize, query: Option<&str>) -> Result<WindowRead, WinError> {
    use windows::Win32::UI::Accessibility::{
        UIA_ButtonControlTypeId, UIA_CustomControlTypeId, UIA_DataItemControlTypeId, UIA_GroupControlTypeId,
        UIA_HyperlinkControlTypeId, UIA_ListItemControlTypeId, UIA_PaneControlTypeId, UIA_StatusBarControlTypeId,
        UIA_TreeItemControlTypeId,
    };
    let needle = query.map(|q| crate::paths::fold(q.trim())).filter(|q| !q.is_empty());
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
        let root = uia.ElementFromHandle(HWND(id as *mut _)).map_err(|_| {
            WinError::Unavailable("não consegui acessar essa janela (app de administrador?)".into())
        })?;
        let walker = uia.ControlViewWalker()?;

        let mut document: Option<String> = None;
        let mut texts: Vec<String> = Vec::new();
        // Botões e links (barras de ferramentas) vão depois: avisos e textos primeiro.
        let mut controls: Vec<String> = Vec::new();
        let mut matches: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![root];
        let mut visited = 0usize;
        while let Some(el) = stack.pop() {
            visited += 1;
            if visited > OVERVIEW_MAX_NODES {
                break;
            }
            if el.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false) {
                continue;
            }
            let control = el.CurrentControlType().unwrap_or_default();
            // O documento/campo principal: pega o texto e não desce nele (o
            // editor expõe uma linha por nó, o que estoura o limite).
            if control == UIA_DocumentControlTypeId || control == UIA_EditControlTypeId {
                if document.is_none() {
                    document = document_text(&el, max_chars);
                }
                if let (Some(n), Some(text)) = (&needle, &document) {
                    for line in text.lines().filter(|l| crate::paths::fold(l).contains(n.as_str())).take(10) {
                        matches.push(format!("no documento: {}", clip(line.trim())));
                    }
                }
                continue;
            }
            let name = el.CurrentName().map(|n| n.to_string()).unwrap_or_default();
            let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
            if !name.is_empty() {
                let (label, shown) = if control == UIA_TextControlTypeId {
                    ("texto", true)
                } else if control == UIA_HyperlinkControlTypeId {
                    ("link", true)
                } else if control == UIA_ButtonControlTypeId {
                    ("botão", true)
                } else if control == UIA_ListItemControlTypeId || control == UIA_DataItemControlTypeId {
                    ("item", true)
                } else if control == UIA_TreeItemControlTypeId {
                    ("árvore", true)
                } else if control == UIA_StatusBarControlTypeId {
                    ("status", true)
                } else if (control == UIA_PaneControlTypeId || control == UIA_GroupControlTypeId || control == UIA_CustomControlTypeId)
                    && name.chars().count() >= 25
                    && name.contains(' ')
                {
                    // Avisos (barras de informação) costumam ser um painel cujo nome é a frase.
                    ("aviso", true)
                } else {
                    ("", false)
                };
                if let Some(n) = &needle {
                    if crate::paths::fold(&name).contains(n.as_str()) && matches.len() < 40 && seen.insert(format!("m|{label}|{name}")) {
                        matches.push(format!("{}: {}", if label.is_empty() { "item" } else { label }, clip(&name)));
                    }
                }
                if shown && seen.insert(format!("t|{label}|{name}")) {
                    let line = match label {
                        "texto" => clip(&name),
                        _ => format!("[{label}] {}", clip(&name)),
                    };
                    if label == "botão" || label == "link" {
                        controls.push(line);
                    } else {
                        texts.push(line);
                    }
                }
            }
            let mut children = Vec::new();
            let mut next = walker.GetFirstChildElement(&el);
            while let Ok(child) = next {
                next = walker.GetNextSiblingElement(&child);
                children.push(child);
            }
            stack.extend(children.into_iter().rev());
        }
        texts.extend(controls);
        if document.is_none() && texts.is_empty() {
            return Err(WinError::NotFound("não achei texto legível nessa janela".into()));
        }
        // Orçamento: o documento ocupa até 60%; o resto é dos demais textos.
        let doc_budget = if texts.is_empty() { max_chars } else { max_chars * 6 / 10 };
        let document = document.map(|d| truncate(d, doc_budget));
        let mut left = max_chars.saturating_sub(document.as_ref().map(|d| d.chars().count()).unwrap_or(0)).max(500);
        let mut kept = Vec::new();
        for t in texts {
            let n = t.chars().count() + 1;
            if n > left {
                break;
            }
            left -= n;
            kept.push(t);
        }
        Ok(WindowRead { document, texts: kept, matches })
    }
}

fn clip(s: &str) -> String {
    let s: String = s.chars().take(OVERVIEW_MAX_TEXT).collect();
    s
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
    UIA_TogglePatternId, UIA_TreeItemControlTypeId, UIA_CONTROLTYPE_ID, UIA_ControlTypePropertyId,
    TreeScope_Descendants,
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

/// Verbos de ação com efeito (pt/en). Clicar em algo com esse nome nunca é
/// "só navegar".
const EFFECT_WORDS: &[&str] = &[
    "enviar", "excluir", "apagar", "deletar", "remover", "sair", "aceitar", "permitir", "confirmar", "pagar",
    "comprar", "fechar", "desinstalar", "instalar", "bloquear", "banir", "expulsar", "transferir", "assinar",
    "formatar", "publicar", "postar", "compartilhar", "convidar", "denunciar", "limpar", "resetar", "salvar",
    "sim", "ok", "send", "delete", "remove", "leave", "accept", "allow", "confirm", "pay", "buy", "close",
    "uninstall", "install", "block", "ban", "kick", "transfer", "sign", "format", "publish", "post", "share",
    "invite", "report", "clear", "reset", "save", "yes",
];

/// Clique de pura navegação (abrir uma conversa, trocar de aba, seguir um
/// link) — pode rodar sem perguntar. Botões, menus, caixas e qualquer item
/// cujo nome tenha verbo de efeito pedem aprovação.
pub fn is_navigation(e: &UiElement) -> bool {
    let navigational = matches!(e.kind.as_str(), "link" | "item de lista" | "item de árvore" | "aba" | "item");
    let name = fold(&e.name);
    let has_effect_word = name
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| EFFECT_WORDS.contains(&w));
    navigational && !has_effect_word
}

/// Localiza o `index`-ésimo melhor elemento (para o cursor mirar antes).
pub fn locate(id: isize, query: &str, index: usize) -> Result<UiElement, WinError> {
    search(id, query)?
        .into_iter()
        .nth(index)
        .map(|(_, e, _)| e)
        .ok_or_else(|| WinError::NotFound(format!("nada chamado '{query}' nessa janela")))
}

/// Como o clique foi dado.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClickVia {
    /// Por acessibilidade — o mouse do usuário não foi usado.
    Accessibility,
    /// Clique real do mouse (devolvido ao lugar): o app ignora acessibilidade.
    Mouse,
}

/// Aciona por acessibilidade e, se o app não aceitar (WhatsApp, Discord…),
/// cai num clique real no centro do elemento.
pub fn click(id: isize, query: &str, index: usize) -> Result<(UiElement, ClickVia), WinError> {
    match activate(id, query, index) {
        Ok(info) => Ok((info, ClickVia::Accessibility)),
        Err(WinError::Failed(_)) => {
            let info = locate(id, query, index)?;
            let (x, y) = info.rect.center();
            crate::input::click_at(x, y)?;
            Ok((info, ClickVia::Mouse))
        }
        Err(e) => Err(e),
    }
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

/// Onde está o campo de mensagem, usando a memória de interface: se já
/// descobrimos onde ele fica neste programa, devolve o retângulo na hora
/// (milissegundos); senão varre uma vez (~2 s no WhatsApp) e guarda.
/// O retângulo guardado NÃO é validado aqui: quem clicar nele confirma com
/// [`focused_is_text_input`] e chama [`forget_message_input`] se errou.
pub fn message_input_for(w: &crate::window::WindowInfo) -> Result<UiElement, WinError> {
    if let Some(rect) = crate::ui_map::lookup(&w.app, MESSAGE_INPUT, w.frame) {
        return Ok(UiElement::field(rect));
    }
    discover_message_input(w)
}

/// Varre, guarda e devolve (ignora o que estava guardado).
pub fn discover_message_input(w: &crate::window::WindowInfo) -> Result<UiElement, WinError> {
    let found = message_input(w.id)?;
    crate::ui_map::remember(&w.app, MESSAGE_INPUT, &found.kind, found.rect, w.frame);
    Ok(found)
}

pub fn confirm_message_input(app: &str) {
    crate::ui_map::confirm(app, MESSAGE_INPUT);
}

pub fn forget_message_input(app: &str) {
    crate::ui_map::forget(app, MESSAGE_INPUT);
}

const MESSAGE_INPUT: &str = "message_input";

/// Nome acessível do elemento com o foco do teclado — no WhatsApp e no
/// Discord o campo de mensagem se chama "…para <chat>" / "Conversar em <chat>",
/// então diz em que conversa estamos (quando o foco está nele).
pub fn focused_name() -> Option<String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        let el = uia.GetFocusedElement().ok()?;
        (el.CurrentControlType().ok()? == UIA_EditControlTypeId)
            .then(|| el.CurrentName().ok().map(|b| b.to_string()))
            .flatten()
    }
}

/// O elemento com o foco do teclado é um campo de texto? Uma chamada só —
/// é como confirmamos que o clique caiu no campo certo (o teste "o que existe
/// neste ponto" não serve em apps web: devolve só um painel genérico).
pub fn focused_is_text_input() -> bool {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(uia) = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER) else {
            return false;
        };
        uia.GetFocusedElement()
            .and_then(|el| el.CurrentControlType())
            .map(|ct| ct == UIA_EditControlTypeId)
            .unwrap_or(false)
    }
}

/// Campo de mensagem da janela, sem depender do nome (que muda em cada app
/// e idioma): entre os campos de texto visíveis e focáveis, o mais baixo —
/// e, em empate, o mais largo. A busca fica acima e é mais estreita.
pub fn message_input(id: isize) -> Result<UiElement, WinError> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
        let root = uia.ElementFromHandle(HWND(id as *mut _)).map_err(|_| {
            WinError::Unavailable("não consegui acessar essa janela (app de administrador?)".into())
        })?;
        // Uma busca por TIPO feita pelo próprio app, de uma vez: caminhar a
        // árvore nó a nó custava ~2 s no WhatsApp (milhares de chamadas entre
        // processos); assim são só os campos de texto.
        let edit = windows::Win32::System::Variant::VARIANT::from(UIA_EditControlTypeId.0);
        let condition = uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &edit)?;
        let found = root.FindAll(TreeScope_Descendants, &condition)?;
        let mut best: Option<UiElement> = None;
        for i in 0..found.Length().unwrap_or(0) {
            let Ok(el) = found.GetElement(i) else { continue };
            let offscreen = el.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false);
            let focusable = el.CurrentIsKeyboardFocusable().map(|b| b.as_bool()).unwrap_or(false);
            let r = el.CurrentBoundingRectangle().unwrap_or_default();
            let rect = Rect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top };
            if offscreen || !focusable || rect.w <= 0 || rect.h <= 0 {
                continue;
            }
            let cand = UiElement {
                name: short(&el.CurrentName().map(|b| b.to_string()).unwrap_or_default(), MAX_NAME),
                kind: "campo de texto".into(),
                context: String::new(),
                rect,
                clickable: true,
            };
            best = Some(match best {
                Some(b) if is_lower_input(&b.rect, &cand.rect) => b,
                _ => cand,
            });
        }
        match best {
            Some(b) => Ok(b),
            // Alguns apps (Teams) não devolvem o campo na busca por tipo, mas
            // ele aparece caminhando a árvore — que nesses casos é pequena.
            None => message_input_by_walking(id),
        }
    }
}

/// Mesmo critério (campo de texto mais baixo e largo), nó a nó.
fn message_input_by_walking(id: isize) -> Result<UiElement, WinError> {
    unsafe {
        let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
        let root = uia.ElementFromHandle(HWND(id as *mut _))?;
        let walker = uia.ControlViewWalker()?;
        let mut stack = vec![root];
        let mut best: Option<UiElement> = None;
        let mut visited = 0usize;
        while let Some(el) = stack.pop() {
            visited += 1;
            if visited > FIND_MAX_NODES {
                break;
            }
            if el.CurrentControlType().unwrap_or_default() == UIA_EditControlTypeId {
                let offscreen = el.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false);
                let focusable = el.CurrentIsKeyboardFocusable().map(|b| b.as_bool()).unwrap_or(false);
                let r = el.CurrentBoundingRectangle().unwrap_or_default();
                let rect = Rect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top };
                if !offscreen && focusable && rect.w > 0 && rect.h > 0 {
                    let cand = UiElement::field(rect);
                    best = Some(match best {
                        Some(b) if is_lower_input(&b.rect, &cand.rect) => b,
                        _ => cand,
                    });
                }
            }
            let mut children = Vec::new();
            let mut next = walker.GetFirstChildElement(&el);
            while let Ok(child) = next {
                next = walker.GetNextSiblingElement(&child);
                children.push(child);
            }
            stack.extend(children.into_iter().rev());
        }
        best.ok_or_else(|| WinError::NotFound("campo de mensagem nessa janela".into()))
    }
}

impl UiElement {
    /// Campo conhecido só pelo retângulo (já localizado antes).
    pub fn field(rect: Rect) -> Self {
        UiElement { name: String::new(), kind: "campo de texto".into(), context: String::new(), rect, clickable: true }
    }
}

/// `a` ganha de `b` como campo de mensagem? (mais baixo; empate: mais largo)
pub(crate) fn is_lower_input(a: &Rect, b: &Rect) -> bool {
    let (ab, bb) = (a.y + a.h, b.y + b.h);
    ab > bb || (ab == bb && a.w >= b.w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_rank_is_accent_and_case_insensitive() {
        assert_eq!(match_rank("User X chat", "user x"), Some(0));
        assert_eq!(match_rank("Chat with User X", "USER X"), Some(1));
        assert_eq!(match_rank("User X chat", "chat user"), Some(2));
        assert_eq!(match_rank("Configurações", "configuracoes"), Some(0));
        assert_eq!(match_rank("user y", "user x"), None);
        assert_eq!(match_rank("qualquer", "  "), None);
    }

    fn el(name: &str, kind: &str) -> UiElement {
        UiElement { name: name.into(), kind: kind.into(), context: String::new(), rect: Rect { x: 0, y: 0, w: 1, h: 1 }, clickable: true }
    }

    #[test]
    fn only_plain_navigation_is_auto_approved() {
        assert!(is_navigation(&el("User X chat (direct message)", "link")));
        assert!(is_navigation(&el("Geral", "aba")));
        assert!(is_navigation(&el("Group A", "item de árvore")));
        // Botões e afins sempre pedem.
        assert!(!is_navigation(&el("User X", "botão")));
        assert!(!is_navigation(&el("Arquivo", "item de menu")));
        // Nome com verbo de efeito, mesmo em link/lista.
        assert!(!is_navigation(&el("Excluir servidor", "item de lista")));
        assert!(!is_navigation(&el("Sair do servidor", "link")));
        assert!(!is_navigation(&el("Delete message", "link")));
        // Palavra inteira: "saida" não é "sair", "okay" não é "ok".
        assert!(is_navigation(&el("Caixa de saída", "link")));
    }

    #[test]
    fn message_box_is_the_lowest_widest_input() {
        let search = Rect { x: 10, y: 480, w: 291, h: 21 };
        let message = Rect { x: 400, y: 973, w: 625, h: 24 };
        assert!(is_lower_input(&message, &search));
        assert!(!is_lower_input(&search, &message));
        let narrow = Rect { x: 0, y: 973, w: 100, h: 24 };
        assert!(is_lower_input(&message, &narrow), "empate de altura: o mais largo");
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
        let earlier = parse_clock("userx , oi , 00:05", now).unwrap();
        let late_yesterday = parse_clock("userx , oi , 23:50", now).unwrap();
        assert!(earlier > late_yesterday);
        let d = Local.timestamp_millis_opt(late_yesterday).unwrap().naive_local();
        assert_eq!(d.format("%Y-%m-%d").to_string(), "2026-10-10");
    }
}
