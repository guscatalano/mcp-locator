//! Authenticode verification (spec/003 §3).
//!
//! Two callers want different things from this. The consent dialog wants a *description* — who
//! signed the program the user is being asked to approve, or the fact that nobody did — and
//! must show it either way rather than hiding an unsigned program behind a blank field. The
//! bootstrap path wants a *decision*, and there the answer has to be conservative: anything
//! other than a good signature is not a signature.
//!
//! The distinction between `Unsigned` and `Invalid` is worth keeping even though both fail a
//! check. Unsigned is the normal state of a program built on someone's laptop; a broken or
//! mismatched signature means the file was modified after signing, which is a different and
//! much louder fact.

use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trust {
    /// Valid chain to a trusted root.
    Signed { signer: String },
    /// No signature at all.
    Unsigned,
    /// Signed, but the signature does not verify: tampered, expired, or an untrusted root.
    Invalid { reason: String },
    /// The check could not be run. Never treat this as either outcome.
    Unknown { reason: String },
}

impl Trust {
    /// Whether this is a signature worth launching on. Only a good one counts — an unverifiable
    /// result must not read as success, because "could not check" is exactly the state an
    /// attacker would like the check to end in.
    pub fn is_trusted(&self) -> bool {
        matches!(self, Trust::Signed { .. })
    }

    /// One line for the consent dialog's Publisher row.
    pub fn describe(&self) -> String {
        match self {
            Trust::Signed { signer } => signer.clone(),
            Trust::Unsigned => "unsigned".to_string(),
            Trust::Invalid { reason } => format!("INVALID SIGNATURE — {reason}"),
            Trust::Unknown { reason } => format!("could not be checked — {reason}"),
        }
    }
}

#[cfg(windows)]
pub fn verify(path: &Path) -> Trust {
    windows_impl::verify(path)
}

/// Signature verification is Authenticode-specific. The macOS port will use `codesign`, and
/// inventing an answer in the meantime would be worse than admitting there is none.
#[cfg(not(windows))]
pub fn verify(_path: &Path) -> Trust {
    Trust::Unknown {
        reason: "no signature verification on this platform yet".to_string(),
    }
}

#[cfg(windows)]
mod windows_impl {
    use super::Trust;
    use std::path::Path;
    use windows_sys::core::GUID;
    use windows_sys::Win32::Foundation::{
        GetLastError, ERROR_SUCCESS, TRUST_E_BAD_DIGEST, TRUST_E_NOSIGNATURE,
        TRUST_E_PROVIDER_UNKNOWN, TRUST_E_SUBJECT_FORM_UNKNOWN, TRUST_E_SUBJECT_NOT_TRUSTED,
    };
    use windows_sys::Win32::Security::Cryptography::{
        CertGetNameStringW, CERT_NAME_SIMPLE_DISPLAY_TYPE,
    };
    use windows_sys::Win32::Security::WinTrust::{
        WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust,
        WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
        WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY,
        WTD_UI_NONE,
    };

    fn wide(path: &Path) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    pub fn verify(path: &Path) -> Trust {
        if !path.is_file() {
            return Trust::Unknown {
                reason: format!("{} is not a file", path.display()),
            };
        }
        let file = wide(path);

        let mut file_info = WINTRUST_FILE_INFO {
            cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: file.as_ptr(),
            hFile: std::ptr::null_mut(),
            pgKnownSubject: std::ptr::null_mut(),
        };

        let mut data: WINTRUST_DATA = unsafe { std::mem::zeroed() };
        data.cbStruct = std::mem::size_of::<WINTRUST_DATA>() as u32;
        // No UI under any circumstances: this runs inside a background service, and a
        // verification call that could block on a dialog would hang the broker.
        data.dwUIChoice = WTD_UI_NONE;
        // Revocation checking would reach the network from a path that must stay fast and work
        // offline. The trade is deliberate and belongs in the spec, not hidden here.
        data.fdwRevocationChecks = WTD_REVOKE_NONE;
        data.dwUnionChoice = WTD_CHOICE_FILE;
        data.dwStateAction = WTD_STATEACTION_VERIFY;
        data.Anonymous = WINTRUST_DATA_0 {
            pFile: &mut file_info,
        };

        let mut action: GUID = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        // SAFETY: `data` and `file_info` outlive both calls; the state handle opened by VERIFY
        // is released by the CLOSE call below on every path.
        let status = unsafe {
            WinVerifyTrust(
                std::ptr::null_mut(),
                &mut action,
                (&mut data as *mut WINTRUST_DATA).cast(),
            )
        };

        let trust = match status {
            0 => match unsafe { signer_name(&data) } {
                Some(signer) => Trust::Signed { signer },
                // A verified file whose signer cannot be read is not something to wave through:
                // the chain checked out, but we cannot say whose it is.
                None => Trust::Unknown {
                    reason: "signature verified but the signer could not be read".to_string(),
                },
            },
            s if s == TRUST_E_NOSIGNATURE => match unsafe { GetLastError() } {
                // WinVerifyTrust reports "no signature" for both an unsigned file and a
                // malformed one; the thread's last error is what separates them.
                e if e == ERROR_SUCCESS
                    || e == TRUST_E_NOSIGNATURE as u32
                    || e == TRUST_E_SUBJECT_FORM_UNKNOWN as u32
                    || e == TRUST_E_PROVIDER_UNKNOWN as u32 =>
                {
                    Trust::Unsigned
                }
                e => Trust::Invalid {
                    reason: format!("0x{e:08x}"),
                },
            },
            s if s == TRUST_E_BAD_DIGEST => Trust::Invalid {
                reason: "the file was modified after it was signed".to_string(),
            },
            s if s == TRUST_E_SUBJECT_NOT_TRUSTED => Trust::Invalid {
                reason: "the signing certificate is not trusted on this machine".to_string(),
            },
            s => Trust::Invalid {
                reason: format!("0x{s:08x}"),
            },
        };

        data.dwStateAction = WTD_STATEACTION_CLOSE;
        // SAFETY: releases the state handle opened above.
        unsafe {
            WinVerifyTrust(
                std::ptr::null_mut(),
                &mut action,
                (&mut data as *mut WINTRUST_DATA).cast(),
            );
        }
        trust
    }

