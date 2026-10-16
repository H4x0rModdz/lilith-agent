//! Medidas leves do PC para os avisos proativos (CPU, discos, bateria),
//! feitas de tempos em tempos sem pesar: o mesmo `System` é reaproveitado
//! entre as chamadas, então o uso de CPU é a média desde a medida anterior
//! (sem dormir esperando um intervalo). Os processos só são lidos quando a
//! CPU está alta, para dizer quem está pesando.

use serde::Serialize;
use std::sync::Mutex;
use sysinfo::{Disks, ProcessesToUpdate, System};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DiskSpace {
    pub mount: String,
    pub free_gb: f64,
    pub total_gb: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Vitals {
    /// Uso médio da CPU (%) desde a medida anterior.
    pub cpu: f64,
    /// Programa que mais usa CPU (só quando a CPU passou de `busy_above`).
    pub top_process: Option<String>,
    pub disks: Vec<DiskSpace>,
    /// (porcentagem, na tomada). `None` = sem bateria.
    pub battery: Option<(u32, bool)>,
}

static SYS: Mutex<Option<System>> = Mutex::new(None);

const GB: f64 = 1024.0 * 1024.0 * 1024.0;

/// Mede agora. Bloqueante (rápido); chame fora do runtime async.
pub fn sample(busy_above: f64) -> Vitals {
    let mut guard = SYS.lock().unwrap_or_else(|e| e.into_inner());
    let sys = guard.get_or_insert_with(|| {
        // A primeira leitura de CPU não vale (vem 0 ou 100%): faz uma de base.
        let mut s = System::new();
        s.refresh_cpu_usage();
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL.max(std::time::Duration::from_millis(400)));
        s
    });
    sys.refresh_cpu_usage();
    let cpu = (sys.global_cpu_usage() as f64 * 10.0).round() / 10.0;
    let top_process = (cpu >= busy_above).then(|| top_cpu(sys)).flatten();
    let disks = Disks::new_with_refreshed_list()
        .list()
        .iter()
        .filter(|d| d.total_space() > 0 && !d.is_removable())
        .map(|d| DiskSpace {
            mount: d.mount_point().to_string_lossy().trim_end_matches('\\').to_string(),
            free_gb: (d.available_space() as f64 / GB * 10.0).round() / 10.0,
            total_gb: (d.total_space() as f64 / GB * 10.0).round() / 10.0,
        })
        .collect();
    let battery = crate::handlers::system::battery().map(|b| (b.percent, b.charging));
    Vitals { cpu, top_process, disks, battery }
}

/// Quem mais usa CPU, juntando os processos do mesmo programa. Na primeira
/// leitura os números vêm zerados; a partir da segunda (CPU continua alta)
/// já valem.
fn top_cpu(sys: &mut System) -> Option<String> {
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let mut by_name: std::collections::HashMap<String, f32> = std::collections::HashMap::new();
    for p in sys.processes().values() {
        let name = p.name().to_string_lossy().trim_end_matches(".exe").to_string();
        if name.is_empty() || name == "System Idle Process" || name == "Idle" {
            continue;
        }
        *by_name.entry(name).or_default() += p.cpu_usage();
    }
    by_name
        .into_iter()
        .filter(|(_, c)| *c > 0.0)
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(n, _)| n)
}
