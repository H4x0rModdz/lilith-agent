use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Log de auditoria em JSONL — uma linha por ação decidida/executada.
pub struct AuditLog {
    path: PathBuf,
    lock: Mutex<()>,
}

impl AuditLog {
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            lock: Mutex::new(()),
        }
    }

    pub fn record(&self, channel: &str, action: &str, params: &Value, outcome: &str, detail: &Value) {
        let line = json!({
            "ts": chrono::Utc::now().to_rfc3339(),
            "channel": channel,
            "action": action,
            "params": params,
            "outcome": outcome,
            "detail": detail,
        });
        let _guard = self.lock.lock().expect("audit lock");
        let result = (|| {
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?;
            writeln!(f, "{line}")
        })();
        if let Err(e) = result {
            tracing::warn!(error = %e, "falha ao gravar auditoria");
        }
    }
}
