//! Creating the registry and state directories with the right permissions (spec/003 §6).
//!
//! This lives in the broker rather than in the installer so there is one implementation of the
//! rule, reachable three ways: the MSI runs it elevated at install time, the broker runs the
//! per-user half on every start, and a person can run it by hand to check or repair a machine.
//!
//! Permissions are applied with `icacls` rather than the security APIs directly. That is a
//! deliberate trade: the ACLs are then exactly what an administrator would type, can be read
//! back with the same tool, and the code stays short enough to audit by eye. The cost is a
//! process launch on a path that runs once per install.
//!
//! Well-known SIDs are used throughout instead of account names, because `Administrators` and
//! `Users` are localized and this has to work on a machine in any language.

use crate::dirs::{resolve_roots, resolve_state_dir, Tier};
use std::path::Path;

/// SYSTEM.
const SID_SYSTEM: &str = "*S-1-5-18";
/// Built-in Administrators.
const SID_ADMINS: &str = "*S-1-5-32-544";
/// Authenticated Users — every logged-in account, including low-integrity processes running as
/// one, which must still be able to *read* the catalog.
const SID_AUTHENTICATED: &str = "*S-1-5-11";
/// ALL APPLICATION PACKAGES, so packaged (MSIX/UWP) apps can enumerate too.
const SID_APP_PACKAGES: &str = "*S-1-15-2-1";

pub struct Report {
    pub steps: Vec<String>,
    pub failures: Vec<String>,
}

impl Report {
    fn ok(&mut self, message: impl Into<String>) {
        self.steps.push(message.into());
    }

    fn fail(&mut self, message: impl Into<String>) {
        self.failures.push(message.into());
    }
}

/// Machine-wide setup: the system-tier registry directory, writable only by administrators.
///
/// This is the directory whose contents the client library trusts enough to launch the broker
/// from, so "only administrators can write here" is the property the whole bootstrap rests on
/// (spec/003 §3). Everything else is a convenience.
pub fn machine() -> Report {
    let mut report = Report {
        steps: Vec::new(),
        failures: Vec::new(),
    };

    let Some(system_root) = resolve_roots().into_iter().find(|r| r.tier == Tier::System) else {
        report.fail("no system-tier directory on this platform");
        return report;
    };

    if let Err(e) = std::fs::create_dir_all(&system_root.path) {
        report.fail(format!("create {}: {e}", system_root.path.display()));
        return report;
    }
    report.ok(format!("created {}", system_root.path.display()));

    if !cfg!(windows) {
        return report;
    }

    // `/inheritance:r` is the load-bearing part. ProgramData's own ACL grants users the right to
    // create things; without cutting inheritance, a directory under it can end up writable by
    // the account it is meant to be protected from.
    icacls(
        &mut report,
        &system_root.path,
        &[
            "/inheritance:r",
            "/grant:r",
            &format!("{SID_SYSTEM}:(OI)(CI)F"),
            "/grant:r",
            &format!("{SID_ADMINS}:(OI)(CI)F"),
            "/grant:r",
            &format!("{SID_AUTHENTICATED}:(OI)(CI)RX"),
            "/grant:r",
            &format!("{SID_APP_PACKAGES}:(OI)(CI)RX"),
        ],
        "system-tier registry: administrators write, everyone reads",
    );

    report
}

/// Per-user setup: the user and low tiers, and the broker's own state directory.
///
/// Runs on every `serve`, so it has to be cheap and idempotent — the permission calls only fire
/// when a directory is missing, which is the only moment they can matter.
pub fn user() -> Report {
    let mut report = Report {
        steps: Vec::new(),
        failures: Vec::new(),
    };

    for root in resolve_roots() {
        if root.tier == Tier::System || root.tier == Tier::Package {
            continue;
        }
        let fresh = !root.path.exists();
        if let Err(e) = std::fs::create_dir_all(&root.path) {
            report.fail(format!("create {}: {e}", root.path.display()));
            continue;
        }
        if !fresh {
            continue;
        }
        report.ok(format!("created {}", root.path.display()));

        // The low tier is the one place a sandboxed process may register, which only works if
        // the directory itself carries the low label — LocalLow gives it by inheritance, and
        // setting it explicitly makes the intent legible rather than incidental.
        if cfg!(windows) && root.tier == Tier::Low {
            icacls(
                &mut report,
                &root.path,
                &["/setintegritylevel", "(OI)(CI)Low"],
                "low-tier registry: writable by sandboxed processes",
            );
        }
    }

    let state = resolve_state_dir();
    let fresh = !state.exists();
    if let Err(e) = std::fs::create_dir_all(&state) {
        report.fail(format!("create {}: {e}", state.display()));
        return report;
    }
    if fresh {
        report.ok(format!("created {}", state.display()));
        // The consent store lives here. A low-integrity process that could write it could
        // approve servers on the user's behalf, so the medium label is what makes "sandboxed
        // code may register but never consent" true rather than merely intended.
        if cfg!(windows) {
            icacls(
                &mut report,
                &state,
                &["/setintegritylevel", "(OI)(CI)Medium"],
                "broker state: not writable from a sandbox",
            );
        }
    }

    report
}

#[cfg(windows)]
fn icacls(report: &mut Report, path: &Path, args: &[&str], intent: &str) {
    let output = std::process::Command::new("icacls")
        .arg(path)
        .args(args)
        .output();

    match output {
        Ok(out) if out.status.success() => report.ok(format!("{intent} ({})", path.display())),
        Ok(out) => report.fail(format!(
            "icacls {}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Err(e) => report.fail(format!("icacls {}: {e}", path.display())),
    }
}

#[cfg(not(windows))]
fn icacls(_report: &mut Report, _path: &Path, _args: &[&str], _intent: &str) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_user_setup_is_idempotent() {
        // Second run must not re-apply permissions: `serve` calls this every start, and a
        // machine that re-labels its state directory on every launch would be both slow and
        // capable of clobbering an administrator's deliberate change.
        let first = user();
        assert!(first.failures.is_empty(), "{:?}", first.failures);
        let second = user();
        assert!(second.steps.is_empty(), "{:?}", second.steps);
    }
}
