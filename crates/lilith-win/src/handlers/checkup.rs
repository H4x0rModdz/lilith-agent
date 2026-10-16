//! `system.checkup`: checkup COMPLETO do PC (só leitura, sem admin, ~15 s).
//! Um script PowerShell embutido coleta o que só o Windows sabe (atualizações,
//! saúde dos discos, drivers, inicialização, antivírus, eventos de
//! estabilidade, rede…); o retrato rápido (`system.status`) entra junto. As
//! REGRAS que decidem o que é problema, atenção ou dica ficam aqui, no código.

use super::{blocking, hidden_command, system};
use crate::WinError;
use serde::Serialize;
use serde_json::{json, Value};

const SCRIPT: &str = include_str!("checkup.ps1");

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Finding {
    /// "problem" | "warn" | "tip"
    pub level: &'static str,
    pub area: &'static str,
    pub text: String,
    pub fix: String,
}

fn f(level: &'static str, area: &'static str, text: impl Into<String>, fix: impl Into<String>) -> Finding {
    Finding { level, area, text: text.into(), fix: fix.into() }
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64()
}

/// Regras do checkup sobre o que o script coletou (`d`) e o retrato rápido.
pub fn rules(d: &Value, quick: &system::Snapshot) -> Vec<Finding> {
    let mut out = Vec::new();

    // Windows
    let os = &d["os"];
    if os["reboot_pending"] == true {
        out.push(f("warn", "Windows", "Há um reinício pendente para concluir atualizações", "Reinicie o PC quando puder."));
    }
    if let Some(days) = num(&os["last_update_days"]).filter(|d| *d > 45.0) {
        out.push(f("warn", "Windows", format!("O Windows está sem atualizações há {days:.0} dias"), "Abra o Windows Update e instale as pendentes."));
    }
    if let Some(h) = num(&os["uptime_hours"]).filter(|h| *h >= 24.0 * 7.0) {
        out.push(f("tip", "Windows", format!("O PC está ligado há {:.0} dias sem reiniciar", h / 24.0), "Reiniciar de vez em quando libera memória e aplica atualizações."));
    }

    // Processador
    let cpu = &d["cpu"];
    if let (Some(cur), Some(max), Some(load)) = (num(&cpu["current_mhz"]), num(&cpu["max_mhz"]), num(&cpu["load"])) {
        if max > 0.0 && load >= 50.0 && cur < max * 0.6 {
            out.push(f(
                "warn",
                "Processador",
                format!("Processador trabalhando a {cur:.0} MHz sob carga, bem abaixo dos {max:.0} MHz"),
                "Pode ser o plano de energia em economia ou superaquecimento; confira a ventilação.",
            ));
        }
    }

    // Memória
    let mem = &d["memory"];
    if let (Some(rated), Some(conf)) = (num(&mem["rated_mhz"]), num(&mem["configured_mhz"])) {
        if conf > 0.0 && rated - conf >= 200.0 {
            out.push(f(
                "tip",
                "Memória",
                format!("A memória está rodando a {conf:.0} MHz, abaixo dos {rated:.0} MHz que ela suporta"),
                "Ative o perfil XMP/EXPO na BIOS para ganhar desempenho.",
            ));
        }
    }
    if num(&mem["modules"]) == Some(1.0) && num(&mem["slots"]).unwrap_or(0.0) >= 2.0 {
        out.push(f("tip", "Memória", "Só um pente de memória: ela roda em canal único (mais lenta)", "Dois pentes iguais rodam em dual channel."));
    }

    // Vídeo
    for g in d["gpus"].as_array().into_iter().flatten() {
        if let Some(age) = num(&g["driver_age_days"]).filter(|a| *a > 365.0) {
            out.push(f(
                "warn",
                "Vídeo",
                format!("O driver de vídeo de {} tem {:.0} meses", g["name"].as_str().unwrap_or("uma placa"), age / 30.0),
                "Atualize pelo site do fabricante (NVIDIA, AMD ou Intel).",
            ));
        }
    }

    // Discos
    for disk in d["disks"].as_array().into_iter().flatten() {
        let name = disk["name"].as_str().unwrap_or("um disco");
        let health = disk["health"].as_str().unwrap_or("");
        if !health.is_empty() && health != "Healthy" {
            out.push(f("problem", "Discos", format!("O disco {name} reporta saúde \"{health}\""), "Faça backup do que importa nesse disco agora e considere trocá-lo."));
        } else if disk["status"].as_str().is_some_and(|s| !s.is_empty() && s != "OK") {
            out.push(f("warn", "Discos", format!("O disco {name} está com estado \"{}\"", disk["status"].as_str().unwrap_or("")), "Verifique o disco (Gerenciamento de Disco)."));
        }
    }
    if d["system_drive_media"].as_str() == Some("HDD") {
        out.push(f("tip", "Discos", "O Windows está num HD (disco mecânico)", "Um SSD deixaria tudo muito mais rápido."));
    }

    // Inicialização
    let startup = d["startup"].as_array().map(|a| a.len()).unwrap_or(0);
    if startup >= 15 {
        out.push(f(
            "warn",
            "Inicialização",
            format!("{startup} programas iniciam junto com o Windows"),
            "Desative os que você não usa: Gerenciador de Tarefas → Aplicativos de inicialização.",
        ));
    } else if startup >= 10 {
        out.push(f("tip", "Inicialização", format!("{startup} programas iniciam junto com o Windows"), "Desativar os que não usa deixa a inicialização mais rápida."));
    }

    // Energia
    let plan = d["power_plan"].as_str().unwrap_or("").to_lowercase();
    if (plan.contains("economia") || plan.contains("saver")) && d["battery"].is_null() {
        out.push(f("warn", "Energia", "O plano de energia está em economia num desktop", "Use o plano Equilibrado ou Alto desempenho."));
    }
    if let Some(h) = num(&d["battery"]["health_pct"]).filter(|h| *h < 70.0) {
        out.push(f("warn", "Energia", format!("A bateria está com {h:.0}% da capacidade original"), "Ela desgastou; a autonomia vai ser menor."));
    }

    // Segurança
    let av = &d["antivirus"];
    if av.is_object() {
        let others = av["products"].as_array().map(|p| p.iter().filter(|x| !x.as_str().unwrap_or("").contains("Defender")).count()).unwrap_or(0);
        if av["defender_enabled"] == false && others == 0 {
            out.push(f("problem", "Segurança", "Nenhum antivírus ativo", "Ative o Microsoft Defender (Segurança do Windows)."));
        } else if av["defender_enabled"] == true && av["realtime"] == false {
            out.push(f("problem", "Segurança", "A proteção em tempo real do Defender está desligada", "Segurança do Windows → Proteção contra vírus → ative a proteção em tempo real."));
        }
        if let Some(age) = num(&av["signature_age_days"]).filter(|a| *a > 7.0) {
            out.push(f("warn", "Segurança", format!("As definições do antivírus têm {age:.0} dias"), "Atualize pela Segurança do Windows."));
        }
    }

    // Estabilidade (últimos 7 dias)
    let ev = &d["events"];
    let count = |k: &str| num(&ev[k]).unwrap_or(0.0) as u64;
    let shutdowns = count("unexpected_shutdowns");
    if shutdowns > 0 {
        out.push(f(
            if shutdowns >= 3 { "problem" } else { "warn" },
            "Estabilidade",
            format!("{shutdowns} desligamento(s) inesperado(s) nos últimos 7 dias"),
            "Pode ser queda de energia, travamento ou fonte; se não foi você, vale investigar.",
        ));
    }
    if count("bluescreens") > 0 {
        out.push(f("problem", "Estabilidade", format!("{} tela(s) azul(is) nos últimos 7 dias", count("bluescreens")), "Atualize drivers (vídeo, chipset) e me peça para olhar de novo depois."));
    }
    if count("hardware_errors") > 0 {
        out.push(f("problem", "Estabilidade", format!("{} erro(s) de hardware registrados (WHEA)", count("hardware_errors")), "Pode ser memória, processador ou overclock instável."));
    }
    let disk_errors = count("disk_errors");
    if disk_errors > 0 {
        out.push(f(
            if disk_errors >= 20 { "problem" } else { "warn" },
            "Discos",
            format!("{disk_errors} erro(s) de disco no log do Windows esta semana"),
            "Faça backup e verifique o disco (chkdsk).",
        ));
    }
    let worst_app = ev["top_crashes"].as_array().and_then(|a| a.first()).and_then(|c| num(&c["count"])).unwrap_or(0.0);
    if count("app_crashes") >= 20 || worst_app >= 3.0 {
        let top: Vec<String> = ev["top_crashes"]
            .as_array()
            .map(|a| a.iter().map(|c| format!("{} ×{}", c["app"].as_str().unwrap_or("?").trim_end_matches(".exe"), c["count"])).collect())
            .unwrap_or_default();
        let who = if top.is_empty() { String::new() } else { format!(" — os que mais travam: {}", top.join(", ")) };
        out.push(f(
            "tip",
            "Estabilidade",
            format!("{} travamentos de programas esta semana{who}", count("app_crashes")),
            "Atualizar ou reinstalar o programa que mais trava costuma resolver.",
        ));
    }

    // Rede
    let net = &d["network"];
    if let Some(loss) = num(&net["loss_pct"]) {
        if loss >= 100.0 {
            out.push(f("problem", "Rede", "Sem resposta da internet (100% de perda)", "Confira o cabo/Wi-Fi e o roteador."));
        } else if loss >= 25.0 {
            out.push(f("warn", "Rede", format!("{loss:.0}% de perda de pacotes"), "Reinicie o roteador; no Wi-Fi, aproxime-se ou use cabo."));
        }
    }
    if let Some(lat) = num(&net["latency_ms"]).filter(|l| *l > 100.0) {
        out.push(f("warn", "Rede", format!("Internet com latência alta ({lat:.0} ms)"), "Jogos e chamadas podem travar; prefira cabo."));
    }
    if let Some(dns) = num(&net["dns_ms"]).filter(|d| *d > 250.0) {
        out.push(f("tip", "Rede", format!("O DNS está lento ({dns:.0} ms para resolver um site)"), "Trocar o DNS para 1.1.1.1 ou 8.8.8.8 costuma deixar a navegação mais ágil."));
    }
    if let Some(sig) = num(&net["wifi_signal"]).filter(|s| *s < 50.0) {
        out.push(f("warn", "Rede", format!("Sinal do Wi-Fi fraco ({sig:.0}%)"), "Aproxime o PC do roteador ou use cabo."));
    }

    // Limpeza
    if let Some(gb) = num(&d["temp_gb"]).filter(|g| *g >= 5.0) {
        out.push(f("tip", "Limpeza", format!("{gb:.0} GB em arquivos temporários"), "Configurações → Sistema → Armazenamento → Arquivos temporários."));
    }

    // O retrato rápido (CPU/memória/discos cheios/GPU agora).
    for text in system::findings(quick) {
        out.push(f("warn", "Agora", text, ""));
    }

    // Problemas primeiro, depois atenção, depois dicas.
    let rank = |l: &str| match l {
        "problem" => 0,
        "warn" => 1,
        _ => 2,
    };
    out.sort_by_key(|x| rank(x.level));
    out
}

