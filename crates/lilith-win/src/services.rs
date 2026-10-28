//! Serviços do Windows (Docker, SQL Server, Spooler…): achar pelo nome que o
//! usuário fala, ver o estado e parar/iniciar. Parar e iniciar precisam de
//! administrador — quem chama isso é o ajudante elevado do app (o UAC é a
//! aprovação); aqui só tem o mecanismo, com o nome do serviço validado.

use crate::handlers::hidden_command;
use crate::WinError;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Service {
    pub name: String,
    pub display: String,
    /// "running", "stopped" ou outro estado do Windows em minúsculas.
    pub state: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServiceOp {
    Stop,
    Start,
    Restart,
}

impl ServiceOp {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "stop" | "parar" | "para" | "desligar" => Some(Self::Stop),
            "start" | "iniciar" | "inicia" | "ligar" => Some(Self::Start),
            "restart" | "reiniciar" | "reinicia" => Some(Self::Restart),
            _ => None,
        }
    }
}

/// Nome interno de serviço: "com.docker.service", "MSSQL$SQLEXPRESS", "AMD
/// Crash Defender Service". Vai como argumento do `sc.exe` (sem shell), então
/// espaço não é risco.
pub fn valid_name(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && name.len() <= 256
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '$' | '@' | ' '))
}

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

/// Todos os serviços (PowerShell em JSON: independe do idioma do Windows).
pub async fn list() -> Result<Vec<Service>, WinError> {
    let out = hidden_command("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            // Sem UTF-8 o "Serviço" do nome quebrava o JSON (caso real, 28/10).
            "[Console]::OutputEncoding=[Text.Encoding]::UTF8; Get-Service | ForEach-Object { [pscustomobject]@{ name = $_.Name; display = $_.DisplayName; state = \"$($_.Status)\".ToLower() } } | ConvertTo-Json -Compress",
        ])
        .output()
        .await?;
    serde_json::from_slice(&out.stdout).map_err(|e| WinError::Failed(format!("lista de serviços: {e}")))
}

/// Serviços que casam com o que o usuário falou ("docker" → "Docker Desktop
/// Service"). Nome interno igual vem sozinho.
pub fn matching(all: &[Service], query: &str) -> Vec<Service> {
    let q = fold(query.trim());
    if q.is_empty() {
        return Vec::new();
    }
    if let Some(exact) = all.iter().find(|s| fold(&s.name) == q || fold(&s.display) == q) {
        return vec![exact.clone()];
    }
    let words: Vec<&str> = q.split_whitespace().filter(|w| !matches!(*w, "servico" | "service" | "o" | "do" | "da")).collect();
    all.iter()
        .filter(|s| {
            let hay = format!("{} {}", fold(&s.name), fold(&s.display));
            !words.is_empty() && words.iter().all(|w| hay.contains(w))
        })
        .cloned()
        .collect()
}

/// `sc query`: o rótulo da linha é traduzido ("STATE"/"ESTADO"), mas o
/// valor não ("4  RUNNING") — acha a linha pelo valor (1 parado, 4 rodando).
pub(crate) fn parse_state(sc_output: &str) -> Option<u32> {
    const STATES: &[&str] = &["STOPPED", "START_PENDING", "STOP_PENDING", "RUNNING", "CONTINUE_PENDING", "PAUSE_PENDING", "PAUSED"];
    sc_output.lines().find_map(|l| {
        let value = l.split_once(':')?.1;
        let mut t = value.split_whitespace();
        let code = t.next()?.parse().ok()?;
        STATES.contains(&t.next()?).then_some(code)
    })
}

fn state_code(name: &str) -> Option<u32> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("sc.exe").args(["query", name]).creation_flags(0x0800_0000).output().ok()?;
    parse_state(&String::from_utf8_lossy(&out.stdout))
}

