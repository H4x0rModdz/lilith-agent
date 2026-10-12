use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewWindow};

/// Contador de movimentos da janela 3D (para saber quando o arrasto parou).
static AVATAR_MOVES: AtomicU64 = AtomicU64::new(0);
/// A janela 3D está encostada num canto da tela (avisa a UI uma vez por mudança).
static AVATAR_NEAR_CORNER: AtomicBool = AtomicBool::new(false);
/// A janela 3D escondida fica pronta (sem renderizar) por este tempo; passado
/// isso é destruída para devolver memória e GPU.
const AVATAR_KEEP_WARM_SECS: u64 = 600;
/// Invalida o "destruir depois" quando a janela volta a ser usada.
static AVATAR_HIDE_GEN: AtomicU64 = AtomicU64::new(0);

/// Quanto o arrasto precisa parar no canto para guardar a Lilith.
const DOCK_SETTLE_MS: u64 = 450;
/// Folga (px lógicos) para considerar a janela encostada nas bordas do canto.
const DOCK_TOLERANCE: f64 = 36.0;

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
    if let Some(av) = app.get_webview_window("avatar").filter(|w| w.is_visible().unwrap_or(false)) {
        if place_beside(&av, chat) {
            return;
        }
    }
    place_above_orb(app, chat, GAP);
}

/// Põe `win` ao lado de `anchor` (à esquerda; à direita se não couber), com a
/// base alinhada, dentro da área útil do monitor. `false` se não deu.
fn place_beside(anchor: &WebviewWindow, win: &WebviewWindow) -> bool {
    let (Ok(ap), Ok(asz), Ok(wsz)) = (anchor.outer_position(), anchor.outer_size(), win.outer_size()) else {
        return false;
    };
    let (min_x, min_y, max_x, max_y) = match anchor.current_monitor().ok().flatten() {
        Some(m) => {
            let a = m.work_area();
            (a.position.x, a.position.y, a.position.x + a.size.width as i32, a.position.y + a.size.height as i32)
        }
        None => return false,
    };
    let (w, h) = (wsz.width as i32, wsz.height as i32);
    let mut x = ap.x - w - GAP;
    if x < min_x {
        x = ap.x + asz.width as i32 + GAP;
    }
    let y = (ap.y + asz.height as i32 - h).clamp(min_y, (max_y - h).max(min_y));
    let x = x.clamp(min_x, (max_x - w).max(min_x));
    let _ = win.set_position(PhysicalPosition::new(x, y));
    true
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

/// A forma 3D está na tela (existir escondida não conta).
pub fn avatar_open(app: &AppHandle) -> bool {
    app.get_webview_window("avatar").is_some_and(|w| w.is_visible().unwrap_or(false))
}

/// Abre a forma 3D. A janela só existe enquanto ela está ativa: ao voltar
/// para a forma 2D é destruída, e a GPU e a memória voltam ao normal.
pub fn show_avatar(app: &AppHandle) {
    // Criar janelas na thread principal (handler de menu/evento) trava no
    // Windows: faz fora dela.
    let app = app.clone();
    tauri::async_runtime::spawn(async move { show_avatar_now(&app) });
}

fn show_avatar_now(app: &AppHandle) {
    let t0 = std::time::Instant::now();
    tracing::info!("forma 3D: pedido de abertura");
    // Sem o arquivo do modelo, fica na forma 2D (a orb continua).
    if !crate::config::avatar_model_exists() {
        tracing::warn!("sem lilith.vrm em %APPDATA%\\Lilith — ficando na forma 2D");
        return;
    }
    AVATAR_HIDE_GEN.fetch_add(1, Ordering::SeqCst);
    if let Some(w) = app.get_webview_window("avatar") {
        // Já carregada (escondida): reposiciona, mostra e toca a entrada.
        if let Some((x, y)) = avatar_position(app) {
            let _ = w.set_position(tauri::LogicalPosition::new(x, y));
        }
        tracing::info!(ms = t0.elapsed().as_millis() as u64, "forma 3D: reposicionada");
        let _ = w.show();
        tracing::info!(ms = t0.elapsed().as_millis() as u64, "forma 3D: janela mostrada");
        let _ = app.emit_to("avatar", "lilith://avatar-visible", true);
        hide_orb(app);
        remember_form(app, "3d");
        tracing::info!(ms = t0.elapsed().as_millis() as u64, "forma 3D: pronta (orb escondida)");
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
    // Já nasce no lugar da orb (no canto onde o usuário a deixou).
    if let Some((x, y)) = avatar_position(app) {
        builder = builder.position(x, y);
    }
    let built = builder
        // Erros do JavaScript desta janela vão para o log do app.
        .initialization_script(
            "window.addEventListener('error',e=>window.__TAURI_INTERNALS__.invoke('js_log',{msg:'erro: '+e.message+' @'+e.filename+':'+e.lineno}));\
             window.addEventListener('unhandledrejection',e=>window.__TAURI_INTERNALS__.invoke('js_log',{msg:'promessa: '+String(e.reason)}));",
        )
        .build();
    match built {
        Ok(w) => {
            // Ao arrastar a Lilith 3D, a orb acompanha escondida: voltar para
            // 2D a deixa no mesmo lugar.
            let handle = app.clone();
            w.on_window_event(move |e| {
                if matches!(e, tauri::WindowEvent::Moved(_)) {
                    sync_orb_to_avatar(&handle);
                    on_avatar_moved(&handle);
                }
            });
            hide_orb(app);
            remember_form(app, "3d");
        }
        Err(e) => tracing::warn!(error = %e, "não abri a forma 3D"),
    }
}

/// A janela 3D encosta em duas bordas perpendiculares da área útil (um canto)?
fn avatar_near_corner(app: &AppHandle) -> bool {
    let Some(av) = app.get_webview_window("avatar") else { return false };
    let (Ok(pos), Ok(size), Ok(Some(monitor))) = (av.outer_position(), av.outer_size(), av.current_monitor()) else {
        return false;
    };
    let tol = (DOCK_TOLERANCE * av.scale_factor().unwrap_or(1.0)) as i32;
    let a = monitor.work_area();
    let (left, top) = (a.position.x, a.position.y);
    let (right, bottom) = (left + a.size.width as i32, top + a.size.height as i32);
    let (w, h) = (size.width as i32, size.height as i32);
    let at_x = pos.x <= left + tol || pos.x + w >= right - tol;
    let at_y = pos.y <= top + tol || pos.y + h >= bottom - tol;
    at_x && at_y
}

/// Chamado a cada movimento da janela 3D: avisa quando ela chega perto de um
/// canto e, se o arrasto para ali, pede para a Lilith se guardar na orb.
fn on_avatar_moved(app: &AppHandle) {
    let seq = AVATAR_MOVES.fetch_add(1, Ordering::Relaxed) + 1;
    let near = avatar_near_corner(app);
    if near != AVATAR_NEAR_CORNER.swap(near, Ordering::Relaxed) {
        let _ = app.emit_to("avatar", "lilith://avatar-dock-hint", near);
    }
    if near {
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(DOCK_SETTLE_MS)).await;
            // Soltou: nenhum movimento novo desde então e ainda no canto.
            if AVATAR_MOVES.load(Ordering::Relaxed) == seq && avatar_near_corner(&handle) {
                let _ = handle.emit_to("avatar", "lilith://avatar-dock", ());
            }
        });
    }
}

