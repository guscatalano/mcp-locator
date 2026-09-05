//! The consent prompt (spec/003 §4).
//!
//! A separate executable rather than a dialog inside the broker, for two reasons. The broker is
//! a headless service that must keep answering other clients while a human decides, and keeping
//! the UI out of it means the broker never links a GUI toolkit. The split also makes the trust
//! story checkable: this binary is the only thing that can turn a request into an approval, it
//! is launched by the broker alone, and its answer travels back as an exit code — there is no
//! input an AI client can supply that reaches this process.
//!
//! Exit codes are the protocol:
//!   10  allow            20  deny            30  dismissed (no decision)
//!    2  usage error       3  no interactive prompt available on this platform
#![cfg_attr(windows, windows_subsystem = "windows")]

pub const ALLOW: i32 = 10;
pub const DENY: i32 = 20;
pub const DISMISSED: i32 = 30;
pub const USAGE: i32 = 2;
pub const UNSUPPORTED: i32 = 3;

/// Everything the dialog shows. The broker fills this in from the card and the connection; none
/// of it is supplied by the client asking for activation.
#[derive(Default)]
pub struct Request {
    pub name: String,
    pub title: String,
    pub summary: String,
    pub command: String,
    pub tier: String,
    pub publisher: String,
    pub client: String,
    pub card: String,
    pub hash: String,
    pub stale: bool,
    pub previous: String,
}