fn wait_for(name: &str, code: u32) -> bool {
    let until = Instant::now() + Duration::from_secs(30);
    while Instant::now() < until {
        if state_code(name) == Some(code) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    false
}

fn sc(verb: &str, name: &str) {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("sc.exe").args([verb, name]).creation_flags(0x0800_0000).output();
}

/// Para/inicia/reinicia e espera o estado final. Precisa de administrador.
/// Bloqueante (até ~30 s por etapa).
pub fn control_blocking(name: &str, op: ServiceOp) -> Result<(), WinError> {
    if !valid_name(name) {
        return Err(WinError::InvalidParams(format!("nome de serviço inválido: {name}")));
    }
    const STOPPED: u32 = 1;
    const RUNNING: u32 = 4;
    if matches!(op, ServiceOp::Stop | ServiceOp::Restart) && state_code(name) != Some(STOPPED) {
        sc("stop", name);
        if !wait_for(name, STOPPED) {
            return Err(WinError::Failed(format!("o serviço {name} não parou")));
        }
    }
    if matches!(op, ServiceOp::Start | ServiceOp::Restart) {
        sc("start", name);
        if !wait_for(name, RUNNING) {
            return Err(WinError::Failed(format!("o serviço {name} não iniciou")));
        }
    }
    Ok(())
}

/// `service.status`: estado dos serviços que casam com o nome.
pub async fn status(params: serde_json::Value) -> Result<serde_json::Value, WinError> {
    let query = params["service"].as_str().unwrap_or("").to_string();
    let found = matching(&list().await?, &query);
    if found.is_empty() {
        return Err(WinError::NotFound(format!("serviço '{query}'")));
    }
    Ok(serde_json::json!({ "services": found }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(name: &str, display: &str) -> Service {
        Service { name: name.into(), display: display.into(), state: "running".into() }
    }

    #[test]
    fn finds_services_by_spoken_name() {
        let all = vec![s("com.docker.service", "Docker Desktop Service"), s("Spooler", "Spooler de Impressão"), s("WSearch", "Windows Search")];
        assert_eq!(matching(&all, "docker")[0].name, "com.docker.service");
        assert_eq!(matching(&all, "serviço do docker")[0].name, "com.docker.service");
        assert_eq!(matching(&all, "spooler de impressao")[0].name, "Spooler");
        assert_eq!(matching(&all, "WSearch").len(), 1);
        assert!(matching(&all, "postgres").is_empty());
    }

    #[test]
    fn reads_the_state_in_any_language() {
        let pt = "NOME_DO_SERVI{fffd}O: Spooler
    TIPO   : 110  WIN32_OWN_PROCESS
    ESTADO : 4  RUNNING
    C{fffd}DIGO_DE_SA{fffd}DA_DO_WIN32 : 0  (0x0)
";
        assert_eq!(parse_state(pt), Some(4));
        assert_eq!(parse_state("SERVICE_NAME: x
        STATE              : 1  STOPPED
"), Some(1));
        assert_eq!(parse_state("[SC] OpenService FALHA 1060"), None);
    }

    #[test]
    fn names_and_ops_are_validated() {
        assert!(valid_name("com.docker.service") && valid_name("MSSQL$SQLEXPRESS"));
        assert!(valid_name("AMD Crash Defender Service"));
        assert!(!valid_name("x & calc") && !valid_name("-x") && !valid_name(""));
        assert_eq!(ServiceOp::parse("parar"), Some(ServiceOp::Stop));
        assert_eq!(ServiceOp::parse("restart"), Some(ServiceOp::Restart));
        assert_eq!(ServiceOp::parse("deletar"), None);
    }
}

#[cfg(test)]
mod live {
    /// `cargo test -p lilith-win services::live -- --ignored --nocapture` (só lê).
    #[tokio::test]
    #[ignore]
    async fn lists_real_services() {
        let all = super::list().await.unwrap();
        println!("{} serviços; docker = {:?}", all.len(), super::matching(&all, "docker"));
        assert!(all.iter().any(|s| s.display.contains("Serviço")), "acentos chegam certos");
    }
}
