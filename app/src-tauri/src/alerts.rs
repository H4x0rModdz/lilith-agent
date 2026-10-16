//! Avisos proativos: download terminou, disco quase cheio, bateria baixa e
//! CPU alta por mais de 2 minutos. Cada um liga/desliga nas Configurações.
//!
//! O código decide tudo com regras fixas e memória do que já avisou (nada de
//! spam): cada aviso tem histerese (só volta a avisar depois de a situação
//! melhorar) e um intervalo mínimo. O aviso sai como notificação do Windows e
//! como nota no chat.

use crate::AppState;
use lilith_memory::Memory;
use lilith_win::vitals::Vitals;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

// ── configurações ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct AlertSettings {
    pub downloads: bool,
    pub disk: bool,
    pub battery: bool,
    pub cpu: bool,
    /// Registrar os programas abertos e sugerir modos a partir da rotina.
    pub routines: bool,
}

const KEYS: [&str; 5] = ["alert_downloads", "alert_disk", "alert_battery", "alert_cpu", "learn_routines"];

impl AlertSettings {
    pub fn load(m: &Memory) -> Self {
        let on = |k: &str| m.setting(k).map(|v| v != "false").unwrap_or(true);
        Self { downloads: on(KEYS[0]), disk: on(KEYS[1]), battery: on(KEYS[2]), cpu: on(KEYS[3]), routines: on(KEYS[4]) }
    }

    pub fn save(&self, m: &Memory) {
        let b = |v: bool| if v { "true" } else { "false" };
        for (k, v) in KEYS.iter().zip([self.downloads, self.disk, self.battery, self.cpu, self.routines]) {
            m.set_setting(k, b(v));
        }
    }
}

#[tauri::command]
pub fn get_alert_settings(state: State<'_, AppState>) -> AlertSettings {
    AlertSettings::load(&state.store)
}

#[tauri::command]
pub fn save_alert_settings(settings: AlertSettings, state: State<'_, AppState>) {
    settings.save(&state.store);
}

// ── entrega ───────────────────────────────────────────────────────────────

/// Nota no chat (aba ativa), sem notificação do Windows.
pub(crate) fn notice(app: &AppHandle, text: &str, icon: &str) {
    let _ = app.emit("lilith://notice", json!({ "text": text, "icon": icon }));
}

/// Notificação do Windows + nota no chat.
fn alert(app: &AppHandle, text: &str, icon: &str) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app.notification().builder().title("Lilith").body(text).show();
    notice(app, text, icon);
}

// ── regras (puras, testáveis) ─────────────────────────────────────────────

/// CPU acima disto conta como "alta"…
pub const CPU_HIGH: f64 = 90.0;
/// …e só volta a contar depois de cair abaixo disto (histerese).
const CPU_CALM: f64 = 75.0;
/// Por quanto tempo seguido antes de avisar.
pub const CPU_SUSTAIN_SECS: u64 = 120;
const CPU_COOLDOWN_SECS: u64 = 30 * 60;
const DISK_COOLDOWN_SECS: u64 = 12 * 3600;
const BATTERY_LOW: u32 = 20;
const BATTERY_CRITICAL: u32 = 10;

/// Mesma regra do diagnóstico (`system.status`): menos de 10% ou de 10 GB livres.
pub fn disk_low(free_gb: f64, total_gb: f64) -> bool {
    total_gb > 0.0 && (free_gb / total_gb * 100.0 < 10.0 || free_gb < 10.0)
}

fn disk_recovered(free_gb: f64, total_gb: f64) -> bool {
    total_gb > 0.0 && free_gb / total_gb * 100.0 >= 12.0 && free_gb >= 12.0
}

/// Memória do que já foi avisado (por tipo).
#[derive(Debug, Default)]
pub struct Watch {
    cpu_high_since: Option<u64>,
    cpu_alerted: bool,
    cpu_last_alert: Option<u64>,
    disk_alerted: HashMap<String, u64>,
    battery_stage: Option<u32>,
}

