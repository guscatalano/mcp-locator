//! Asking the user (spec/003 §4).
//!
//! The broker never draws the dialog itself; it runs `mcp-locator-consent`, which lives beside
//! it in the install directory, and reads the answer from the exit code. Two properties fall out
//! of that split and both are deliberate:
//!
//! * Nothing an AI client sends reaches the prompt. Every string shown comes from the card on
//!   disk or from the OS (the requesting process name is read from its PID, not self-reported),
//!   so a server cannot write its own consent text to make itself look official.
//! * The helper is found only next to the broker binary. If it is missing, activation fails the
//!   way it always did — `CONSENT_REQUIRED` — rather than proceeding unasked.

use crate::catalog::Entry;
use crate::consent::{ConsentRecord, ConsentState};
use std::path::PathBuf;
use std::time::Duration;

/// How long a prompt may sit unanswered before the broker gives up. The client's `activate` call
/// is blocked for the whole time, so an abandoned dialog must not wedge it forever.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(180);

const HELPER: &str = if cfg!(windows) {
    "mcp-locator-consent.exe"
} else {
    "mcp-locator-consent"
};

// Exit codes, matching crates/consent-ui.
const ALLOW: i32 = 10;
const DENY: i32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
    /// No answer: dialog dismissed, timed out, helper missing, or prompting disabled.
    Unanswered,
}

pub struct Prompter {
    helper: Option<PathBuf>,
}

impl Prompter {
    /// Locate the helper next to this executable. Deliberately not a PATH lookup: PATH is
    /// writable by the user, and a consent dialog is exactly the thing worth impersonating.
    pub fn discover() -> Self {
        if std::env::var("MCP_LOCATOR_NO_PROMPT").is_ok() {
            return Self { helper: None };
        }
        let helper = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join(HELPER)))
            .filter(|path| path.is_file());
        Self { helper }
    }

    pub fn available(&self) -> bool {
        self.helper.is_some()
    }

    /// Whether a consent state is one the user should be asked about. `Denied` is not: the user
    /// already answered, and re-asking on every activation would train them to click through.
    pub fn should_ask(state: ConsentState) -> bool {
        matches!(state, ConsentState::NotAsked | ConsentState::Stale)
    }

    pub async fn ask(
        &self,
        entry: &Entry,
        record: &ConsentRecord,
        client_pid: Option<u32>,
    ) -> Decision {
        let Some(helper) = self.helper.as_ref() else {
            return Decision::Unanswered;
        };

        // A low-integrity process must not be able to raise a consent dialog. It can register
        // and enumerate (spec/003 §6), but letting sandboxed code summon a prompt turns the
        // user's attention into an attack surface: spam the dialog until someone clicks Allow.
        if let Some(pid) = client_pid {
            if platform::is_low_integrity(pid) {
                return Decision::Unanswered;
            }
        }

        let mut command = tokio::process::Command::new(helper);
        command.arg("--name").arg(&entry.name);
        command
            .arg("--title")
            .arg(entry.card.title.as_deref().unwrap_or(&entry.name));
        command.arg("--tier").arg(tier_name(entry));
        command.arg("--card").arg(&entry.path);
        command.arg("--hash").arg(&entry.launch_hash);
        if let Some(summary) = consent_summary(entry) {
            command.arg("--summary").arg(summary);
        }
        command.arg("--command").arg(launch_summary(entry));
        if let Some(client) = client_pid.and_then(describe_client) {
            command.arg("--client").arg(client);
        }
        if record.state == ConsentState::Stale {
            command.arg("--stale");
            if let Some(previous) = record.launch_command.as_deref() {
                command.arg("--previous").arg(previous);
            }
        }

        let Ok(mut child) = command.spawn() else {
            return Decision::Unanswered;
        };
        let status = match tokio::time::timeout(PROMPT_TIMEOUT, child.wait()).await {
            Ok(Ok(status)) => status,
            // Timed out or failed: kill the dialog so an ignored prompt cannot linger on the
            // desktop and be answered long after the request that raised it is gone.
            _ => {
                let _ = child.kill().await;
                return Decision::Unanswered;
            }
        };

        match status.code() {
            Some(ALLOW) => Decision::Allow,
            Some(DENY) => Decision::Deny,
            _ => Decision::Unanswered,
        }
    }
}

fn tier_name(entry: &Entry) -> &'static str {
    use crate::dirs::Tier;
    match entry.tier {
        Tier::Package => "package",
        Tier::System => "system",
        Tier::User => "user",
        Tier::Low => "low",
    }
}

fn consent_summary(entry: &Entry) -> Option<&str> {
    entry
        .card
        .local
        .as_ref()?
        .consent
        .as_ref()?
        .summary
        .as_deref()
}

