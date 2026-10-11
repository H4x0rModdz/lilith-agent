//! Janelas e monitores: listar, achar por nome, focar, minimizar/maximizar,
//! mover (inclusive entre monitores) e dividir a tela entre vários apps.
//!
//! Tudo por API (`SetWindowPos`), sem tocar no mouse do usuário — o cursor
//! rosa da Lilith é só a representação visual, desenhada pelo app. Este
//! módulo calcula os [`MovePlan`]s; quem chama decide se aplica na hora
//! ([`apply_now`]) ou anima o "arrastar" quadro a quadro ([`set_frame`]).
//!
//! Coordenadas são físicas (pixels reais) e se referem à moldura *visível*
//! da janela: no Windows 10/11 o retângulo da janela inclui bordas
//! invisíveis de redimensionamento, que descontamos via DWM para os encaixes
//! ficarem colados nas bordas como no Snap do Windows.

use crate::paths::fold;
use crate::WinError;
use serde::Serialize;
use serde_json::{json, Value};
use windows::core::BOOL;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromWindow, HDC, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_KEYUP, VK_MENU};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowLongW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, IsZoomed, PostMessageW, SetForegroundWindow, SetWindowPos,
    ShowWindow, GWL_EXSTYLE, GW_OWNER, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_MAXIMIZE,
    SW_MINIMIZE, SW_RESTORE, WM_CLOSE, WS_EX_TOOLWINDOW,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    fn from_win(r: RECT) -> Self {
        Rect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top }
    }

    pub fn center(&self) -> (i32, i32) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }

    /// Interpolação linear entre dois retângulos (`t` em 0..=1).
    pub fn lerp(&self, to: &Rect, t: f64) -> Rect {
        let l = |a: i32, b: i32| a + ((b - a) as f64 * t).round() as i32;
        Rect { x: l(self.x, to.x), y: l(self.y, to.y), w: l(self.w, to.w), h: l(self.h, to.h) }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Monitor {
    /// 1 = principal; os demais da esquerda para a direita.
    pub index: usize,
    pub primary: bool,
    /// Área útil (sem a barra de tarefas).
    pub work: Rect,
    #[serde(skip)]
    handle: isize,
}

#[derive(Debug, Clone, Serialize)]
pub struct WindowInfo {
    pub id: isize,
    pub title: String,
    pub app: String,
    pub monitor: usize,
    pub frame: Rect,
    pub maximized: bool,
    pub minimized: bool,
    /// O app roda como administrador: um processo comum não consegue
    /// movê-lo nem fechá-lo (UIPI) — precisa do ajudante elevado.
    pub elevated: bool,
}

/// Movimento de uma janela: de onde para onde (moldura visível).
#[derive(Debug, Clone, Serialize)]
pub struct MovePlan {
    pub id: isize,
    pub title: String,
    pub from: Rect,
    pub to: Rect,
    /// Maximizar ao final (posição "maximize", ou janela que já estava
    /// maximizada e mudou de monitor).
    pub maximize_after: bool,
}

fn hwnd(id: isize) -> HWND {
    HWND(id as *mut _)
}

// ── monitores ─────────────────────────────────────────────────────────────

pub fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn collect(h: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let list = &mut *(data.0 as *mut Vec<(isize, MONITORINFO)>);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(h, &mut info).as_bool() {
            list.push((h.0 as isize, info));
        }
        BOOL(1)
    }
    let mut raw: Vec<(isize, MONITORINFO)> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut raw as *mut _ as isize));
    }
    // Principal primeiro, depois esquerda→direita, cima→baixo: "tela 2" é a
    // segunda que o usuário vê, não a ordem arbitrária do driver.
    raw.sort_by_key(|(_, i)| (i.dwFlags & 1 == 0, i.rcMonitor.left, i.rcMonitor.top));
    raw.into_iter()
        .enumerate()
        .map(|(n, (handle, info))| Monitor {
            index: n + 1,
            primary: info.dwFlags & 1 == 1,
            work: Rect::from_win(info.rcWork),
            handle,
        })
        .collect()
}

