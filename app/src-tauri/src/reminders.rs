//! Datas de lembretes: o modelo manda AAAA-MM-DD, mas aceita-se também
//! "12/10" (ano atual), "12/10/2026", "hoje" e "amanhã".

use chrono::{Datelike, Duration, NaiveDate};

/// Interpreta `raw` em relação a `today`. "12/10" sem ano é no ano atual.
pub fn parse_date(raw: &str, today: NaiveDate) -> Option<NaiveDate> {
    let t = raw.trim().to_lowercase();
    match t.as_str() {
        "hoje" | "today" => return Some(today),
        "amanhã" | "amanha" | "tomorrow" => return Some(today + Duration::days(1)),
        _ => {}
    }
    if let Ok(d) = NaiveDate::parse_from_str(&t, "%Y-%m-%d") {
        return Some(d);
    }
    let parts: Vec<u32> = t.split('/').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    match parts[..] {
        // Sem ano: é o ano atual.
        [day, month] => NaiveDate::from_ymd_opt(today.year(), month, day),
        [day, month, year] => NaiveDate::from_ymd_opt(if year < 100 { 2000 + year as i32 } else { year as i32 }, month, day),
        _ => None,
    }
}

/// "15:30", "15h", "15h30", "9" → "15:30"/"15:00"/"09:00". Vazio/inválido → None.
pub fn parse_time(raw: &str) -> Option<String> {
    let t = raw.trim().to_lowercase().replace([' ', 'h'], ":");
    let t = t.trim_end_matches(':');
    let (h, m) = match t.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, if m.is_empty() { 0 } else { m.parse::<u32>().ok()? }),
        None => (t.parse::<u32>().ok()?, 0),
    };
    (h < 24 && m < 60).then(|| format!("{h:02}:{m:02}"))
}

pub use recurring::*;

