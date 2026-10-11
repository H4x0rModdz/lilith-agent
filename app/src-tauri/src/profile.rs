//! Perfil e comportamento da Lilith, configurados no painel e guardados no
//! SQLite, mais o que ela aprendeu — viram um contexto de sistema em cada
//! pedido.

use lilith_memory::Memory;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Como ela chama o usuário (ex.: "Lobo").
    pub user_name: String,
    /// Jeito/personalidade extra, em texto livre.
    pub persona: String,
    /// Aprender fatos sozinha com o que o usuário digita.
    pub auto_learn: bool,
    /// Minutos de mensagens lidas num chat quando o usuário não diz quais.
    pub chat_minutes: u32,
}

const USER_NAME: &str = "user_name";
const PERSONA: &str = "persona";
const AUTO_LEARN: &str = "auto_learn";
const CHAT_MINUTES: &str = "chat_minutes";

impl Settings {
    pub fn load(m: &Memory) -> Self {
        Self {
            user_name: m.setting(USER_NAME).unwrap_or_default(),
            persona: m.setting(PERSONA).unwrap_or_default(),
            auto_learn: m.setting(AUTO_LEARN).map(|v| v != "false").unwrap_or(true),
            chat_minutes: m.setting(CHAT_MINUTES).and_then(|v| v.parse().ok()).unwrap_or(5),
        }
    }

    pub fn save(&self, m: &Memory) {
        let clip = |s: &str, n: usize| s.trim().chars().take(n).collect::<String>();
        m.set_setting(USER_NAME, &clip(&self.user_name, 40));
        m.set_setting(PERSONA, &clip(&self.persona, 800));
        m.set_setting(AUTO_LEARN, if self.auto_learn { "true" } else { "false" });
        m.set_setting(CHAT_MINUTES, &self.chat_minutes.clamp(1, 1440).to_string());
    }
}

pub fn auto_learn(m: &Memory) -> bool {
    Settings::load(m).auto_learn
}

pub fn chat_minutes(m: &Memory) -> u32 {
    Settings::load(m).chat_minutes
}

/// Contexto de sistema do pedido: como chamar o usuário, o jeito pedido e
/// os fatos lembrados (configurados primeiro).
pub fn context(m: &Memory) -> String {
    let s = Settings::load(m);
    let mut out = Vec::new();
    if !s.user_name.trim().is_empty() {
        out.push(format!("Chame o usuário de \"{}\".", s.user_name.trim()));
    }
    if !s.persona.trim().is_empty() {
        out.push(format!("Jeito que o usuário pediu para você ter: {}", s.persona.trim()));
    }
    let facts = m.facts();
    if !facts.is_empty() {
        let list = facts
            .iter()
            .take(40)
            .map(|f| format!("- {}: {}", f.key, f.value))
            .collect::<Vec<_>>()
            .join("\n");
        out.push(format!(
            "O que você sabe sobre o usuário (use com naturalidade, sem listar):\n{list}"
        ));
    }
    out.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_combines_settings_and_facts() {
        let m = Memory::in_memory().unwrap();
        assert_eq!(context(&m), "");
        Settings { user_name: "Lobo".into(), persona: "fala com gírias".into(), auto_learn: true, chat_minutes: 5 }.save(&m);
        m.remember("time", "Corinthians", "learned");
        let ctx = context(&m);
        assert!(ctx.contains("Chame o usuário de \"Lobo\""));
        assert!(ctx.contains("gírias"));
        assert!(ctx.contains("time: Corinthians"));
    }

    #[test]
    fn settings_defaults_and_clamping() {
        let m = Memory::in_memory().unwrap();
        let s = Settings::load(&m);
        assert!(s.auto_learn);
        assert_eq!(s.chat_minutes, 5);
        Settings { user_name: "x".repeat(100), persona: String::new(), auto_learn: false, chat_minutes: 99999 }.save(&m);
        let s = Settings::load(&m);
        assert_eq!(s.user_name.chars().count(), 40);
        assert!(!s.auto_learn);
        assert_eq!(s.chat_minutes, 1440);
    }
}
