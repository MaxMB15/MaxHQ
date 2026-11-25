// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A power request, which holds whichever thread lets it go, unlike
//! `SetThreadExecutionState`, which holds only while the thread that
//! called it lives, and the power status for the charger.

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::System::Power::{
    GetSystemPowerStatus, PowerClearRequest, PowerCreateRequest, PowerRequestDisplayRequired,
    PowerRequestSystemRequired, PowerSetRequest, POWER_REQUEST_TYPE, SYSTEM_POWER_STATUS,
};
use windows_sys::Win32::System::SystemServices::POWER_REQUEST_CONTEXT_VERSION;
use windows_sys::Win32::System::Threading::{
    POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0,
};

use super::Keep;

/// What a hold's request asks for. The display's keeps the display on
/// and the PC out of idle sleep in one request.
fn kinds(keep: Keep) -> &'static [POWER_REQUEST_TYPE] {
    match keep {
        Keep::System => &[PowerRequestSystemRequired],
        Keep::Display => &[PowerRequestDisplayRequired, PowerRequestSystemRequired],
    }
}

/// A power request, cleared and closed when it's dropped.
pub struct Hold {
    handle: OwnedHandle,
    /// The kinds set on it so far, so a drop clears only those.
    set: &'static [POWER_REQUEST_TYPE],
    /// The reason, as UTF-16, which the system shows while it's held.
    _reason: Vec<u16>,
}

pub const fn supports(_keep: Keep) -> bool {
    true
}

/// Makes the request and sets each of its kinds. When one is refused,
/// dropping the request clears what was set.
#[allow(unsafe_code)]
pub fn take(keep: Keep, reason: &str) -> io::Result<Hold> {
    let mut wide: Vec<u16> = reason.encode_utf16().chain(Some(0)).collect();
    let context = REASON_CONTEXT {
        Version: POWER_REQUEST_CONTEXT_VERSION,
        Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
        Reason: REASON_CONTEXT_0 {
            SimpleReasonString: wide.as_mut_ptr(),
        },
    };
    // SAFETY: `context` names a string that ends in a 0 and outlives the
    // call.
    let handle = unsafe { PowerCreateRequest(&context) };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `handle` is a request this process just made and owns, so
    // closing it once, when this drops, is right.
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    let kinds = kinds(keep);
    let mut hold = Hold {
        handle,
        set: &[],
        _reason: wide,
    };
    for (count, &kind) in kinds.iter().enumerate() {
        // SAFETY: `handle` is a power request that's open.
        if unsafe { PowerSetRequest(hold.handle.as_raw_handle(), kind) } == 0 {
            return Err(io::Error::last_os_error());
        }
        hold.set = &kinds[..=count];
    }
    Ok(hold)
}

impl Drop for Hold {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        for &kind in self.set {
            // SAFETY: the handle is a power request that's open until the
            // OwnedHandle closes it after this, and `kind` was set on it.
            // Closing it would end the request too, so a failure here
            // changes nothing.
            unsafe { PowerClearRequest(self.handle.as_raw_handle(), kind) };
        }
    }
}

#[allow(unsafe_code)]
pub fn on_charger() -> io::Result<bool> {
    let mut status = SYSTEM_POWER_STATUS::default();
    // SAFETY: `status` is a valid place for the status.
    if unsafe { GetSystemPowerStatus(&mut status) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(super::charging(status.ACLineStatus, status.BatteryFlag))
}
