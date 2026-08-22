//! Registry and state directory resolution (spec/001 §2). Mirrors `dirs.ts`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Low,
    User,
    System,
    Package,
}

impl Tier {
    /// Higher rank shadows lower when the same `name` appears in multiple tiers.
    pub fn rank(self) -> u8 {
        match self {
            Tier::Package => 3,
            Tier::System => 2,
            Tier::User => 1,
            Tier::Low => 0,
        }
    }

    pub fn parse(s: &str) -> Option<Tier> {
        match s {
            "package" => Some(Tier::Package),
            "system" => Some(Tier::System),
            "user" => Some(Tier::User),
            "low" => Some(Tier::Low),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Root {
    pub tier: Tier,
    pub path: PathBuf,
}

const APP: &str = "mcp-locator";
const SERVERS: &str = "servers";

/// Registry directories in tier order. Missing directories are returned anyway — callers skip
/// what does not exist, and watchers need the paths regardless.
pub fn resolve_roots() -> Vec<Root> {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map(PathBuf::from)
        .unwrap_or_default();

    if cfg!(windows) {
        let program_data = std::env::var("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("C:\\ProgramData"));
        let local_app_data = std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join("AppData").join("Local"));

        vec![
            Root {
                tier: Tier::System,
                path: program_data.join(APP).join(SERVERS),
            },
            Root {
                tier: Tier::User,
                path: local_app_data.join(APP).join(SERVERS),
            },
            // LocalLow carries the low-integrity label, so it is the one place a sandboxed
            // process can register at all (spec/003 §6).
            Root {
                tier: Tier::Low,
                path: home
                    .join("AppData")
                    .join("LocalLow")
                    .join(APP)
                    .join(SERVERS),
            },
        ]
    } else if cfg!(target_os = "macos") {
        vec![
            Root {
                tier: Tier::System,
                path: PathBuf::from("/Library/Application Support")
                    .join(APP)
                    .join(SERVERS),
            },
            Root {
                tier: Tier::User,
                path: home
                    .join("Library/Application Support")
                    .join(APP)
                    .join(SERVERS),
            },
        ]
    } else {
        let xdg_data_home = std::env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".local").join("share"));
        let xdg_data_dirs =
            std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());

        let mut roots: Vec<Root> = xdg_data_dirs
            .split(':')
            .filter(|d| !d.is_empty())
            .map(|d| Root {
                tier: Tier::System,
                path: PathBuf::from(d).join(APP).join(SERVERS),
            })
            .collect();
        roots.push(Root {
            tier: Tier::User,
            path: xdg_data_home.join(APP).join(SERVERS),
        });
        roots
    }
}

/// State directory the broker owns: consent store, audit log, runtime snapshot (spec/002 §5).
pub fn resolve_state_dir() -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map(PathBuf::from)
        .unwrap_or_default();

    if cfg!(windows) {
        std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join("AppData").join("Local"))
            .join(APP)
            .join("state")
    } else if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
            .join(APP)
            .join("state")
    } else {
        std::env::var("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".local").join("state"))
            .join(APP)
    }
}

/// Default pipe/socket address the broker listens on (spec/002 §2).
pub fn default_endpoint() -> String {
    if cfg!(windows) {
        r"\\.\pipe\mcp-locator\broker\v1".to_string()
    } else {
        let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
        format!("{runtime}/mcp-locator-broker-v1.sock")
    }
}
