// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! IOKit power assertions, which keep a Mac from sleeping while they're
//! held, and IOKit's battery estimate for the charger. Closing the lid
//! still sleeps the Mac under any of them, and macOS lets them all go when
//! the process that took them ends.

use std::ffi::{c_char, c_void, CStr, CString};
use std::io;

use super::Keep;

type CfStringRef = *const c_void;
type AssertionId = u32;

/// `kCFStringEncodingUTF8`.
const UTF8: u32 = 0x0800_0100;
/// `kIOPMAssertionLevelOn`.
const LEVEL_ON: u32 = 255;
/// `kIOPSTimeRemainingUnlimited`, the estimate on external power and on a
/// Mac with no battery.
const UNLIMITED: f64 = -2.0;

#[allow(unsafe_code)]
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(
        alloc: *const c_void,
        text: *const c_char,
        encoding: u32,
    ) -> CfStringRef;
    fn CFRelease(value: *const c_void);
}

#[allow(unsafe_code)]
#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPMAssertionCreateWithName(
        kind: CfStringRef,
        level: u32,
        name: CfStringRef,
        id: *mut AssertionId,
    ) -> i32;
    fn IOPMAssertionRelease(id: AssertionId) -> i32;
    fn IOPSGetTimeRemainingEstimate() -> f64;
}

/// What an assertion keeps from happening.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prevent {
    /// Sleep after the Mac sits idle. The display still dims and turns off.
    IdleSleep,
    /// The display dimming and then turning off after the Mac sits idle.
    DisplaySleep,
}

impl Prevent {
    /// IOKit's name for the assertion type.
    fn name(self) -> &'static CStr {
        match self {
            Self::IdleSleep => c"PreventUserIdleSystemSleep",
            Self::DisplaySleep => c"PreventUserIdleDisplaySleep",
        }
    }
}

/// The assertions a hold takes, in order. The display's takes two, the
/// display one first.
fn prevents(keep: Keep) -> &'static [Prevent] {
    match keep {
        Keep::System => &[Prevent::IdleSleep],
        Keep::Display => &[Prevent::DisplaySleep, Prevent::IdleSleep],
    }
}

/// A hold's assertions. Dropping them lets them go.
pub struct Hold(#[allow(dead_code)] Vec<Assertion>);

pub const fn supports(_keep: Keep) -> bool {
    true
}

/// Takes each of the hold's assertions. When one is refused, the ones
/// taken before it are let go.
pub fn take(keep: Keep, reason: &str) -> io::Result<Hold> {
    let reason = CString::new(reason).map_err(|_| unnamed())?;
    prevents(keep)
        .iter()
        .map(|&prevent| Assertion::take(prevent, &reason))
        .collect::<io::Result<_>>()
        .map(Hold)
}

pub fn on_charger() -> io::Result<bool> {
    Ok(is_unlimited(time_remaining()))
}

/// The error when CoreFoundation can't make the type or the reason into a
/// string.
fn unnamed() -> io::Error {
    io::Error::other("couldn't name the power assertion")
}

/// A held assertion. Dropping it lets it go.
#[derive(Debug, PartialEq, Eq)]
struct Assertion(AssertionId);

impl Assertion {
    /// Takes an assertion against `prevent`. `reason` is what Activity
    /// Monitor and `pmset -g assertions` show for it.
    #[allow(unsafe_code)]
    fn take(prevent: Prevent, reason: &CStr) -> io::Result<Self> {
        let kind = CfString::new(prevent.name()).ok_or_else(unnamed)?;
        let name = CfString::new(reason).ok_or_else(unnamed)?;
        let mut id: AssertionId = 0;
        // SAFETY: both strings live until after the call, and `id` is a
        // valid place for the result.
        let code = unsafe { IOPMAssertionCreateWithName(kind.0, LEVEL_ON, name.0, &mut id) };
        if code != 0 {
            return Err(io::Error::other(format!(
                "macOS refused the power assertion (code {code})"
            )));
        }
        Ok(Self(id))
    }

    /// IOKit's id for it, which no other held assertion shares.
    #[cfg(test)]
    fn id(&self) -> u32 {
        self.0
    }
}

impl Drop for Assertion {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: the id came from a create call and is released once. It
        // can only fail for an id IOKit doesn't know, so the code is
        // ignored.
        unsafe { IOPMAssertionRelease(self.0) };
    }
}

/// A CoreFoundation string, released when dropped.
struct CfString(CfStringRef);

impl CfString {
    #[allow(unsafe_code)]
    fn new(text: &CStr) -> Option<Self> {
        // SAFETY: `text` ends in a zero byte, and a null allocator means
        // the default one.
        let value = unsafe { CFStringCreateWithCString(std::ptr::null(), text.as_ptr(), UTF8) };
        (!value.is_null()).then_some(Self(value))
    }
}

impl Drop for CfString {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: the string came from a create call and is released once.
        unsafe { CFRelease(self.0) };
    }
}

/// IOKit's estimate of the time left on battery.
#[allow(unsafe_code)]
fn time_remaining() -> f64 {
    // SAFETY: it takes nothing and only reads.
    unsafe { IOPSGetTimeRemainingEstimate() }
}

/// Whether the estimate means there's no battery to run down.
fn is_unlimited(estimate: f64) -> bool {
    estimate == UNLIMITED
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_is_taken_and_let_go() {
        for prevent in [Prevent::IdleSleep, Prevent::DisplaySleep] {
            let first = Assertion::take(prevent, c"MaxHQ's tests").unwrap();
            let second = Assertion::take(prevent, c"MaxHQ's tests").unwrap();
            assert_ne!(first.id(), second.id(), "{prevent:?}");
        }
    }

    #[test]
    fn each_hold_takes_its_assertions() {
        assert_eq!(take(Keep::System, "MaxHQ's tests").unwrap().0.len(), 1);
        assert_eq!(take(Keep::Display, "MaxHQ's tests").unwrap().0.len(), 2);
        let err = take(Keep::System, "a\0reason").err().unwrap();
        assert_eq!(err.to_string(), "couldn't name the power assertion");
    }

    #[test]
    fn only_an_unlimited_estimate_reads_as_the_charger() {
        assert!(is_unlimited(UNLIMITED));
        // Still working it out, and then the time left.
        assert!(!is_unlimited(-1.0));
        assert!(!is_unlimited(0.0));
        assert!(!is_unlimited(7200.0));
        let now = time_remaining();
        assert!(now == UNLIMITED || now == -1.0 || now >= 0.0, "{now}");
    }
}