/// The command line as the user would see it. This is also what gets stored alongside the
/// approval, so a later change can be shown as a before/after rather than two opaque hashes.
pub fn launch_summary(entry: &Entry) -> String {
    match entry.card.local.as_ref().and_then(|l| l.launch.as_ref()) {
        Some(launch) => {
            let args = launch
                .args
                .as_ref()
                .map(|a| a.join(" "))
                .unwrap_or_default();
            format!("{} {}", launch.command, args)
                .trim_end()
                .to_string()
        }
        None => entry
            .card
            .local
            .as_ref()
            .and_then(|l| l.endpoint.as_ref())
            .map(|e| e.address.clone())
            .unwrap_or_else(|| "(no launch stanza)".to_string()),
    }
}

fn describe_client(pid: u32) -> Option<String> {
    platform::process_name(pid).map(|name| format!("{name} (pid {pid})"))
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel,
        TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    /// Everything at or above Medium is an ordinary user process. Below it is AppContainer,
    /// Low, or Untrusted — sandboxes, which spec/003 lets register but never prompt.
    const SECURITY_MANDATORY_MEDIUM_RID: u32 = 0x2000;

    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: only constructed from a successful Open* call.
                unsafe { CloseHandle(self.0) };
            }
        }
    }

    fn open(pid: u32) -> Option<Handle> {
        // SAFETY: plain FFI; the returned handle is wrapped so it is always closed.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        (!handle.is_null()).then_some(Handle(handle))
    }

    /// Full path of the running image, read from the PID. Used for the "requested by" line, so
    /// it must come from the OS rather than from anything the client said about itself.
    pub fn process_name(pid: u32) -> Option<String> {
        let process = open(pid)?;
        let mut buffer = [0u16; 260];
        let mut len = buffer.len() as u32;
        // SAFETY: buffer and len describe the same array.
        let ok = unsafe { QueryFullProcessImageNameW(process.0, 0, buffer.as_mut_ptr(), &mut len) };
        if ok == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buffer[..len as usize]);
        Some(path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string())
    }

    pub fn is_low_integrity(pid: u32) -> bool {
        match integrity_level(pid) {
            Some(level) => level < SECURITY_MANDATORY_MEDIUM_RID,
            // An unreadable token is not evidence of low integrity — a protected or already-exited
            // process reads the same way — so fall back to allowing the prompt.
            None => false,
        }
    }

    fn integrity_level(pid: u32) -> Option<u32> {
        let process = open(pid)?;
        let mut raw_token = std::ptr::null_mut();
        // SAFETY: `process` is live; `raw_token` receives an owned handle.
        if unsafe { OpenProcessToken(process.0, TOKEN_QUERY, &mut raw_token) } == 0 {
            return None;
        }
        let token = Handle(raw_token);

        let mut needed = 0u32;
        // SAFETY: querying the required size with a null buffer is the documented first call.
        unsafe {
            GetTokenInformation(
                token.0,
                TokenIntegrityLevel,
                std::ptr::null_mut(),
                0,
                &mut needed,
            )
        };
        if needed == 0 {
            return None;
        }

        let mut buffer = vec![0u8; needed as usize];
        // SAFETY: the buffer is exactly the size the previous call asked for.
        let ok = unsafe {
            GetTokenInformation(
                token.0,
                TokenIntegrityLevel,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        };
        if ok == 0 {
            return None;
        }

        // SAFETY: on success the buffer holds a TOKEN_MANDATORY_LABEL whose SID points into
        // memory owned by the caller, i.e. `buffer`, which outlives the reads below.
        unsafe {
            let label = &*(buffer.as_ptr() as *const TOKEN_MANDATORY_LABEL);
            let sid = label.Label.Sid;
            let count = GetSidSubAuthorityCount(sid);
            if count.is_null() || *count == 0 {
                return None;
            }
            Some(*GetSidSubAuthority(sid, (*count - 1) as u32))
        }
    }
}

#[cfg(not(windows))]
mod platform {
    /// Integrity levels are a Windows concept. The unix port will use a different rule (session
    /// membership), so refusing to guess here is better than inventing one.
    pub fn is_low_integrity(_pid: u32) -> bool {
        false
    }

    pub fn process_name(pid: u32) -> Option<String> {
        std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .ok()
            .map(|name| name.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denial_is_not_re_asked() {
        assert!(Prompter::should_ask(ConsentState::NotAsked));
        assert!(Prompter::should_ask(ConsentState::Stale));
        assert!(!Prompter::should_ask(ConsentState::Denied));
        assert!(!Prompter::should_ask(ConsentState::Granted));
    }

    #[test]
    fn prompting_can_be_switched_off() {
        std::env::set_var("MCP_LOCATOR_NO_PROMPT", "1");
        assert!(!Prompter::discover().available());
        std::env::remove_var("MCP_LOCATOR_NO_PROMPT");
    }

    #[test]
    fn this_process_is_not_low_integrity() {
        // Sanity check on the token plumbing: the test runner is an ordinary medium-IL process,
        // so a `true` here would mean the SID walk is reading the wrong sub-authority.
        assert!(!platform::is_low_integrity(std::process::id()));
    }

    #[test]
    fn the_running_image_can_be_named() {
        let name = platform::process_name(std::process::id());
        assert!(name.is_some_and(|n| !n.is_empty()));
    }
}
