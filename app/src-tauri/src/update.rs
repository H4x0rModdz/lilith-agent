//! Atualização automática (GitHub Releases) e início com o Windows.
//!
//! O instalador assinado e o `latest.json` ficam no release mais recente; o
//! app confere a assinatura com a chave pública do `tauri.conf.json`.

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_updater::UpdaterExt;

#[derive(Serialize, Clone)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: String,
}

/// Procura uma versão nova. `None` = já está na última.
pub async fn check(app: &AppHandle) -> Result<Option<UpdateInfo>, String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let found = updater.check().await.map_err(|e| e.to_string())?;
    Ok(found.map(|u| UpdateInfo { version: u.version.clone(), notes: u.body.clone().unwrap_or_default() }))
}

/// Baixa e instala. No Windows o instalador (modo passivo) fecha o app e o
/// abre de novo já atualizado.
pub async fn install(app: &AppHandle) -> Result<(), String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
        return Ok(());
    };
    update.download_and_install(|_, _| {}, || {}).await.map_err(|e| e.to_string())?;
    app.restart();
}

/// Na abertura: procura atualização e, se o usuário deixou, instala na hora.
pub async fn on_startup(app: AppHandle, auto_install: bool) {
    // Deixa o app terminar de subir antes de usar a rede.
    tokio::time::sleep(std::time::Duration::from_secs(12)).await;
    match check(&app).await {
        Ok(Some(info)) => {
            tracing::info!(version = %info.version, "atualização disponível");
            let _ = app.emit("lilith://update", &info);
            if auto_install {
                if let Err(e) = install(&app).await {
                    tracing::warn!("não consegui atualizar: {e}");
                }
            }
        }
        Ok(None) => tracing::info!("já está na última versão"),
        Err(e) => tracing::debug!("sem checar atualização: {e}"),
    }
}

/// Primeira execução da versão instalada: liga o início com o Windows (o
/// usuário desliga em Configurações). Em desenvolvimento não mexe.
pub fn init_autostart(app: &AppHandle, first_run: bool) {
    if cfg!(debug_assertions) || !first_run {
        return;
    }
    if let Err(e) = app.autolaunch().enable() {
        tracing::warn!("não consegui ligar o início com o Windows: {e}");
    }
}

pub fn autostart_enabled(app: &AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

pub fn set_autostart(app: &AppHandle, on: bool) -> bool {
    let manager = app.autolaunch();
    let _ = if on { manager.enable() } else { manager.disable() };
    manager.is_enabled().unwrap_or(false)
}
