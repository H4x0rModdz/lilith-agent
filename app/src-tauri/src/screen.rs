//! Presença da Lilith na tela: o cursor rosa dela e a borda translúcida que
//! avisa "ela está mexendo na tela". Enquanto houver uma ação de tela em
//! curso, a borda fica em todos os monitores e o cursor visível; ao terminar,
//! ambos somem. O mouse do usuário nunca é tocado — o cursor é uma janela
//! overlay que ignora cliques, e as janelas são movidas por API.

use lilith_win::window::{self, MovePlan, Rect};
use lilith_win::WinError;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindowBuilder};

const FRAME: Duration = Duration::from_millis(16);
/// Ponta da seta dentro da janela do cursor (px lógicos, ver cursor.ts).
const TIP: (f64, f64) = (6.0, 4.0);
/// Quanto tempo a borda/cursor ficam depois da última ação.
const LINGER: Duration = Duration::from_millis(900);

pub struct Screen {
    app: AppHandle,
    pos: Mutex<(i32, i32)>,
    active: AtomicUsize,
    generation: AtomicU64,
    borders_ready: Mutex<bool>,
}

/// Mantém a presença ativa enquanto vive; ao cair, agenda o recolhimento.
pub struct Presence(Arc<Screen>);

impl Drop for Presence {
    fn drop(&mut self) {
        self.0.end();
    }
}

impl Screen {
    pub fn new(app: AppHandle) -> Arc<Self> {
        Arc::new(Self {
            app,
            pos: Mutex::new((0, 0)),
            active: AtomicUsize::new(0),
            generation: AtomicU64::new(0),
            borders_ready: Mutex::new(false),
        })
    }

    /// Começa uma ação de tela: borda em todos os monitores e cursor saindo
    /// da orb (ou de onde parou, se ainda estiver visível).
    pub fn begin(self: &Arc<Self>) -> Presence {
        self.generation.fetch_add(1, Ordering::Relaxed);
        if self.active.fetch_add(1, Ordering::Relaxed) == 0 {
            self.show_borders();
            let cursor_visible = self.cursor().and_then(|c| c.is_visible().ok()).unwrap_or(false);
            if !cursor_visible {
                let start = self.orb_center().unwrap_or((0, 0));
                self.place_cursor(start);
                if let Some(c) = self.cursor() {
                    let _ = c.show();
                }
            }
        }
        Presence(self.clone())
    }

