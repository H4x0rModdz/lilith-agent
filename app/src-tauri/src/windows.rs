use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Manager, PhysicalPosition, WebviewWindow};

/// O usuário arrastou o chat nesta sessão — não reposicionar ao lado da orb.
pub static CHAT_MOVED: AtomicBool = AtomicBool::new(false);
/// O painel já tem posição (salva de uma execução anterior pelo
/// window-state, arrastada, ou já posicionada automaticamente uma vez).
pub static PANEL_PLACED: AtomicBool = AtomicBool::new(false);

const MARGIN: i32 = 16;
const GAP: i32 = 8;

/// Põe a orb no canto inferior direito da área de trabalho (acima da barra
/// de tarefas) do monitor principal.
pub fn place_orb(orb: &WebviewWindow) {
    let Ok(Some(monitor)) = orb.primary_monitor() else { return };
    let Ok(size) = orb.outer_size() else { return };
    let area = monitor.work_area();
    let x = area.position.x + area.size.width as i32 - size.width as i32 - MARGIN;
    let y = area.position.y + area.size.height as i32 - size.height as i32 - MARGIN;
    let _ = orb.set_position(PhysicalPosition::new(x, y));
}

/// Posiciona o chat logo acima da orb (ou abaixo, se não couber), alinhado
/// pela direita e preso dentro do monitor onde a orb está.
fn place_chat(app: &AppHandle, chat: &WebviewWindow) {
    let Some(orb) = app.get_webview_window("orb") else { return };
    let (Ok(orb_pos), Ok(orb_size), Ok(chat_size)) = (orb.outer_position(), orb.outer_size(), chat.outer_size())
    else {
        return;
    };
    let monitor = orb.current_monitor().ok().flatten();
    let (min_x, min_y, max_x, max_y) = match &monitor {
        Some(m) => {
            let a = m.work_area();
            (
                a.position.x,
                a.position.y,
                a.position.x + a.size.width as i32,
                a.position.y + a.size.height as i32,
            )
        }
        None => (i32::MIN / 2, i32::MIN / 2, i32::MAX / 2, i32::MAX / 2),
    };
    let (cw, ch) = (chat_size.width as i32, chat_size.height as i32);
    let mut x = orb_pos.x + orb_size.width as i32 - cw;
    let mut y = orb_pos.y - ch - GAP;
    if y < min_y {
        y = orb_pos.y + orb_size.height as i32 + GAP;
    }
    x = x.clamp(min_x, (max_x - cw).max(min_x));
    y = y.clamp(min_y, (max_y - ch).max(min_y));
    let _ = chat.set_position(PhysicalPosition::new(x, y));
}

pub fn show_chat(app: &AppHandle) {
    if let Some(chat) = app.get_webview_window("chat") {
        if !CHAT_MOVED.load(Ordering::Relaxed) {
            place_chat(app, &chat);
        }
        let _ = chat.show();
        let _ = chat.set_focus();
    }
}

pub fn toggle_chat(app: &AppHandle) {
    if let Some(chat) = app.get_webview_window("chat") {
        if chat.is_visible().unwrap_or(false) {
            let _ = chat.hide();
        } else {
            show_chat(app);
        }
    }
}

/// Na primeira vez, o painel abre no canto inferior direito, colado na área
/// de notificação (onde fica o ícone da Lilith em "Mostrar ícones ocultos").
/// Depois fica onde o usuário o deixar.
pub fn show_panel(app: &AppHandle) {
    let Some(panel) = app.get_webview_window("panel") else { return };
    if PANEL_PLACED.swap(true, Ordering::Relaxed) {
        let _ = panel.show();
        let _ = panel.set_focus();
        return;
    }
    if let (Ok(Some(monitor)), Ok(size)) = (panel.primary_monitor(), panel.outer_size()) {
        let area = monitor.work_area();
        let x = area.position.x + area.size.width as i32 - size.width as i32 - GAP;
        let y = area.position.y + area.size.height as i32 - size.height as i32 - GAP;
        let _ = panel.set_position(PhysicalPosition::new(x, y));
    }
    let _ = panel.show();
    let _ = panel.set_focus();
}

pub fn toggle_panel(app: &AppHandle) {
    if let Some(panel) = app.get_webview_window("panel") {
        if panel.is_visible().unwrap_or(false) {
            let _ = panel.hide();
        } else {
            show_panel(app);
        }
    }
}
