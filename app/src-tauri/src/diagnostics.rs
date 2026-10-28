//! Autodiagnóstico da Lilith, decidido por código: cada peça (Ollama e modelo,
//! voz, forma 3D, atualização, banco, ações recentes que falharam) é checada
//! com regras fixas e vira um item com veredito e, quando há problema, o que
//! fazer. A IA não opina aqui — só o texto do relatório é montado depois.

use serde::Serialize;
use serde_json::{json, Value};
use std::path::Path;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Ok,
    Warn,
    Problem,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: String,
    pub level: Level,
    pub detail: String,
    /// O que fazer (vazio quando está tudo certo).
    pub fix: String,
}

fn check(name: &str, level: Level, detail: impl Into<String>, fix: impl Into<String>) -> Check {
    Check { name: name.into(), level, detail: detail.into(), fix: fix.into() }
}

/// O Ollama responde e tem o modelo configurado?
pub(crate) fn ollama_check(health: &Result<(String, Vec<String>), String>, model: &str) -> Check {
    match health {
        Err(e) => check("Cérebro (Ollama)", Level::Problem, e.clone(), "Abra o Ollama (menu Iniciar) ou rode `ollama serve`."),
        Ok((version, models)) => {
            let wanted = model.trim();
            let has = models.iter().any(|m| m == wanted || m.split(':').next() == Some(wanted) || m == &format!("{wanted}:latest"));
            if has {
                check("Cérebro (Ollama)", Level::Ok, format!("Ollama {version} com o modelo {wanted}"), "")
            } else {
                check(
                    "Cérebro (Ollama)",
                    Level::Problem,
                    format!("Ollama {version} está aberto, mas o modelo {wanted} não está baixado"),
                    format!("Rode `ollama pull {wanted}`."),
                )
            }
        }
    }
}

/// Erros de ações nas últimas 24 h, a partir do registro de auditoria.
pub(crate) fn recent_failures(audit_lines: &[String], now: chrono::DateTime<chrono::Utc>) -> (usize, Vec<(String, usize, String)>) {
    let since = now - chrono::Duration::hours(24);
    let mut total = 0;
    let mut by_action: std::collections::BTreeMap<String, (usize, String)> = Default::default();
    for line in audit_lines {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v["outcome"] != "error" {
            continue;
        }
        let ts = v["ts"].as_str().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok());
        if ts.is_none_or(|t| t.with_timezone(&chrono::Utc) < since) {
            continue;
        }
        total += 1;
        let action = v["action"].as_str().unwrap_or("?").to_string();
        let msg = v["detail"]["response"]["error"]["message"].as_str().unwrap_or("").chars().take(120).collect::<String>();
        let e = by_action.entry(action).or_insert((0, String::new()));
        e.0 += 1;
        e.1 = msg; // a mais recente
    }
    let mut top: Vec<(String, usize, String)> = by_action.into_iter().map(|(a, (n, m))| (a, n, m)).collect();
    top.sort_by(|a, b| b.1.cmp(&a.1));
    top.truncate(3);
    (total, top)
}

fn failures_check(total: usize, top: &[(String, usize, String)]) -> Check {
    if total == 0 {
        return check("Ações (últimas 24 h)", Level::Ok, "nenhuma ação falhou", "");
    }
    let list = top.iter().map(|(a, n, m)| format!("{a} ×{n} ({m})")).collect::<Vec<_>>().join("; ");
    let level = if total >= 10 { Level::Problem } else { Level::Warn };
    check("Ações (últimas 24 h)", level, format!("{total} falharam — {list}"), "Me diga o que você pediu nessas horas que eu investigo pelo log.")
}

fn last_lines(path: &Path, max: usize) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let skip = lines.len().saturating_sub(max);
    lines.into_iter().skip(skip).collect()
}