    fn end(&self) {
        if self.active.fetch_sub(1, Ordering::Relaxed) != 1 {
            return;
        }
        let gen = self.generation.load(Ordering::Relaxed);
        let app = self.app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(LINGER).await;
            let screen = app.state::<Arc<Screen>>();
            if screen.active.load(Ordering::Relaxed) == 0 && screen.generation.load(Ordering::Relaxed) == gen {
                screen.hide_now();
            }
        });
    }

    /// Esconde tudo na hora (kill switch).
    pub fn hide_now(&self) {
        if let Some(c) = self.cursor() {
            let _ = c.hide();
        }
        for (label, w) in self.app.webview_windows() {
            if label.starts_with("border-") {
                let _ = w.hide();
            }
        }
    }

    // ── cursor ────────────────────────────────────────────────────────────

    fn cursor(&self) -> Option<tauri::WebviewWindow> {
        self.app.get_webview_window("cursor")
    }

    fn orb_center(&self) -> Option<(i32, i32)> {
        let orb = self.app.get_webview_window("orb")?;
        let (p, s) = (orb.outer_position().ok()?, orb.outer_size().ok()?);
        Some((p.x + s.width as i32 / 2, p.y + s.height as i32 / 2))
    }

    /// Põe a ponta da seta exatamente em `(x, y)` (pixels físicos).
    fn place_cursor(&self, (x, y): (i32, i32)) {
        *self.pos.lock().unwrap() = (x, y);
        if let Some(c) = self.cursor() {
            let scale = c.scale_factor().unwrap_or(1.0);
            let _ = c.set_position(PhysicalPosition::new(
                x - (TIP.0 * scale).round() as i32,
                y - (TIP.1 * scale).round() as i32,
            ));
        }
    }

    fn cursor_event(&self, state: &str) {
        if let Some(c) = self.cursor() {
            let _ = c.emit("lilith://cursor", state);
        }
    }

    /// Voa até `(x, y)` com aceleração suave, como um mouse de verdade.
    pub async fn fly_to(&self, to: (i32, i32)) {
        let from = *self.pos.lock().unwrap();
        let dist = (((to.0 - from.0).pow(2) + (to.1 - from.1).pow(2)) as f64).sqrt();
        let frames = ((dist / 45.0) as u32).clamp(8, 36);
        for i in 1..=frames {
            let t = ease(i as f64 / frames as f64);
            let p = (
                from.0 + ((to.0 - from.0) as f64 * t).round() as i32,
                from.1 + ((to.1 - from.1) as f64 * t).round() as i32,
            );
            self.place_cursor(p);
            tokio::time::sleep(FRAME).await;
        }
    }

    /// "Clique" visual (o efeito no app é feito por API).
    pub async fn click(&self) {
        self.cursor_event("press");
        tokio::time::sleep(Duration::from_millis(120)).await;
        self.cursor_event("release");
    }

    pub async fn point(&self, at: (i32, i32)) {
        self.fly_to(at).await;
        self.cursor_event("point");
        tokio::time::sleep(Duration::from_millis(700)).await;
    }

    /// Arrasta a janela pela barra de título até o destino, quadro a quadro.
    pub async fn drag(&self, plan: &MovePlan) -> Result<(), WinError> {
        let grip = title_grip(plan.from);
        self.fly_to(grip).await;
        window::raise(plan.id);
        self.cursor_event("press");
        tokio::time::sleep(Duration::from_millis(90)).await;

        let (dx, dy) = (grip.0 - plan.from.x, grip.1 - plan.from.y);
        let dist = (((plan.to.x - plan.from.x).pow(2) + (plan.to.y - plan.from.y).pow(2)) as f64).sqrt();
        let frames = ((dist / 40.0) as u32).clamp(14, 40);
        for i in 1..=frames {
            let t = ease(i as f64 / frames as f64);
            let r = plan.from.lerp(&plan.to, t);
            let id = plan.id;
            tokio::task::spawn_blocking(move || window::set_frame(id, r))
                .await
                .map_err(|e| WinError::Failed(e.to_string()))??;
            // A pegada acompanha a janela, proporcional se ela mudar de tamanho.
            let gx = r.x + (dx as f64 * r.w as f64 / plan.from.w.max(1) as f64).round() as i32;
            self.place_cursor((gx, r.y + dy));
            tokio::time::sleep(FRAME).await;
        }
        let p = plan.clone();
        tokio::task::spawn_blocking(move || window::finish(&p))
            .await
            .map_err(|e| WinError::Failed(e.to_string()))??;
        self.cursor_event("release");
        Ok(())
    }

    // ── borda ─────────────────────────────────────────────────────────────

    /// Uma janela de borda por monitor (criadas sob demanda, depois só
    /// mostradas/escondidas), cobrindo o monitor inteiro e ignorando cliques.
    fn show_borders(&self) {
        let Ok(monitors) = self.app.available_monitors() else { return };
        let mut ready = self.borders_ready.lock().unwrap();
        if !*ready {
            for (i, m) in monitors.iter().enumerate() {
                let label = format!("border-{i}");
                let built = WebviewWindowBuilder::new(&self.app, &label, WebviewUrl::App("index.html".into()))
                    .title("Lilith — borda")
                    .decorations(false)
                    .transparent(true)
                    .shadow(false)
                    .always_on_top(true)
                    .skip_taskbar(true)
                    .focused(false)
                    .focusable(false)
                    .resizable(false)
                    .visible(false)
                    .build();
                match built {
                    Ok(w) => {
                        let _ = w.set_ignore_cursor_events(true);
                        let _ = w.set_position(PhysicalPosition::new(m.position().x, m.position().y));
                        let _ = w.set_size(PhysicalSize::new(m.size().width, m.size().height));
                    }
                    Err(e) => tracing::warn!(error = %e, "não criei a borda do monitor {i}"),
                }
            }
            *ready = true;
        }
        for (label, w) in self.app.webview_windows() {
            if label.starts_with("border-") {
                let _ = w.show();
            }
        }
        // A borda não pode ficar por cima do cursor.
        if let Some(c) = self.cursor() {
            let _ = c.set_always_on_top(true);
        }
    }
}

/// Ponto da barra de título por onde "pegar" a janela: um terço da largura,
/// longe dos botões de minimizar/fechar e das abas no meio.
fn title_grip(frame: Rect) -> (i32, i32) {
    (frame.x + frame.w / 3, frame.y + 14)
}

/// Pontos aproximados dos botões da barra de título (padrão do Windows 11).
pub fn caption_button(frame: Rect, action: &str) -> (i32, i32) {
    let offset = match action {
        "window.close" => 23,
        "window.maximize" | "window.restore" => 69,
        _ => 115, // minimizar
    };
    (frame.x + frame.w - offset, frame.y + 15)
}

fn ease(t: f64) -> f64 {
    // ease-in-out cúbica
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}
