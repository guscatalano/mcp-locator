//! The consent store (spec/003 §4).
//!
//! One record per server, written only by the broker and shared by every AI client — that is
//! the whole point of centralizing it: the user answers once, not once per client.
//!
//! Consent binds to `launchHash`, not to the server name. If a card's launch stanza or endpoint
//! changes after approval, the record goes `stale` and the user is asked again. That rule is
//! what stops an approved card being swapped for `cmd.exe` behind the user's back.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConsentState {
    Granted,
    Denied,
    NotAsked,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConsentScope {
    /// Applies to every AI client on the machine.
    User,
    /// Applies only to the client identified in the record.
    Client,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsentRecord {
    pub state: ConsentState,
    #[serde(rename = "grantedAt", skip_serializing_if = "Option::is_none")]
    pub granted_at: Option<String>,
    #[serde(rename = "launchHash", skip_serializing_if = "Option::is_none")]
    pub launch_hash: Option<String>,
    /// The approved command line, kept so a later change can be shown to the user as a
    /// before/after. The hash alone proves something changed but cannot say what.
    #[serde(rename = "launchCommand", skip_serializing_if = "Option::is_none")]
    pub launch_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<ConsentScope>,
}

impl ConsentRecord {
    pub fn not_asked() -> Self {
        Self {
            state: ConsentState::NotAsked,
            granted_at: None,
            launch_hash: None,
            launch_command: None,
            scope: None,
        }
    }
}

#[derive(Debug, Default)]
pub struct ConsentStore {
    path: PathBuf,
    records: BTreeMap<String, ConsentRecord>,
    /// Identity of the file as last read, so an outside change can be noticed. See `refresh`.
    seen: Option<(std::time::SystemTime, u64)>,
}

impl ConsentStore {
    /// Load from `consent.json`. An absent or unreadable store means nothing has been decided
    /// yet — never treat it as denial, or a fresh install would look like a wall of refusals.
    pub fn load(state_dir: &Path) -> Self {
        let mut store = Self {
            path: state_dir.join("consent.json"),
            records: BTreeMap::new(),
            seen: None,
        };
        store.refresh();
        store
    }

    /// Re-read the file if anything else has written it since we last looked.
    ///
    /// The store is shared: `mcp-locator-broker consent grant/deny/forget` writes it directly,
    /// and that is the documented way to script an approval or revoke one. A broker that read it
    /// only at startup got this wrong in both directions — it kept prompting for servers already
    /// approved on disk, and, worse, its next write persisted the stale map and silently erased
    /// those decisions. Losing a security decision without telling anyone is the bad half.
    ///
    /// Identity is (mtime, len) rather than a hash: this runs on every consent read, the file is
    /// written atomically by rename, and a same-instant write of identical length is a race the
    /// refresh-before-write in `put` already closes.
    fn refresh(&mut self) {
        let identity = std::fs::metadata(&self.path)
            .ok()
            .and_then(|m| Some((m.modified().ok()?, m.len())));
        if identity == self.seen && self.seen.is_some() {
            return;
        }
        self.records = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        self.seen = identity;
    }

    /// Consent as it applies to one card. `Granted` decays to `Stale` when the launch stanza has
    /// changed since approval; `Denied` is returned verbatim, never reinterpreted.
    pub fn evaluate(&mut self, name: &str, launch_hash: &str) -> ConsentRecord {
        self.refresh();
        let Some(record) = self.records.get(name) else {
            return ConsentRecord::not_asked();
        };
        if record.state != ConsentState::Granted {
            return record.clone();
        }
        match record.launch_hash.as_deref() {
            Some(stored) if stored == launch_hash => record.clone(),
            _ => ConsentRecord {
                state: ConsentState::Stale,
                ..record.clone()
            },
        }
    }

    pub fn grant(
        &mut self,
        name: &str,
        launch_hash: &str,
        launch_command: &str,
        scope: ConsentScope,
    ) -> std::io::Result<()> {
        self.put(
            name,
            ConsentRecord {
                state: ConsentState::Granted,
                granted_at: Some(rfc3339_now()),
                launch_hash: Some(launch_hash.to_string()),
                launch_command: Some(launch_command.to_string()),
                scope: Some(scope),
            },
        )
    }

    pub fn deny(&mut self, name: &str) -> std::io::Result<()> {
        self.put(
            name,
            ConsentRecord {
                state: ConsentState::Denied,
                granted_at: Some(rfc3339_now()),
                launch_hash: None,
                launch_command: None,
                scope: None,
            },
        )
    }

    pub fn forget(&mut self, name: &str) -> std::io::Result<()> {
        self.refresh();
        self.records.remove(name);
        self.persist()
    }

    pub fn records(&mut self) -> &BTreeMap<String, ConsentRecord> {
        self.refresh();
        &self.records
    }

    /// Read-modify-write. The refresh is what makes this a modification of whatever is currently
    /// on disk rather than a wholesale replacement with a map that may be minutes out of date.
    fn put(&mut self, name: &str, record: ConsentRecord) -> std::io::Result<()> {
        self.refresh();
        self.records.insert(name.to_string(), record);
        self.persist()
    }

    /// Write via a temp file and rename, so a crash mid-write cannot leave a truncated store —
    /// which would read as "nothing decided" and silently re-prompt for everything.
    fn persist(&mut self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = self.path.with_extension("json.tmp");
        std::fs::write(&temp, serde_json::to_string_pretty(&self.records)?)?;
        std::fs::rename(&temp, &self.path)?;
        // Record what we just wrote, so our own write is not mistaken for someone else's on the
        // next refresh and re-read for nothing.
        self.seen = std::fs::metadata(&self.path)
            .ok()
            .and_then(|m| Some((m.modified().ok()?, m.len())));
        Ok(())
    }
}

/// RFC 3339 timestamp in UTC. Hand-rolled rather than pulling in a date library for one format.
pub fn rfc3339_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Howard Hinnant's civil-from-days: days since 1970-01-01 to a proleptic Gregorian date.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &Path) -> ConsentStore {
        ConsentStore::load(dir)
    }

    #[test]
    fn a_decision_written_by_someone_else_is_picked_up() {
        // The broker holds this store for its whole life while `consent grant` writes the same
        // file from a separate process. Missing that meant re-prompting for an approved server.
        let dir = std::env::temp_dir().join(format!("mcp-locator-shared-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut held = store(&dir);
        assert_eq!(
            held.evaluate("com.example.app", "sha256:aaa").state,
            ConsentState::NotAsked
        );

        let mut other = store(&dir);
        other
            .grant(
                "com.example.app",
                "sha256:aaa",
                "app.exe",
                ConsentScope::User,
            )
            .unwrap();

        assert_eq!(
            held.evaluate("com.example.app", "sha256:aaa").state,
            ConsentState::Granted,
            "the long-lived store must see a decision made by another process"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writing_does_not_erase_someone_elses_decision() {
        // The dangerous half: a stale in-memory map persisted wholesale would silently drop an
        // approval granted out of band since the broker started.
        let dir = std::env::temp_dir().join(format!("mcp-locator-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut held = store(&dir);
        held.grant(
            "com.example.first",
            "sha256:a",
            "first.exe",
            ConsentScope::User,
        )
        .unwrap();

        let mut other = store(&dir);
        other
            .grant(
                "com.example.second",
                "sha256:b",
                "second.exe",
                ConsentScope::User,
            )
            .unwrap();

        // `held` has never seen `second`; writing through it must not remove it.
        held.deny("com.example.third").unwrap();

        let reloaded = store(&dir);
        let names: Vec<&String> = reloaded.records.keys().collect();
        assert_eq!(
            names,
            vec![
                "com.example.first",
                "com.example.second",
                "com.example.third"
            ],
            "a write must merge with what is on disk, not replace it"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn absent_store_reads_as_not_asked() {
        let dir = std::env::temp_dir().join("mcp-locator-consent-absent");
        assert_eq!(
            store(&dir).evaluate("com.example.app", "sha256:x").state,
            ConsentState::NotAsked
        );
    }

    #[test]
    fn granted_survives_an_unchanged_launch_stanza() {
        let dir = std::env::temp_dir().join(format!("mcp-locator-consent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = store(&dir);
        s.grant(
            "com.example.app",
            "sha256:aaa",
            "notes.exe --serve",
            ConsentScope::User,
        )
        .unwrap();
        assert_eq!(
            s.evaluate("com.example.app", "sha256:aaa").state,
            ConsentState::Granted
        );
        // A changed launch command must invalidate the approval rather than ride on the name.
        assert_eq!(
            s.evaluate("com.example.app", "sha256:bbb").state,
            ConsentState::Stale
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn denial_is_never_reinterpreted() {
        let dir = std::env::temp_dir().join(format!("mcp-locator-deny-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = store(&dir);
        s.deny("com.example.app").unwrap();
        assert_eq!(
            s.evaluate("com.example.app", "sha256:anything").state,
            ConsentState::Denied
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn timestamps_are_rfc3339() {
        let now = rfc3339_now();
        assert_eq!(now.len(), 20, "{now}");
        assert!(now.ends_with('Z'));
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
    }
}
