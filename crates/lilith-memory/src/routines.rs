//! Rotinas do usuário, no mesmo SQLite:
//!
//! - **modos** (`modes`): sequências nomeadas de ações ("modo foco" =
//!   minimizar o monitor 1, mutar, abrir o VS Code). Cada passo é uma chamada
//!   do catálogo já validada pelo código antes de salvar.
//! - **layouts** (`layouts`): fotografia das janelas (app, título, monitor,
//!   posição) para restaurar depois. O formato do JSON é do `lilith-win`.
//! - **programas abertos** (`app_launches`): o primeiro horário de cada
//!   programa em cada dia — só o nome do processo — para sugerir modos.
//! - **lembretes repetidos**: colunas `recur` e `occurrence` em `reminders`.

use crate::{now_ms, Memory, Reminder};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Um passo de um modo: a chamada (ação + parâmetros) e a frase de onde veio.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModeStep {
    pub action: String,
    #[serde(default)]
    pub params: Value,
    /// Como o usuário disse o passo ("muta o pc"); vazio se veio só do modelo.
    #[serde(default)]
    pub say: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Mode {
    pub name: String,
    pub steps: Vec<ModeStep>,
    pub updated_ms: i64,
}

/// Layout salvo: `windows` é a lista de janelas no formato do `lilith-win`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Layout {
    pub name: String,
    pub windows: Value,
    pub updated_ms: i64,
}

/// Primeira vez que um programa apareceu num dia.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Launch {
    /// AAAA-MM-DD.
    pub day: String,
    /// Minuto do dia (0..1440).
    pub minute: u32,
    pub app: String,
}

/// Limites de um modo (o editor e o chat respeitam os mesmos).
pub const MAX_MODE_STEPS: usize = 20;
pub const MAX_NAME: usize = 40;

