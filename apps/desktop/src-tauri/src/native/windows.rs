// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What the app tells Windows about itself as it starts. Windows groups a
//! window on the taskbar, and hands a toast's click back, by the app's
//! AppUserModelID. Both setups give the Start menu shortcut the app's
//! identifier as that ID, and the app claims the same one for its
//! process, so its window, its toasts and the shortcut are one app. A
//! build no setup installed has no shortcut, so the app lists the ID for
//! the current user, where Windows finds the name a toast shows.

#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::io;

/// The app's AppUserModelID: its identifier in `tauri.conf.json`, which
/// the setups put on the Start menu shortcut.
pub const APP_ID: &str = "com.maxagenthq.maxhq";

/// The name a toast shows for a build no setup installed.
const NAME: &str = "MaxHQ";

/// Claims [`APP_ID`] for this process before its window opens, and lists
/// it for the current user when no setup installed this copy.
#[cfg(windows)]
pub fn claim_app_id() {
    if let Err(error) = set_process_id(APP_ID) {
        tracing::warn!(%error, "Windows didn't take the app's ID, so toasts may not show");
    }
    let installed = std::env::current_exe().map_or(true, |exe| {
        maxhq_app::app_update::WindowsApp::here(&exe).installed()
    });
    if !installed && let Err(error) = list_app_id(APP_ID, NAME) {
        tracing::warn!(%error, "couldn't list the app's ID, so toasts may not show");
    }
}

/// The key under the current user that names an app's ID for its toasts.
fn app_id_key(id: &str) -> String {
    format!(r"Software\Classes\AppUserModelId\{id}")
}

/// `text` as a null-terminated UTF-16 string for Windows.
#[cfg(windows)]
fn wide(text: impl AsRef<OsStr>) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    text.as_ref().encode_wide().chain(Some(0)).collect()
}

/// Sets the ID Windows groups this process's windows and toasts by.
#[cfg(windows)]
#[allow(unsafe_code)]
fn set_process_id(id: &str) -> io::Result<()> {
    use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

    let id = wide(id);
    // SAFETY: `id` is a null-terminated UTF-16 string that outlives the
    // call, and Windows copies it.
    let result = unsafe { SetCurrentProcessExplicitAppUserModelID(id.as_ptr()) };
    if result < 0 {
        return Err(io::Error::other(format!("HRESULT {result:#010x}")));
    }
    Ok(())
}

/// Lists `id` with its display `name` under the current user's classes.
#[cfg(windows)]
#[allow(unsafe_code)]
fn list_app_id(id: &str, name: &str) -> io::Result<()> {
    use windows_sys::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};

    let key = wide(app_id_key(id));
    let value = wide("DisplayName");
    let data = wide(name);
    let bytes = u32::try_from(data.len() * 2)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "the name is too long"))?;
    // SAFETY: the key, the value's name and its data are null-terminated
    // UTF-16 strings that outlive the call, and `bytes` is the data's
    // length in bytes with its null, as a REG_SZ value takes.
    let result = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            REG_SZ,
            data.as_ptr().cast(),
            bytes,
        )
    };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_id_is_the_identifier_the_setups_give_the_shortcut() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        assert_eq!(config["identifier"], APP_ID);
        assert_eq!(config["productName"], NAME);
    }

    #[test]
    fn the_window_on_windows_is_the_main_one_with_a_native_title_bar() {
        let read = |text: &str| serde_json::from_str::<serde_json::Value>(text).unwrap();
        let main = read(include_str!("../../tauri.conf.json"));
        let windows = read(include_str!("../../tauri.windows.conf.json"));
        let mut window = main["app"]["windows"][0].as_object().unwrap().clone();
        for mac in ["titleBarStyle", "hiddenTitle", "trafficLightPosition"] {
            assert!(window.remove(mac).is_some(), "{mac} is the Mac's");
        }
        assert_eq!(
            windows["app"]["windows"],
            serde_json::json!([window]),
            "the window matches the main config's, less the Mac's title bar"
        );
        assert_eq!(
            windows["bundle"]["windows"]["nsis"]["installMode"], "currentUser",
            "the NSIS setup installs for you, without an administrator"
        );
    }

    #[test]
    fn the_id_is_listed_under_the_current_users_classes() {
        assert_eq!(
            app_id_key(APP_ID),
            r"Software\Classes\AppUserModelId\com.maxagenthq.maxhq"
        );
    }
}
