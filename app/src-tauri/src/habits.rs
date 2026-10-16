//! Aprender rotinas: registra quando cada programa aparece pela primeira vez
//! no dia (só o nome do processo, no SQLite local) e, se o mesmo conjunto de
//! programas abre nos primeiros 30 minutos do dia em pelo menos 3 dos
//! últimos 5 dias de uso, sugere criar um modo que abre tudo de uma vez —
//! num cartão de escolha (sim/não). Uma sugestão por dia, no máximo; um
//! "não" vale para sempre para aquele conjunto.

use crate::AppState;
use chrono::{Duration, NaiveDateTime, Timelike};
use lilith_memory::{Launch, Memory, ModeStep};
use serde_json::json;
use std::collections::{BTreeMap, HashSet};
use tauri::{AppHandle, Emitter, Manager};

/// Janela "logo que liga o PC": minutos depois da primeira atividade do dia.
pub const EARLY_MINUTES: u32 = 30;
/// Quantos dias de uso olhar, e em quantos o conjunto precisa se repetir.
pub const LOOK_BACK_DAYS: usize = 5;
pub const MIN_DAYS: usize = 3;

/// Processos que não são "abrir um programa" (shell, partes do Windows).
const IGNORED: &[&str] = &[
    "explorer",
    "applicationframehost",
    "textinputhost",
    "shellexperiencehost",
    "searchhost",
    "startmenuexperiencehost",
    "lockapp",
    "systemsettings",
    "taskmgr",
    "lilith-companion",
];

const CHECKED: &str = "routine_suggest_checked";
const DECLINED: &str = "routine_suggest_declined";

fn key(app: &str) -> String {
    app.trim().to_lowercase()
}

/// Programas abertos agora → aberturas novas no registro.
#[derive(Default)]
pub struct Tracker {
    seen: HashSet<String>,
}

impl Tracker {
    /// Uma olhada: registra os programas que apareceram desde a anterior e,
    /// uma vez por dia (passada a janela do começo do dia), vê se há rotina.
    pub async fn tick(&mut self, app: &AppHandle, store: &std::sync::Arc<Memory>) {
        let Ok(windows) = tokio::task::spawn_blocking(lilith_win::window::list).await else { return };
        let now = chrono::Local::now().naive_local();
        let apps: Vec<String> = windows.into_iter().map(|w| w.app).collect();
        self.observe(store, &apps, now);
        if let Some(apps) = due_suggestion(store, now) {
            let (app, store) = (app.clone(), store.clone());
            tauri::async_runtime::spawn(async move { offer(app, store, apps).await });
        }
    }

    fn observe(&mut self, store: &Memory, apps: &[String], now: NaiveDateTime) {
        let day = now.format("%Y-%m-%d").to_string();
        let minute = now.hour() * 60 + now.minute();
        let mut current = HashSet::new();
        for a in apps {
            let k = key(a);
            if k.is_empty() || IGNORED.contains(&k.as_str()) || !current.insert(k.clone()) {
                continue;
            }
            if !self.seen.contains(&k) {
                store.log_launch(&day, minute, a.trim());
            }
        }
        self.seen = current;
    }
}

/// O conjunto de programas da rotina da manhã, se houver: apps que abrem
/// nos primeiros `EARLY_MINUTES` do dia em pelo menos `MIN_DAYS` dos últimos
/// `LOOK_BACK_DAYS` dias de uso (todos juntos nos mesmos dias). Precisa de
/// 2+ programas para valer um modo.
pub fn suggest(launches: &[Launch]) -> Option<Vec<String>> {
    let mut days: BTreeMap<&str, Vec<&Launch>> = BTreeMap::new();
    for l in launches {
        days.entry(l.day.as_str()).or_default().push(l);
    }
    let recent: Vec<HashSet<String>> = days
        .values()
        .rev()
        .take(LOOK_BACK_DAYS)
        .map(|ls| {
            let first = ls.iter().map(|l| l.minute).min().unwrap_or(0);
            ls.iter().filter(|l| l.minute <= first + EARLY_MINUTES).map(|l| key(&l.app)).collect()
        })
        .collect();
    if recent.len() < MIN_DAYS {
        return None;
    }
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for set in &recent {
        for a in set {
            *counts.entry(a.clone()).or_default() += 1;
        }
    }
    let mut candidates: Vec<(String, usize)> = counts.into_iter().filter(|(_, n)| *n >= MIN_DAYS).collect();
    candidates.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    // Tira o menos frequente até o conjunto inteiro se repetir em MIN_DAYS dias.
    while candidates.len() >= 2 {
        let together = recent.iter().filter(|set| candidates.iter().all(|(a, _)| set.contains(a))).count();
        if together >= MIN_DAYS {
            let mut apps: Vec<String> = candidates.into_iter().map(|(a, _)| a).collect();
            apps.sort();
            // Nome como o processo se chama (maiúsculas do registro).
            return Some(
                apps.into_iter()
                    .map(|k| launches.iter().rev().find(|l| key(&l.app) == k).map(|l| l.app.clone()).unwrap_or(k))
                    .collect(),
            );
        }
        candidates.pop();
    }
    None
}

