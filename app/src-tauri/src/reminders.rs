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

#[cfg(test)]
mod tests {
    use super::*;

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
