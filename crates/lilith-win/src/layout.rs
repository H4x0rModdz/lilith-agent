//! Layouts de janelas: "salva esse layout como trabalho" fotografa onde cada
//! janela está (app, título, monitor, moldura, maximizada) e "restaura o
//! layout trabalho" planeja como voltar a esse arranjo.
//!
//! Tudo aqui é cálculo puro sobre listas de janelas e monitores — nada é
//! movido. Quem aplica o plano (abrindo os apps que faltam, arrastando as
//! janelas com o cursor da Lilith) é o app, passando pelos guardrails.

use crate::paths::fold;
use crate::window::{Rect, WindowInfo};
use serde::{Deserialize, Serialize};

/// Uma janela do layout salvo.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SavedWindow {
    /// Nome do processo ("Code", "chrome").
    pub app: String,
    /// Título na hora de salvar: desempata entre janelas do mesmo app.
    pub title: String,
    pub monitor: usize,
    /// Moldura relativa ao canto da área útil do monitor.
    pub rel: Rect,
    /// Tamanho da área útil quando foi salvo (para escalar se a tela mudou).
    pub work_w: i32,
    pub work_h: i32,
    pub maximized: bool,
}

/// Área útil de cada monitor: (número, retângulo). 1 = principal.
pub type Screens = [(usize, Rect)];

/// Teto de janelas num layout (o resto é ruído: janelas pequenas, pop-ups).
pub const MAX_WINDOWS: usize = 20;

/// Processos que não são "apps de usuário" para um layout.
const IGNORED: &[&str] = &["textinputhost", "shellexperiencehost", "searchhost", "startmenuexperiencehost", "lockapp"];

/// Monitores reais agora.
pub fn screens() -> Vec<(usize, Rect)> {
    crate::window::monitors().into_iter().map(|m| (m.index, m.work)).collect()
}

fn work_of(screens: &Screens, monitor: usize) -> Option<Rect> {
    screens.iter().find(|(i, _)| *i == monitor).map(|(_, r)| *r)
}

/// Fotografa as janelas visíveis (as minimizadas ficam de fora: o layout é
/// o que está na tela). Na ordem Z, a de cima primeiro.
pub fn capture(windows: &[WindowInfo], screens: &Screens) -> Vec<SavedWindow> {
    windows
        .iter()
        .filter(|w| !w.minimized && !w.app.trim().is_empty() && !IGNORED.contains(&fold(&w.app).as_str()))
        .filter_map(|w| {
            let work = work_of(screens, w.monitor).or_else(|| screens.first().map(|(_, r)| *r))?;
            Some(SavedWindow {
                app: w.app.clone(),
                title: w.title.clone(),
                monitor: w.monitor,
                rel: Rect { x: w.frame.x - work.x, y: w.frame.y - work.y, w: w.frame.w, h: w.frame.h },
                work_w: work.w,
                work_h: work.h,
                maximized: w.maximized,
            })
        })
        .take(MAX_WINDOWS)
        .collect()
}

/// Onde a janela deve ficar agora: no mesmo monitor (ou no principal, se
/// ele não existe mais), escalada se a área útil mudou de tamanho. Na mesma
/// tela, volta exatamente para onde estava (mesmo um pouco para fora da
/// borda, como o usuário deixou), mas com a barra de título sempre
/// alcançável; numa tela diferente, cabe inteira. Devolve (monitor, moldura).
pub fn target_frame(w: &SavedWindow, screens: &Screens) -> Option<(usize, Rect)> {
    let (monitor, work) = match work_of(screens, w.monitor) {
        Some(work) => (w.monitor, work),
        None => *screens.iter().find(|(i, _)| *i == 1).or(screens.first())?,
    };
    let same_screen = monitor == w.monitor && work.w == w.work_w && work.h == w.work_h;
    let scale = |v: i32, from: i32, to: i32| if from > 0 && from != to { (v as i64 * to as i64 / from as i64) as i32 } else { v };
    let mut r = Rect {
        x: scale(w.rel.x, w.work_w, work.w),
        y: scale(w.rel.y, w.work_h, work.h),
        w: scale(w.rel.w, w.work_w, work.w),
        h: scale(w.rel.h, w.work_h, work.h),
    };
    if same_screen {
        // Pelo menos 100 px da janela e a barra de título visíveis.
        r.w = r.w.max(100);
        r.h = r.h.max(40);
        r.x = r.x.clamp(100 - r.w, (work.w - 100).max(0));
        r.y = r.y.clamp(0, (work.h - 40).max(0));
    } else {
        r.w = r.w.clamp(200.min(work.w), work.w);
        r.h = r.h.clamp(120.min(work.h), work.h);
        r.x = r.x.clamp(0, work.w - r.w);
        r.y = r.y.clamp(0, work.h - r.h);
    }
    Some((monitor, Rect { x: work.x + r.x, y: work.y + r.y, ..r }))
}

fn words(s: &str) -> Vec<String> {
    fold(s).split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() > 1).map(str::to_string).collect()
}

