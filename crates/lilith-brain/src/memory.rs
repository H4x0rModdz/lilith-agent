use crate::tools::ToolCall;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Anel de turnos recentes da sessão (em memória). A persistência em SQLite
/// e os resumos do jarvis-os (`turn_store.rs`, `summarizer.rs`) entram depois.
pub struct SessionMemory {
    turns: Mutex<Vec<Turn>>,
    capacity: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    pub user_text: String,
    pub tool_call: Option<ToolCall>,
    pub reply_text: String,
    /// Todas as ferramentas usadas no pedido, em ordem — para pedidos de
    /// continuação ("e no chat da fulana?") repetirem o caminho.
    #[serde(default)]
    pub actions: Vec<ToolCall>,
}

impl Turn {
    /// "ui.click {…} → window.read {…}" (vazio se não usou nenhuma).
    pub fn actions_summary(&self) -> String {
        self.actions
            .iter()
            .map(|c| format!("{} {}", c.action, c.params))
            .collect::<Vec<_>>()
            .join(" → ")
    }
}

impl SessionMemory {
    pub fn new(capacity: usize) -> Self {
        Self {
            turns: Mutex::new(Vec::with_capacity(capacity)),
            capacity,
        }
    }

    pub fn record(&self, turn: Turn) {
        let mut t = self.turns.lock().unwrap();
        if t.len() == self.capacity {
            t.remove(0);
        }
        t.push(turn);
    }

    pub fn reset(&self) {
        self.turns.lock().unwrap().clear();
    }

    pub fn recent(&self, n: usize) -> Vec<Turn> {
        let t = self.turns.lock().unwrap();
        let skip = t.len().saturating_sub(n);
        t.iter().skip(skip).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(text: &str) -> Turn {
        Turn {
            user_text: text.into(),
            tool_call: None,
            reply_text: "ok".into(),
            actions: vec![],
        }
    }

    #[test]
    fn ring_drops_oldest() {
        let m = SessionMemory::new(2);
        m.record(turn("a"));
        m.record(turn("b"));
        m.record(turn("c"));
        let r = m.recent(10);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].user_text, "b");
        assert_eq!(r[1].user_text, "c");
    }
}
