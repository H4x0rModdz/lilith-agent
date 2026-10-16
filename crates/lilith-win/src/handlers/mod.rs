pub mod apps;
pub mod capture;
pub mod checkup;
pub mod audio;
pub mod clipboard;
pub mod desktop;
pub mod download;
pub mod files;
pub mod media;
pub mod power;
pub mod screen;
pub mod system;
pub mod web;

use crate::WinError;

/// Processo filho sem janela de console piscando (o app roda sem console).
pub(crate) fn hidden_command(program: &str) -> tokio::process::Command {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = tokio::process::Command::new(program);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// Roda código bloqueante (COM, Win32, varredura de disco) fora do runtime async.
pub(crate) async fn blocking<T, F>(f: F) -> Result<T, WinError>
where
    F: FnOnce() -> Result<T, WinError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| WinError::Failed(format!("tarefa interrompida: {e}")))?
}
