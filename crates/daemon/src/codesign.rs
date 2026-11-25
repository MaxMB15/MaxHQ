// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The Mac's code signature check, for a connection that asks the device
//! to trust a new app key.
//!
//! `maxd` takes the audit token of the process on the other end with
//! `LOCAL_PEERTOKEN` as it accepts the connection, before the hello and its
//! challenge go out, and again when the key comes. Both must match, so a
//! program that connects and then execs the app, or hands the socket to it,
//! fails. The token, never the pid, then goes to
//! `SecCodeCopyGuestWithAttributes` and `SecCodeCheckValidity`, against a
//! requirement built from `maxd`'s own team: Apple issued the certificate,
//! to that team, for the app's identifier, and the app runs with the
//! hardened runtime. A `maxd` with no team, unsigned or off the Mac, has
//! the check off.
//!
//! On Windows the check is off too, as on Linux, so a key beyond the first
//! asks you. What a Windows connection proves is that its program runs as
//! you, since the pipe's access list lets in your user alone. The admission
//! card names the program it runs, from its image path, and whether it's
//! `maxd`'s own. That says which file Windows started, but not that it's
//! MaxHQ: any program of yours may replace a program in your folders, or
//! run code inside one, as nothing like the hardened runtime keeps it out.
//! Checking the app's Authenticode signature would mean something once the
//! Windows app is signed.

#[cfg(target_os = "macos")]
use std::os::fd::AsFd;
use std::path::PathBuf;

use crate::app_keys::Signature;

/// The app's code signing identifier.
#[cfg(target_os = "macos")]
const APP_IDENTIFIER: &str = "com.maxagenthq.maxhq";

/// The audit token of the process on the other end of a socket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token([u8; 32]);

/// What the program on the other end must be.
#[derive(Debug)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct Policy {
    /// A code requirement, in Apple's requirement language.
    requirement: String,
    /// Whether the program must run with the hardened runtime, which keeps
    /// other code out of it.
    runtime: bool,
    /// The requirement every program your certificate signed meets, for
    /// the admission card to say so.
    signer: String,
}

/// The program on the other end of a connection, as an admission card
/// describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct Program {
    /// Where it is on disk, when the system says.
    pub path: Option<PathBuf>,
    /// Whether your certificate signed it.
    pub signed: Signed,
}

/// Whether your certificate, the one that signed this `maxd`, signed a
/// program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub enum Signed {
    /// It did.
    Yours,
    /// It didn't, or the system couldn't say which program it is.
    NotYours,
    /// This `maxd` has no team to check against.
    Unchecked,
}

/// The program `token` names, for the admission card: where it is, and
/// whether your certificate signed it. Blocks on the Security framework.
#[cfg(target_os = "macos")]
pub fn program(token: Option<Token>) -> Program {
    describe(token, Policy::for_this_maxd())
}

/// There's no signature to read on Windows, so only the path tells, and
/// [`crate::admission`] finds that from the process.
#[cfg(windows)]
pub fn program(_token: Option<Token>) -> Program {
    Program {
        path: None,
        signed: Signed::Unchecked,
    }
}

/// `maxd`'s own program at `path`, for a connection from `maxd` itself.
/// Whatever signed this `maxd` signed it, so the Security framework needn't
/// look, which takes it seconds on a large program.
#[cfg(any(target_os = "macos", windows))]
pub fn own(path: Option<PathBuf>) -> Program {
    let signed = match Policy::for_this_maxd() {
        Some(_) => Signed::Yours,
        None => Signed::Unchecked,
    };
    Program { path, signed }
}

/// The program `token` names, checked against `policy`'s signer.
#[cfg(target_os = "macos")]
fn describe(token: Option<Token>, policy: Option<&Policy>) -> Program {
    let code = token.and_then(|token| mac::guest(&token).ok());
    let path = code.as_ref().and_then(mac::path);
    let signed = match (policy, &code) {
        (None, _) => Signed::Unchecked,
        (Some(policy), Some(code)) if mac::meets(code, &policy.signer).is_ok() => Signed::Yours,
        (Some(_), _) => Signed::NotYours,
    };
    Program { path, signed }
}

/// The audit token of the process on the other end of `socket`, or none
/// off the Mac or when the kernel gives none.
#[cfg(target_os = "macos")]
pub fn token(socket: &impl AsFd) -> Option<Token> {
    mac::token(socket.as_fd())
}