/// Chave estável de um conjunto (para lembrar um "não").
fn set_key(apps: &[String]) -> String {
    let mut k: Vec<String> = apps.iter().map(|a| key(a)).collect();
    k.sort();
    k.join("|")
}

/// Já existe um modo que abre exatamente esses programas?
fn covered_by_mode(store: &Memory, apps: &[String]) -> bool {
    let wanted: HashSet<String> = apps.iter().filter_map(|a| lilith_win::layout::launch_name(a)).map(|a| key(&a)).collect();
    store.modes().iter().any(|m| {
        let opens: HashSet<String> =
            m.steps.iter().filter(|s| s.action == "app.open").filter_map(|s| s.params["app"].as_str().map(key)).collect();
        wanted.is_subset(&opens)
    })
}

/// Hora de sugerir? Uma vez por dia, depois da janela do começo do dia,
/// conjunto novo (sem "não" e sem modo que já o cubra).
fn due_suggestion(store: &Memory, now: NaiveDateTime) -> Option<Vec<String>> {
    let today = now.format("%Y-%m-%d").to_string();
    if store.setting(CHECKED).as_deref() == Some(today.as_str()) {
        return None;
    }
    let since = (now.date() - Duration::days(30)).format("%Y-%m-%d").to_string();
    let launches = store.launches_since(&since);
    let first_today = launches.iter().filter(|l| l.day == today).map(|l| l.minute).min()?;
    if now.hour() * 60 + now.minute() <= first_today + EARLY_MINUTES {
        return None;
    }
    store.set_setting(CHECKED, &today);
    store.prune_launches(&since);
    let apps = suggest(&launches)?;
    let declined: Vec<String> = store.setting(DECLINED).and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default();
    if declined.contains(&set_key(&apps)) || covered_by_mode(store, &apps) {
        return None;
    }
    Some(apps)
}

/// Nome livre para o modo novo: "manhã", senão "rotina", "rotina 2"…
fn free_name(store: &Memory) -> String {
    std::iter::once("manhã".to_string())
        .chain(std::iter::once("rotina".to_string()))
        .chain((2..50).map(|n| format!("rotina {n}")))
        .find(|n| store.mode(n).is_none())
        .unwrap_or_else(|| "rotina nova".into())
}

/// Passos do modo sugerido: abrir cada programa.
pub fn steps_for(apps: &[String]) -> Vec<ModeStep> {
    apps.iter()
        .filter_map(|a| lilith_win::layout::launch_name(a))
        .map(|name| ModeStep { action: "app.open".into(), params: json!({ "app": name }), say: format!("abre o {}", name.to_lowercase()) })
        .collect()
}