fn monitor_of(h: HWND, mons: &[Monitor]) -> usize {
    let m = unsafe { MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST) };
    mons.iter().find(|x| x.handle == m.0 as isize).map(|x| x.index).unwrap_or(1)
}

// ── janelas ───────────────────────────────────────────────────────────────

/// Janelas "de app" visíveis, na ordem Z (a de cima primeiro), excluindo as
/// da própria Lilith, janelas de ferramenta, ocultas (cloaked) e owned.
pub fn list() -> Vec<WindowInfo> {
    unsafe extern "system" fn collect(h: HWND, data: LPARAM) -> BOOL {
        (*(data.0 as *mut Vec<isize>)).push(h.0 as isize);
        BOOL(1)
    }
    let mut handles: Vec<isize> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut handles as *mut _ as isize));
    }
    let mons = monitors();
    let own_pid = unsafe { GetCurrentProcessId() };
    handles.into_iter().filter_map(|id| describe(id, own_pid, &mons)).collect()
}

fn describe(id: isize, own_pid: u32, mons: &[Monitor]) -> Option<WindowInfo> {
    let h = hwnd(id);
    unsafe {
        if !IsWindowVisible(h).as_bool() || GetWindowTextLengthW(h) == 0 {
            return None;
        }
        if GetWindow(h, GW_OWNER).is_ok_and(|o| !o.is_invalid()) {
            return None;
        }
        if GetWindowLongW(h, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
            return None;
        }
        let mut cloaked: u32 = 0;
        let _ = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, 4);
        if cloaked != 0 {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        if pid == own_pid {
            return None;
        }
        let title = window_title(h);
        if title == "Program Manager" {
            return None;
        }
        Some(WindowInfo {
            id,
            title,
            app: process_name(pid).unwrap_or_default(),
            monitor: monitor_of(h, mons),
            frame: frame_rect(h),
            maximized: IsZoomed(h).as_bool(),
            minimized: IsIconic(h).as_bool(),
            elevated: process_is_elevated(pid),
        })
    }
}

fn window_title(h: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn process_name(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(handle);
        ok.ok()?;
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned())
    }
}

/// O processo roda elevado? Se nem dá para ler o token dele, é porque está
/// acima de nós — também conta como elevado.
pub fn process_is_elevated(pid: u32) -> bool {
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let elevated = token_elevated(process).unwrap_or(true);
        let _ = CloseHandle(process);
        elevated
    }
}

/// A própria Lilith está elevada (ex.: rodando como ajudante)?
pub fn self_elevated() -> bool {
    unsafe { token_elevated(GetCurrentProcess()).unwrap_or(false) }
}

unsafe fn token_elevated(process: windows::Win32::Foundation::HANDLE) -> Option<bool> {
    let mut token = windows::Win32::Foundation::HANDLE::default();
    OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
    let mut info = TOKEN_ELEVATION::default();
    let mut len = 0u32;
    let ok = GetTokenInformation(
        token,
        TokenElevation,
        Some(&mut info as *mut _ as *mut _),
        std::mem::size_of::<TOKEN_ELEVATION>() as u32,
        &mut len,
    );
    let _ = CloseHandle(token);
    ok.ok()?;
    Some(info.TokenIsElevated != 0)
}

/// O id ainda é uma janela de verdade? (o ajudante elevado valida antes de agir)
pub fn exists(id: isize) -> bool {
    unsafe { IsWindow(Some(hwnd(id))).as_bool() }
}

/// Moldura visível (sem as bordas invisíveis do Win10/11).
fn frame_rect(h: HWND) -> Rect {
    unsafe {
        let mut r = RECT::default();
        if DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut RECT as *mut _, std::mem::size_of::<RECT>() as u32)
            .is_ok()
        {
            return Rect::from_win(r);
        }
        let _ = GetWindowRect(h, &mut r);
        Rect::from_win(r)
    }
}

/// Acha uma janela: `"id:123"`, `"active"`/vazio (a de cima que não é da
/// Lilith — o chat dela costuma estar em foco quando o pedido chega), nome
/// do app ("chrome") ou trecho do título. Empates vão para a de cima.
pub fn find(target: &str) -> Result<WindowInfo, WinError> {
    find_in(&list(), target, &[])
}

