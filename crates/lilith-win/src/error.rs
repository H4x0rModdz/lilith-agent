use thiserror::Error;

#[derive(Debug, Error)]
pub enum WinError {
    #[error("parâmetros inválidos: {0}")]
    InvalidParams(String),
    #[error("não encontrado: {0}")]
    NotFound(String),
    #[error("indisponível: {0}")]
    Unavailable(String),
    #[error("falhou: {0}")]
    Failed(String),
}

impl WinError {
    pub fn code(&self) -> &'static str {
        match self {
            WinError::InvalidParams(_) => "INVALID_PARAMS",
            WinError::NotFound(_) => "NOT_FOUND",
            WinError::Unavailable(_) => "UNAVAILABLE",
            WinError::Failed(_) => "EXECUTION_FAILED",
        }
    }
}

impl From<std::io::Error> for WinError {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::NotFound => WinError::NotFound(e.to_string()),
            _ => WinError::Failed(e.to_string()),
        }
    }
}

impl From<windows::core::Error> for WinError {
    fn from(e: windows::core::Error) -> Self {
        WinError::Failed(e.message().to_string())
    }
}
