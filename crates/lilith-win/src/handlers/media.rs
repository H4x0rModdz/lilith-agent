//! Mídia: tocar/pausar, próxima, anterior, parar e "o que está tocando" pela
//! API de mídia do Windows (a mesma do painel de volume — funciona com
//! Spotify, navegador, players), e volume por aplicativo (Core Audio).

use super::blocking;
use crate::{paths, WinError};
use serde_json::{json, Value};

fn winrt(e: windows::core::Error) -> WinError {
    WinError::Failed(format!("mídia: {e}"))
}

/// Sessão de mídia atual (a que o Windows mostra no painel de volume).
fn current_session() -> Result<Option<windows::Media::Control::GlobalSystemMediaTransportControlsSession>, WinError> {
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager as Manager;
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
    }
    let manager = Manager::RequestAsync().map_err(winrt)?.join().map_err(winrt)?;
    Ok(manager.GetCurrentSession().ok())
}

fn app_name(aumid: &str) -> String {
    // "Spotify.exe", "Chrome", "308046B0AF4A39CB" (Firefox), "SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify".
    let s = aumid.rsplit('!').next().unwrap_or(aumid);
    // Firefox/Waterfox se identificam por um código hexadecimal.
    if s.len() == 16 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        return "navegador".into();
    }
    s.trim_end_matches(".exe").to_string()
}

fn now_playing_sync() -> Result<Value, WinError> {
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status;
    let Some(session) = current_session()? else {
        return Ok(json!({ "playing": false, "note": "nada tocando agora" }));
    };
    let props = session.TryGetMediaPropertiesAsync().map_err(winrt)?.join().map_err(winrt)?;
    let status = session.GetPlaybackInfo().and_then(|i| i.PlaybackStatus()).ok();
    let text = |r: windows::core::Result<windows::core::HSTRING>| r.map(|h| h.to_string()).unwrap_or_default();
    Ok(json!({
        "app": app_name(&text(session.SourceAppUserModelId())),
        "title": text(props.Title()),
        "artist": text(props.Artist()),
        "album": text(props.AlbumTitle()),
        "playing": status == Some(Status::Playing),
        "status": match status { Some(Status::Playing) => "tocando", Some(Status::Paused) => "pausado", Some(Status::Stopped) => "parado", _ => "desconhecido" },
    }))
}

pub async fn now_playing(_params: Value) -> Result<Value, WinError> {
    blocking(now_playing_sync).await
}

/// `action`: play_pause | play | pause | next | previous | stop.
pub async fn control(params: Value) -> Result<Value, WinError> {
    let action = params["action"].as_str().unwrap_or("play_pause").to_string();
    if !matches!(action.as_str(), "play_pause" | "play" | "pause" | "next" | "previous" | "stop") {
        return Err(WinError::InvalidParams("action: play_pause, play, pause, next, previous ou stop".into()));
    }
    blocking(move || {
        let done = match current_session()? {
            Some(s) => {
                let op = match action.as_str() {
                    "play" => s.TryPlayAsync(),
                    "pause" => s.TryPauseAsync(),
                    "next" => s.TrySkipNextAsync(),
                    "previous" => s.TrySkipPreviousAsync(),
                    "stop" => s.TryStopAsync(),
                    _ => s.TryTogglePlayPauseAsync(),
                };
                op.map_err(winrt)?.join().map_err(winrt)?
            }
            None => false,
        };
        // Sem sessão (ou o app recusou): a tecla de mídia do teclado.
        if !done {
            crate::input::press_media(&action)?;
        }
        std::thread::sleep(std::time::Duration::from_millis(350));
        let mut out = now_playing_sync().unwrap_or_else(|_| json!({}));
        out["done"] = json!(action);
        out["via"] = json!(if done { "sessão de mídia" } else { "tecla de mídia" });
        Ok(out)
    })
    .await
}

/// Nome do executável de um processo (sem ".exe").
fn process_name(pid: u32) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_FORMAT(0), PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().to_string())
    }
}

/// Volume (0–100) e/ou mudo de um aplicativo só: todas as sessões de áudio dele.
pub async fn app_volume(params: Value) -> Result<Value, WinError> {
    let app = params["app"]
        .as_str()
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .ok_or_else(|| WinError::InvalidParams("parâmetro 'app' obrigatório".into()))?
        .to_string();
    let level = params["level"].as_f64().map(|l| (l.clamp(0.0, 100.0) / 100.0) as f32);
    let muted = params["muted"].as_bool();
    if level.is_none() && muted.is_none() {
        return Err(WinError::InvalidParams("informe 'level' (0–100) ou 'muted'".into()));
    }
    blocking(move || {
        use windows::core::Interface;
        use windows::Win32::Media::Audio::{
            eConsole, eRender, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume,
            MMDeviceEnumerator,
        };
        use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};
        let want = paths::fold(&app);
        let mut changed: Vec<String> = Vec::new();
        let mut available: Vec<String> = Vec::new();
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
            let sessions = manager.GetSessionEnumerator()?;
            for i in 0..sessions.GetCount()? {
                let Ok(control) = sessions.GetSession(i) else { continue };
                let Ok(control2) = control.cast::<IAudioSessionControl2>() else { continue };
                let pid = control2.GetProcessId().unwrap_or(0);
                if pid == 0 {
                    continue; // sons do sistema
                }
                let Some(name) = process_name(pid) else { continue };
                if !available.contains(&name) {
                    available.push(name.clone());
                }
                if !paths::fold(&name).contains(&want) {
                    continue;
                }
                let volume: ISimpleAudioVolume = control.cast()?;
                if let Some(l) = level {
                    volume.SetMasterVolume(l, std::ptr::null())?;
                }
                if let Some(m) = muted {
                    volume.SetMute(m, std::ptr::null())?;
                }
                if !changed.contains(&name) {
                    changed.push(name);
                }
            }
        }
        if changed.is_empty() {
            return Err(WinError::NotFound(format!(
                "'{app}' não está tocando som agora (apps com som: {})",
                if available.is_empty() { "nenhum".into() } else { available.join(", ") }
            )));
        }
        Ok(json!({
            "apps": changed,
            "level": level.map(|l| (l * 100.0).round() as u32),
            "muted": muted,
        }))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_names_from_media_sessions() {
        assert_eq!(app_name("Spotify.exe"), "Spotify");
        assert_eq!(app_name("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"), "Spotify");
        assert_eq!(app_name("Chrome"), "Chrome");
        assert_eq!(app_name("6F940AC27A98DD61"), "navegador");
    }
}