fn find_in(windows: &[WindowInfo], target: &str, exclude: &[isize]) -> Result<WindowInfo, WinError> {
    let candidates: Vec<&WindowInfo> = windows.iter().filter(|w| !exclude.contains(&w.id)).collect();
    let t = fold(target);
    if let Some(id) = t.strip_prefix("id:").and_then(|s| s.trim().parse::<isize>().ok()) {
        return candidates
            .into_iter()
            .find(|w| w.id == id)
            .cloned()
            .ok_or_else(|| WinError::NotFound(format!("janela id:{id}")));
    }
    if t.is_empty() || matches!(t.as_str(), "active" | "ativa" | "atual" | "essa" | "esta") {
        return candidates
            .into_iter()
            .find(|w| !w.minimized)
            .cloned()
            .ok_or_else(|| WinError::NotFound("nenhuma janela aberta".into()));
    }
    let t = t.trim_end_matches(".exe");
    let rank = |w: &WindowInfo| {
        let app = fold(&w.app);
        let title = fold(&w.title);
        if app == t {
            Some(0)
        } else if title.contains(t) {
            Some(1)
        } else if app.contains(t) {
            Some(2)
        } else {
            None
        }
    };
    candidates
        .into_iter()
        .enumerate()
        .filter_map(|(z, w)| rank(w).map(|r| (r, z, w)))
        .min_by_key(|(r, z, _)| (*r, *z))
        .map(|(_, _, w)| w.clone())
        .ok_or_else(|| WinError::NotFound(format!("janela '{target}'")))
}

// ── ações instantâneas ────────────────────────────────────────────────────

pub fn focus(id: isize) -> Result<(), WinError> {
    let h = hwnd(id);
    unsafe {
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        if !SetForegroundWindow(h).as_bool() {
            // O Windows só deixa trocar o foco quem recebeu input por último;
            // um toque de ALT libera a troca (truque documentado e comum).
            keybd_event(VK_MENU.0 as u8, 0, Default::default(), 0);
            let ok = SetForegroundWindow(h).as_bool();
            keybd_event(VK_MENU.0 as u8, 0, KEYEVENTF_KEYUP, 0);
            if !ok {
                return Err(WinError::Failed("o Windows não deixou focar a janela".into()));
            }
        }
    }
    Ok(())
}