/// Checa tudo e devolve o relatório (para o chat e para a tela de diagnóstico).
pub async fn run(app: &tauri::AppHandle) -> Value {
    use tauri::Manager;
    let state = app.state::<crate::AppState>();
    let store = state.store.clone();
    let settings = crate::profile::Settings::load(&store);
    let mut checks = Vec::new();

    // Cérebro: o escolhido nas Configurações.
    let cfg = crate::config::Config::load();
    if state.brain.uses_claude_code() {
        checks.push(if state.brain.claude_code_found() {
            check("Cérebro (Claude Code)", Level::Ok, format!("Claude Code encontrado; modelo {}", cfg.claude.agent_model), "")
        } else {
            check(
                "Cérebro (Claude Code)",
                Level::Problem,
                "o Claude Code foi escolhido mas não está instalado — estou usando o cérebro local",
                "Instale o Claude Code e rode `claude` uma vez para fazer login, ou troque para Local nas Configurações.",
            )
        });
    } else {
        let health = lilith_brain::ollama::health(&cfg.ollama).await;
        checks.push(ollama_check(&health, &cfg.ollama.model));
    }

    // Voz.
    let voice = if !settings.voice {
        check("Voz", Level::Ok, "desligada nas Configurações", "")
    } else if settings.voice_engine == "serena" {
        if crate::voice_qwen::is_installed() {
            let detail = if crate::voice_qwen::is_loaded() { "Serena carregada na GPU" } else { "Serena instalada (carrega quando falar)" };
            check("Voz", Level::Ok, detail, "")
        } else {
            check("Voz", Level::Problem, "a voz Serena foi escolhida mas não está instalada", "Configurações → Voz → Instalar a Serena (ou escolha a voz padrão).")
        }
    } else if crate::voice::is_installed() {
        check("Voz", Level::Ok, "voz padrão instalada", "")
    } else {
        check("Voz", Level::Warn, "a voz está ligada mas não instalada — ela fica muda", "Configurações → Voz → Instalar a voz padrão.")
    };
    checks.push(voice);

    // Forma 3D.
    checks.push(if crate::config::avatar_model_exists() {
        check("Forma 3D", Level::Ok, "modelo lilith.vrm encontrado", "")
    } else {
        check("Forma 3D", Level::Warn, "sem o arquivo lilith.vrm — só a forma 2D funciona", "Coloque o seu lilith.vrm em %APPDATA%\\Lilith.")
    });

    // Atualização.
    let version = app.package_info().version.to_string();
    checks.push(match crate::update::last_check() {
        None => check("Atualização", Level::Ok, format!("versão {version}; ainda não procurou nesta sessão"), ""),
        Some(c) if c.error.is_some() => check(
            "Atualização",
            Level::Problem,
            format!("versão {version}; a última busca falhou: {}", c.error.unwrap_or_default()),
            "Confira a internet; se continuar, me mande o log de hoje (Configurações → Abrir pasta de logs).",
        ),
        Some(c) => match c.available {
            Some(v) => check("Atualização", Level::Warn, format!("versão {version}; a {v} está disponível"), "Configurações → Atualizar."),
            None => check("Atualização", Level::Ok, format!("versão {version}, a mais recente"), ""),
        },
    });

    // Banco de dados.
    let db = crate::config::data_dir().join("lilith.db");
    let db_mb = std::fs::metadata(&db).map(|m| m.len() as f64 / 1_048_576.0).unwrap_or(0.0);
    checks.push(if db.is_file() {
        check("Memória (banco)", Level::Ok, format!("{db_mb:.1} MB"), "")
    } else {
        check("Memória (banco)", Level::Problem, "banco não encontrado", "Reinicie a Lilith.")
    });

    // Ações que falharam.
    let (total, top) = recent_failures(&last_lines(&state.audit_path, 2000), chrono::Utc::now());
    checks.push(failures_check(total, &top));

    let worst = if checks.iter().any(|c| c.level == Level::Problem) {
        "problem"
    } else if checks.iter().any(|c| c.level == Level::Warn) {
        "warn"
    } else {
        "ok"
    };
    json!({
        "version": version,
        "overall": worst,
        "checks": checks,
        "logs": crate::config::data_dir().join("logs").display().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ollama_verdicts() {
        assert_eq!(ollama_check(&Err("o Ollama não respondeu".into()), "qwen3.5:4b").level, Level::Problem);
        let up = Ok(("0.12.0".to_string(), vec!["qwen3.5:4b".to_string()]));
        assert_eq!(ollama_check(&up, "qwen3.5:4b").level, Level::Ok);
        let missing = Ok(("0.12.0".to_string(), vec!["llama3:8b".to_string()]));
        let c = ollama_check(&missing, "qwen3.5:4b");
        assert_eq!(c.level, Level::Problem);
        assert!(c.fix.contains("ollama pull qwen3.5:4b"));
    }

    #[test]
    fn counts_recent_failures_only() {
        let now = chrono::Utc::now();
        let recent = (now - chrono::Duration::hours(1)).to_rfc3339();
        let old = (now - chrono::Duration::hours(30)).to_rfc3339();
        let line = |ts: &str, action: &str, outcome: &str| {
            json!({ "ts": ts, "action": action, "outcome": outcome, "detail": { "response": { "error": { "message": "falhou" } } } }).to_string()
        };
        let lines = vec![
            line(&recent, "discord.voice", "error"),
            line(&recent, "discord.voice", "error"),
            line(&recent, "app.open", "success"),
            line(&old, "window.read", "error"),
            "lixo".to_string(),
        ];
        let (total, top) = recent_failures(&lines, now);
        assert_eq!(total, 2);
        assert_eq!(top[0], ("discord.voice".to_string(), 2, "falhou".to_string()));
        assert_eq!(failures_check(0, &[]).level, Level::Ok);
        assert_eq!(failures_check(2, &top).level, Level::Warn);
    }
}
