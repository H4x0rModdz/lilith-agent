//! `system.status`: como está o PC agora. O código mede tudo (CPU, memória,
//! discos, GPU, bateria, internet, processos que mais pesam) e DECIDE os
//! achados com regras fixas; a IA só os conta em palavras.

use super::{blocking, hidden_command};
use crate::WinError;
use serde::Serialize;
use serde_json::{json, Value};
use sysinfo::{Disks, ProcessesToUpdate, System};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Proc {
    pub name: String,
    pub pid: u32,
    /// % de um núcleo somado (como o Gerenciador de Tarefas, dividido pelos núcleos).
    pub cpu: f64,
    pub memory_mb: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskInfo {
    pub mount: String,
    pub free_gb: f64,
    pub total_gb: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Gpu {
    pub name: String,
    pub util: u32,
    pub memory_used_mb: u64,
    pub memory_total_mb: u64,
    pub temp_c: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Battery {
    pub percent: u32,
    pub charging: bool,
}

/// Fotografia do PC (o que as regras de [`findings`] analisam).
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub cpu: f64,
    pub memory_used_gb: f64,
    pub memory_total_gb: f64,
    pub disks: Vec<DiskInfo>,
    pub top_cpu: Vec<Proc>,
    pub top_memory: Vec<Proc>,
    pub gpu: Option<Gpu>,
    pub battery: Option<Battery>,
    pub internet: Option<bool>,
    pub uptime_hours: f64,
}

const GB: f64 = 1024.0 * 1024.0 * 1024.0;

pub(crate) fn measure() -> Snapshot {
    let mut sys = System::new();
    sys.refresh_memory();
    sys.refresh_cpu_usage();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    // Uso de CPU precisa de duas medidas com um intervalo.
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL.max(std::time::Duration::from_millis(400)));
    sys.refresh_cpu_usage();
    sys.refresh_processes(ProcessesToUpdate::All, true);

    let cores = sys.cpus().len().max(1) as f64;
    // Junta processos do mesmo programa (o Chrome tem dezenas).
    let mut by_name: std::collections::HashMap<String, Proc> = std::collections::HashMap::new();
    for (pid, p) in sys.processes() {
        let name = p.name().to_string_lossy().trim_end_matches(".exe").to_string();
        if name.is_empty() || name == "System Idle Process" || name == "Idle" {
            continue;
        }
        let e = by_name.entry(name.clone()).or_insert(Proc { name, pid: pid.as_u32(), cpu: 0.0, memory_mb: 0 });
        e.cpu += p.cpu_usage() as f64 / cores;
        e.memory_mb += p.memory() / (1024 * 1024);
    }
    let procs: Vec<Proc> = by_name.into_values().collect();
    let mut top_cpu = procs.clone();
    top_cpu.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal));
    top_cpu.truncate(5);
    for p in &mut top_cpu {
        p.cpu = (p.cpu * 10.0).round() / 10.0;
    }
    let mut top_memory = procs;
    top_memory.sort_by(|a, b| b.memory_mb.cmp(&a.memory_mb));
    top_memory.truncate(5);

    let disks = Disks::new_with_refreshed_list()
        .list()
        .iter()
        .filter(|d| d.total_space() > 0)
        .map(|d| DiskInfo {
            mount: d.mount_point().to_string_lossy().trim_end_matches('\\').to_string(),
            free_gb: round1(d.available_space() as f64 / GB),
            total_gb: round1(d.total_space() as f64 / GB),
        })
        .collect();

    Snapshot {
        cpu: round1(sys.global_cpu_usage() as f64),
        memory_used_gb: round1(sys.used_memory() as f64 / GB),
        memory_total_gb: round1(sys.total_memory() as f64 / GB),
        disks,
        top_cpu,
        top_memory,
        gpu: None,
        battery: battery(),
        internet: internet(),
        uptime_hours: round1(System::uptime() as f64 / 3600.0),
    }
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

pub(crate) fn battery() -> Option<Battery> {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut s = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut s).ok()? };
    // 128 = sem bateria (desktop); 255 = desconhecido.
    if s.BatteryFlag & 128 != 0 || s.BatteryLifePercent == 255 {
        return None;
    }
    Some(Battery { percent: s.BatteryLifePercent as u32, charging: s.ACLineStatus == 1 })
}

fn internet() -> Option<bool> {
    use windows::Networking::Connectivity::{NetworkConnectivityLevel, NetworkInformation};
    let profile = NetworkInformation::GetInternetConnectionProfile().ok();
    match profile {
        None => Some(false),
        Some(p) => p.GetNetworkConnectivityLevel().ok().map(|l| l == NetworkConnectivityLevel::InternetAccess),
    }
}