fn list_pt(items: &[String]) -> String {
    match items.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} e {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// Cartão sim/não no chat; "sim" cria o modo, "não" não pergunta mais.
async fn offer(app: AppHandle, store: std::sync::Arc<Memory>, apps: Vec<String>) {
    let steps = steps_for(&apps);
    if steps.len() < 2 {
        return;
    }
    let name = free_name(&store);
    let shown: Vec<String> = steps.iter().filter_map(|s| s.params["app"].as_str().map(str::to_string)).collect();
    let title = format!(
        "Percebi que, quase todo dia, logo que você liga o PC, abre {}. Quer que eu crie o modo «{name}» para abrir tudo de uma vez?",
        list_pt(&shown)
    );
    const YES: &str = "Sim, criar o modo";
    const NO: &str = "Não, obrigada";
    let choices = app.state::<AppState>().choices.clone();
    match choices.ask(&app, "rotinas", &title, vec![YES.into(), NO.into()]).await.as_deref() {
        Some(YES) => {
            if store.save_mode(&name, &steps) {
                let _ = app.emit("lilith://routines", ());
                crate::alerts::notice(&app, &format!("Criei o modo «{name}». É só dizer \"modo {name}\" (ou editar nas Configurações)."), "sparkles");
            }
        }
        Some(_) => {
            let mut declined: Vec<String> = store.setting(DECLINED).and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default();
            declined.push(set_key(&apps));
            declined.truncate(50);
            store.set_setting(DECLINED, &serde_json::to_string(&declined).unwrap_or_default());
        }
        // Sem resposta: tenta de novo outro dia.
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(day: &str, minute: u32, app: &str) -> Launch {
        Launch { day: day.into(), minute, app: app.into() }
    }

    fn week(apps_per_day: &[(&str, &[(&str, u32)])]) -> Vec<Launch> {
        apps_per_day.iter().flat_map(|(d, apps)| apps.iter().map(move |(a, m)| l(d, *m, a))).collect()
    }

    #[test]
    fn same_morning_apps_on_three_of_five_days_become_a_suggestion() {
        let launches = week(&[
            ("2026-10-12", &[("Code", 480), ("Discord", 485), ("Spotify", 490)]),
            ("2026-10-13", &[("Discord", 540), ("Code", 545), ("chrome", 700)]),
            ("2026-10-14", &[("Code", 470), ("Discord", 471), ("Spotify", 600)]),
            ("2026-10-15", &[("steam", 1200)]),
            ("2026-10-16", &[("Code", 500), ("Discord", 520)]),
        ]);
        assert_eq!(suggest(&launches), Some(vec!["Code".to_string(), "Discord".to_string()]));
    }

    #[test]
    fn no_suggestion_without_a_repeated_set() {
        // Só 2 dias com o mesmo conjunto.
        let launches = week(&[
            ("2026-10-12", &[("Code", 480), ("Discord", 485)]),
            ("2026-10-13", &[("Code", 480), ("Discord", 485)]),
            ("2026-10-14", &[("Spotify", 480), ("chrome", 485)]),
        ]);
        assert_eq!(suggest(&launches), None);
        // Um programa só não vira modo.
        let launches = week(&[
            ("2026-10-12", &[("Code", 480)]),
            ("2026-10-13", &[("Code", 480)]),
            ("2026-10-14", &[("Code", 480)]),
        ]);
        assert_eq!(suggest(&launches), None);
        // Abriu, mas não no começo do dia.
        let launches = week(&[
            ("2026-10-12", &[("steam", 400), ("Code", 480), ("Discord", 485)]),
            ("2026-10-13", &[("steam", 400), ("Code", 480), ("Discord", 485)]),
            ("2026-10-14", &[("steam", 400), ("Code", 480), ("Discord", 485)]),
        ]);
        assert_eq!(suggest(&launches), None, "Code/Discord vieram 80 min depois da primeira atividade");
        // Só os últimos 5 dias de uso contam.
        let old = week(&[
            ("2026-10-01", &[("Code", 480), ("Discord", 485)]),
            ("2026-10-02", &[("Code", 480), ("Discord", 485)]),
            ("2026-10-03", &[("Code", 480), ("Discord", 485)]),
            ("2026-10-10", &[("x", 1)]),
            ("2026-10-11", &[("x", 1)]),
            ("2026-10-12", &[("x", 1)]),
            ("2026-10-13", &[("x", 1)]),
            ("2026-10-14", &[("x", 1)]),
        ]);
        assert_eq!(suggest(&old), None);
    }

    #[test]
    fn observing_logs_each_app_once_per_day_and_skips_the_shell() {
        let store = Memory::in_memory().unwrap();
        let mut t = Tracker::default();
        let at = |h: u32, m: u32| chrono::NaiveDate::from_ymd_opt(2026, 10, 16).unwrap().and_hms_opt(h, m, 0).unwrap();
        t.observe(&store, &["explorer".into(), "Code".into(), "Code".into()], at(8, 0));
        t.observe(&store, &["Code".into(), "Discord".into()], at(8, 5));
        t.observe(&store, &["Discord".into()], at(9, 0));
        t.observe(&store, &["Code".into(), "Discord".into()], at(10, 0));
        let logged = store.launches_since("2026-10-16");
        let got: Vec<(&str, u32)> = logged.iter().map(|l| (l.app.as_str(), l.minute)).collect();
        assert_eq!(got, vec![("Code", 480), ("Discord", 485)], "reabrir no mesmo dia não conta");
    }

    #[test]
    fn suggestion_is_checked_once_a_day_and_respects_existing_modes() {
        let store = Memory::in_memory().unwrap();
        for (d, base) in [("2026-10-13", 480), ("2026-10-14", 470), ("2026-10-15", 490), ("2026-10-16", 500)] {
            store.log_launch(d, base, "Code");
            store.log_launch(d, base + 2, "Discord");
        }
        let at = |h: u32, m: u32| chrono::NaiveDate::from_ymd_opt(2026, 10, 16).unwrap().and_hms_opt(h, m, 0).unwrap();
        assert_eq!(due_suggestion(&store, at(8, 25)), None, "ainda na janela do começo do dia");
        assert_eq!(due_suggestion(&store, at(9, 0)), Some(vec!["Code".to_string(), "Discord".to_string()]));
        assert_eq!(due_suggestion(&store, at(10, 0)), None, "uma vez por dia");
        // Com um modo que já abre os dois, não sugere.
        store.set_setting(CHECKED, "");
        assert!(store.save_mode("manhã", &steps_for(&["Code".into(), "Discord".into()])));
        assert_eq!(due_suggestion(&store, at(11, 0)), None);
        assert_eq!(free_name(&store), "rotina");
    }
}
