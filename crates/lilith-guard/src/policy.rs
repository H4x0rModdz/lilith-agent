use crate::scope::{classify, matches_any, scope_for, Verdict};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// Regra que o usuário define para um scope num canal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    Allow,
    Ask,
    Deny,
}

/// Resultado de `Policy::decide`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow { scope: &'static str },
    Ask { scope: &'static str },
    Deny { scope: &'static str, reason: String },
}

/// Arquivo `permissions.json`:
///
/// ```json
/// { "channels": { "discord": { "filesystem.delete": "deny", "app.launch": "ask" } } }
/// ```
///
/// Canal sem regra para o scope cai no padrão embutido (seguro → allow,
/// perigoso → ask). As regras casam por prefixo pontuado como em `classify`,
/// e a mais específica vence.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct PolicyFile {
    #[serde(default)]
    channels: BTreeMap<String, BTreeMap<String, Rule>>,
}

pub struct Policy {
    path: Option<PathBuf>,
    file: RwLock<PolicyFile>,
}

impl Policy {
    /// Carrega de `path`; arquivo ausente ou inválido vira política padrão
    /// (a Lilith sempre sobe — um JSON quebrado não pode liberar nada a mais).
    pub fn load(path: &Path) -> Self {
        let file = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| match serde_json::from_str(&s) {
                Ok(f) => Some(f),
                Err(e) => {
                    tracing::warn!(error = %e, path = %path.display(), "permissions.json inválido — usando padrão");
                    None
                }
            })
            .unwrap_or_default();
        Self {
            path: Some(path.to_path_buf()),
            file: RwLock::new(file),
        }
    }

    /// Política só com os padrões, sem persistência (testes).
    pub fn in_memory() -> Self {
        Self {
            path: None,
            file: RwLock::new(PolicyFile::default()),
        }
    }

    pub fn decide(&self, channel: &str, action: &str) -> Decision {
        let scope = scope_for(action);
        let verdict = classify(scope);
        if verdict == Verdict::Unknown {
            return Decision::Deny {
                scope,
                reason: format!("ação '{action}' sem scope conhecido"),
            };
        }
        let rule = self.user_rule(channel, scope).unwrap_or(match verdict {
            Verdict::Safe => Rule::Allow,
            _ => Rule::Ask,
        });
        match rule {
            Rule::Allow => Decision::Allow { scope },
            Rule::Ask => Decision::Ask { scope },
            Rule::Deny => Decision::Deny {
                scope,
                reason: format!("'{scope}' bloqueado no canal '{channel}'"),
            },
        }
    }

    /// "Permitir sempre": grava `allow` para o scope no canal e persiste.
    pub fn set_rule(&self, channel: &str, scope: &str, rule: Rule) -> std::io::Result<()> {
        let snapshot = {
            let mut f = self.file.write().expect("policy lock");
            f.channels
                .entry(channel.to_string())
                .or_default()
                .insert(scope.to_string(), rule);
            f.clone()
        };
        if let Some(path) = &self.path {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let json = serde_json::to_string_pretty(&snapshot).expect("policy serializa");
            std::fs::write(path, json)?;
        }
        Ok(())
    }

    fn user_rule(&self, channel: &str, scope: &str) -> Option<Rule> {
        let f = self.file.read().expect("policy lock");
        let rules = f.channels.get(channel)?;
        rules
            .iter()
            .filter(|(key, _)| matches_any(scope, &[key.as_str()]))
            .max_by_key(|(key, _)| key.len())
            .map(|(_, rule)| *rule)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_classification() {
        let p = Policy::in_memory();
        assert_eq!(p.decide("local", "app.open"), Decision::Allow { scope: "app.launch" });
        assert_eq!(
            p.decide("local", "file.delete"),
            Decision::Ask { scope: "filesystem.delete" }
        );
        assert!(matches!(p.decide("local", "rm.rf"), Decision::Deny { .. }));
    }

    #[test]
    fn remote_channel_uses_same_defaults() {
        let p = Policy::in_memory();
        assert_eq!(p.decide("discord", "app.open"), p.decide("local", "app.open"));
    }

    #[test]
    fn user_rules_override_per_channel_and_most_specific_wins() {
        let p = Policy::in_memory();
        p.set_rule("discord", "filesystem", Rule::Deny).unwrap();
        p.set_rule("discord", "filesystem.write.downloads", Rule::Allow).unwrap();
        assert!(matches!(p.decide("discord", "file.delete"), Decision::Deny { .. }));
        assert_eq!(
            p.decide("discord", "download.file"),
            Decision::Allow { scope: "filesystem.write.downloads" }
        );
        // Outro canal não é afetado.
        assert_eq!(
            p.decide("local", "file.delete"),
            Decision::Ask { scope: "filesystem.delete" }
        );
    }

    #[test]
    fn user_rule_cannot_unlock_unknown_action() {
        let p = Policy::in_memory();
        p.set_rule("local", "unknown", Rule::Allow).unwrap();
        assert!(matches!(p.decide("local", "rm.rf"), Decision::Deny { .. }));
    }
}