/// Quantas palavras do título salvo a janela ainda tem (desempate).
fn title_score(saved: &str, now: &str) -> usize {
    let now = words(now);
    words(saved).iter().filter(|w| now.contains(w)).count()
}

/// Para cada janela salva, a janela aberta que corresponde (índice em
/// `windows`): mesmo app e o título mais parecido. Cada janela aberta é
/// usada uma vez só; na dúvida, a de cima.
pub fn match_windows(saved: &[SavedWindow], windows: &[WindowInfo]) -> Vec<Option<usize>> {
    let mut used = vec![false; windows.len()];
    // Primeiro as salvas com título mais distintivo (mais palavras em comum
    // com alguma candidata) escolhem; assim "projeto A" não rouba a janela do
    // "projeto B" só por estar antes na lista.
    let mut order: Vec<usize> = (0..saved.len()).collect();
    let best = |s: &SavedWindow| {
        windows.iter().filter(|w| fold(&w.app) == fold(&s.app)).map(|w| title_score(&s.title, &w.title)).max().unwrap_or(0)
    };
    order.sort_by_key(|&i| std::cmp::Reverse(best(&saved[i])));
    let mut out = vec![None; saved.len()];
    for i in order {
        let s = &saved[i];
        let pick = windows
            .iter()
            .enumerate()
            .filter(|(j, w)| !used[*j] && fold(&w.app) == fold(&s.app))
            .max_by_key(|(j, w)| (title_score(&s.title, &w.title), std::cmp::Reverse(*j)))
            .map(|(j, _)| j);
        if let Some(j) = pick {
            used[j] = true;
            out[i] = Some(j);
        }
    }
    out
}

