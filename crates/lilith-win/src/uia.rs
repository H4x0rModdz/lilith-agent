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