/// Linhas da visão geral (montadas pelo código).
fn overview(d: &Value, quick: &system::Snapshot) -> Vec<String> {
    let mut lines = Vec::new();
    let os = &d["os"];
    if os.is_object() {
        lines.push(format!(
            "Windows build {}, ligado há {:.0} h",
            os["build"].as_str().unwrap_or("?"),
            num(&os["uptime_hours"]).unwrap_or(0.0)
        ));
    }
    let cpu = &d["cpu"];
    if cpu.is_object() {
        lines.push(format!(
            "Processador {} ({} núcleos/{} threads), uso {:.0}%",
            cpu["name"].as_str().unwrap_or("?"),
            cpu["cores"],
            cpu["threads"],
            quick.cpu
        ));
    }
    let mem = &d["memory"];
    lines.push(format!(
        "Memória {:.1} de {:.0} GB em uso{}",
        quick.memory_used_gb,
        quick.memory_total_gb,
        if mem.is_object() { format!(", {} pente(s) a {} MHz", mem["modules"], mem["configured_mhz"]) } else { String::new() }
    ));
    if let Some(g) = &quick.gpu {
        lines.push(format!("Vídeo {}: {}% de uso, {} °C, {} de {} MB", g.name, g.util, g.temp_c, g.memory_used_mb, g.memory_total_mb));
    }
    let disks = d["disks"].as_array().map(|a| a.len()).unwrap_or(0);
    let healthy = d["disks"].as_array().map(|a| a.iter().filter(|x| x["health"] == "Healthy").count()).unwrap_or(0);
    if disks > 0 {
        lines.push(format!("{disks} disco(s), {healthy} saudável(is)"));
    }
    let net = &d["network"];
    if net.is_object() {
        lines.push(format!(
            "Internet: {} ms, {}% de perda, DNS {} ms",
            net["latency_ms"].as_f64().map(|v| format!("{v:.0}")).unwrap_or("?".into()),
            net["loss_pct"],
            net["dns_ms"]
        ));
    }
    let av = &d["antivirus"];
    if av.is_object() {
        lines.push(format!(
            "Antivírus: {}",
            if av["realtime"] == true { "proteção em tempo real ativa" } else { "verifique a proteção" }
        ));
    }
    lines.push(format!("{} programa(s) iniciam com o Windows", d["startup"].as_array().map(|a| a.len()).unwrap_or(0)));
    lines
}

