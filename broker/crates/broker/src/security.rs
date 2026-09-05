//! Security descriptors for the broker's pipes (spec/003 §5).
//!
//! Two pipes, two different questions.
//!
//! The **broker pipe** has to be reachable by every AI client in the session, including
//! sandboxed ones: discovery is deliberately open, and a low-integrity client that cannot even
//! connect cannot enumerate. A named pipe created with the default descriptor carries the
//! creator's integrity level, so low-integrity processes are refused by the mandatory policy
//! before the DACL is ever consulted — which is why the label has to be lowered explicitly. The
//! DACL still limits it to this user, SYSTEM, and administrators.
//!
//! The **relay pipes** are the opposite case. Each one carries a single client's live session
//! with an activated server, so it is labelled at the requesting client's own integrity level.
//! A lower-integrity process on the same account then cannot write to it, which is what stops a
//! sandboxed process from hijacking a medium-integrity client's grant by guessing the pipe name.
//!
//! Neither of these defends against same-user, same-integrity code: nothing at this layer can,
//! and spec/003 §1 says so plainly rather than implying otherwise.

#[cfg(windows)]
pub use windows::{broker_pipe_sddl, relay_pipe_sddl, SecurityAttributes};

#[cfg(windows)]
mod windows {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::{LocalFree, HANDLE};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
        TOKEN_USER,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// An owned security descriptor, kept alive for as long as the pipe creation call needs it.
    pub struct SecurityAttributes {
        attributes: SECURITY_ATTRIBUTES,
        descriptor: PSECURITY_DESCRIPTOR,
    }

    impl SecurityAttributes {
        /// Build from an SDDL string. Returns `None` if the string will not parse, so callers
        /// can fall back to the default descriptor rather than failing to listen at all — a
        /// broker that refuses to start is a worse outcome than one with tighter-than-intended
        /// permissions, and the fallback is *tighter*, not looser.
        pub fn from_sddl(sddl: &str) -> Option<Self> {
            let wide: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
            let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
            // SAFETY: `wide` is a NUL-terminated UTF-16 buffer; the callee allocates the
            // descriptor with LocalAlloc, which `Drop` releases.
            let ok = unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    wide.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 || descriptor.is_null() {
                return None;
            }
            Some(Self {
                attributes: SECURITY_ATTRIBUTES {
                    nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                    lpSecurityDescriptor: descriptor,
                    bInheritHandle: 0,
                },
                descriptor,
            })
        }

        /// Pointer for `ServerOptions::create_with_security_attributes_raw`. Valid only while
        /// `self` lives.
        pub fn as_ptr(&mut self) -> *mut c_void {
            &mut self.attributes as *mut SECURITY_ATTRIBUTES as *mut c_void
        }
    }

    impl Drop for SecurityAttributes {
        fn drop(&mut self) {
            if !self.descriptor.is_null() {
                // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
                unsafe { LocalFree(self.descriptor) };
            }
        }
    }

    /// SID of the account this process runs as, as a string for use in SDDL.
    fn current_user_sid() -> Option<String> {
        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: plain FFI; the token handle is closed below.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return None;
        }
        let result = (|| {
            let mut needed = 0u32;
            // SAFETY: documented two-call pattern — size query first, then the real read.
            unsafe {
                GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
            }
            if needed == 0 {
                return None;
            }
            let mut buffer = vec![0u8; needed as usize];
            // SAFETY: the buffer is exactly the size the previous call asked for.
            let ok = unsafe {
                GetTokenInformation(
                    token,
                    TokenUser,
                    buffer.as_mut_ptr().cast(),
                    needed,
                    &mut needed,
                )
            };
            if ok == 0 {
                return None;
            }
            // SAFETY: on success the buffer holds a TOKEN_USER whose SID points into it.
            unsafe {
                let user = &*(buffer.as_ptr() as *const TOKEN_USER);
                let mut raw: *mut u16 = std::ptr::null_mut();
                if ConvertSidToStringSidW(user.User.Sid, &mut raw) == 0 || raw.is_null() {
                    return None;
                }
                let mut len = 0;
                while *raw.add(len) != 0 {
                    len += 1;
                }
                let sid = String::from_utf16_lossy(std::slice::from_raw_parts(raw, len));
                LocalFree(raw.cast());
                Some(sid)
            }
        })();
        // SAFETY: opened above and not used after this point.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(token) };
        result
    }

    /// Full access for this user, SYSTEM, and administrators; mandatory label lowered to Low so
    /// sandboxed clients in the same session can still connect and enumerate.
    pub fn broker_pipe_sddl() -> Option<String> {
        let sid = current_user_sid()?;
        Some(format!(
            "D:(A;;GA;;;{sid})(A;;GA;;;SY)(A;;GA;;;BA)S:(ML;;NW;;;LW)"
        ))
    }

    /// Full access for this user only, labelled at the requesting client's integrity level so
    /// nothing below it can write to this grant's pipe.
    ///
    /// Administrators are deliberately not granted here. It buys nothing — an administrator can
    /// take ownership regardless — and leaving them out keeps the descriptor an honest statement
    /// of who the pipe is for.
    pub fn relay_pipe_sddl(client_is_low_integrity: bool) -> Option<String> {
        let sid = current_user_sid()?;
        // A low-integrity client needs its own relay labelled Low, or it could not use the grant
        // it just took.
        let label = if client_is_low_integrity { "LW" } else { "ME" };
        Some(format!("D:(A;;GA;;;{sid})S:(ML;;NW;;;{label})"))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_current_user_has_a_readable_sid() {
            let sid = current_user_sid().expect("a process always runs as someone");
            assert!(sid.starts_with("S-1-"), "{sid}");
        }

        #[test]
        fn both_descriptors_parse() {
            // A descriptor that fails to parse silently downgrades to the default, so the SDDL
            // being well-formed is the whole guarantee here.
            for sddl in [
                broker_pipe_sddl().unwrap(),
                relay_pipe_sddl(false).unwrap(),
                relay_pipe_sddl(true).unwrap(),
            ] {
                assert!(
                    SecurityAttributes::from_sddl(&sddl).is_some(),
                    "failed to parse: {sddl}"
                );
            }
        }

        #[test]
        fn a_malformed_descriptor_is_reported_rather_than_panicking() {
            assert!(SecurityAttributes::from_sddl("this is not sddl").is_none());
        }

        #[test]
        fn the_broker_pipe_admits_low_integrity_and_the_relay_does_not() {
            // The asymmetry is the design: discovery is open to sandboxes, an activated session
            // is not.
            assert!(broker_pipe_sddl().unwrap().contains("(ML;;NW;;;LW)"));
            assert!(relay_pipe_sddl(false).unwrap().contains("(ML;;NW;;;ME)"));
        }
    }
}
