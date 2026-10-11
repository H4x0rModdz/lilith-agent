use lilith_agents::AgentSpec;
use lilith_brain::OllamaConfig;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// `%APPDATA%\Lilith\` — config, permissões e logs.
pub fn data_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Lilith")
}

/// Avatar da Lilith escolhido pelo usuário: `%APPDATA%\Lilith\avatar.{png,jpg,jpeg,webp}`
/// como data URL. Fica fora do repositório (a imagem é do usuário); sem
/// arquivo, a UI usa o sigilo padrão.
pub fn avatar_data_url() -> Option<String> {
    use base64::Engine;
    const MAX_BYTES: u64 = 2 * 1024 * 1024;
    ["png", "jpg", "jpeg", "webp"].iter().find_map(|ext| {
        let path = data_dir().join(format!("avatar.{ext}"));
        let meta = std::fs::metadata(&path).ok()?;
        if meta.len() > MAX_BYTES {
            tracing::warn!(path = %path.display(), "avatar maior que 2 MB — ignorado");
            return None;
        }
        let bytes = std::fs::read(&path).ok()?;
        let mime = if *ext == "jpg" { "jpeg" } else { ext };
        Some(format!(
            "data:image/{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ))
    })
}

/// `%APPDATA%\Lilith\config.json`. Ausente/inválido → padrões.
#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub ollama: OllamaConfig,
    /// Agentes de código para `agent.delegate`. Adicione outros CLIs aqui.
    pub agents: BTreeMap<String, AgentSpec>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ollama: OllamaConfig::default(),
            agents: lilith_agents::default_agents(),
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let path = data_dir().join("config.json");
        match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "config.json inválido — usando padrões");
                Self::default()
            }),
            Err(_) => {
                let cfg = Self::default();
                // Grava o padrão para o usuário ter onde editar.
                let _ = std::fs::create_dir_all(data_dir());
                let _ = std::fs::write(&path, serde_json::to_string_pretty(&cfg).unwrap_or_default());
                cfg
            }
        }
    }
}