async fn collect() -> Result<Value, WinError> {
    // O script vai codificado (UTF-16LE em base64): sem problema de aspas.
    use base64::Engine;
    let utf16: Vec<u8> = SCRIPT.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
    let run = hidden_command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &encoded])
        .output();
    let out = tokio::time::timeout(std::time::Duration::from_secs(60), run)
        .await
        .map_err(|_| WinError::Failed("o checkup demorou demais".into()))?
        .map_err(|e| WinError::Failed(format!("não consegui rodar o checkup: {e}")))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().rev().find(|l| l.trim_start().starts_with('{')).unwrap_or("{}");
    serde_json::from_str(line.trim()).map_err(|e| WinError::Failed(format!("checkup ilegível: {e}")))
}

pub async fn checkup(_params: Value) -> Result<Value, WinError> {
    let (data, quick) = tokio::join!(collect(), async {
        let mut s = blocking(|| Ok(system::measure())).await;
        if let Ok(s) = s.as_mut() {
            s.gpu = system::gpu().await;
        }
        s
    });
    let data = data?;
    let quick = quick?;
    let findings = rules(&data, &quick);
    let count = |l: &str| findings.iter().filter(|x| x.level == l).count();
    Ok(json!({
        "problems": count("problem"),
        "warnings": count("warn"),
        "tips": count("tip"),
        "findings": findings,
        "overview": overview(&data, &quick),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quick() -> system::Snapshot {
        system::Snapshot {
            cpu: 10.0,
            memory_used_gb: 8.0,
            memory_total_gb: 32.0,
            disks: vec![],
            top_cpu: vec![],
            top_memory: vec![],
            gpu: None,
            battery: None,
            internet: Some(true),
            uptime_hours: 1.0,
        }
    }

    #[test]
    fn a_healthy_pc_has_no_findings() {
        let d = json!({
            "os": { "build": "26200", "uptime_hours": 3, "last_update_days": 2, "reboot_pending": false },
            "cpu": { "name": "CPU X", "cores": 8, "threads": 16, "max_mhz": 4000, "current_mhz": 4000, "load": 20 },
            "memory": { "modules": 2, "slots": 4, "total_gb": 32, "rated_mhz": 5600, "configured_mhz": 5600 },
            "gpus": [{ "name": "GPU X", "driver_age_days": 30 }],
            "disks": [{ "name": "SSD X", "media": "SSD", "health": "Healthy", "status": "OK" }],
            "system_drive_media": "SSD",
            "startup": ["a", "b"],
            "power_plan": "Equilibrado",
            "antivirus": { "defender_enabled": true, "realtime": true, "signature_age_days": 0, "products": ["Windows Defender"] },
            "events": { "unexpected_shutdowns": 0, "bluescreens": 0, "hardware_errors": 0, "disk_errors": 0, "app_crashes": 2 },
            "network": { "loss_pct": 0, "latency_ms": 10, "dns_ms": 40, "wifi_signal": null },
            "temp_gb": 1
        });
        assert!(rules(&d, &quick()).is_empty(), "{:?}", rules(&d, &quick()));
        // Poucos travamentos, mas sempre do mesmo programa: vale a dica.
        let mut d2 = d.clone();
        d2["events"]["app_crashes"] = json!(5);
        d2["events"]["top_crashes"] = json!([{ "app": "Tool.exe", "count": 3 }]);
        let f = rules(&d2, &quick());
        assert_eq!(f.len(), 1);
        assert!(f[0].text.contains("os que mais travam: Tool ×3"), "{}", f[0].text);
        assert!(overview(&d, &quick()).iter().any(|l| l.contains("CPU X")));
    }

    #[test]
    fn rules_catch_real_problems_in_order() {
        let d = json!({
            "os": { "reboot_pending": true, "last_update_days": 90, "uptime_hours": 400 },
            "cpu": { "max_mhz": 4000, "current_mhz": 1200, "load": 80 },
            "memory": { "modules": 1, "slots": 2, "rated_mhz": 3200, "configured_mhz": 2133 },
            "gpus": [{ "name": "GPU X", "driver_age_days": 800 }],
            "disks": [{ "name": "HD X", "health": "Warning", "status": "OK" }],
            "system_drive_media": "HDD",
            "startup": ["a","b","c","d","e","f","g","h","i","j","k","l","m","n","o","p"],
            "power_plan": "Economia de energia",
            "antivirus": { "defender_enabled": true, "realtime": false, "signature_age_days": 30, "products": [] },
            "events": { "unexpected_shutdowns": 4, "bluescreens": 1, "hardware_errors": 2, "disk_errors": 25, "app_crashes": 40, "top_crashes": [{ "app": "game.exe", "count": 30 }] },
            "network": { "loss_pct": 50, "latency_ms": 180, "dns_ms": 400, "wifi_signal": 30 },
            "temp_gb": 12
        });
        let found = rules(&d, &quick());
        let all = found.iter().map(|x| x.text.clone()).collect::<Vec<_>>().join(" | ");
        for want in [
            "reinício pendente", "sem atualizações há 90 dias", "ligado há 17 dias", "1200 MHz sob carga", "2133 MHz",
            "Só um pente", "driver de vídeo de GPU X tem 27 meses", "saúde \"Warning\"", "num HD", "16 programas",
            "economia num desktop", "tempo real", "definições do antivírus têm 30 dias", "4 desligamento",
            "tela(s) azul", "erro(s) de hardware", "25 erro(s) de disco", "40 travamentos de programas esta semana — os que mais travam: game ×30", "50% de perda",
            "latência alta (180 ms)", "DNS está lento (400 ms para", "Wi-Fi fraco (30%)", "12 GB em arquivos temporários",
        ] {
            assert!(all.contains(want), "faltou {want:?} em {all}");
        }
        assert_eq!(found[0].level, "problem", "problemas primeiro");
        assert_eq!(found.last().unwrap().level, "tip");
    }
}
