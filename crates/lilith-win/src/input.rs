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
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEINPUT, VIRTUAL_KEY, VK_CONTROL, VK_RETURN, VK_TAB,
};
use std::sync::atomic::{AtomicBool, Ordering};

/// O kill switch liga isto; a digitação para na próxima letra.
static CANCELLED: AtomicBool = AtomicBool::new(false);

pub fn cancel() {
    CANCELLED.store(true, Ordering::Relaxed);
}

pub fn reset_cancel() {
    CANCELLED.store(false, Ordering::Relaxed);
}

fn check_cancel() -> Result<(), WinError> {
    if CANCELLED.load(Ordering::Relaxed) {
        Err(WinError::Failed("interrompido pelo usuário".into()))
    } else {
        Ok(())
    }
}
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
/// layout do teclado não importa). Rápido — para campos de busca.
pub fn type_text(text: &str) -> Result<(), WinError> {
    for unit in text.encode_utf16() {
        check_cancel()?;
        type_unit(unit)?;
        std::thread::sleep(Duration::from_millis(4));
    }
    Ok(())
}

fn type_unit(unit: u16) -> Result<(), WinError> {
    send(&[
        key(VIRTUAL_KEY(0), unit, KEYEVENTF_UNICODE),
        key(VIRTUAL_KEY(0), unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
    ])
}

/// Gerador pseudoaleatório simples (xorshift) — só para variar o ritmo.
struct Jitter(u64);

impl Jitter {
    fn new() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Jitter(seed | 1)
    }
    /// Número em 0.0..1.0.
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Teto da digitação humana inteira: texto longo digita mais rápido em vez
/// de levar minutos.
const HUMAN_BUDGET_MS: f64 = 15_000.0;

/// Atraso depois de cada caractere: base por letra (mais lento após espaço e
/// pontuação), com variação, e uma pausa ocasional de "pensar".
pub(crate) fn human_delay_ms(prev: char, jitter: f64, think: f64, base_ms: f64) -> u64 {
    let mut ms = base_ms * (0.55 + jitter * 0.9);
    if prev == ' ' {
        ms *= 1.5;
    } else if matches!(prev, '.' | ',' | '!' | '?' | ':' | ';') {
        ms *= 2.5;
    }
    // ~4% dos caracteres: pausa curta, como quem pensa na próxima palavra.
    if think < 0.04 {
        ms += 180.0 + think * 4000.0;
    }
    ms as u64
}

/// Um pedaço do texto: letras normais ou uma menção (`@nome`).
#[derive(Debug, PartialEq)]
pub(crate) enum Piece {
    Text(String),
    Mention(String),
}

/// Separa `@nome` (no começo ou depois de espaço) do resto do texto.
pub(crate) fn split_mentions(text: &str) -> Vec<Piece> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<Piece> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    while i < chars.len() {
        let boundary = i == 0 || chars[i - 1].is_whitespace();
        if chars[i] == '@' && boundary {
            let mut j = i + 1;
            while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            if j > i + 1 {
                if !buf.is_empty() {
                    out.push(Piece::Text(std::mem::take(&mut buf)));
                }
                out.push(Piece::Mention(chars[i + 1..j].iter().collect()));
                i = j;
                continue;
            }
        }
        buf.push(chars[i]);
        i += 1;
    }
    if !buf.is_empty() {
        out.push(Piece::Text(buf));
    }
    out
}

/// Digita como uma pessoa: ritmo irregular, pausas, e `@menções` de
/// verdade — digita `@nome`, espera a lista de sugestões abrir e escolhe com
/// Tab (que, ao contrário do Enter, nunca envia a mensagem).
pub fn type_human(text: &str, mentions: bool) -> Result<(), WinError> {
    let total = text.chars().count().max(1) as f64;
    // Média por letra que cabe no orçamento (4–90 ms).
    let base = (HUMAN_BUDGET_MS / total / 1.3).clamp(4.0, 90.0);
    let mut jitter = Jitter::new();
    let mut prev = ' ';
    let pieces = if mentions { split_mentions(text) } else { vec![Piece::Text(text.to_string())] };
    for piece in pieces {
        match piece {
            Piece::Text(t) => {
                for ch in t.chars() {
                    check_cancel()?;
                    let mut buf = [0u16; 2];
                    for unit in ch.encode_utf16(&mut buf) {
                        type_unit(*unit)?;
                    }
                    std::thread::sleep(Duration::from_millis(human_delay_ms(prev, jitter.next(), jitter.next(), base)));
                    prev = ch;
                }
            }
            Piece::Mention(name) => {
                check_cancel()?;
                type_text(&format!("@{name}"))?;
                // A lista de sugestões demora um instante para filtrar e abrir.
                std::thread::sleep(Duration::from_millis(900));
                check_cancel()?;
                press_key(VK_TAB)?;
                std::thread::sleep(Duration::from_millis(250));
                prev = ' ';
            }
        }
    }
    Ok(())
}

fn press_key(vk: VIRTUAL_KEY) -> Result<(), WinError> {
    send(&[key(vk, 0, Default::default()), key(vk, 0, KEYEVENTF_KEYUP)])
}

/// Ctrl + uma letra (A–Z), ex.: Ctrl+F, Ctrl+K.
pub fn press_ctrl(letter: char) -> Result<(), WinError> {
    let c = letter.to_ascii_uppercase();
    if !c.is_ascii_uppercase() {
        return Err(WinError::InvalidParams("atalho: só letras A–Z".into()));
    }
    let vk = VIRTUAL_KEY(c as u16); // as teclas A–Z têm o código do próprio caractere
    send(&[
        key(VK_CONTROL, 0, Default::default()),
        key(vk, 0, Default::default()),
        key(vk, 0, KEYEVENTF_KEYUP),
        key(VK_CONTROL, 0, KEYEVENTF_KEYUP),
    ])
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
    fn mentions_split_only_at_word_starts() {
        assert_eq!(
            split_mentions("oi @todos, vamos? mail a@b.com @ana"),
            vec![
                Piece::Text("oi ".into()),
                Piece::Mention("todos".into()),
                Piece::Text(", vamos? mail a@b.com ".into()),
                Piece::Mention("ana".into()),
            ]
        );
        assert_eq!(split_mentions("só @ sozinho"), vec![Piece::Text("só @ sozinho".into())]);
        assert_eq!(split_mentions("@everyone"), vec![Piece::Mention("everyone".into())]);
    }

    #[test]
    fn human_pace_varies_and_slows_after_punctuation() {
        let letter = human_delay_ms('a', 0.5, 0.9, 60.0);
        let after_space = human_delay_ms(' ', 0.5, 0.9, 60.0);
        let after_dot = human_delay_ms('.', 0.5, 0.9, 60.0);
        assert!(after_space > letter && after_dot > after_space);
        assert!(human_delay_ms('a', 0.0, 0.9, 60.0) < human_delay_ms('a', 1.0, 0.9, 60.0));
        assert!(human_delay_ms('a', 0.5, 0.0, 60.0) > letter + 150, "pausa de pensar");
    }

    #[test]
    fn idle_wait_returns_quickly_when_mouse_is_still() {
        let t = Instant::now();
        wait_for_idle_mouse(Duration::from_millis(80), Duration::from_secs(2));
        assert!(t.elapsed() < Duration::from_secs(2), "parado deve liberar logo");
    }
}