/// There's no audit token off the Mac.
#[cfg(not(target_os = "macos"))]
pub fn token<S: ?Sized>(_socket: &S) -> Option<Token> {
    None
}

/// What the check says about the program on the other end of a
/// connection, from its token when it was accepted and its token now.
pub async fn check(accepted: Option<Token>, now: Option<Token>) -> Signature {
    let Some(policy) = Policy::for_this_maxd() else {
        return Signature::Off;
    };
    tokio::task::spawn_blocking(move || judge(accepted, now, policy))
        .await
        .unwrap_or(Signature::Failed)
}

/// Whether the program the two tokens name passes `policy`. They must name
/// the same process image, since the challenge went out after the first.
fn judge(accepted: Option<Token>, now: Option<Token>, policy: &Policy) -> Signature {
    match (accepted, now) {
        (Some(accepted), Some(now)) if accepted == now && passes(&now, policy) => Signature::Passed,
        _ => Signature::Failed,
    }
}

#[cfg(target_os = "macos")]
fn passes(token: &Token, policy: &Policy) -> bool {
    match mac::passes(token, policy) {
        Ok(()) => true,
        Err(err) => {
            tracing::info!(%err, "a program asked to add an app key without the app's signature");
            false
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn passes(_token: &Token, _policy: &Policy) -> bool {
    false
}

impl Policy {
    /// The app signed by `team`.
    #[cfg(target_os = "macos")]
    fn app(team: &str) -> Self {
        Self {
            requirement: format!(
                "anchor apple generic and identifier \"{APP_IDENTIFIER}\" and certificate \
                 leaf[subject.OU] = \"{team}\""
            ),
            runtime: true,
            signer: format!("anchor apple generic and certificate leaf[subject.OU] = \"{team}\""),
        }
    }

    /// The policy this `maxd` checks with, from its own signature, or none
    /// when it has no team.
    #[cfg(target_os = "macos")]
    fn for_this_maxd() -> Option<&'static Policy> {
        static POLICY: std::sync::OnceLock<Option<Policy>> = std::sync::OnceLock::new();
        POLICY
            .get_or_init(|| match mac::own_team() {
                Ok(None) => None,
                Ok(Some(team)) if !team.is_empty() && team.chars().all(|c| c.is_ascii_alphanumeric()) => {
                    Some(Self::app(&team))
                }
                // A signed maxd that can't say its team passes nobody,
                // rather than taking the first key offered.
                Ok(Some(team)) => {
                    tracing::warn!(%team, "maxd's own team isn't one, so no program passes its signature check");
                    Some(Self::nobody())
                }
                Err(err) => {
                    tracing::warn!(%err, "maxd couldn't read its own signature, so no program passes its signature check");
                    Some(Self::nobody())
                }
            })
            .as_ref()
    }

    /// There's no check off the Mac.
    #[cfg(not(target_os = "macos"))]
    fn for_this_maxd() -> Option<&'static Policy> {
        None
    }

    /// A policy no program passes.
    #[cfg(target_os = "macos")]
    fn nobody() -> Self {
        Self {
            requirement: "never".to_owned(),
            runtime: true,
            signer: "never".to_owned(),
        }
    }
}

/// The calls into the Security framework.
#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;
    use std::os::fd::{AsRawFd, BorrowedFd};
    use std::path::PathBuf;

    use core_foundation::base::{CFType, TCFType};
    use core_foundation::data::CFData;
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::number::CFNumber;
    use core_foundation::string::{CFString, CFStringRef};
    use security_framework::os::macos::code_signing::{
        Flags, GuestAttributes, SecCode, SecRequirement,
    };

    use super::{Policy, Token};

    /// `kSecCSSigningInformation`: the signing information asks for the
    /// certificates and the team.
    const SIGNING_INFORMATION: u32 = 1 << 1;
    /// `kSecCodeSignatureRuntime`: the hardened runtime is on.
    const RUNTIME: i64 = 0x1_0000;

    #[allow(unsafe_code)]
    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        static kSecCodeInfoTeamIdentifier: CFStringRef;
        static kSecCodeInfoFlags: CFStringRef;
        /// Takes a static code, or a code it finds the static code of.
        fn SecCodeCopySigningInformation(
            code: *const c_void,
            flags: u32,
            information: *mut CFDictionaryRef,
        ) -> i32;
    }

    #[allow(unsafe_code)]
    pub(super) fn token(socket: BorrowedFd<'_>) -> Option<Token> {
        let mut token = [0u8; 32];
        let mut len = libc::socklen_t::try_from(token.len()).ok()?;
        // SAFETY: the buffer is `len` bytes, and the call writes at most
        // that many and says how many it wrote.
        let got = unsafe {
            libc::getsockopt(
                socket.as_raw_fd(),
                libc::SOL_LOCAL,
                libc::LOCAL_PEERTOKEN,
                token.as_mut_ptr().cast(),
                &raw mut len,
            )
        };
        (got == 0 && usize::try_from(len).ok() == Some(token.len())).then_some(Token(token))
    }

    /// Checks the process `token` names against `policy`, or says why it
    /// fails.
    pub(super) fn passes(token: &Token, policy: &Policy) -> Result<(), String> {
        let code = guest(token)?;
        meets(&code, &policy.requirement)?;
        if policy.runtime {
            let key = info_key(KeyName::Flags);
            let flags = signing_information(&code)?
                .find(&key)
                .and_then(|value| value.downcast::<CFNumber>())
                .and_then(|flags| flags.to_i64())
                .unwrap_or(0);
            if flags & RUNTIME == 0 {
                return Err("the hardened runtime is off".to_owned());
            }
        }
        Ok(())
    }

    /// The code of the process `token` names.
    pub(super) fn guest(token: &Token) -> Result<SecCode, String> {
        let token = CFData::from_buffer(&token.0);
        let mut attributes = GuestAttributes::new();
        attributes.set_audit_token(token.as_concrete_TypeRef());
        SecCode::copy_guest_with_attribues(None, &attributes, Flags::NONE)
            .map_err(|err| format!("no code for the process: {err}"))
    }

    /// Where `code` is on disk.
    pub(super) fn path(code: &SecCode) -> Option<PathBuf> {
        code.path(Flags::NONE).ok()?.to_path()
    }

    /// Checks `code` against `requirement`, or says why it fails.
    pub(super) fn meets(code: &SecCode, requirement: &str) -> Result<(), String> {
        let requirement: SecRequirement = requirement
            .parse()
            .map_err(|err| format!("the requirement doesn't parse: {err}"))?;
        code.check_validity(Flags::NONE, &requirement)
            .map_err(|err| format!("the signature doesn't pass: {err}"))
    }

    /// The team in this process's own signature, or none when it has none.
    pub(super) fn own_team() -> Result<Option<String>, String> {
        let code = SecCode::for_self(Flags::NONE).map_err(|err| err.to_string())?;
        let key = info_key(KeyName::Team);
        Ok(signing_information(&code)?
            .find(&key)
            .and_then(|value| value.downcast::<CFString>())
            .map(|team| team.to_string()))
    }

    /// Which entry of the signing information to read.
    enum KeyName {
        Team,
        Flags,
    }

    #[allow(unsafe_code)]
    fn info_key(name: KeyName) -> CFString {
        // SAFETY: the keys are constants the framework keeps for the life
        // of the process, so taking them under the get rule retains them.
        unsafe {
            let key = match name {
                KeyName::Team => kSecCodeInfoTeamIdentifier,
                KeyName::Flags => kSecCodeInfoFlags,
            };
            CFString::wrap_under_get_rule(key)
        }
    }

    #[allow(unsafe_code)]
    fn signing_information(code: &SecCode) -> Result<CFDictionary<CFString, CFType>, String> {
        let mut information: CFDictionaryRef = std::ptr::null();
        // SAFETY: `code` is a live code object, which the call accepts in
        // place of a static one, and on success it writes a dictionary the
        // caller owns into `information`.
        let status = unsafe {
            SecCodeCopySigningInformation(
                code.as_CFTypeRef(),
                SIGNING_INFORMATION,
                &raw mut information,
            )
        };
        if status != 0 || information.is_null() {
            return Err(format!("no signing information, status {status}"));
        }
        // SAFETY: a Copy call's result is the caller's to release, which
        // the create rule does on drop.
        Ok(unsafe { CFDictionary::wrap_under_create_rule(information) })
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests;
