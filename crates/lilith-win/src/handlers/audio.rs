use super::blocking;
use crate::WinError;
use serde_json::{json, Value};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};

/// Volume do dispositivo de saída padrão via Core Audio (sem processos externos).
fn endpoint() -> Result<IAudioEndpointVolume, WinError> {
    unsafe {
        // S_FALSE (já inicializado nesta thread) também é ok.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        Ok(device.Activate(CLSCTX_ALL, None)?)
    }
}

fn read_state(vol: &IAudioEndpointVolume) -> Result<Value, WinError> {
    unsafe {
        let level = vol.GetMasterVolumeLevelScalar()?;
        let muted = vol.GetMute()?.as_bool();
        Ok(json!({ "level": (level * 100.0).round() as u32, "muted": muted }))
    }
}

pub async fn set_volume(params: Value) -> Result<Value, WinError> {
    let level = params["level"]
        .as_f64()
        .ok_or_else(|| WinError::InvalidParams("parâmetro 'level' (0–100) obrigatório".into()))?
        .clamp(0.0, 100.0) as f32;
    blocking(move || {
        let vol = endpoint()?;
        unsafe { vol.SetMasterVolumeLevelScalar(level / 100.0, std::ptr::null())? };
        read_state(&vol)
    })
    .await
}

pub async fn get_volume() -> Result<Value, WinError> {
    blocking(|| read_state(&endpoint()?)).await
}

pub async fn set_mute(params: Value) -> Result<Value, WinError> {
    let muted = params["muted"]
        .as_bool()
        .ok_or_else(|| WinError::InvalidParams("parâmetro 'muted' obrigatório".into()))?;
    blocking(move || {
        let vol = endpoint()?;
        unsafe { vol.SetMute(muted, std::ptr::null())? };
        read_state(&vol)
    })
    .await
}