/// Placa NVIDIA (se houver): uso, memória e temperatura pelo `nvidia-smi`.
pub(crate) async fn gpu() -> Option<Gpu> {
    let out = hidden_command("nvidia-smi")
        .args([
            "--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .await
        .ok()?;
    parse_nvidia(&String::from_utf8_lossy(&out.stdout))
}

pub(crate) fn parse_nvidia(csv: &str) -> Option<Gpu> {
    let line = csv.lines().next()?;
    let f: Vec<&str> = line.split(',').map(str::trim).collect();
    if f.len() < 5 {
        return None;
    }
    Some(Gpu {
        name: f[0].to_string(),
        util: f[1].parse().ok()?,
        memory_used_mb: f[2].parse().ok()?,
        memory_total_mb: f[3].parse().ok()?,
        temp_c: f[4].parse().ok()?,
    })
}

/// Regras fixas: o que merece atenção, em frases curtas (pt-BR).
pub fn findings(s: &Snapshot) -> Vec<String> {
    let mut out = Vec::new();
    if s.cpu >= 85.0 {
        let who = s.top_cpu.first().map(|p| format!(" — quem mais usa: {} ({:.0}%)", p.name, p.cpu)).unwrap_or_default();
        out.push(format!("CPU muito alta ({:.0}%){who}", s.cpu));
    }
    for p in s.top_cpu.iter().filter(|p| p.cpu >= 30.0).skip(usize::from(s.cpu >= 85.0)) {
        out.push(format!("{} está usando {:.0}% da CPU", p.name, p.cpu));
    }
    let mem_pct = if s.memory_total_gb > 0.0 { s.memory_used_gb / s.memory_total_gb * 100.0 } else { 0.0 };
    if mem_pct >= 90.0 {
        let who = s.top_memory.first().map(|p| format!(" — quem mais usa: {} ({:.1} GB)", p.name, p.memory_mb as f64 / 1024.0)).unwrap_or_default();
        out.push(format!("Memória quase cheia ({:.0}% de {:.0} GB){who}", mem_pct, s.memory_total_gb));
    }
    for p in s.top_memory.iter().filter(|p| p.memory_mb >= 4096) {
        out.push(format!("{} está usando {:.1} GB de memória", p.name, p.memory_mb as f64 / 1024.0));
    }
    for d in &s.disks {
        let pct_free = if d.total_gb > 0.0 { d.free_gb / d.total_gb * 100.0 } else { 100.0 };
        if pct_free < 10.0 || d.free_gb < 10.0 {
            out.push(format!("Disco {} quase cheio: {:.1} GB livres de {:.0} GB", d.mount, d.free_gb, d.total_gb));
        }
    }
    if let Some(g) = &s.gpu {
        if g.util >= 90 {
            out.push(format!("Placa de vídeo no limite ({}% de uso)", g.util));
        }
        if g.memory_total_mb > 0 && g.memory_used_mb * 100 / g.memory_total_mb >= 90 {
            out.push(format!("Memória da placa de vídeo quase cheia ({} de {} MB)", g.memory_used_mb, g.memory_total_mb));
        }
        if g.temp_c >= 85 {
            out.push(format!("Placa de vídeo quente ({} °C)", g.temp_c));
        }
    }
    if let Some(b) = &s.battery {
        if b.percent <= 20 && !b.charging {
            out.push(format!("Bateria baixa ({}%) e fora da tomada", b.percent));
        }
    }
    if s.internet == Some(false) {
        out.push("Sem acesso à internet agora".into());
    }
    if s.uptime_hours >= 24.0 * 7.0 {
        out.push(format!("O PC está ligado há {:.0} dias sem reiniciar", s.uptime_hours / 24.0));
    }
    out
}

pub async fn status(_params: Value) -> Result<Value, WinError> {
    let mut snap = blocking(|| Ok(measure())).await?;
    snap.gpu = gpu().await;
    let found = findings(&snap);
    let summary = if found.is_empty() { "Tudo normal: nada pesando no momento." } else { "Encontrei pontos de atenção." };
    Ok(json!({ "summary": summary, "findings": found, "snapshot": snap }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Snapshot {
        Snapshot {
            cpu: 20.0,
            memory_used_gb: 8.0,
            memory_total_gb: 32.0,
            disks: vec![DiskInfo { mount: "C:".into(), free_gb: 200.0, total_gb: 500.0 }],
            top_cpu: vec![Proc { name: "chrome".into(), pid: 1, cpu: 5.0, memory_mb: 900 }],
            top_memory: vec![Proc { name: "chrome".into(), pid: 1, cpu: 5.0, memory_mb: 900 }],
            gpu: None,
            battery: None,
            internet: Some(true),
            uptime_hours: 10.0,
        }
    }

    #[test]
    fn a_healthy_pc_has_no_findings() {
        assert!(findings(&base()).is_empty());
    }

    #[test]
    fn rules_flag_what_matters() {
        let mut s = base();
        s.cpu = 95.0;
        s.top_cpu = vec![Proc { name: "renderer".into(), pid: 2, cpu: 70.0, memory_mb: 100 }];
        s.memory_used_gb = 30.0;
        s.top_memory = vec![Proc { name: "chrome".into(), pid: 1, cpu: 1.0, memory_mb: 6000 }];
        s.disks[0].free_gb = 5.0;
        s.battery = Some(Battery { percent: 12, charging: false });
        s.internet = Some(false);
        s.gpu = Some(Gpu { name: "RTX".into(), util: 97, memory_used_mb: 11500, memory_total_mb: 12000, temp_c: 88 });
        let f = findings(&s).join(" | ");
        for want in ["CPU muito alta (95%)", "renderer", "Memória quase cheia", "chrome está usando 5.9 GB", "Disco C: quase cheio", "Bateria baixa (12%)", "Sem acesso à internet", "Placa de vídeo no limite", "Memória da placa de vídeo", "quente (88 °C)"] {
            assert!(f.contains(want), "faltou {want:?} em {f}");
        }
    }

    #[test]
    fn parses_nvidia_smi_csv() {
        let g = parse_nvidia("NVIDIA GeForce RTX 5070, 4, 1557, 12227, 51\n").unwrap();
        assert_eq!((g.util, g.memory_used_mb, g.memory_total_mb, g.temp_c), (4, 1557, 12227, 51));
        assert!(parse_nvidia("").is_none());
    }
}
