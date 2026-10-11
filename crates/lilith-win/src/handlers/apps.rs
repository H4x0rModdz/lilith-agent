use super::{blocking, hidden_command};
use crate::paths::{fold, resolve};
use crate::{req_str, WinError};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Mutex;
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Abre um app pelo nome. Ordem: apelidos pt-BR → entradas do Menu Iniciar
/// (`Get-StartApps`, cobre apps Win32 e da Store) → ShellExecute direto
/// (executáveis no PATH / App Paths, ex.: "chrome", "code").
pub async fn open(params: Value) -> Result<Value, WinError> {
    let app = req_str(&params, "app")?.to_string();

    if let Some(target) = alias(&app) {
        shell_open(target.to_string()).await?;
        return Ok(json!({ "opened": app, "via": "alias" }));
    }

    if let Some(entry) = find_start_app(&app).await {
        shell_open(format!("shell:AppsFolder\\{}", entry.app_id)).await?;
        return Ok(json!({ "opened": entry.name, "via": "start_menu" }));
    }

    shell_open(app.clone())
        .await
        .map_err(|_| WinError::NotFound(format!("app '{app}' (não está no Menu Iniciar nem no PATH)")))?;
    Ok(json!({ "opened": app, "via": "shell" }))
}

/// Fecha processos cujo nome de imagem contém `app`.
pub async fn close(params: Value) -> Result<Value, WinError> {
    let app = fold(req_str(&params, "app")?);
    let wanted = app.trim_end_matches(".exe").to_string();
    let force = params["force"].as_bool().unwrap_or(false);

    let out = hidden_command("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .output()
        .await?;
    let listing = String::from_utf8_lossy(&out.stdout);
    let mut images: Vec<String> = listing
        .lines()
        .filter_map(|l| l.split("\",\"").next())
        .map(|s| s.trim_start_matches('"').to_string())
        .filter(|img| fold(img).trim_end_matches(".exe").contains(&wanted))
        .collect();
    images.sort();
    images.dedup();
    if images.is_empty() {
        return Err(WinError::NotFound(format!("processo '{wanted}' em execução")));
    }

    for img in &images {
        let mut cmd = hidden_command("taskkill");
        cmd.args(["/IM", img, "/T"]);
        if force {
            cmd.arg("/F");
        }
        let status = cmd.status().await?;
        if !status.success() && !force {
            // Sem /F o taskkill só pede pra janela fechar; processo sem janela
            // ignora. Reporta em vez de escalar sozinho para kill forçado.
            return Err(WinError::Failed(format!(
                "'{img}' não fechou educadamente — peça para forçar"
            )));
        }
    }
    Ok(json!({ "closed": images, "forced": force }))
}

pub async fn browser_open(params: Value) -> Result<Value, WinError> {
    let url = req_str(&params, "url")?;
    let url = if url.contains("://") {
        url.to_string()
    } else {
        format!("https://{url}")
    };
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(WinError::InvalidParams("só URLs http(s)".into()));
    }
    shell_open(url.clone()).await?;
    Ok(json!({ "opened": url }))
}

pub async fn file_open(params: Value) -> Result<Value, WinError> {
    let path = resolve(req_str(&params, "path")?);
    if !path.exists() {
        return Err(WinError::NotFound(path.display().to_string()));
    }
    shell_open(path.to_string_lossy().into_owned()).await?;
    Ok(json!({ "opened": path }))
}

/// `ShellExecuteW(open)` — o mesmo que dar duplo clique / Win+R.
pub(crate) async fn shell_open(target: String) -> Result<(), WinError> {
    blocking(move || unsafe {
        // ShellExecute pode delegar para extensões COM do Shell.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let h = ShellExecuteW(
            None,
            &HSTRING::from("open"),
            &HSTRING::from(target.as_str()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        // Valores <= 32 são códigos de erro (doc do ShellExecute).
        if h.0 as usize > 32 {
            Ok(())
        } else {
            Err(WinError::Failed(format!("ShellExecute '{target}' → código {}", h.0 as usize)))
        }
    })
    .await
}

fn alias(name: &str) -> Option<&'static str> {
    Some(match fold(name).as_str() {
        "bloco de notas" | "notepad" => "notepad.exe",
        "calculadora" | "calculator" => "calc.exe",
        "paint" => "mspaint.exe",
        "explorador de arquivos" | "explorer" | "arquivos" => "explorer.exe",
        "configuracoes" | "configuracao" | "settings" => "ms-settings:",
        "gerenciador de tarefas" | "task manager" => "taskmgr.exe",
        "terminal" => "wt.exe",
        "prompt de comando" | "cmd" => "cmd.exe",
        "painel de controle" | "control panel" => "control.exe",
        _ => return None,
    })
}

#[derive(Debug, Clone, Deserialize)]
struct StartApp {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "AppID")]
    app_id: String,
}

static START_APPS: Mutex<Option<Vec<StartApp>>> = Mutex::new(None);

/// Busca no cache de `Get-StartApps`; se não achar, recarrega uma vez
/// (o app pode ter sido instalado depois do cache).
async fn find_start_app(query: &str) -> Option<StartApp> {
    let cached = START_APPS.lock().unwrap().clone();
    if let Some(apps) = cached {
        if let Some(hit) = best_match(&apps, query) {
            return Some(hit);
        }
    }
    let fresh = load_start_apps().await.ok()?;
    let hit = best_match(&fresh, query);
    *START_APPS.lock().unwrap() = Some(fresh);
    hit
}

async fn load_start_apps() -> Result<Vec<StartApp>, WinError> {
    let out = hidden_command("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::OutputEncoding=[Text.Encoding]::UTF8; Get-StartApps | ConvertTo-Json -Compress",
        ])
        .output()
        .await?;
    let text = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(text.trim()).map_err(|e| WinError::Failed(format!("Get-StartApps: {e}")))
}

/// Exato > começa com > contém; empate vai para o nome mais curto
/// ("spotify" casa "Spotify" antes de "Spotify Uninstaller").
fn best_match(apps: &[StartApp], query: &str) -> Option<StartApp> {
    let q = fold(query);
    apps.iter()
        .filter_map(|a| {
            let n = fold(&a.name);
            let rank = if n == q {
                0
            } else if n.starts_with(&q) {
                1
            } else if n.contains(&q) {
                2
            } else {
                return None;
            };
            Some((rank, n.len(), a))
        })
        .min_by_key(|(rank, len, _)| (*rank, *len))
        .map(|(_, _, a)| a.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str) -> StartApp {
        StartApp { name: name.into(), app_id: name.into() }
    }

    #[test]
    fn best_match_prefers_exact_then_shortest() {
        let apps = vec![app("Spotify Uninstaller"), app("Spotify"), app("Visual Studio Code")];
        assert_eq!(best_match(&apps, "spotify").unwrap().name, "Spotify");
        assert_eq!(best_match(&apps, "code").unwrap().name, "Visual Studio Code");
        assert!(best_match(&apps, "discord").is_none());
    }

    #[test]
    fn aliases_ignore_accents() {
        assert_eq!(alias("Configurações"), Some("ms-settings:"));
        assert_eq!(alias("Bloco de Notas"), Some("notepad.exe"));
    }
}