impl Request {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut req = Request::default();
        let mut i = 0;
        while i < args.len() {
            let flag = args[i].as_str();
            if flag == "--stale" {
                req.stale = true;
                i += 1;
                continue;
            }
            let Some(value) = args.get(i + 1) else {
                return Err(format!("{flag} needs a value"));
            };
            match flag {
                "--name" => req.name = value.clone(),
                "--title" => req.title = value.clone(),
                "--summary" => req.summary = value.clone(),
                "--command" => req.command = value.clone(),
                "--tier" => req.tier = value.clone(),
                "--publisher" => req.publisher = value.clone(),
                "--client" => req.client = value.clone(),
                "--card" => req.card = value.clone(),
                "--hash" => req.hash = value.clone(),
                "--previous" => req.previous = value.clone(),
                other => return Err(format!("unknown flag {other}")),
            }
            i += 2;
        }
        if req.name.is_empty() {
            return Err("--name is required".into());
        }
        if req.title.is_empty() {
            req.title = req.name.clone();
        }
        Ok(req)
    }

    /// The heading. A stale record is a different question from a first approval: the user
    /// already said yes once, so the prompt has to lead with what changed rather than repeat
    /// the original ask as if nothing had happened.
    fn instruction(&self) -> String {
        if self.stale {
            format!("\"{}\" has changed since you allowed it", self.title)
        } else {
            format!("Allow AI clients to use \"{}\"?", self.title)
        }
    }

    fn content(&self) -> String {
        let mut out = String::new();
        if self.stale {
            out.push_str(
                "The program it starts is not the one you approved. That can mean an update, \
                 or that something replaced it.\n\n",
            );
            if !self.previous.is_empty() {
                out.push_str(&format!("Was:  {}\n", self.previous));
            }
            out.push_str(&format!("Now:  {}\n\n", self.command));
        }
        if !self.summary.is_empty() {
            out.push_str(&self.summary);
            out.push_str("\n\n");
        }
        if !self.client.is_empty() {
            out.push_str(&format!("Requested by {}.\n", self.client));
        }
        out.push_str(
            "Allowing this applies to every AI client on this machine until you revoke it.",
        );
        out
    }

    /// The detail a suspicious user needs, behind the expander so it does not shout at the user
    /// who just wants to answer the question.
    ///
    /// Laid out with spaces rather than tabs: the task dialog's expanded area does not render a
    /// tab, so a tab-separated `Starts:` came out with the label welded to its value.
    fn expanded(&self) -> String {
        let mut out = String::new();
        if !self.stale && !self.command.is_empty() {
            out.push_str(&format!("Starts:  {}\n", self.command));
        }
        if !self.card.is_empty() {
            out.push_str(&format!("Registered by:  {}\n", self.card));
        }
        if !self.tier.is_empty() {
            out.push_str(&format!("Trust tier:  {}\n", self.tier));
        }
        if !self.publisher.is_empty() {
            out.push_str(&format!("Publisher:  {}\n", self.publisher));
        }
        if !self.hash.is_empty() {
            out.push_str(&format!("Bound to:  {}\n", self.hash));
        }
        out.push_str(&format!("Identifier:  {}", self.name));
        out
    }

    /// A `user`-tier card was written by anything running as this user, which includes anything
    /// the user ever ran by accident. Say so, rather than presenting every registration as
    /// equally vouched-for.
    fn footer(&self) -> String {
        match self.tier.as_str() {
            "system" | "package" => String::new(),
            "low" => "Registered by a sandboxed program, the least trusted source.".into(),
            _ => "Any program running as you can register a server here.".into(),
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let request = match Request::parse(&args) {
        Ok(request) => request,
        Err(message) => {
            eprintln!("mcp-locator-consent: {message}");
            std::process::exit(USAGE);
        }
    };
    std::process::exit(prompt(&request));
}

#[cfg(windows)]
fn prompt(request: &Request) -> i32 {
    dialog::show(request).unwrap_or_else(|code| {
        // A dialog that cannot be shown must never read as approval.
        eprintln!("mcp-locator-consent: TaskDialogIndirect failed (0x{code:08x})");
        DISMISSED
    })
}

/// Until there is a native prompt on this platform, print what would have been asked and
/// refuse. Writing it out is not decoration: the broker logs this stream, so a maintainer
/// bringing up the unix port can see the exact question the dialog would have posed, and a
/// user who hits the refusal learns which server wanted what rather than only that something
/// was denied.
#[cfg(not(windows))]
fn prompt(request: &Request) -> i32 {
    eprintln!("mcp-locator-consent: no interactive prompt on this platform yet");
    eprintln!("{}", request.instruction());
    eprintln!("{}", request.content());
    eprintln!("{}", request.expanded());
    let footer = request.footer();
    if !footer.is_empty() {
        eprintln!("{footer}");
    }
    UNSUPPORTED
}

#[cfg(windows)]
mod dialog {
    use super::{Request, ALLOW, DENY, DISMISSED};
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows_sys::Win32::UI::Controls::{
        TaskDialogIndirect, TASKDIALOGCONFIG, TASKDIALOG_BUTTON, TDCBF_CANCEL_BUTTON,
        TDF_EXPAND_FOOTER_AREA, TDF_USE_COMMAND_LINKS, TDN_CREATED, TD_SHIELD_ICON,
        TD_WARNING_ICON,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetForegroundWindow, SetWindowPos, HWND_TOPMOST, SWP_NOMOVE, SWP_NOSIZE,
    };

    const ID_ALLOW: i32 = 101;
    const ID_DENY: i32 = 102;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Brings the prompt to the front and keeps it there.
    ///
    /// `SetForegroundWindow` alone is not enough and was observed failing on a clean machine:
    /// Windows only grants the foreground to a process that already has it or was handed it, and
    /// the broker is a background service that has neither. The dialog opened *behind* the AI
    /// client that triggered it, where it blocked that client for the full prompt timeout with
    /// nothing on screen to explain why.
    ///
    /// Topmost is the right answer for this particular window rather than a trick: it is a modal
    /// security question that a person has to answer before anything proceeds, which is the same
    /// reason the UAC prompt does not sit politely in the z-order either.
    unsafe extern "system" fn callback(
        hwnd: HWND,
        notification: u32,
        _wparam: WPARAM,
        _lparam: LPARAM,
        _data: isize,
    ) -> windows_sys::core::HRESULT {
        if notification == TDN_CREATED as u32 {
            // SAFETY: `hwnd` is the live dialog, passed in by the task dialog itself.
            unsafe {
                SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
                SetForegroundWindow(hwnd);
            }
        }
        0
    }

    pub fn show(request: &Request) -> Result<i32, i32> {
        let window_title = wide("mcp-locator");
        let instruction = wide(&request.instruction());
        let content = wide(&request.content());
        let expanded = wide(&request.expanded());
        let footer = wide(&request.footer());
        let details = wide("Details");

        let allow = wide(if request.stale {
            "Allow it anyway\nUse the new program. Only do this if you expected it to change."
        } else {
            "Allow\nThe server may be started by any AI client on this machine."
        });
        let deny = wide(if request.stale {
            "Do not allow\nThe server stays blocked until you approve the change."
        } else {
            "Do not allow\nAI clients will not be able to start this server."
        });
        let buttons = [
            TASKDIALOG_BUTTON {
                nButtonID: ID_ALLOW,
                pszButtonText: allow.as_ptr(),
            },
            TASKDIALOG_BUTTON {
                nButtonID: ID_DENY,
                pszButtonText: deny.as_ptr(),
            },
        ];

        let mut config: TASKDIALOGCONFIG = unsafe { std::mem::zeroed() };
        config.cbSize = std::mem::size_of::<TASKDIALOGCONFIG>() as u32;
        config.dwFlags = TDF_USE_COMMAND_LINKS | TDF_EXPAND_FOOTER_AREA;
        config.dwCommonButtons = TDCBF_CANCEL_BUTTON;
        config.pszWindowTitle = window_title.as_ptr();
        config.Anonymous1.pszMainIcon = if request.stale {
            TD_WARNING_ICON
        } else {
            TD_SHIELD_ICON
        };
        config.pszMainInstruction = instruction.as_ptr();
        config.pszContent = content.as_ptr();
        config.cButtons = buttons.len() as u32;
        config.pButtons = buttons.as_ptr();
        // Defaulting to "do not allow" costs a click and stops a stray Enter from approving.
        config.nDefaultButton = ID_DENY;
        config.pszExpandedInformation = expanded.as_ptr();
        config.pszExpandedControlText = details.as_ptr();
        config.pszFooter = if footer.len() > 1 {
            footer.as_ptr()
        } else {
            null()
        };
        config.pfCallback = Some(callback);

        let mut pressed = 0i32;
        // SAFETY: every pointer in `config` refers to a buffer that outlives this call, and the
        // call is synchronous.
        let hr = unsafe { TaskDialogIndirect(&config, &mut pressed, null_mut(), null_mut()) };
        if hr < 0 {
            return Err(hr);
        }
        Ok(match pressed {
            ID_ALLOW => ALLOW,
            ID_DENY => DENY,
            _ => DISMISSED,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Request;

    fn request() -> Request {
        Request {
            name: "com.example.notes".into(),
            title: "Example Notes".into(),
            tier: "user".into(),
            command: "C:\\notes\\notes-mcp.exe --serve".into(),
            ..Request::default()
        }
    }

    #[test]
    fn a_stale_record_asks_a_different_question() {
        let mut req = request();
        assert!(req.instruction().starts_with("Allow AI clients"));

        req.stale = true;
        req.previous = "C:\\notes\\old.exe".into();
        assert!(req.instruction().contains("has changed"));
        // The whole point of the stale prompt is showing the swap, so both sides must appear.
        assert!(req.content().contains("C:\\notes\\old.exe"));
        assert!(req.content().contains("notes-mcp.exe"));
    }

    #[test]
    fn user_tier_carries_a_warning_and_system_tier_does_not() {
        let mut req = request();
        assert!(req.footer().contains("running as you"));
        req.tier = "system".into();
        assert!(req.footer().is_empty());
    }

    #[test]
    fn details_name_the_card_and_its_origin() {
        let mut req = request();
        req.card = "/etc/mcp-locator/servers/com.example.notes.card.json".into();
        let detail = req.expanded();
        // Whoever opens the expander is checking provenance, so the two facts that establish
        // it — what runs and which file asked for it — have to be there.
        assert!(detail.contains("notes-mcp.exe"));
        assert!(detail.contains("com.example.notes.card.json"));
    }

    #[test]
    fn parse_rejects_a_request_without_a_server() {
        assert!(Request::parse(&["--title".into(), "x".into()]).is_err());
        let ok = Request::parse(&["--name".into(), "a.b".into()]).unwrap();
        // An untitled card still needs something to show in the heading.
        assert_eq!(ok.title, "a.b");
    }
}
