//! Guardrails da Lilith.
//!
//! Toda ação passa por aqui antes de tocar no sistema: a ação vira um
//! **scope**, o scope é classificado (seguro / perigoso / desconhecido) e a
//! política do **canal** de origem (local, discord, …) decide se executa,
//! pergunta ao usuário ou nega. Portado de `jarvis-os/system/permission` e
//! `action-bus/src/permission.rs`, trocando o DBus por chamadas diretas e
//! adicionando regras por canal configuráveis pelo usuário.

mod audit;
mod policy;
mod scope;

pub use audit::AuditLog;
pub use policy::{Decision, Policy, Rule};
pub use scope::{classify, scope_for, Verdict};

/// Canal do próprio PC (orb/chat). Outros canais (ex.: `discord`) usam o
/// mesmo padrão; o usuário ajusta a política de cada um.
pub const CHANNEL_LOCAL: &str = "local";
