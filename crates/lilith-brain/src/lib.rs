//! Cérebro da Lilith, portado de `jarvis-os/ai/lilith`.
//!
//! Mudanças em relação ao original: sem DBus (o chamador injeta um
//! [`Dispatcher`] e um [`Sink`]), catálogo de tools do Windows e prompt de
//! companion. O LLM é sempre local (Ollama).

pub mod agent;
pub mod brain;
pub mod claude_code;
pub mod dev;
pub mod embed;
pub mod emotion;
pub mod error;
pub mod interpret;
pub mod knowledge;
pub mod learn;
pub mod memory;
pub mod native;
pub mod ollama;
pub mod refine;
pub mod routines;
pub mod refine_files;
pub mod shortcut;
pub mod shortcut_files;
pub mod tools;

pub use agent::{Agent, Dispatcher, Reply, Sink};
pub use knowledge::{Example, Knowledge};
pub use brain::Brain;
pub use claude_code::ClaudeCodeConfig;
pub use ollama::{Ollama, OllamaClient, OllamaConfig};
pub use tools::{all_tools, Tool, ToolCall};
