//! Atualização automática (GitHub Releases) e início com o Windows.
//!
//! O instalador assinado e o `latest.json` ficam no release mais recente; o
//! app confere a assinatura com a chave pública do `tauri.conf.json`.

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_updater::UpdaterExt;

/// Resultado da última busca de atualização (para o autodiagnóstico).
#[derive(Clone, Debug)]
pub struct LastCheck {
    pub available: Option<String>,
    pub error: Option<String>,
}

static LAST: std::sync::Mutex<Option<LastCheck>> = std::sync::Mutex::new(None);

pub fn last_check() -> Option<LastCheck> {
    LAST.lock().ok().and_then(|g| g.clone())
}

fn remember(available: Option<String>, error: Option<String>) {
    if let Ok(mut g) = LAST.lock() {
        *g = Some(LastCheck { available, error });
    }
}

#[derive(Serialize, Clone)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: String,
}

/// Procura uma versão nova. `None` = já está na última.
pub async fn check(app: &AppHandle) -> Result<Option<UpdateInfo>, String> {
    let result: Result<Option<UpdateInfo>, String> = async {
        let updater = app.updater().map_err(|e| e.to_string())?;
        let found = updater.check().await.map_err(|e| e.to_string())?;
        Ok(found.map(|u| UpdateInfo { version: u.version.clone(), notes: u.body.clone().unwrap_or_default() }))
    }
    .await;
    match &result {
        Ok(found) => remember(found.as_ref().map(|i| i.version.clone()), None),
        Err(e) => {
            tracing::warn!("busca de atualização falhou: {e}");
            remember(None, Some(e.clone()));
        }
    }
    result
}

/// Baixa e instala. No Windows o instalador (modo passivo) fecha o app e o
/// abre de novo já atualizado.
pub async fn install(app: &AppHandle) -> Result<(), String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
        return Ok(());
    };
    // Avisa antes: o instalador fecha e reabre a Lilith.
    {
        use tauri_plugin_notification::NotificationExt;
        let _ = app
            .notification()
            .builder()
            .title("Lilith")
            .body(format!("Atualizando para a versão {}… volto em instantes.", update.version))
            .show();
    }
    tracing::info!(version = %update.version, "baixando e instalando atualização");
    update.download_and_install(|_, _| {}, || {}).await.map_err(|e| {
        tracing::warn!("instalação da atualização falhou: {e}");
        e.to_string()
    })?;
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
        // `check` já registrou o erro (log + autodiagnóstico).
        Err(_) => {}
    }
}

/// Depois de uma atualização, conta o que aconteceu (uma vez): compara a versão
/// de agora com a da última execução, guardada no banco.
pub fn announce_if_updated(app: &AppHandle, store: &lilith_memory::Memory) {
    let now = app.package_info().version.to_string();
    let before = store.setting("last_run_version");
    store.set_setting("last_run_version", &now);
    let Some(before) = before.filter(|b| *b != now) else { return };
    tracing::info!(from = %before, to = %now, "atualizada");
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Espera o chat carregar para a nota aparecer.
        tokio::time::sleep(std::time::Duration::from_secs(6)).await;
        let text = format!("Atualizei da versão {before} para a {now}. As novidades estão nas notas do release no GitHub.");
        {
            use tauri_plugin_notification::NotificationExt;
            let _ = app.notification().builder().title("Lilith").body(&text).show();
        }
        let _ = app.emit("lilith://notice", text);
    });
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