impl Watch {
    /// Avisos a dar com a medida de agora (`now` em segundos).
    pub fn on_vitals(&mut self, v: &Vitals, now: u64, s: &AlertSettings) -> Vec<String> {
        let mut out = Vec::new();
        if s.cpu {
            if v.cpu >= CPU_HIGH {
                let since = *self.cpu_high_since.get_or_insert(now);
                let cooled = self.cpu_last_alert.is_none_or(|t| now.saturating_sub(t) >= CPU_COOLDOWN_SECS);
                if !self.cpu_alerted && cooled && now.saturating_sub(since) >= CPU_SUSTAIN_SECS {
                    let who = v.top_process.as_ref().map(|p| format!(" Quem mais usa: {p}.")).unwrap_or_default();
                    out.push(format!(
                        "A CPU está acima de {:.0}% há mais de {} minutos (agora {:.0}%).{who} Se quiser, pergunte \"o que tá pesando no pc?\".",
                        CPU_HIGH,
                        CPU_SUSTAIN_SECS / 60,
                        v.cpu
                    ));
                    self.cpu_alerted = true;
                    self.cpu_last_alert = Some(now);
                }
            } else {
                self.cpu_high_since = None;
                if v.cpu < CPU_CALM {
                    self.cpu_alerted = false;
                }
            }
        }
        if s.disk {
            for d in &v.disks {
                if disk_low(d.free_gb, d.total_gb) {
                    let due = self.disk_alerted.get(&d.mount).is_none_or(|t| now.saturating_sub(*t) >= DISK_COOLDOWN_SECS);
                    if due {
                        out.push(format!("Disco {} quase cheio: {:.1} GB livres de {:.0} GB.", d.mount, d.free_gb, d.total_gb));
                        self.disk_alerted.insert(d.mount.clone(), now);
                    }
                } else if disk_recovered(d.free_gb, d.total_gb) {
                    self.disk_alerted.remove(&d.mount);
                }
            }
        }
        if s.battery {
            match v.battery {
                Some((_, true)) | None => self.battery_stage = None,
                Some((pct, false)) if pct <= BATTERY_CRITICAL && self.battery_stage != Some(BATTERY_CRITICAL) => {
                    out.push(format!("Bateria em {pct}%! Liga na tomada logo para não desligar."));
                    self.battery_stage = Some(BATTERY_CRITICAL);
                }
                Some((pct, false)) if pct <= BATTERY_LOW && self.battery_stage.is_none() => {
                    out.push(format!("Bateria baixa: {pct}% e fora da tomada."));
                    self.battery_stage = Some(BATTERY_LOW);
                }
                _ => {}
            }
        }
        out
    }
}

/// Um arquivo da pasta Downloads.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    pub size: u64,
}

const PARTIAL: &[&str] = &[".crdownload", ".part", ".partial", ".tmp", ".download", ".opdownload"];

/// Arquivo de download em andamento (Chrome/Edge, Firefox, outros).
pub fn is_partial(name: &str) -> bool {
    let n = name.to_lowercase();
    PARTIAL.iter().any(|ext| n.ends_with(ext)) || n.starts_with("~$")
}

fn stem_of_partial(name: &str) -> String {
    let n = name.to_lowercase();
    PARTIAL.iter().find_map(|ext| n.strip_suffix(ext)).unwrap_or(&n).to_string()
}

/// Downloads que terminaram entre duas olhadas: um arquivo parcial sumiu e
/// o arquivo final apareceu (ou mudou de tamanho — o Firefox cria o final
/// vazio antes). Parcial que sumiu sem arquivo novo = cancelado, sem aviso.
pub fn finished_downloads(prev: &[Entry], cur: &[Entry]) -> Vec<String> {
    let gone: Vec<&Entry> = prev.iter().filter(|p| is_partial(&p.name) && !cur.iter().any(|c| c.name == p.name)).collect();
    if gone.is_empty() {
        return Vec::new();
    }
    let mut fresh: Vec<&Entry> = cur
        .iter()
        .filter(|c| !is_partial(&c.name) && c.size > 0)
        .filter(|c| prev.iter().find(|p| p.name == c.name).is_none_or(|p| p.size != c.size))
        .collect();
    let mut out = Vec::new();
    let mut unmatched = 0;
    for p in gone {
        let stem = stem_of_partial(&p.name);
        match fresh.iter().position(|c| c.name.to_lowercase() == stem) {
            Some(i) => out.push(fresh.remove(i).name.clone()),
            None => unmatched += 1,
        }
    }
    // "Unconfirmed 123.crdownload" → "arquivo.zip": nomes que não batem.
    out.extend(fresh.into_iter().take(unmatched).map(|c| c.name.clone()));
    out
}

