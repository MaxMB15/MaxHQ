// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Windows keeps the screen on for as long as a power request asks it to.
//! One request carries both halves: the display stays on and the PC
//! doesn't sleep when idle. Windows also ends it when the process exits,
//! so a crash can't leave the screen on.

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Power::{
    PowerClearRequest, PowerCreateRequest, PowerRequestDisplayRequired, PowerRequestSystemRequired,
    PowerSetRequest, POWER_REQUEST_TYPE,
};
use windows_sys::Win32::System::Threading::{
    POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0,
};

use super::{ScreenError, REASON};

/// What the request asks for: the display on, and no idle sleep.
const KINDS: [POWER_REQUEST_TYPE; 2] = [PowerRequestDisplayRequired, PowerRequestSystemRequired];

/// The version `REASON_CONTEXT` has had since Windows 7.
const POWER_REQUEST_CONTEXT_VERSION: u32 = 0;

/// A power request with both of [`KINDS`] set. Dropping it clears them
/// and closes the request.
pub struct PowerRequest {
    handle: HANDLE,
    /// How many of [`KINDS`] were set, so a drop clears only those.
    set: usize,
}

// SAFETY: the handle is a kernel object any thread may use or close,
// and the struct is only ever used behind the switch's mutex.
#[allow(unsafe_code)]
unsafe impl Send for PowerRequest {}

/// Makes the request and sets both of its kinds.
#[allow(unsafe_code)]
pub fn take() -> Result<PowerRequest, ScreenError> {
    let mut reason: Vec<u16> = REASON.to_string_lossy().encode_utf16().collect();
    reason.push(0);
    let context = REASON_CONTEXT {
        Version: POWER_REQUEST_CONTEXT_VERSION,
        Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
        Reason: REASON_CONTEXT_0 {
            SimpleReasonString: reason.as_mut_ptr(),
        },
    };
    // SAFETY: `context` and the string it points at outlive the call,
    // which copies the reason.
    let handle = unsafe { PowerCreateRequest(&context) };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(refused());
    }
    let mut request = PowerRequest { handle, set: 0 };
    for kind in KINDS {
        // SAFETY: the handle is the request just made, still open.
        if unsafe { PowerSetRequest(request.handle, kind) } == 0 {
            // Dropping `request` clears what was set and closes it.
            return Err(refused());
        }
        request.set += 1;
    }
    Ok(request)
}

/// The error Windows gave the last call.
fn refused() -> ScreenError {
    ScreenError::Refused(std::io::Error::last_os_error().to_string())
}

#[allow(unsafe_code)]
impl Drop for PowerRequest {
    fn drop(&mut self) {
        for &kind in &KINDS[..self.set] {
            // SAFETY: the request is open and `kind` was set on it.
            if unsafe { PowerClearRequest(self.handle, kind) } == 0 {
                tracing::debug!(error = %std::io::Error::last_os_error(), "Windows didn't clear a power request");
            }
        }
        // SAFETY: the handle is open and nothing uses it after this.
        unsafe { CloseHandle(self.handle) };
    }
}