/// Lembretes que se repetem: a frase do usuário vira uma regra guardada no
/// SQLite ("daily", "weekdays", "weekly:1,3", "monthly:15"); o agendador
/// calcula a próxima data depois de cada aviso.
mod recurring {
    use super::{parse_date, parse_time};
    use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime};
    use serde_json::{json, Value};

    #[derive(Debug, Clone, PartialEq)]
    pub enum Repeat {
        Daily,
        /// Segunda a sexta.
        Weekdays,
        /// Dias da semana: 1 = segunda … 7 = domingo.
        Weekly(Vec<u32>),
        /// Dia do mês (meses mais curtos: o último dia).
        Monthly(u32),
    }

    const DAYS: [&str; 7] = ["segunda", "terça", "quarta", "quinta", "sexta", "sábado", "domingo"];

    fn fold(s: &str) -> String {
        s.to_lowercase()
            .chars()
            .map(|c| match c {
                'á' | 'à' | 'â' | 'ã' => 'a',
                'é' | 'ê' => 'e',
                'í' => 'i',
                'ó' | 'ô' | 'õ' => 'o',
                'ú' => 'u',
                'ç' => 'c',
                other => other,
            })
            .collect()
    }

    impl Repeat {
        /// Como fica guardada.
        pub fn rule(&self) -> String {
            match self {
                Repeat::Daily => "daily".into(),
                Repeat::Weekdays => "weekdays".into(),
                Repeat::Weekly(days) => format!("weekly:{}", days.iter().map(u32::to_string).collect::<Vec<_>>().join(",")),
                Repeat::Monthly(day) => format!("monthly:{day}"),
            }
        }

        pub fn from_rule(rule: &str) -> Option<Self> {
            let (kind, arg) = rule.trim().split_once(':').unwrap_or((rule.trim(), ""));
            match kind {
                "daily" => Some(Repeat::Daily),
                "weekdays" => Some(Repeat::Weekdays),
                "weekly" => {
                    let mut days: Vec<u32> = arg.split(',').map(|d| d.trim().parse().ok()).collect::<Option<_>>()?;
                    days.sort_unstable();
                    days.dedup();
                    (!days.is_empty() && days.iter().all(|d| (1..=7).contains(d))).then_some(Repeat::Weekly(days))
                }
                "monthly" => arg.parse().ok().filter(|d| (1..=31).contains(d)).map(Repeat::Monthly),
                _ => None,
            }
        }

        /// "todo dia", "de segunda a sexta", "toda segunda e quarta", "todo dia 15".
        pub fn label(&self) -> String {
            match self {
                Repeat::Daily => "todo dia".into(),
                Repeat::Weekdays => "de segunda a sexta".into(),
                Repeat::Weekly(days) => {
                    let names: Vec<&str> = days.iter().map(|d| DAYS[(*d as usize - 1).min(6)]).collect();
                    let list = match names.split_last() {
                        Some((last, [])) => last.to_string(),
                        Some((last, rest)) => format!("{} e {last}", rest.join(", ")),
                        None => String::new(),
                    };
                    // "toda segunda", "todo sábado": concorda com o primeiro dia.
                    format!("{} {list}", if days.first().is_some_and(|d| *d <= 5) { "toda" } else { "todo" })
                }
                Repeat::Monthly(day) => format!("todo dia {day}"),
            }
        }

        fn matches(&self, d: NaiveDate) -> bool {
            let wd = d.weekday().number_from_monday();
            match self {
                Repeat::Daily => true,
                Repeat::Weekdays => wd <= 5,
                Repeat::Weekly(days) => days.contains(&wd),
                Repeat::Monthly(day) => d.day() == (*day).min(last_day(d)),
            }
        }
    }

    fn last_day(d: NaiveDate) -> u32 {
        let (y, m) = if d.month() == 12 { (d.year() + 1, 1) } else { (d.year(), d.month() + 1) };
        NaiveDate::from_ymd_opt(y, m, 1).and_then(|n| n.pred_opt()).map(|n| n.day()).unwrap_or(28)
    }

    /// A frase do usuário (ou uma regra já pronta) → repetição. `today` dá o
    /// dia da semana de "toda semana" e o dia do mês de "todo mês".
    pub fn parse_repeat(raw: &str, today: NaiveDate) -> Option<Repeat> {
        if let Some(r) = Repeat::from_rule(raw) {
            return Some(r);
        }
        let t = fold(raw);
        let t = t.trim();
        let words: Vec<&str> = t.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
        // "todo dia 15", "dia 15 de cada mês".
        if let Some(i) = words.iter().position(|w| *w == "dia") {
            if let Some(n) = words.get(i + 1).and_then(|w| w.parse::<u32>().ok()) {
                return (1..=31).contains(&n).then_some(Repeat::Monthly(n));
            }
        }
        if t.contains("segunda a sexta") || t.contains("dias uteis") {
            return Some(Repeat::Weekdays);
        }
        const NAMES: [&str; 7] = ["segunda", "terca", "quarta", "quinta", "sexta", "sabado", "domingo"];
        let mut days: Vec<u32> = words
            .iter()
            .filter_map(|w| NAMES.iter().position(|n| w.trim_end_matches('s') == *n || w.starts_with(n)).map(|i| i as u32 + 1))
            .collect();
        days.sort_unstable();
        days.dedup();
        if !days.is_empty() {
            return Some(Repeat::Weekly(days));
        }
        if t.contains("todo dia") || t.contains("todos os dias") || t.contains("diariamente") || t == "diario" {
            return Some(Repeat::Daily);
        }
        if t.contains("semana") || t.contains("semanalmente") {
            return Some(Repeat::Weekly(vec![today.weekday().number_from_monday()]));
        }
        if t.contains("todo mes") || t.contains("mensalmente") {
            return Some(Repeat::Monthly(today.day()));
        }
        None
    }

    /// Próxima data em que o lembrete toca, estritamente depois de `after`.
    pub fn next_due(rep: &Repeat, at: &str, after: NaiveDateTime) -> NaiveDate {
        let time = NaiveTime::parse_from_str(at, "%H:%M").unwrap_or(NaiveTime::MIN);
        let start = after.date();
        (0..=400)
            .map(|i| start + Duration::days(i))
            .find(|d| rep.matches(*d) && d.and_time(time) > after)
            .unwrap_or(start + Duration::days(1))
    }

    /// Rótulo de uma regra guardada (para as Configurações).
    pub fn label_for_rule(rule: &str) -> String {
        Repeat::from_rule(rule).map(|r| r.label()).unwrap_or_default()
    }

    /// "às 9h", "as 15:30", "9" → "09:00". Vazio → None.
    fn clean_time(raw: &str) -> Option<String> {
        let t = fold(raw);
        let t = t.trim().trim_start_matches("a partir das").trim_start_matches("as ").trim_start_matches("a ").trim();
        let t = t.trim_end_matches("horas").trim_end_matches("hrs").trim();
        if t == "meio-dia" || t == "meio dia" {
            return Some("12:00".into());
        }
        parse_time(t)
    }

    /// `reminder.add`: data, horário e (opcional) repetição, tudo decidido
    /// aqui a partir do que o modelo/atalho mandou.
    pub fn add_from_params(store: &lilith_memory::Memory, params: &Value, now: NaiveDateTime) -> Value {
        let s = |k: &str| params[k].as_str().unwrap_or("").trim().to_string();
        let text = s("text");
        let at = clean_time(&s("time")).unwrap_or_else(|| "00:00".into());
        let time_out = if at == "00:00" { Value::Null } else { json!(at) };
        let today = now.date();
        if text.is_empty() {
            return json!({ "reminder": false, "error": "faltou o que lembrar" });
        }
        let repeat_raw = s("repeat");
        if !repeat_raw.is_empty() {
            let Some(rep) = parse_repeat(&repeat_raw, today) else {
                return json!({ "reminder": false, "error": format!("não entendi a repetição \"{repeat_raw}\" (ex.: todo dia, de segunda a sexta, toda segunda, todo dia 5)") });
            };
            // Com data de início, conta a partir dela.
            let from = parse_date(&s("date"), today)
                .filter(|d| *d > today)
                .map(|d| d.and_time(NaiveTime::MIN) - Duration::seconds(1))
                .unwrap_or(now);
            let first = next_due(&rep, &at, from);
            let id = store.add_recurring_reminder(&first.format("%Y-%m-%d").to_string(), &at, &text, &rep.rule());
            return match id {
                Some(_) => json!({
                    "reminder": true,
                    "text": text,
                    "date": first.format("%d/%m/%Y").to_string(),
                    "time": time_out,
                    "repeat": rep.label(),
                }),
                None => json!({ "reminder": false, "error": "não consegui salvar o lembrete" }),
            };
        }
        let due = parse_date(&s("date"), today);
        let id = due.and_then(|d| store.add_reminder(&d.format("%Y-%m-%d").to_string(), &at, &text));
        match (id, due) {
            (Some(_), Some(d)) => json!({
                "reminder": true,
                "text": text,
                "date": d.format("%d/%m/%Y").to_string(),
                "time": time_out,
            }),
            _ => json!({ "reminder": false, "error": "data inválida; use AAAA-MM-DD ou dia/mês" }),
        }
    }

    /// Repetidos na hora: avisa (devolve para a notificação) e já agenda a
    /// próxima vez. Perdeu vários dias (PC desligado)? Avisa uma vez só.
    pub fn fire_recurring(store: &lilith_memory::Memory, now: NaiveDateTime) -> Vec<lilith_memory::Reminder> {
        let due = store.due_recurring(&now.format("%Y-%m-%d").to_string(), &now.format("%H:%M").to_string());
        for r in &due {
            let next = match Repeat::from_rule(&r.repeat) {
                Some(rep) => next_due(&rep, &r.at, now),
                None => {
                    tracing::warn!(id = r.id, rule = %r.repeat, "regra de repetição inválida");
                    now.date() + Duration::days(1)
                }
            };
            store.advance_recurring(r.id, &next.format("%Y-%m-%d").to_string());
        }
        due
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn d(y: i32, m: u32, dd: u32) -> NaiveDate {
            NaiveDate::from_ymd_opt(y, m, dd).unwrap()
        }

        fn at(date: NaiveDate, h: u32, m: u32) -> NaiveDateTime {
            date.and_hms_opt(h, m, 0).unwrap()
        }

        // 2026-10-16 é uma sexta-feira.
        const FRI: (i32, u32, u32) = (2026, 10, 16);

        #[test]
        fn phrases_become_rules() {
            let today = d(FRI.0, FRI.1, FRI.2);
            let p = |s: &str| parse_repeat(s, today);
            assert_eq!(p("todo dia"), Some(Repeat::Daily));
            assert_eq!(p("todos os dias"), Some(Repeat::Daily));
            assert_eq!(p("de segunda a sexta"), Some(Repeat::Weekdays));
            assert_eq!(p("dias úteis"), Some(Repeat::Weekdays));
            assert_eq!(p("toda segunda"), Some(Repeat::Weekly(vec![1])));
            assert_eq!(p("toda segunda e quarta"), Some(Repeat::Weekly(vec![1, 3])));
            assert_eq!(p("todas as terças-feiras"), Some(Repeat::Weekly(vec![2])));
            assert_eq!(p("todo sábado"), Some(Repeat::Weekly(vec![6])));
            assert_eq!(p("toda semana"), Some(Repeat::Weekly(vec![5])), "o dia de hoje");
            assert_eq!(p("todo dia 5"), Some(Repeat::Monthly(5)));
            assert_eq!(p("todo mês"), Some(Repeat::Monthly(16)));
            assert_eq!(p("weekly:3,1"), Some(Repeat::Weekly(vec![1, 3])));
            assert_eq!(p("todo dia 40"), None);
            assert_eq!(p("às vezes"), None);
        }

        #[test]
        fn rules_round_trip_and_labels() {
            for r in [Repeat::Daily, Repeat::Weekdays, Repeat::Weekly(vec![1, 3]), Repeat::Monthly(31)] {
                assert_eq!(Repeat::from_rule(&r.rule()), Some(r));
            }
            assert_eq!(Repeat::Weekly(vec![1, 3, 5]).label(), "toda segunda, quarta e sexta");
            assert_eq!(Repeat::Weekly(vec![6]).label(), "todo sábado");
            assert_eq!(label_for_rule("monthly:5"), "todo dia 5");
            assert_eq!(label_for_rule("lixo"), "");
        }

        #[test]
        fn next_occurrences() {
            let fri = d(FRI.0, FRI.1, FRI.2);
            // Diário às 9h: antes das 9h é hoje; depois, amanhã.
            assert_eq!(next_due(&Repeat::Daily, "09:00", at(fri, 8, 0)), fri);
            assert_eq!(next_due(&Repeat::Daily, "09:00", at(fri, 9, 0)), d(2026, 10, 17));
            // Dias úteis numa sexta à noite: segunda.
            assert_eq!(next_due(&Repeat::Weekdays, "18:00", at(fri, 19, 0)), d(2026, 10, 19));
            assert_eq!(next_due(&Repeat::Weekly(vec![3]), "00:00", at(fri, 10, 0)), d(2026, 10, 21));
            // Dia 31 em novembro: o último dia do mês.
            assert_eq!(next_due(&Repeat::Monthly(31), "08:00", at(d(2026, 11, 1), 0, 0)), d(2026, 11, 30));
            assert_eq!(next_due(&Repeat::Monthly(5), "08:00", at(fri, 10, 0)), d(2026, 11, 5));
        }

        #[test]
        fn recurring_reminder_is_created_fired_and_rescheduled() {
            let store = lilith_memory::Memory::in_memory().unwrap();
            let fri = d(FRI.0, FRI.1, FRI.2);
            let out = add_from_params(&store, &json!({ "text": "tomar remédio", "repeat": "todo dia", "time": "às 9h" }), at(fri, 10, 0));
            assert_eq!(out["reminder"], true);
            assert_eq!((out["date"].as_str(), out["time"].as_str(), out["repeat"].as_str()), (Some("17/10/2026"), Some("09:00"), Some("todo dia")));
            assert!(fire_recurring(&store, at(fri, 23, 0)).is_empty(), "ainda não");
            let sat = d(2026, 10, 17);
            let fired = fire_recurring(&store, at(sat, 9, 0));
            assert_eq!(fired.len(), 1);
            assert!(fire_recurring(&store, at(sat, 9, 1)).is_empty(), "uma vez só");
            assert_eq!(store.reminders()[0].due, "2026-10-18", "já agendado para amanhã");
            // PC desligado por dias: avisa uma vez e segue do dia de hoje.
            let later = d(2026, 10, 25);
            assert_eq!(fire_recurring(&store, at(later, 12, 0)).len(), 1);
            assert_eq!(store.reminders()[0].due, "2026-10-26");
            let bad = add_from_params(&store, &json!({ "text": "x", "repeat": "às vezes" }), at(fri, 10, 0));
            assert_eq!(bad["reminder"], false);
            // Sem repetição continua como antes.
            let once = add_from_params(&store, &json!({ "text": "dentista", "date": "20/10", "time": "15h" }), at(fri, 10, 0));
            assert_eq!((once["date"].as_str(), once["time"].as_str()), (Some("20/10/2026"), Some("15:00")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_times() {
        assert_eq!(parse_time("15:30").as_deref(), Some("15:30"));
        assert_eq!(parse_time("15h").as_deref(), Some("15:00"));
        assert_eq!(parse_time("9h05").as_deref(), Some("09:05"));
        assert_eq!(parse_time("8").as_deref(), Some("08:00"));
        assert_eq!(parse_time("25:00"), None);
        assert_eq!(parse_time(""), None);
    }

    #[test]
    fn parses_common_forms() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 11).unwrap();
        let d = |y, m, dd| NaiveDate::from_ymd_opt(y, m, dd).unwrap();
        assert_eq!(parse_date("2026-10-12", today), Some(d(2026, 10, 12)));
        assert_eq!(parse_date("12/10", today), Some(d(2026, 10, 12)));
        assert_eq!(parse_date("05/03", today), Some(d(2026, 3, 5)), "sem ano é o ano atual");
        assert_eq!(parse_date("12/10/26", today), Some(d(2026, 10, 12)));
        assert_eq!(parse_date("amanhã", today), Some(d(2026, 10, 12)));
        assert_eq!(parse_date("31/02", today), None);
        assert_eq!(parse_date("qualquer dia", today), None);
    }
}
