//! Entrada "de verdade" (mouse e teclado do sistema) para apps que ignoram a
//! acessibilidade — WhatsApp, Discord e outros baseados em web não aceitam
//! Invoke/Select num item de lista, só um clique real.
//!
//! É o último recurso: o mouse do usuário é emprestado por poucos
//! milissegundos e devolvido ao lugar, e só depois de ele ficar parado.

use crate::WinError;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEINPUT, VIRTUAL_KEY, VK_RETURN,
};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos};

/// Espera o usuário largar o mouse (parado por `quiet`), no máximo `max`.
/// Evita clicar no meio de um movimento dele.
fn wait_for_idle_mouse(quiet: Duration, max: Duration) {
    let started = Instant::now();
    let mut last = cursor();
    let mut still_since = Instant::now();
    while started.elapsed() < max {
        std::thread::sleep(Duration::from_millis(40));
        let now = cursor();
        if now != last {
            last = now;
            still_since = Instant::now();
        } else if still_since.elapsed() >= quiet {
            return;
        }
    }
}

fn cursor() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

fn mouse(flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

fn send(inputs: &[INPUT]) -> Result<(), WinError> {
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        Err(WinError::Failed(
            "o Windows bloqueou a entrada simulada (app de administrador em foco?)".into(),
        ))
    }
}

/// Clique esquerdo em (x, y), devolvendo o mouse do usuário ao lugar.
pub fn click_at(x: i32, y: i32) -> Result<(), WinError> {
    wait_for_idle_mouse(Duration::from_millis(300), Duration::from_secs(3));
    let (ox, oy) = cursor();
    let result = (|| {
        unsafe { SetCursorPos(x, y)? };
        std::thread::sleep(Duration::from_millis(40));
        send(&[mouse(MOUSEEVENTF_LEFTDOWN), mouse(MOUSEEVENTF_LEFTUP)])
    })();
    // Sempre devolve, mesmo se o clique falhou.
    unsafe {
        let _ = SetCursorPos(ox, oy);
    }
    result
}

fn key(vk: VIRTUAL_KEY, scan: u16, flags: windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

/// Digita `text` no campo em foco (Unicode: acentos e emoji funcionam, e o
/// layout do teclado não importa). Texto longo vai em blocos pequenos.
pub fn type_text(text: &str) -> Result<(), WinError> {
    for unit in text.encode_utf16() {
        send(&[
            key(VIRTUAL_KEY(0), unit, KEYEVENTF_UNICODE),
            key(VIRTUAL_KEY(0), unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
        ])?;
        std::thread::sleep(Duration::from_millis(4));
    }
    Ok(())
}

pub fn press_enter() -> Result<(), WinError> {
    send(&[
        key(VK_RETURN, 0, Default::default()),
        key(VK_RETURN, 0, KEYEVENTF_KEYUP),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_wait_returns_quickly_when_mouse_is_still() {
        let t = Instant::now();
        wait_for_idle_mouse(Duration::from_millis(80), Duration::from_secs(2));
        assert!(t.elapsed() < Duration::from_secs(2), "parado deve liberar logo");
    }
}
