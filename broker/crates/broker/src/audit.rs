//! Append-only audit log (spec/003 §7).
//!
//! One JSON object per line answering "what ran, when, and who approved it" — the property the
//! Windows ODR advertises, available here down-level and cross-platform. Failures to write are
//! reported but never block an operation: losing a log line is better than wedging the broker.

use crate::consent::rfc3339_now;
use serde_json::json;
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug)]
pub struct AuditLog {
    path: Option<PathBuf>,
}

impl AuditLog {
    pub fn new(state_dir: &std::path::Path) -> Self {
        Self {
            path: Some(state_dir.join("audit.log")),
        }
    }

    /// A sink that drops everything, for tests that should not touch the user's state directory.
    pub fn disabled() -> Self {
        Self { path: None }
    }

    pub fn record(&self, event: &str, server: &str, client_pid: Option<u32>, detail: &str) {
        let Some(path) = &self.path else { return };

        let line = json!({
            "at": rfc3339_now(),
            "event": event,
            "server": server,
            "clientPid": client_pid,
            "detail": detail,
        });

        if let Err(e) = append(path, &line.to_string()) {
            eprintln!("audit write failed: {e}");
        }
    }
}

fn append(path: &std::path::Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{line}")
}