/// Tabelas novas e colunas novas em bancos antigos.
pub(crate) fn migrate(conn: &Connection) {
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS modes (
             name       TEXT PRIMARY KEY COLLATE NOCASE,
             steps      TEXT NOT NULL,
             updated_ms INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS layouts (
             name       TEXT PRIMARY KEY COLLATE NOCASE,
             windows    TEXT NOT NULL,
             updated_ms INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS app_launches (
             day    TEXT NOT NULL,
             app    TEXT NOT NULL COLLATE NOCASE,
             minute INTEGER NOT NULL,
             PRIMARY KEY (day, app)
         );",
    );
    let _ = conn.execute("ALTER TABLE reminders ADD COLUMN recur TEXT NOT NULL DEFAULT ''", []);
    let _ = conn.execute("ALTER TABLE reminders ADD COLUMN occurrence INTEGER NOT NULL DEFAULT 0", []);
}

/// Nome de modo/layout: uma linha, sem aspas em volta, até `MAX_NAME`.
pub fn clean_name(raw: &str) -> Option<String> {
    let t: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let t = t.trim_matches(|c: char| matches!(c, '"' | '\'' | '«' | '»' | '“' | '”' | '‘' | '’' | '.' | '!' | '?') || c.is_whitespace());
    let ok = !t.is_empty()
        && t.chars().count() <= MAX_NAME
        && t.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'));
    ok.then(|| t.to_string())
}

fn is_date(s: &str) -> bool {
    s.len() == 10 && s.bytes().enumerate().all(|(i, b)| if i == 4 || i == 7 { b == b'-' } else { b.is_ascii_digit() })
}

fn is_time(s: &str) -> bool {
    s.len() == 5 && s.bytes().enumerate().all(|(i, b)| if i == 2 { b == b':' } else { b.is_ascii_digit() })
}

impl Memory {
    // ── modos ─────────────────────────────────────────────────────────────

    /// Cria ou substitui um modo. Os passos já devem vir validados pelo app.
    pub fn save_mode(&self, name: &str, steps: &[ModeStep]) -> bool {
        let Some(name) = clean_name(name) else { return false };
        if steps.is_empty() || steps.len() > MAX_MODE_STEPS || steps.iter().any(|s| s.action.trim().is_empty()) {
            return false;
        }
        let Ok(json) = serde_json::to_string(steps) else { return false };
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO modes (name, steps, updated_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT(name) DO UPDATE SET name = excluded.name, steps = excluded.steps,
                                            updated_ms = excluded.updated_ms",
            params![name, json, now_ms()],
        )
        .is_ok()
    }

    pub fn mode(&self, name: &str) -> Option<Mode> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT name, steps, updated_ms FROM modes WHERE name = ?1",
            params![name.trim()],
            |r| {
                let steps: String = r.get(1)?;
                Ok(Mode { name: r.get(0)?, steps: serde_json::from_str(&steps).unwrap_or_default(), updated_ms: r.get(2)? })
            },
        )
        .optional()
        .ok()
        .flatten()
    }

    pub fn modes(&self) -> Vec<Mode> {
        let conn = self.conn.lock().unwrap();
        conn.prepare("SELECT name, steps, updated_ms FROM modes ORDER BY name")
            .and_then(|mut s| {
                Ok(s.query_map([], |r| {
                    let steps: String = r.get(1)?;
                    Ok(Mode { name: r.get(0)?, steps: serde_json::from_str(&steps).unwrap_or_default(), updated_ms: r.get(2)? })
                })?
                .filter_map(|r| r.ok())
                .collect())
            })
            .unwrap_or_default()
    }

    pub fn remove_mode(&self, name: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM modes WHERE name = ?1", params![name.trim()]).map(|n| n > 0).unwrap_or(false)
    }

    // ── layouts de janelas ────────────────────────────────────────────────

    pub fn save_layout(&self, name: &str, windows: &Value) -> bool {
        let Some(name) = clean_name(name) else { return false };
        if !windows.as_array().is_some_and(|a| !a.is_empty()) {
            return false;
        }
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO layouts (name, windows, updated_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT(name) DO UPDATE SET name = excluded.name, windows = excluded.windows,
                                            updated_ms = excluded.updated_ms",
            params![name, windows.to_string(), now_ms()],
        )
        .is_ok()
    }

    pub fn layout(&self, name: &str) -> Option<Layout> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT name, windows, updated_ms FROM layouts WHERE name = ?1",
            params![name.trim()],
            |r| {
                let w: String = r.get(1)?;
                Ok(Layout { name: r.get(0)?, windows: serde_json::from_str(&w).unwrap_or(Value::Null), updated_ms: r.get(2)? })
            },
        )
        .optional()
        .ok()
        .flatten()
    }

    pub fn layouts(&self) -> Vec<Layout> {
        let conn = self.conn.lock().unwrap();
        conn.prepare("SELECT name, windows, updated_ms FROM layouts ORDER BY name")
            .and_then(|mut s| {
                Ok(s.query_map([], |r| {
                    let w: String = r.get(1)?;
                    Ok(Layout { name: r.get(0)?, windows: serde_json::from_str(&w).unwrap_or(Value::Null), updated_ms: r.get(2)? })
                })?
                .filter_map(|r| r.ok())
                .collect())
            })
            .unwrap_or_default()
    }

    pub fn remove_layout(&self, name: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM layouts WHERE name = ?1", params![name.trim()]).map(|n| n > 0).unwrap_or(false)
    }

    // ── programas abertos (para aprender rotinas) ─────────────────────────

    /// Registra a primeira vez que `app` apareceu em `day`. Vezes seguintes
    /// no mesmo dia são ignoradas. Devolve se era novo.
    pub fn log_launch(&self, day: &str, minute: u32, app: &str) -> bool {
        let app = app.trim();
        if !is_date(day) || app.is_empty() || app.chars().count() > 80 || minute >= 24 * 60 {
            return false;
        }
        let conn = self.conn.lock().unwrap();
        conn.execute("INSERT OR IGNORE INTO app_launches (day, app, minute) VALUES (?1, ?2, ?3)", params![day, app, minute])
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    /// Aberturas a partir de `day` (inclusive), por dia e horário.
    pub fn launches_since(&self, day: &str) -> Vec<Launch> {
        let conn = self.conn.lock().unwrap();
        conn.prepare("SELECT day, minute, app FROM app_launches WHERE day >= ?1 ORDER BY day, minute, app")
            .and_then(|mut s| {
                Ok(s.query_map(params![day], |r| Ok(Launch { day: r.get(0)?, minute: r.get(1)?, app: r.get(2)? }))?
                    .filter_map(|r| r.ok())
                    .collect())
            })
            .unwrap_or_default()
    }

    /// Esquece aberturas antigas (o registro só serve para os últimos dias).
    pub fn prune_launches(&self, before_day: &str) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute("DELETE FROM app_launches WHERE day < ?1", params![before_day]);
    }

    // ── lembretes repetidos ───────────────────────────────────────────────

    /// Lembrete que se repete (`repeat` não vazio). `due` é a primeira vez.
    pub fn add_recurring_reminder(&self, due: &str, at: &str, text: &str, repeat: &str) -> Option<i64> {
        let (due, at, text, repeat) = (due.trim(), at.trim(), text.trim(), repeat.trim());
        if !is_date(due) || !is_time(at) || text.is_empty() || text.chars().count() > 300 || repeat.is_empty() || repeat.len() > 40 {
            return None;
        }
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO reminders (due, at, text, recur, created_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![due, at, text, repeat, now_ms()],
        )
        .ok()?;
        Some(conn.last_insert_rowid())
    }

    /// Repetidos já na hora (data/horário <= agora). Não muda nada: quem
    /// avisa chama [`Memory::advance_recurring`] com a próxima data.
    pub fn due_recurring(&self, today: &str, now: &str) -> Vec<Reminder> {
        self.reminders_where("done = 0 AND recur != '' AND (due < ?1 OR (due = ?1 AND at <= ?2))", &[today, now])
    }

    /// Avisou: cria a ocorrência (aparece no chat uma vez, como um lembrete
    /// comum já notificado) e leva o repetido para `next_due`.
    pub fn advance_recurring(&self, id: i64, next_due: &str) -> bool {
        if !is_date(next_due) {
            return false;
        }
        let conn = self.conn.lock().unwrap();
        let copied = conn
            .execute(
                "INSERT INTO reminders (due, at, text, done, notified, recur, occurrence, created_ms)
                 SELECT due, at, text, 0, 1, '', 1, ?2 FROM reminders WHERE id = ?1 AND recur != ''",
                params![id, now_ms()],
            )
            .map(|n| n > 0)
            .unwrap_or(false);
        copied && conn.execute("UPDATE reminders SET due = ?2 WHERE id = ?1", params![id, next_due]).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn step(action: &str, params: Value, say: &str) -> ModeStep {
        ModeStep { action: action.into(), params, say: say.into() }
    }

    #[test]
    fn modes_round_trip_and_replace() {
        let m = Memory::in_memory().unwrap();
        let steps = vec![
            step("window.minimize", json!({ "monitor": 1 }), "minimiza o monitor 1"),
            step("audio.set_mute", json!({ "muted": true }), "muta"),
        ];
        assert!(m.save_mode("Foco", &steps));
        assert_eq!(m.mode("foco").unwrap().steps, steps, "nome sem diferenciar maiúsculas");
        assert!(m.save_mode("foco", &steps[..1]), "mesmo nome substitui");
        assert_eq!(m.modes().len(), 1);
        assert_eq!(m.mode("FOCO").unwrap().steps.len(), 1);
        assert!(!m.save_mode("vazio", &[]));
        assert!(!m.save_mode("  ", &steps));
        assert!(!m.save_mode("a/b", &steps), "nome com barra não");
        assert!(m.remove_mode("Foco"));
        assert!(m.mode("foco").is_none());
    }

    #[test]
    fn names_are_cleaned() {
        assert_eq!(clean_name("  «trabalho» ").as_deref(), Some("trabalho"));
        assert_eq!(clean_name("\"modo stream\"").as_deref(), Some("modo stream"));
        assert_eq!(clean_name("pós-almoço").as_deref(), Some("pós-almoço"));
        assert_eq!(clean_name(&"x".repeat(41)), None);
        assert_eq!(clean_name("rm -rf /"), None);
    }

    #[test]
    fn layouts_round_trip() {
        let m = Memory::in_memory().unwrap();
        let w = json!([{ "app": "Code", "monitor": 1 }]);
        assert!(m.save_layout("trabalho", &w));
        assert!(!m.save_layout("vazio", &json!([])));
        assert_eq!(m.layout("Trabalho").unwrap().windows, w);
        assert_eq!(m.layouts().len(), 1);
        assert!(m.remove_layout("trabalho"));
        assert!(m.layouts().is_empty());
    }

    #[test]
    fn launches_keep_the_first_time_of_each_day() {
        let m = Memory::in_memory().unwrap();
        assert!(m.log_launch("2026-10-12", 480, "Code"));
        assert!(!m.log_launch("2026-10-12", 600, "code"), "a segunda vez no dia não conta");
        assert!(m.log_launch("2026-10-13", 485, "Code"));
        assert!(!m.log_launch("13/10", 485, "Code"));
        assert_eq!(m.launches_since("2026-10-13").len(), 1);
        assert_eq!(m.launches_since("2026-10-01")[0].minute, 480);
        m.prune_launches("2026-10-13");
        assert_eq!(m.launches_since("2026-01-01").len(), 1);
    }

    #[test]
    fn recurring_reminders_fire_and_move_forward() {
        let m = Memory::in_memory().unwrap();
        let id = m.add_recurring_reminder("2026-10-12", "09:00", "tomar remédio", "daily").unwrap();
        assert!(m.add_recurring_reminder("2026-10-12", "9h", "x", "daily").is_none());
        // O caminho dos lembretes comuns nunca pega os repetidos.
        assert!(m.take_to_notify("2026-10-12", "10:00").is_empty());
        assert!(m.take_due_reminders("2026-10-12", "10:00").is_empty());
        assert!(m.due_recurring("2026-10-12", "08:59").is_empty());
        let due = m.due_recurring("2026-10-12", "09:00");
        assert_eq!((due.len(), due[0].repeat.as_str()), (1, "daily"));
        assert!(m.advance_recurring(id, "2026-10-13"));
        assert!(m.due_recurring("2026-10-12", "23:59").is_empty(), "já foi para amanhã");
        // A ocorrência aparece no chat uma vez e não na lista de pendentes.
        let shown = m.take_due_reminders("2026-10-12", "10:00");
        assert_eq!((shown.len(), shown[0].text.as_str()), (1, "tomar remédio"));
        assert!(m.take_due_reminders("2026-10-12", "10:00").is_empty());
        let pending = m.reminders();
        assert_eq!((pending.len(), pending[0].due.as_str()), (1, "2026-10-13"));
        assert!(m.remove_reminder(id));
    }
}
