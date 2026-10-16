//! Leitura de janelas de desenvolvimento para achar erros de build. Difere de
//! `uia::read_window_overview` em dois pontos: pega o FIM de cada área de
//! texto (terminal e janela Saída crescem para baixo; o começo é histórico
//! velho) e lê as linhas de tabela/lista célula por célula (Lista de Erros do
//! Visual Studio, painel Problemas). Nada disso vai cru para o modelo: só o
//! que o parser reconhece como erro (ver `errors`).

use crate::WinError;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTreeWalker,
    UIA_CustomControlTypeId, UIA_DataItemControlTypeId, UIA_DocumentControlTypeId, UIA_EditControlTypeId,
    UIA_ListItemControlTypeId, UIA_PaneControlTypeId, UIA_TextControlTypeId, UIA_TextPatternId, UIA_TreeItemControlTypeId,
};

/// O que a janela mostra de útil para achar erros.
#[derive(Debug, Default)]
pub struct DevRead {
    /// Áreas de texto (terminal, Saída, editores visíveis): o final de cada uma.
    pub texts: Vec<String>,
    /// Linhas de tabela/lista: o nome da linha e os nomes das células.
    pub rows: Vec<Vec<String>>,
}

const MAX_NODES: usize = 6000;
/// Quanto do fim de cada área de texto entra (a saída do build mais recente).
const TAIL_CHARS: usize = 60_000;
const MAX_TEXT_AREAS: usize = 8;
const MAX_ROWS: usize = 400;
const MAX_CELLS: usize = 16;

/// Lê a janela `id`. Só o que está visível na tela.
pub fn read(id: isize) -> Result<DevRead, WinError> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
        let root = uia.ElementFromHandle(HWND(id as *mut _)).map_err(|_| {
            WinError::Unavailable("não consegui acessar essa janela (app de administrador?)".into())
        })?;
        let walker = uia.ControlViewWalker()?;
        let mut out = DevRead::default();
        let mut stack = vec![root];
        let mut visited = 0usize;
        while let Some(el) = stack.pop() {
            visited += 1;
            if visited > MAX_NODES {
                break;
            }
            if el.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false) {
                continue;
            }
            let control = el.CurrentControlType().unwrap_or_default();
            if control == UIA_DataItemControlTypeId || control == UIA_ListItemControlTypeId || control == UIA_TreeItemControlTypeId {
                if out.rows.len() < MAX_ROWS {
                    let cells = row_cells(&walker, &el);
                    if !cells.is_empty() {
                        out.rows.push(cells);
                    }
                }
                // Árvore (painel Problemas do VS Code) tem filhos que também são linhas.
                if control != UIA_TreeItemControlTypeId {
                    continue;
                }
            }
            // Áreas de texto: documento/campo e, no terminal, o controle do buffer
            // (painel/controle próprio com TextPattern).
            let texty = control == UIA_DocumentControlTypeId
                || control == UIA_EditControlTypeId
                || control == UIA_PaneControlTypeId
                || control == UIA_CustomControlTypeId
                || control == UIA_TextControlTypeId;
            if texty && out.texts.len() < MAX_TEXT_AREAS {
                if let Some(text) = text_tail(&el) {
                    out.texts.push(text);
                    continue; // não desce: o editor expõe uma linha por nó
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
        Ok(out)
    }
}

/// Final do texto (TextPattern) de um elemento; textos curtos (rótulos) não contam.
unsafe fn text_tail(el: &IUIAutomationElement) -> Option<String> {
    let pattern = el.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId).ok()?;
    let text = pattern.DocumentRange().ok()?.GetText(-1).ok()?.to_string();
    if text.trim().chars().count() < 20 {
        return None;
    }
    let total = text.chars().count();
    Some(if total > TAIL_CHARS { text.chars().skip(total - TAIL_CHARS).collect() } else { text })
}

/// Nome da linha e dos descendentes (células), até dois níveis.
unsafe fn row_cells(walker: &IUIAutomationTreeWalker, row: &IUIAutomationElement) -> Vec<String> {
    let name = |el: &IUIAutomationElement| {
        el.CurrentName().map(|n| n.to_string().split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default()
    };
    let mut cells = Vec::new();
    let mut push = |s: String| {
        if !s.is_empty() && cells.len() < MAX_CELLS && cells.last() != Some(&s) {
            cells.push(s);
        }
    };
    push(name(row));
    let mut level = vec![row.clone()];
    for _ in 0..2 {
        let mut next_level = Vec::new();
        for parent in &level {
            let mut next = walker.GetFirstChildElement(parent);
            while let Ok(child) = next {
                next = walker.GetNextSiblingElement(&child);
                push(name(&child));
                next_level.push(child);
            }
        }
        level = next_level;
    }
    cells
}
