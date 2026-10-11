//! Liga o que a Lilith aprende (SQLite) ao cérebro: exemplos de planos que
//! deram certo e o glossário pessoal de gírias.

use lilith_brain::{Example, Knowledge};
use lilith_memory::Memory;
use std::sync::Arc;

/// Quantos planos aprendidos entram na disputa por exemplos parecidos.
const LEARNED_WINDOW: usize = 300;

pub struct StoreKnowledge(pub Arc<Memory>);

impl Knowledge for StoreKnowledge {
    fn plans(&self) -> Vec<Example> {
        self.0.good_plans(LEARNED_WINDOW).into_iter().map(|p| Example { text: p.text, plan: p.plan }).collect()
    }

    fn glossary(&self) -> Vec<(String, String)> {
        self.0.glossary().into_iter().map(|t| (t.term, t.meaning)).collect()
    }

    fn record_plan(&self, text: &str, plan: &[String]) {
        self.0.record_plan(text, plan);
    }
}