fn hide_orb(app: &AppHandle) {
    if let Some(orb) = app.get_webview_window("orb") {
        let _ = orb.hide();
    }
}

/// Guarda a forma escolhida ("2d"/"3d") para a próxima abertura.
fn remember_form(app: &AppHandle, form: &str) {
    if let Some(state) = app.try_state::<crate::AppState>() {
        state.store.set_setting("form", form);
    }
}

/// Coloca a orb (escondida) onde ficaria para a janela 3D estar no lugar
/// atual — o inverso de `avatar_position`.
fn sync_orb_to_avatar(app: &AppHandle) {
    let (Some(av), Some(orb)) = (app.get_webview_window("avatar"), app.get_webview_window("orb")) else { return };
    if !av.is_visible().unwrap_or(false) {
        return;
    }
    let (Ok(ap), Ok(asz), Ok(osz)) = (av.outer_position(), av.outer_size(), orb.outer_size()) else { return };
    let x = ap.x + asz.width as i32 - osz.width as i32;
    let y = ap.y + asz.height as i32 - (osz.height as f64 * 0.45) as i32;
    let _ = orb.set_position(PhysicalPosition::new(x, y));
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
        sync_orb_to_avatar(app);
        let _ = app.emit_to("avatar", "lilith://avatar-visible", false);
        let _ = w.hide();
        // Fica pronta por um tempo (voltar ao 3D é instantâneo); depois sai.
        let gen = AVATAR_HIDE_GEN.fetch_add(1, Ordering::SeqCst) + 1;
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(AVATAR_KEEP_WARM_SECS)).await;
            if AVATAR_HIDE_GEN.load(Ordering::SeqCst) == gen {
                if let Some(w) = handle.get_webview_window("avatar") {
                    if !w.is_visible().unwrap_or(false) {
                        let _ = w.destroy();
                    }
                }
            }
        });
    }
    AVATAR_NEAR_CORNER.store(false, Ordering::Relaxed);
    if let Some(orb) = app.get_webview_window("orb") {
        let _ = orb.show();
        // A orb "pula" de volta (animação no front).
        let _ = app.emit_to("orb", "lilith://orb-pop", ());
    }
    remember_form(app, "2d");
}

pub fn toggle_avatar(app: &AppHandle) {
    if avatar_open(app) {
        hide_avatar(app);
    } else {
        show_avatar(app);
    }
}