/// Traz para frente sem roubar o foco (antes de arrastar).
pub fn raise(id: isize) {
    unsafe {
        let _ = SetWindowPos(hwnd(id), Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

fn show(id: isize, cmd: windows::Win32::UI::WindowsAndMessaging::SHOW_WINDOW_CMD) {
    unsafe {
        let _ = ShowWindow(hwnd(id), cmd);
    }
}

/// minimize / maximize / restore.
pub fn set_state(id: isize, state: &str) -> Result<(), WinError> {
    let cmd = match state {
        "minimize" => SW_MINIMIZE,
        "maximize" => SW_MAXIMIZE,
        "restore" => SW_RESTORE,
        other => return Err(WinError::InvalidParams(format!("estado '{other}'"))),
    };
    show(id, cmd);
    Ok(())
}

/// WM_CLOSE = clicar no X: o app pode perguntar se quer salvar.
pub fn close(id: isize) -> Result<(), WinError> {
    unsafe { PostMessageW(Some(hwnd(id)), WM_CLOSE, WPARAM(0), LPARAM(0))? };
    Ok(())
}

/// Posiciona a janela de modo que a moldura *visível* fique em `frame`.
pub fn set_frame(id: isize, frame: Rect) -> Result<(), WinError> {
    let h = hwnd(id);
    unsafe {
        let mut outer = RECT::default();
        GetWindowRect(h, &mut outer)?;
        let visible = frame_rect(h);
        let outer = Rect::from_win(outer);
        // Bordas invisíveis = diferença entre o retângulo real e o visível.
        let (l, t) = (visible.x - outer.x, visible.y - outer.y);
        let (r, b) = ((outer.x + outer.w) - (visible.x + visible.w), (outer.y + outer.h) - (visible.y + visible.h));
        SetWindowPos(
            h,
            None,
            frame.x - l,
            frame.y - t,
            frame.w + l + r,
            frame.h + t + b,
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
        .map_err(|e| {
            WinError::Failed(format!(
                "não consegui mover '{}' ({}). Janelas de administrador não aceitam ser movidas por apps comuns.",
                window_title(h),
                e.message()
            ))
        })
    }
}

/// Fim do movimento: posição exata e maximização, se pedida.
pub fn finish(plan: &MovePlan) -> Result<(), WinError> {
    set_frame(plan.id, plan.to)?;
    if plan.maximize_after {
        show(plan.id, SW_MAXIMIZE);
    }
    Ok(())
}

pub fn apply_now(plans: &[MovePlan]) -> Result<(), WinError> {
    for p in plans {
        finish(p)?;
    }
    Ok(())
}

// ── planejamento ──────────────────────────────────────────────────────────

/// Restaura janela maximizada/minimizada antes de mover (o Windows ignora
/// SetWindowPos em janela maximizada) e devolve o estado atualizado.
fn prepare(w: &WindowInfo) -> WindowInfo {
    if w.maximized || w.minimized {
        show(w.id, SW_RESTORE);
        std::thread::sleep(std::time::Duration::from_millis(60));
        let mons = monitors();
        if let Some(fresh) = describe(w.id, 0, &mons) {
            return WindowInfo { maximized: w.maximized, ..fresh };
        }
    }
    w.clone()
}

fn pick_monitor<'a>(spec: &Value, current: usize, mons: &'a [Monitor]) -> Result<&'a Monitor, WinError> {
    let n = mons.len();
    let idx = match spec {
        Value::Null => current,
        Value::Number(x) => x.as_u64().unwrap_or(0) as usize,
        Value::String(s) => match fold(s).as_str() {
            "next" | "proxima" | "proximo" | "outra" | "outro" => current % n + 1,
            "previous" | "anterior" => (current + n - 2) % n + 1,
            "primary" | "principal" => 1,
            other => other
                .trim_start_matches("tela")
                .trim_start_matches("monitor")
                .trim()
                .parse()
                .map_err(|_| WinError::InvalidParams(format!("monitor '{s}' inválido")))?,
        },
        _ => return Err(WinError::InvalidParams("monitor inválido".into())),
    };
    mons.iter()
        .find(|m| m.index == idx)
        .ok_or_else(|| WinError::NotFound(format!("monitor {idx} (há {n})")))
}

/// Retângulo de uma posição nomeada dentro da área útil.
fn position_rect(pos: &str, work: Rect, current: Rect) -> Result<(Rect, bool), WinError> {
    let Rect { x, y, w, h } = work;
    let (hw, hh, tw) = (w / 2, h / 2, w / 3);
    let r = |x, y, w, h| Rect { x, y, w, h };
    Ok(match fold(pos).replace([' ', '-'], "_").as_str() {
        "maximize" | "maximizar" | "full" | "tela_cheia" => (work, true),
        "left" | "esquerda" => (r(x, y, hw, h), false),
        "right" | "direita" => (r(x + hw, y, w - hw, h), false),
        "top" | "cima" | "topo" => (r(x, y, w, hh), false),
        "bottom" | "baixo" => (r(x, y + hh, w, h - hh), false),
        "top_left" => (r(x, y, hw, hh), false),
        "top_right" => (r(x + hw, y, w - hw, hh), false),
        "bottom_left" => (r(x, y + hh, hw, h - hh), false),
        "bottom_right" => (r(x + hw, y + hh, w - hw, h - hh), false),
        "left_third" => (r(x, y, tw, h), false),
        "center_third" => (r(x + tw, y, tw, h), false),
        "right_third" => (r(x + 2 * tw, y, w - 2 * tw, h), false),
        "left_two_thirds" => (r(x, y, 2 * tw, h), false),
        "right_two_thirds" => (r(x + tw, y, w - tw, h), false),
        "center" | "centro" => {
            let cw = current.w.min(w * 9 / 10);
            let ch = current.h.min(h * 9 / 10);
            (r(x + (w - cw) / 2, y + (h - ch) / 2, cw, ch), false)
        }
        other => return Err(WinError::InvalidParams(format!("posição '{other}' desconhecida"))),
    })
}

/// Mesma posição relativa e tamanho proporcional no outro monitor
/// (como Win+Shift+Seta).
fn carry_over(frame: Rect, from: Rect, to: Rect) -> Rect {
    let sx = to.w as f64 / from.w.max(1) as f64;
    let sy = to.h as f64 / from.h.max(1) as f64;
    let w = ((frame.w as f64 * sx).round() as i32).min(to.w);
    let h = ((frame.h as f64 * sy).round() as i32).min(to.h);
    let x = to.x + (((frame.x - from.x) as f64 * sx).round() as i32).clamp(0, to.w - w);
    let y = to.y + (((frame.y - from.y) as f64 * sy).round() as i32).clamp(0, to.h - h);
    Rect { x, y, w, h }
}

/// Desloca a janela um pouco, sem mudar o tamanho e sem sair da área útil.
/// `amount`: pixels, ou small (padrão, ~6% da tela) / medium (~15%) /
/// large (~30%) — "um pouquinho pro lado" não pode virar um encaixe do Snap.
pub fn nudge_rect(frame: Rect, work: Rect, direction: &str, amount: &Value) -> Result<Rect, WinError> {
    // Passo em pixels: número explícito, ou fração da dimensão da tela.
    let step = |is_x: bool| -> i32 {
        if let Some(px) = amount.as_i64() {
            return px.unsigned_abs().min(i32::MAX as u64) as i32;
        }
        let fraction = match amount.as_str().map(fold).as_deref() {
            Some("medium" | "medio") => 0.15,
            Some("large" | "grande" | "muito") => 0.30,
            _ => 0.06,
        };
        let side = if is_x { work.w } else { work.h };
        (side as f64 * fraction).round() as i32
    };
    let (dx, dy) = match fold(direction).as_str() {
        "left" | "esquerda" => (-step(true), 0),
        "right" | "direita" => (step(true), 0),
        "up" | "cima" => (0, -step(false)),
        "down" | "baixo" => (0, step(false)),
        other => return Err(WinError::InvalidParams(format!("direção '{other}' (use left/right/up/down)"))),
    };
    let w = frame.w.min(work.w);
    let h = frame.h.min(work.h);
    Ok(Rect {
        x: (frame.x + dx).clamp(work.x, work.x + work.w - w),
        y: (frame.y + dy).clamp(work.y, work.y + work.h - h),
        w,
        h,
    })
}

/// `window.move`: `target`, e `nudge` (deslocamento leve) ou `position`
/// (encaixe) e/ou `monitor`.
pub fn plan_move(params: &Value) -> Result<MovePlan, WinError> {
    let target = params["target"].as_str().unwrap_or("active");
    let position = params["position"].as_str().filter(|s| !s.trim().is_empty());
    let nudge = params["nudge"].as_str().filter(|s| !s.trim().is_empty());
    if position.is_none() && nudge.is_none() && params["monitor"].is_null() {
        return Err(WinError::InvalidParams("informe 'nudge', 'position' e/ou 'monitor'".into()));
    }
    let original = find(target)?;
    let mons = monitors();
    if let Some(direction) = nudge {
        let w = prepare(&original);
        let work = mons.iter().find(|m| m.index == original.monitor).map(|m| m.work).unwrap_or(w.frame);
        let to = nudge_rect(w.frame, work, direction, &params["amount"])?;
        return Ok(MovePlan { id: w.id, title: w.title, from: w.frame, to, maximize_after: false });
    }
    let dest = pick_monitor(&params["monitor"], original.monitor, &mons)?;
    let src_work = mons.iter().find(|m| m.index == original.monitor).map(|m| m.work).unwrap_or(dest.work);
    let w = prepare(&original);

    let (to, maximize_after) = match position {
        Some(pos) => position_rect(pos, dest.work, w.frame)?,
        None => (carry_over(w.frame, src_work, dest.work), original.maximized),
    };
    Ok(MovePlan { id: w.id, title: w.title, from: w.frame, to, maximize_after })
}

/// `window.tile`: divide a área útil de um monitor entre várias janelas,
/// como os Snap Layouts do Windows. `targets` (nomes) ou `count` (as N
/// janelas de cima); `layout`: auto | columns | rows | grid | main_left |
/// main_right.
pub fn plan_tile(params: &Value) -> Result<Vec<MovePlan>, WinError> {
    let all = list();
    let mut chosen: Vec<WindowInfo> = Vec::new();
    match params["targets"].as_array().filter(|a| !a.is_empty()) {
        Some(targets) => {
            for t in targets {
                let name = t.as_str().unwrap_or_default();
                let exclude: Vec<isize> = chosen.iter().map(|w| w.id).collect();
                chosen.push(find_in(&all, name, &exclude)?);
            }
        }
        None => {
            let count = params["count"].as_u64().unwrap_or(2).clamp(2, 9) as usize;
            chosen.extend(all.iter().filter(|w| !w.minimized).take(count).cloned());
        }
    }
    if chosen.len() < 2 {
        return Err(WinError::InvalidParams(
            "window.tile divide a tela entre 2+ janelas; para posicionar uma só, use window.move com `position`".into(),
        ));
    }

    let mons = monitors();
    let first_monitor = chosen[0].monitor;
    let dest = pick_monitor(&params["monitor"], first_monitor, &mons)?;
    let layout = params["layout"].as_str().unwrap_or("auto");
    let cells = layout_cells(layout, chosen.len(), dest.work)?;

    Ok(chosen
        .iter()
        .zip(cells)
        .map(|(w, to)| {
            let w = prepare(w);
            MovePlan { id: w.id, title: w.title, from: w.frame, to, maximize_after: false }
        })
        .collect())
}

/// Células de cada layout. "auto" segue o Windows: 2 lado a lado, 3 com
/// uma principal à esquerda, 4 em grade 2×2, mais que isso em grade.
pub fn layout_cells(layout: &str, n: usize, work: Rect) -> Result<Vec<Rect>, WinError> {
    let layout = match fold(layout).replace([' ', '-'], "_").as_str() {
        "auto" => match n {
            2 => "columns",
            3 => "main_left",
            _ => "grid",
        }
        .to_string(),
        other => other.to_string(),
    };
    let split = |start: i32, len: i32, parts: usize, i: usize| {
        let a = start + (len as i64 * i as i64 / parts as i64) as i32;
        let b = start + (len as i64 * (i + 1) as i64 / parts as i64) as i32;
        (a, b - a)
    };
    let Rect { x, y, w, h } = work;
    Ok(match layout.as_str() {
        "columns" | "colunas" => (0..n)
            .map(|i| {
                let (cx, cw) = split(x, w, n, i);
                Rect { x: cx, y, w: cw, h }
            })
            .collect(),
        "rows" | "linhas" => (0..n)
            .map(|i| {
                let (cy, ch) = split(y, h, n, i);
                Rect { x, y: cy, w, h: ch }
            })
            .collect(),
        "main_left" | "main_right" => {
            let (lx, lw) = split(x, w, 2, 0);
            let (rx, rw) = split(x, w, 2, 1);
            let (mx, mw, sx, sw) = if layout == "main_left" { (lx, lw, rx, rw) } else { (rx, rw, lx, lw) };
            let rest = n - 1;
            std::iter::once(Rect { x: mx, y, w: mw, h })
                .chain((0..rest).map(|i| {
                    let (cy, ch) = split(y, h, rest, i);
                    Rect { x: sx, y: cy, w: sw, h: ch }
                }))
                .collect()
        }
        "grid" | "grade" => {
            let cols = (n as f64).sqrt().ceil() as usize;
            let rows = n.div_ceil(cols);
            (0..n)
                .map(|i| {
                    let (row, col) = (i / cols, i % cols);
                    // A última linha incompleta se estica para não deixar buraco.
                    let in_row = if row == rows - 1 { n - row * cols } else { cols };
                    let (cx, cw) = split(x, w, in_row, col);
                    let (cy, ch) = split(y, h, rows, row);
                    Rect { x: cx, y: cy, w: cw, h: ch }
                })
                .collect()
        }
        other => return Err(WinError::InvalidParams(format!("layout '{other}' desconhecido"))),
    })
}

// ── handlers (sem animação) ───────────────────────────────────────────────

pub fn handle(action: &str, params: &Value) -> Result<Value, WinError> {
    let target = params["target"].as_str().unwrap_or("active");
    match action {
        "window.list" => {
            let windows: Vec<Value> = list()
                .into_iter()
                .take(25)
                .map(|w| {
                    json!({
                        "id": w.id, "title": w.title, "app": w.app, "monitor": w.monitor,
                        "maximized": w.maximized, "minimized": w.minimized, "admin": w.elevated,
                    })
                })
                .collect();
            let monitors: Vec<Value> = monitors()
                .into_iter()
                .map(|m| json!({ "index": m.index, "primary": m.primary, "width": m.work.w, "height": m.work.h }))
                .collect();
            Ok(json!({ "monitors": monitors, "windows": windows }))
        }
        "window.focus" => {
            let w = find(target)?;
            focus(w.id)?;
            Ok(json!({ "focused": w.title }))
        }
        "window.minimize" | "window.maximize" | "window.restore" => {
            let w = find(target)?;
            set_state(w.id, action.trim_start_matches("window."))?;
            Ok(json!({ "window": w.title, "done": action }))
        }
        "window.close" => {
            let w = find(target)?;
            close(w.id)?;
            Ok(json!({ "closed": w.title }))
        }
        "window.move" => {
            let plan = plan_move(params)?;
            apply_now(std::slice::from_ref(&plan))?;
            Ok(json!({ "moved": plan.title, "to": plan.to }))
        }
        "window.tile" => {
            let plans = plan_tile(params)?;
            apply_now(&plans)?;
            Ok(json!({ "tiled": plans.iter().map(|p| &p.title).collect::<Vec<_>>() }))
        }
        other => Err(WinError::NotFound(format!("ação '{other}'"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect { x: 0, y: 0, w: 1920, h: 1040 };

    fn covers_exactly(cells: &[Rect], work: Rect) {
        let area: i64 = cells.iter().map(|c| c.w as i64 * c.h as i64).sum();
        assert_eq!(area, work.w as i64 * work.h as i64, "células devem cobrir a área sem sobra");
        for c in cells {
            assert!(c.x >= work.x && c.y >= work.y && c.x + c.w <= work.x + work.w && c.y + c.h <= work.y + work.h);
        }
    }

    #[test]
    fn auto_layout_matches_windows_snap() {
        let two = layout_cells("auto", 2, WORK).unwrap();
        assert_eq!(two, vec![Rect { x: 0, y: 0, w: 960, h: 1040 }, Rect { x: 960, y: 0, w: 960, h: 1040 }]);

        let three = layout_cells("auto", 3, WORK).unwrap();
        assert_eq!(three[0], Rect { x: 0, y: 0, w: 960, h: 1040 });
        assert_eq!(three[1], Rect { x: 960, y: 0, w: 960, h: 520 });
        assert_eq!(three[2], Rect { x: 960, y: 520, w: 960, h: 520 });

        let four = layout_cells("auto", 4, WORK).unwrap();
        assert_eq!(four[3], Rect { x: 960, y: 520, w: 960, h: 520 });

        for n in 2..=9 {
            covers_exactly(&layout_cells("auto", n, WORK).unwrap(), WORK);
        }
    }

    #[test]
    fn other_layouts_cover_work_area() {
        for layout in ["columns", "rows", "grid", "main_left", "main_right"] {
            for n in 2..=6 {
                covers_exactly(&layout_cells(layout, n, WORK).unwrap(), WORK);
            }
        }
        // Monitor com origem fora de (0,0) e largura ímpar.
        let odd = Rect { x: -1921, y: 120, w: 1921, h: 1001 };
        covers_exactly(&layout_cells("columns", 3, odd).unwrap(), odd);
        assert!(layout_cells("hexagon", 3, WORK).is_err());
    }

    #[test]
    fn named_positions() {
        let cur = Rect { x: 100, y: 100, w: 800, h: 600 };
        assert_eq!(position_rect("esquerda", WORK, cur).unwrap().0, Rect { x: 0, y: 0, w: 960, h: 1040 });
        assert_eq!(position_rect("top-right", WORK, cur).unwrap().0, Rect { x: 960, y: 0, w: 960, h: 520 });
        assert!(position_rect("maximize", WORK, cur).unwrap().1);
        assert_eq!(position_rect("centro", WORK, cur).unwrap().0, Rect { x: 560, y: 220, w: 800, h: 600 });
    }

    #[test]
    fn nudge_moves_a_little_and_stays_on_screen() {
        let frame = Rect { x: 500, y: 200, w: 800, h: 600 };
        // Pouquinho = ~6% da largura (1920 → 115 px), tamanho intacto.
        assert_eq!(
            nudge_rect(frame, WORK, "direita", &Value::Null).unwrap(),
            Rect { x: 615, y: 200, w: 800, h: 600 }
        );
        assert_eq!(nudge_rect(frame, WORK, "left", &json!(50)).unwrap().x, 450);
        assert_eq!(nudge_rect(frame, WORK, "up", &json!("medio")).unwrap().y, 44);
        // Nunca sai da área útil.
        assert_eq!(nudge_rect(frame, WORK, "right", &json!("muito")).unwrap().x, 1076);
        assert_eq!(nudge_rect(frame, WORK, "right", &json!(5000)).unwrap().x, 1120);
        assert_eq!(nudge_rect(frame, WORK, "left", &json!(5000)).unwrap().x, 0);
        assert!(nudge_rect(frame, WORK, "diagonal", &Value::Null).is_err());
    }

    #[test]
    fn carry_over_keeps_relative_place() {
        let left = Rect { x: 0, y: 0, w: 1920, h: 1040 };
        let right = Rect { x: 1920, y: 0, w: 3440, h: 1400 };
        let r = carry_over(Rect { x: 960, y: 0, w: 960, h: 1040 }, left, right);
        assert_eq!(r, Rect { x: 1920 + 1720, y: 0, w: 1720, h: 1400 });
    }

    #[test]
    fn find_prefers_app_name_then_title_then_z_order() {
        let w = |id, title: &str, app: &str| WindowInfo {
            id,
            title: title.into(),
            app: app.into(),
            monitor: 1,
            frame: WORK,
            maximized: false,
            minimized: false,
            elevated: false,
        };
        let wins = vec![
            w(1, "Notas sobre chrome", "notepad"),
            w(2, "YouTube - Google Chrome", "chrome"),
            w(3, "Docs - Google Chrome", "chrome"),
        ];
        assert_eq!(find_in(&wins, "chrome", &[]).unwrap().id, 2);
        assert_eq!(find_in(&wins, "chrome", &[2]).unwrap().id, 3);
        assert_eq!(find_in(&wins, "youtube", &[]).unwrap().id, 2);
        assert_eq!(find_in(&wins, "active", &[]).unwrap().id, 1);
        assert_eq!(find_in(&wins, "id:3", &[]).unwrap().id, 3);
        assert!(find_in(&wins, "spotify", &[]).is_err());
    }

    #[test]
    fn monitor_specs() {
        let m = |i, x| Monitor { index: i, primary: i == 1, work: Rect { x, y: 0, w: 100, h: 100 }, handle: i as isize };
        let mons = vec![m(1, 0), m(2, 100), m(3, 200)];
        assert_eq!(pick_monitor(&json!(2), 1, &mons).unwrap().index, 2);
        assert_eq!(pick_monitor(&json!("tela 3"), 1, &mons).unwrap().index, 3);
        assert_eq!(pick_monitor(&json!("next"), 3, &mons).unwrap().index, 1);
        assert_eq!(pick_monitor(&json!("anterior"), 1, &mons).unwrap().index, 3);
        assert_eq!(pick_monitor(&Value::Null, 2, &mons).unwrap().index, 2);
        assert!(pick_monitor(&json!(7), 1, &mons).is_err());
    }
}