/// Uma janela a levar para o lugar.
#[derive(Debug, Clone, Serialize)]
pub struct RestoreMove {
    pub window: WindowInfo,
    pub to: Rect,
    pub maximize: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct RestorePlan {
    pub moves: Vec<RestoreMove>,
    /// Já estavam no lugar.
    pub in_place: Vec<String>,
    /// Apps do layout sem nenhuma janela aberta (sem repetir).
    pub missing: Vec<String>,
    /// Janelas de administrador: um app comum não as move (ficam como estão).
    pub skipped_admin: Vec<String>,
}

/// Distância tolerada para "já está no lugar" (bordas e arredondamentos).
const SLACK: i32 = 8;

fn close_enough(a: Rect, b: Rect) -> bool {
    (a.x - b.x).abs() <= SLACK && (a.y - b.y).abs() <= SLACK && (a.w - b.w).abs() <= SLACK && (a.h - b.h).abs() <= SLACK
}

/// O plano para voltar ao layout `saved` a partir das janelas abertas agora.
pub fn plan_restore(saved: &[SavedWindow], windows: &[WindowInfo], screens: &Screens) -> RestorePlan {
    let mut plan = RestorePlan::default();
    for (s, m) in saved.iter().zip(match_windows(saved, windows)) {
        let Some(j) = m else {
            if !plan.missing.iter().any(|a| fold(a) == fold(&s.app)) {
                plan.missing.push(s.app.clone());
            }
            continue;
        };
        let w = &windows[j];
        if w.elevated {
            plan.skipped_admin.push(w.title.clone());
            continue;
        }
        let Some((monitor, to)) = target_frame(s, screens) else { continue };
        let placed = !w.minimized
            && if s.maximized { w.maximized && w.monitor == monitor } else { !w.maximized && close_enough(w.frame, to) };
        if placed {
            plan.in_place.push(w.title.clone());
        } else {
            plan.moves.push(RestoreMove { window: w.clone(), to, maximize: s.maximized });
        }
    }
    plan
}

/// Nome para reabrir um app pelo `app.open` a partir do nome do processo.
/// `None` = não dá para reabrir de forma confiável (Explorer, apps hospedados).
pub fn launch_name(process: &str) -> Option<String> {
    let p = fold(process);
    Some(
        match p.as_str() {
            "explorer" | "applicationframehost" | "" => return None,
            "code" => "Visual Studio Code",
            "windowsterminal" => "terminal",
            "msedge" => "Microsoft Edge",
            "notepad" => "bloco de notas",
            "systemsettings" => "configurações",
            "taskmgr" => return None,
            _ => process.trim(),
        }
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(id: isize, app: &str, title: &str, monitor: usize, frame: Rect) -> WindowInfo {
        WindowInfo {
            id,
            title: title.into(),
            app: app.into(),
            monitor,
            frame,
            maximized: false,
            minimized: false,
            elevated: false,
        }
    }

    fn r(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    fn two_screens() -> Vec<(usize, Rect)> {
        vec![(1, r(0, 0, 1920, 1040)), (2, r(1920, 0, 2560, 1400))]
    }

    #[test]
    fn capture_keeps_visible_windows_relative_to_their_monitor() {
        let mut min = win(3, "Spotify", "Spotify", 1, r(-32000, -32000, 160, 28));
        min.minimized = true;
        let ws = vec![
            win(1, "Code", "main.rs - projeto - Visual Studio Code", 2, r(1920, 0, 1280, 1400)),
            win(2, "chrome", "Docs - Google Chrome", 1, r(0, 0, 960, 1040)),
            min,
        ];
        let saved = capture(&ws, &two_screens());
        assert_eq!(saved.len(), 2, "minimizada fica de fora");
        assert_eq!(saved[0].rel, r(0, 0, 1280, 1400));
        assert_eq!((saved[0].monitor, saved[0].work_w), (2, 2560));
    }

    #[test]
    fn target_frame_scales_and_falls_back_to_primary() {
        let s = SavedWindow {
            app: "Code".into(),
            title: String::new(),
            monitor: 2,
            rel: r(1280, 0, 1280, 1400),
            work_w: 2560,
            work_h: 1400,
            maximized: false,
        };
        assert_eq!(target_frame(&s, &two_screens()), Some((2, r(3200, 0, 1280, 1400))));
        // O monitor 2 sumiu: vai para o principal, na mesma proporção.
        let only_one = vec![(1, r(0, 0, 1920, 1040))];
        assert_eq!(target_frame(&s, &only_one), Some((1, r(960, 0, 960, 1040))));
        // Nunca some da tela: a barra de título fica alcançável.
        let off = SavedWindow { rel: r(5000, 5000, 800, 600), ..s.clone() };
        let (_, t) = target_frame(&off, &two_screens()).unwrap();
        assert!(t.x + 100 <= 1920 + 2560 && t.y + 40 <= 1400);
        // Um pouco para fora da borda, como o usuário deixou: volta igual.
        let edge = SavedWindow { rel: r(-30, 0, 900, 1500), ..s.clone() };
        assert_eq!(target_frame(&edge, &two_screens()), Some((2, r(1890, 0, 900, 1500))));
        // Em outra tela, cabe inteira.
        assert_eq!(target_frame(&edge, &only_one).map(|(_, f)| f.x >= 0 && f.h <= 1040), Some(true));
        assert_eq!(target_frame(&s, &[]), None);
    }

    #[test]
    fn windows_match_by_app_and_title() {
        let saved = vec![
            SavedWindow { app: "Code".into(), title: "a.rs - alpha - Visual Studio Code".into(), monitor: 1, rel: r(0, 0, 10, 10), work_w: 0, work_h: 0, maximized: false },
            SavedWindow { app: "Code".into(), title: "b.rs - beta - Visual Studio Code".into(), monitor: 1, rel: r(0, 0, 10, 10), work_w: 0, work_h: 0, maximized: false },
            SavedWindow { app: "Spotify".into(), title: "Spotify".into(), monitor: 1, rel: r(0, 0, 10, 10), work_w: 0, work_h: 0, maximized: false },
        ];
        let open = vec![
            win(10, "Code", "c.rs - beta - Visual Studio Code", 1, r(0, 0, 10, 10)),
            win(11, "code", "x.rs - alpha - Visual Studio Code", 1, r(0, 0, 10, 10)),
            win(12, "chrome", "Spotify - Web Player", 1, r(0, 0, 10, 10)),
        ];
        assert_eq!(match_windows(&saved, &open), vec![Some(1), Some(0), None], "título de aba não engana o app");
    }

    #[test]
    fn restore_plan_moves_only_what_is_out_of_place() {
        let screens = two_screens();
        let ws = vec![
            win(1, "Code", "Code", 2, r(1920, 0, 1280, 1400)),
            win(2, "chrome", "Chrome", 1, r(0, 0, 960, 1040)),
        ];
        let mut saved = capture(&ws, &screens);
        saved.push(SavedWindow { app: "Spotify".into(), title: "Spotify".into(), monitor: 1, rel: r(960, 0, 960, 1040), work_w: 1920, work_h: 1040, maximized: false });
        saved[1].maximized = true;
        // Agora: o Code foi para o monitor 1; o Chrome está igual (mas devia maximizar).
        let mut now = ws.clone();
        now[0].monitor = 1;
        now[0].frame = r(100, 100, 800, 600);
        let mut admin = win(3, "Spotify", "Spotify", 1, r(0, 0, 10, 10));
        admin.elevated = true;
        let plan = plan_restore(&saved, &now, &screens);
        assert_eq!(plan.moves.len(), 2);
        assert_eq!(plan.moves[0].to, r(1920, 0, 1280, 1400));
        assert!(plan.moves[1].maximize);
        assert_eq!(plan.missing, vec!["Spotify".to_string()]);
        // Tudo no lugar: nada a mover.
        let plan = plan_restore(&capture(&ws, &screens), &ws, &screens);
        assert!(plan.moves.is_empty() && plan.in_place.len() == 2);
        // Janela de administrador fica como está.
        now.push(admin);
        let plan = plan_restore(&saved, &now, &screens);
        assert_eq!(plan.skipped_admin, vec!["Spotify".to_string()]);
        assert!(plan.missing.is_empty());
    }

    #[test]
    fn launch_names_for_common_processes() {
        assert_eq!(launch_name("Code").as_deref(), Some("Visual Studio Code"));
        assert_eq!(launch_name("Spotify").as_deref(), Some("Spotify"));
        assert_eq!(launch_name("explorer"), None);
    }
}
