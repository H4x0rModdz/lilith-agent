//! Ficha técnica do PC para os relatórios de bug: só o que descreve a máquina
//! (Windows, CPU, RAM, GPU, navegador padrão e versões). Nada que identifique
//! a pessoa — sem nome de usuário, nome do PC, caminhos ou números de série.

use crate::browsers::{self, parse_reg_value, reg_query};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Specs {
    /// Ex.: "Windows 11 25H2 (build 26200)".
    pub os: String,
    pub cpu: String,
    pub cores: u32,
    pub ram_gb: u32,
    pub gpu: Option<String>,
    /// Nome do executável do navegador padrão (ex.: "chrome", "waterfox").
    pub browser: Option<String>,
    pub browser_version: Option<String>,
}

const NT: &str = r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion";
const CPU: &str = r"HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0";
/// Classe de dispositivo "Adaptadores de vídeo".
const DISPLAY_CLASS: &str = r"HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";

fn reg(key: &str, value: &str) -> Option<String> {
    reg_query(key, &["/v", value]).and_then(|o| parse_reg_value(&o))
}

/// Mede tudo. Bloqueante (alguns `reg query`); chame fora do runtime async.
pub fn collect() -> Specs {
    let build = reg(NT, "CurrentBuildNumber").and_then(|b| b.parse::<u32>().ok()).unwrap_or(0);
    let display = reg(NT, "DisplayVersion");
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let browser = browsers::default_browser();
    Specs {
        os: os_name(build, display.as_deref()),
        cpu: reg(CPU, "ProcessorNameString").map(|c| c.split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default(),
        cores: std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(0),
        ram_gb: (sys.total_memory() as f64 / (1024.0 * 1024.0 * 1024.0)).round() as u32,
        gpu: reg_query(DISPLAY_CLASS, &["/s", "/v", "DriverDesc"]).and_then(|o| pick_gpu(&o)),
        browser_version: browser.as_ref().and_then(|b| browser_version(&b.exe)),
        browser: browser.map(|b| b.id),
    }
}

/// O registro diz "Windows 10" até no 11; o build decide.
fn os_name(build: u32, display: Option<&str>) -> String {
    let name = if build >= 22000 { "Windows 11" } else { "Windows 10" };
    match display {
        Some(d) => format!("{name} {d} (build {build})"),
        None => format!("{name} (build {build})"),
    }
}

/// Placa de vídeo de verdade entre os adaptadores listados (os virtuais e o
/// adaptador básico da Microsoft ficam de fora; dedicada vem antes).
fn pick_gpu(reg_output: &str) -> Option<String> {
    const VIRTUAL: &[&str] = &["microsoft basic", "remote", "virtual", "parsec", "meta", "citrix", "idd"];
    let names: Vec<String> = reg_output
        .lines()
        .filter(|l| l.contains("DriverDesc"))
        .filter_map(parse_reg_value)
        .filter(|n| !VIRTUAL.iter().any(|v| n.to_lowercase().contains(v)))
        .collect();
    let dedicated = ["nvidia", "geforce", "radeon rx", "arc "];
    names
        .iter()
        .find(|n| dedicated.iter().any(|d| n.to_lowercase().contains(d)))
        .or(names.first())
        .cloned()
}

/// Versão do navegador sem abrir nada: o Chromium guarda cada versão numa
/// pasta ao lado do executável ("141.0.7390.55"); o Firefox e os forks
/// escrevem `Version=` no `application.ini`.
pub fn browser_version(exe: &Path) -> Option<String> {
    let dir = exe.parent()?;
    let from_dirs = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| is_version(n))
        .max_by(|a, b| version_key(a).cmp(&version_key(b)));
    from_dirs.or_else(|| {
        let ini = std::fs::read_to_string(dir.join("application.ini")).ok()?;
        ini.lines().find_map(|l| l.strip_prefix("Version=")).map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
    })
}

fn is_version(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    (2..=4).contains(&parts.len()) && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

fn version_key(s: &str) -> Vec<u64> {
    s.split('.').filter_map(|p| p.parse().ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_windows_by_build() {
        assert_eq!(os_name(26200, Some("25H2")), "Windows 11 25H2 (build 26200)");
        assert_eq!(os_name(19045, None), "Windows 10 (build 19045)");
    }

    #[test]
    fn picks_the_real_gpu() {
        let out = "\
HKEY_LOCAL_MACHINE\\...\\0000
    DriverDesc    REG_SZ    Microsoft Basic Display Adapter

HKEY_LOCAL_MACHINE\\...\\0001
    DriverDesc    REG_SZ    Intel(R) UHD Graphics 770

HKEY_LOCAL_MACHINE\\...\\0002
    DriverDesc    REG_SZ    NVIDIA GeForce RTX 5070
";
        assert_eq!(pick_gpu(out).as_deref(), Some("NVIDIA GeForce RTX 5070"));
        assert_eq!(pick_gpu("    DriverDesc    REG_SZ    Intel(R) UHD Graphics 770").as_deref(), Some("Intel(R) UHD Graphics 770"));
        assert_eq!(pick_gpu("    DriverDesc    REG_SZ    Microsoft Basic Display Adapter"), None);
    }

    #[test]
    fn reads_browser_version_from_folders_or_ini() {
        let root = std::env::temp_dir().join(format!("lilith-specs-{}", std::process::id()));
        let chromium = root.join("chrome");
        std::fs::create_dir_all(chromium.join("140.0.7339.80")).unwrap();
        std::fs::create_dir_all(chromium.join("141.0.7390.55")).unwrap();
        std::fs::create_dir_all(chromium.join("Locales")).unwrap();
        assert_eq!(browser_version(&chromium.join("chrome.exe")).as_deref(), Some("141.0.7390.55"));

        let firefox = root.join("waterfox");
        std::fs::create_dir_all(&firefox).unwrap();
        std::fs::write(firefox.join("application.ini"), "[App]\nVendor=Waterfox\nVersion=6.6.4\n").unwrap();
        assert_eq!(browser_version(&firefox.join("waterfox.exe")).as_deref(), Some("6.6.4"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn version_folders_only() {
        assert!(is_version("141.0.7390.55") && is_version("6.6"));
        assert!(!is_version("Locales") && !is_version("1") && !is_version("1.a"));
    }
}

#[cfg(test)]
mod live {
    /// `cargo test -p lilith-win specs::live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn this_pc() {
        println!("{:#?}", super::collect());
    }
}
