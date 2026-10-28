//! Privilégio de administrador sob demanda.
//!
//! A Lilith roda como usuário comum. Janelas de apps elevados (Gerenciador
//! de Tarefas, instaladores, apps "como administrador") não aceitam ser
//! movidas por um processo comum (UIPI). Para elas, o próprio executável é
//! relançado com `runas` — o **UAC do Windows é a aprovação** — e executa só
//! uma lista curta de operações de janela, depois sai.
//!
//! Superfície mínima de propósito: o ajudante não aceita comando livre, só
//! as [`Op`]s abaixo, sobre ids que ainda sejam janelas. Os parâmetros vão
//! na linha de comando (não num arquivo temporário que outro processo comum
//! pudesse trocar entre a escrita e a leitura do ajudante).

use base64::Engine;
use lilith_win::window::{self, Rect};
use lilith_win::WinError;
use serde::{Deserialize, Serialize};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

pub const FLAG: &str = "--lilith-elevated-window-ops";
// Reiniciar um serviço espera ele parar e voltar (até 30 s cada).
const TIMEOUT_MS: u32 = 90_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Frame { id: isize, rect: Rect, maximize_after: bool },
    State { id: isize, state: String },
    Close { id: isize },
    Focus { id: isize },
    /// Parar/iniciar/reiniciar um serviço do Windows (nome validado de novo
    /// dentro do ajudante).
    Service { name: String, action: lilith_win::services::ServiceOp },
}

impl Op {
    fn id(&self) -> Option<isize> {
        match self {
            Op::Frame { id, .. } | Op::State { id, .. } | Op::Close { id } | Op::Focus { id } => Some(*id),
            Op::Service { .. } => None,
        }
    }

    fn apply(&self) -> Result<(), WinError> {
        if let Op::Service { name, action } = self {
            return lilith_win::services::control_blocking(name, *action);
        }
        if let Some(id) = self.id().filter(|id| !window::exists(*id)) {
            return Err(WinError::NotFound(format!("janela {id}")));
        }
        match self {
            Op::Frame { id, rect, maximize_after } => {
                window::set_frame(*id, *rect)?;
                if *maximize_after {
                    window::set_state(*id, "maximize")?;
                }
                Ok(())
            }
            Op::State { id, state } => window::set_state(*id, state),
            Op::Close { id } => window::close(*id),
            Op::Focus { id } => window::focus(*id),
            Op::Service { .. } => unreachable!("tratado no começo"),
        }
    }
}

/// Executa as operações: direto se já estivermos elevados, senão pelo
/// ajudante (um prompt do UAC para a lista inteira).
pub async fn run(ops: Vec<Op>) -> Result<(), WinError> {
    if ops.is_empty() {
        return Ok(());
    }
    tokio::task::spawn_blocking(move || {
        if window::self_elevated() {
            return ops.iter().try_for_each(Op::apply);
        }
        launch_helper(&ops)
    })
    .await
    .map_err(|e| WinError::Failed(e.to_string()))?
}

fn launch_helper(ops: &[Op]) -> Result<(), WinError> {
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(ops).map_err(|e| WinError::Failed(e.to_string()))?);
    let exe = std::env::current_exe()?;
    let file = HSTRING::from(exe.as_os_str());
    let params = HSTRING::from(format!("{FLAG} {payload}"));
    let verb = HSTRING::from("runas");

    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        if let Err(e) = ShellExecuteExW(&mut info) {
            if e.code() == ERROR_CANCELLED.to_hresult() {
                return Err(WinError::Failed("o pedido de administrador (UAC) foi recusado".into()));
            }
            return Err(WinError::Failed(format!("não consegui pedir privilégio de administrador: {}", e.message())));
        }
        let process = info.hProcess;
        let waited = WaitForSingleObject(process, TIMEOUT_MS);
        let mut code = 1u32;
        let _ = GetExitCodeProcess(process, &mut code);
        let _ = CloseHandle(process);
        if waited != WAIT_OBJECT_0 {
            return Err(WinError::Failed("o ajudante de administrador não respondeu".into()));
        }
        match code {
            0 => Ok(()),
            2 => Err(WinError::InvalidParams("o ajudante recusou as operações".into())),
            _ => Err(WinError::Failed("o ajudante de administrador falhou".into())),
        }
    }
}

/// Ponto de entrada do processo ajudante (já elevado). Códigos de saída:
/// 0 ok, 2 parâmetros inválidos, 3 falha numa operação.
pub fn helper_main(args: &[String]) -> i32 {
    let Some(ops) = args.first().and_then(|p| decode(p)) else {
        return 2;
    };
    match ops.iter().try_for_each(Op::apply) {
        Ok(()) => 0,
        Err(_) => 3,
    }
}

fn decode(payload: &str) -> Option<Vec<Op>> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload).ok()?;
    let ops: Vec<Op> = serde_json::from_slice(&bytes).ok()?;
    // Uma ação da Lilith nunca precisa de mais que isso (tile de 9 janelas).
    (!ops.is_empty() && ops.len() <= 9).then_some(ops)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_round_trip_and_limits() {
        let ops = vec![
            Op::Frame { id: 42, rect: Rect { x: 0, y: 0, w: 960, h: 1040 }, maximize_after: false },
            Op::State { id: 42, state: "minimize".into() },
        ];
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&ops).unwrap());
        assert_eq!(decode(&payload), Some(ops));
        assert_eq!(decode("not base64 !!"), None);
        let too_many: Vec<Op> = (0..10).map(|id| Op::Close { id }).collect();
        let big = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&too_many).unwrap());
        assert_eq!(decode(&big), None);
        // Operação desconhecida não desserializa.
        let evil = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"[{"op":"run","cmd":"calc"}]"#);
        assert_eq!(decode(&evil), None);
    }
}
