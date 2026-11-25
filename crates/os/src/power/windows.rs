// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::io;
use std::os::windows::io::OwnedHandle;

use windows_sys::Win32::System::Power::{
    GetSystemPowerStatus, PowerClearRequest, PowerCreateRequest, PowerRequestSystemRequired,
    PowerSetRequest, SYSTEM_POWER_STATUS,
};
use windows_sys::Win32::System::SystemServices::POWER_REQUEST_CONTEXT_VERSION;
use windows_sys::Win32::System::Threading::{
    POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0,
};

use crate::win;

/// A power request that keeps the system required, cleared and closed
/// when it's dropped.
#[derive(Debug)]
pub struct Request {
    handle: OwnedHandle,
    /// The reason, as UTF-16, which the system shows while it's held.
    _reason: Vec<u16>,
}

#[allow(unsafe_code)]
pub fn take(reason: &str) -> io::Result<Request> {
    let mut wide: Vec<u16> = reason.encode_utf16().chain(Some(0)).collect();
    let context = REASON_CONTEXT {
        Version: POWER_REQUEST_CONTEXT_VERSION,
        Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
        Reason: REASON_CONTEXT_0 {
            SimpleReasonString: wide.as_mut_ptr(),
        },
    };
    // SAFETY: `context` names a string that ends in a 0 and outlives the
    // call, and the request it makes is owned here, closed once when this
    // drops.
    let handle = unsafe { win::owned(PowerCreateRequest(&context)) }?;
    // SAFETY: `handle` is a power request that's open.
    win::check(unsafe { PowerSetRequest(win::raw(&handle), PowerRequestSystemRequired) })?;
    Ok(Request {
        handle,
        _reason: wide,
    })
}

impl Drop for Request {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: the handle is a power request that's open until the
        // OwnedHandle closes it after this. Closing it would end the
        // request too, so a failure here changes nothing.
        unsafe { PowerClearRequest(win::raw(&self.handle), PowerRequestSystemRequired) };
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
