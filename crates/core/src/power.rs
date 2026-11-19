// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! IOKit power assertions, which keep a Mac from sleeping while they're
//! held. `maxd` takes one so its agents keep running, and the app takes
//! them to keep the screen on. Closing the lid still sleeps the Mac under
//! any of them, and macOS lets them all go when the process that took them
//! ends.

use std::ffi::{c_char, c_void, CStr};

use thiserror::Error;

type CfStringRef = *const c_void;
type AssertionId = u32;

/// `kCFStringEncodingUTF8`.
const UTF8: u32 = 0x0800_0100;
/// `kIOPMAssertionLevelOn`.
const LEVEL_ON: u32 = 255;

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
}

/// What an assertion keeps from happening.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prevent {
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

/// Why an assertion couldn't be taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum Refused {
    /// CoreFoundation couldn't make the type or the reason into a string.
    #[error("couldn't name the power assertion")]
    Name,
    /// IOKit said no, with this code.
    #[error("macOS refused the power assertion (code {0})")]
    Code(i32),
}

/// A held assertion. Dropping it lets it go.
#[derive(Debug, PartialEq, Eq)]
pub struct Assertion(AssertionId);

impl Assertion {
    /// Takes an assertion against `prevent`. `reason` is what Activity
    /// Monitor and `pmset -g assertions` show for it.
    #[allow(unsafe_code)]
    pub fn take(prevent: Prevent, reason: &CStr) -> Result<Self, Refused> {
        let kind = CfString::new(prevent.name()).ok_or(Refused::Name)?;
        let name = CfString::new(reason).ok_or(Refused::Name)?;
        let mut id: AssertionId = 0;
        // SAFETY: both strings live until after the call, and `id` is a
        // valid place for the result.
        let code = unsafe { IOPMAssertionCreateWithName(kind.0, LEVEL_ON, name.0, &mut id) };
        if code != 0 {
            return Err(Refused::Code(code));
        }
        Ok(Self(id))
    }

    /// IOKit's id for it, which no other held assertion shares.
    pub fn id(&self) -> u32 {
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
}