/// Texto do aviso de downloads (agrupado: no máximo um aviso por olhada).
pub fn downloads_text(names: &[String]) -> Option<String> {
    match names {
        [] => None,
        [one] => Some(format!("Download concluído: {one}")),
        many => Some(format!("{} downloads concluídos: {}", many.len(), many.join(", "))),
    }
}

fn list_downloads(dir: &Path) -> Option<Vec<Entry>> {
    let entries = std::fs::read_dir(dir).ok()?;
    Some(
        entries
            .flatten()
            .take(5000)
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                meta.is_file().then(|| Entry { name: e.file_name().to_string_lossy().into_owned(), size: meta.len() })
            })
            .collect(),
    )
}

// ── laço em segundo plano ─────────────────────────────────────────────────

/// Olha a pasta Downloads a cada 10 s; mede o PC a cada 20 s; registra os
/// programas abertos (rotinas) a cada minuto.
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        let started = std::time::Instant::now();
        let downloads = dirs::download_dir();
        let mut prev: Option<Vec<Entry>> = None;
        let mut watch = Watch::default();
        let mut habits = crate::habits::Tracker::default();
        let mut tick: u64 = 0;
        loop {
            let store = app.state::<AppState>().store.clone();
            let settings = AlertSettings::load(&store);
            if settings.downloads {
                if let Some(dir) = downloads.clone() {
                    let cur = tokio::task::spawn_blocking(move || list_downloads(&dir)).await.ok().flatten();
                    if let (Some(before), Some(now)) = (&prev, &cur) {
                        if let Some(text) = downloads_text(&finished_downloads(before, now)) {
                            alert(&app, &text, "download");
                        }
                    }
                    prev = cur;
                }
            } else {
                prev = None;
            }
            if tick % 2 == 0 && (settings.cpu || settings.disk || settings.battery) {
                if let Ok(v) = tokio::task::spawn_blocking(|| lilith_win::vitals::sample(CPU_HIGH)).await {
                    for text in watch.on_vitals(&v, started.elapsed().as_secs(), &settings) {
                        alert(&app, &text, "warn");
                    }
                }
            }
            if tick % 6 == 0 && settings.routines {
                habits.tick(&app, &store).await;
            }
            tick += 1;
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use lilith_win::vitals::DiskSpace;

    fn all_on() -> AlertSettings {
        AlertSettings { downloads: true, disk: true, battery: true, cpu: true, routines: true }
    }

    fn vitals(cpu: f64) -> Vitals {
        Vitals { cpu, top_process: Some("render".into()), disks: vec![], battery: None }
    }

    #[test]
    fn settings_default_on_and_round_trip() {
        let m = Memory::in_memory().unwrap();
        assert_eq!(AlertSettings::load(&m), all_on());
        let off = AlertSettings { cpu: false, ..all_on() };
        off.save(&m);
        assert_eq!(AlertSettings::load(&m), off);
    }

    #[test]
    fn cpu_alert_only_after_two_sustained_minutes_and_once() {
        let mut w = Watch::default();
        let s = all_on();
        assert!(w.on_vitals(&vitals(95.0), 0, &s).is_empty());
        assert!(w.on_vitals(&vitals(97.0), 100, &s).is_empty(), "ainda não fez 2 min");
        let a = w.on_vitals(&vitals(96.0), 120, &s);
        assert_eq!(a.len(), 1);
        assert!(a[0].contains("render"));
        assert!(w.on_vitals(&vitals(96.0), 200, &s).is_empty(), "não repete enquanto continua alta");
        // Um pico curto zera a contagem.
        let mut w = Watch::default();
        w.on_vitals(&vitals(95.0), 0, &s);
        w.on_vitals(&vitals(40.0), 60, &s);
        assert!(w.on_vitals(&vitals(95.0), 130, &s).is_empty());
        // Desligado nas Configurações: nunca.
        let mut w = Watch::default();
        let off = AlertSettings { cpu: false, ..s };
        w.on_vitals(&vitals(99.0), 0, &off);
        assert!(w.on_vitals(&vitals(99.0), 500, &off).is_empty());
    }

    #[test]
    fn cpu_alert_respects_hysteresis_and_cooldown() {
        let mut w = Watch::default();
        let s = all_on();
        w.on_vitals(&vitals(95.0), 0, &s);
        assert_eq!(w.on_vitals(&vitals(95.0), 120, &s).len(), 1);
        w.on_vitals(&vitals(50.0), 200, &s);
        w.on_vitals(&vitals(95.0), 300, &s);
        assert!(w.on_vitals(&vitals(95.0), 500, &s).is_empty(), "intervalo mínimo de 30 min");
        w.on_vitals(&vitals(50.0), 2000, &s);
        w.on_vitals(&vitals(95.0), 2100, &s);
        assert_eq!(w.on_vitals(&vitals(95.0), 2220, &s).len(), 1);
    }

    #[test]
    fn disk_and_battery_warn_once_until_they_recover() {
        let mut w = Watch::default();
        let s = all_on();
        let disk = |free: f64| Vitals {
            cpu: 5.0,
            top_process: None,
            disks: vec![DiskSpace { mount: "C:".into(), free_gb: free, total_gb: 500.0 }],
            battery: None,
        };
        let a = w.on_vitals(&disk(8.0), 0, &s);
        assert_eq!(a, vec!["Disco C: quase cheio: 8.0 GB livres de 500 GB.".to_string()]);
        assert!(w.on_vitals(&disk(7.5), 60, &s).is_empty());
        assert!(w.on_vitals(&disk(100.0), 120, &s).is_empty());
        assert_eq!(w.on_vitals(&disk(8.0), 180, &s).len(), 1, "liberou espaço e encheu de novo: avisa");
        assert_eq!(w.on_vitals(&disk(8.0), 180 + 12 * 3600, &s).len(), 1, "lembra de novo depois de 12 h");

        let bat = |pct: u32, plugged: bool| Vitals { cpu: 5.0, top_process: None, disks: vec![], battery: Some((pct, plugged)) };
        let mut w = Watch::default();
        assert!(w.on_vitals(&bat(50, false), 0, &s).is_empty());
        assert_eq!(w.on_vitals(&bat(19, false), 10, &s).len(), 1);
        assert!(w.on_vitals(&bat(15, false), 20, &s).is_empty());
        assert!(w.on_vitals(&bat(9, false), 30, &s)[0].contains("9%"), "crítica avisa de novo");
        assert!(w.on_vitals(&bat(8, false), 40, &s).is_empty());
        w.on_vitals(&bat(8, true), 50, &s);
        assert_eq!(w.on_vitals(&bat(18, false), 60, &s).len(), 1, "tirou da tomada de novo");
    }

    fn e(name: &str, size: u64) -> Entry {
        Entry { name: name.into(), size }
    }

    #[test]
    fn finished_downloads_from_partial_files() {
        // Chrome/Edge: x.crdownload vira x.
        let before = vec![e("foto.png", 10), e("setup.exe.crdownload", 500)];
        let after = vec![e("foto.png", 10), e("setup.exe", 900)];
        assert_eq!(finished_downloads(&before, &after), vec!["setup.exe"]);
        // Firefox: o final existe vazio enquanto baixa, com o .part ao lado.
        let before = vec![e("doc.pdf", 0), e("doc.pdf.part", 300)];
        let after = vec![e("doc.pdf", 800)];
        assert_eq!(finished_downloads(&before, &after), vec!["doc.pdf"]);
        // "Unconfirmed 123.crdownload" → nome final diferente.
        let before = vec![e("Unconfirmed 123.crdownload", 50)];
        let after = vec![e("video.mp4", 999)];
        assert_eq!(finished_downloads(&before, &after), vec!["video.mp4"]);
        // Cancelado: o parcial some e nada aparece.
        let before = vec![e("x.zip.crdownload", 5)];
        assert!(finished_downloads(&before, &[]).is_empty());
        // Arquivo copiado para a pasta (sem parcial): não é download.
        assert!(finished_downloads(&[], &[e("novo.txt", 3)]).is_empty());
        assert!(finished_downloads(&[e("a.txt", 1)], &[e("a.txt", 1)]).is_empty());
    }

    #[test]
    fn download_alerts_are_grouped() {
        assert_eq!(downloads_text(&[]), None);
        assert_eq!(downloads_text(&["a.zip".into()]).unwrap(), "Download concluído: a.zip");
        assert_eq!(downloads_text(&["a".into(), "b".into()]).unwrap(), "2 downloads concluídos: a, b");
    }
}
