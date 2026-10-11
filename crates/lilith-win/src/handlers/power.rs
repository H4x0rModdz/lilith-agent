use super::{blocking, hidden_command};
use crate::{req_str, WinError};
use serde_json::{json, Value};
use windows::Win32::System::Power::SetSuspendState;
use windows::Win32::System::Shutdown::LockWorkStation;

pub async fn run(params: Value) -> Result<Value, WinError> {
    let op = req_str(&params, "op")?.to_string();
    match op.as_str() {
        "lock" => blocking(|| unsafe { LockWorkStation().map_err(WinError::from) }).await?,
        "sleep" => {
            blocking(|| unsafe {
                if SetSuspendState(false, false, false) {
                    Ok(())
                } else {
                    Err(WinError::Failed("SetSuspendState recusou".into()))
                }
            })
            .await?
        }
        // Pequeno atraso para a Lilith conseguir responder antes de cair.
        "restart" => shutdown(&["/r", "/t", "5"]).await?,
        "shutdown" => shutdown(&["/s", "/t", "5"]).await?,
        other => return Err(WinError::InvalidParams(format!("op '{other}' desconhecida"))),
    }
    Ok(json!({ "op": op }))
}

async fn shutdown(args: &[&str]) -> Result<(), WinError> {
    let status = hidden_command("shutdown").args(args).status().await?;
    if status.success() {
        Ok(())
    } else {
        Err(WinError::Failed(format!("shutdown saiu com {status}")))
    }
}