    /// Display name of the leaf certificate that signed the file.
    ///
    /// # Safety
    /// `data` must be a WINTRUST_DATA whose VERIFY call succeeded and whose state handle has not
    /// yet been closed.
    unsafe fn signer_name(data: &WINTRUST_DATA) -> Option<String> {
        let provider = unsafe { WTHelperProvDataFromStateData(data.hWVTStateData) };
        if provider.is_null() {
            return None;
        }
        let signer = unsafe { WTHelperGetProvSignerFromChain(provider, 0, 0, 0) };
        if signer.is_null() || unsafe { (*signer).csCertChain } == 0 {
            return None;
        }
        let chain = unsafe { (*signer).pasCertChain };
        if chain.is_null() {
            return None;
        }
        let cert = unsafe { (*chain).pCert };
        if cert.is_null() {
            return None;
        }

        // Size query first, then the read: the name length is not known in advance.
        let needed = unsafe {
            CertGetNameStringW(
                cert,
                CERT_NAME_SIMPLE_DISPLAY_TYPE,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
            )
        };
        if needed <= 1 {
            return None;
        }
        let mut buffer = vec![0u16; needed as usize];
        let written = unsafe {
            CertGetNameStringW(
                cert,
                CERT_NAME_SIMPLE_DISPLAY_TYPE,
                0,
                std::ptr::null_mut(),
                buffer.as_mut_ptr(),
                needed,
            )
        };
        if written <= 1 {
            return None;
        }
        Some(String::from_utf16_lossy(&buffer[..written as usize - 1]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_is_unknown_not_unsigned() {
        // The difference decides whether a caller refuses loudly or quietly treats the file as
        // an ordinary unsigned build.
        let trust = verify(Path::new("C:/nonexistent/mcp-locator-not-here.exe"));
        assert!(matches!(trust, Trust::Unknown { .. }), "{trust:?}");
        assert!(!trust.is_trusted());
    }

    #[test]
    fn only_a_good_signature_is_trusted() {
        assert!(!Trust::Unsigned.is_trusted());
        assert!(!Trust::Invalid { reason: "x".into() }.is_trusted());
        assert!(!Trust::Unknown { reason: "x".into() }.is_trusted());
        assert!(Trust::Signed {
            signer: "Contoso".into()
        }
        .is_trusted());
    }

    #[test]
    fn every_state_says_something_a_person_can_act_on() {
        assert_eq!(Trust::Unsigned.describe(), "unsigned");
        assert!(Trust::Invalid {
            reason: "modified".into()
        }
        .describe()
        .contains("INVALID"));
        assert_eq!(
            Trust::Signed {
                signer: "Contoso Ltd".into()
            }
            .describe(),
            "Contoso Ltd"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_system_binary_is_signed_by_microsoft() {
        // The one positive case available without a certificate of our own: every Windows
        // machine ships a correctly signed binary, so this proves the chain walk and the name
        // lookup actually work rather than only that the error paths compile.
        let system = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
        let trust = verify(Path::new(&format!("{system}\\System32\\kernel32.dll")));
        match &trust {
            Trust::Signed { signer } => assert!(signer.contains("Microsoft"), "{signer}"),
            other => panic!("expected a valid Microsoft signature, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn our_own_test_binary_is_unsigned() {
        // Distinguishing "no signature" from "bad signature" is the point; a build from this
        // repo has none, and must report exactly that rather than an error.
        let exe = std::env::current_exe().unwrap();
        assert_eq!(verify(&exe), Trust::Unsigned, "{}", exe.display());
    }
}
