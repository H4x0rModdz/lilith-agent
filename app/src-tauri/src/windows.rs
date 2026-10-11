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
    place_above_orb(app, chat, GAP);
}

/// Põe `win` logo acima da orb, alinhada pela direita (ou abaixo, se não couber).
fn place_above_orb(app: &AppHandle, chat: &WebviewWindow, gap: i32) {
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
    let mut y = orb_pos.y - ch - gap;
    if y < min_y {
        y = orb_pos.y + orb_size.height as i32 + gap;
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

/// Tamanho (lógico) da janela da forma 3D.
const AVATAR_SIZE: (f64, f64) = (340.0, 420.0);

pub fn avatar_open(app: &AppHandle) -> bool {
    app.get_webview_window("avatar").is_some()
}

/// Abre a forma 3D. A janela só existe enquanto ela está ativa: ao voltar
/// para a forma 2D é destruída, e a GPU e a memória voltam ao normal.
pub fn show_avatar(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("avatar") {
        let _ = w.show();
        return;
    }
    let mut builder = tauri::WebviewWindowBuilder::new(app, "avatar", tauri::WebviewUrl::App("index.html".into()))
        .title("Lilith 3D")
        .inner_size(AVATAR_SIZE.0, AVATAR_SIZE.1)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .focused(false);
    // Já nasce acima da orb (no canto onde o usuário a deixou).
    if let Some((x, y)) = avatar_position(app) {
        builder = builder.position(x, y);
    }
    let built = builder
        // Erros do JavaScript desta janela vão para o log do app.
        .initialization_script(
            "window.addEventListener('error',e=>window.__TAURI_INTERNALS__.invoke('js_log',{msg:'erro: '+e.message+' @'+e.filename+':'+e.lineno}));             window.addEventListener('unhandledrejection',e=>window.__TAURI_INTERNALS__.invoke('js_log',{msg:'promessa: '+String(e.reason)}));",
        )
        .build();
    match built {
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "não abri a forma 3D"),
    }
}

/// Posição lógica da janela 3D: alinhada à direita da orb, com a base
/// (que se dissolve) um pouco sobre ela, e dentro da área útil do monitor.
fn avatar_position(app: &AppHandle) -> Option<(f64, f64)> {
    let orb = app.get_webview_window("orb")?;
    let (pos, size) = (orb.outer_position().ok()?, orb.outer_size().ok()?);
    let scale = orb.scale_factor().unwrap_or(1.0);
    let (w, h) = ((AVATAR_SIZE.0 * scale) as i32, (AVATAR_SIZE.1 * scale) as i32);
    let mut x = pos.x + size.width as i32 - w;
    let mut y = pos.y - h + (size.height as f64 * 0.45) as i32;
    if let Some(m) = orb.current_monitor().ok().flatten() {
        let a = m.work_area();
        x = x.clamp(a.position.x, (a.position.x + a.size.width as i32 - w).max(a.position.x));
        y = y.clamp(a.position.y, (a.position.y + a.size.height as i32 - h).max(a.position.y));
    }
    Some((x as f64 / scale, y as f64 / scale))
}

pub fn hide_avatar(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("avatar") {
        let _ = w.destroy();
    }
}

pub fn toggle_avatar(app: &AppHandle) {
    if avatar_open(app) {
        hide_avatar(app);
    } else {
        show_avatar(app);
    }
}
