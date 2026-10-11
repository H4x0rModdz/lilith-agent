use thiserror::Error;

#[derive(Debug, Error)]
pub enum LilithError {
    #[error("Ollama inacessível: {0}")]
    OllamaUnreachable(String),
    #[error("resposta inválida do Ollama: {0}")]
    OllamaInvalid(String),
}
