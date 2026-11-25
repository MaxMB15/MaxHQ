// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Where this computer's app key is kept, for the key dialog and Settings.
//! A Mac always keeps it in the Keychain, so these answer nothing there.
//! On Linux and Windows the key goes in the system keyring, and when none
//! answers the app asks before it keeps the key in a file.

use maxhq_app::app_keys::{self, AppKeys, KeyHome, KeyHomeState};
use maxhq_app::App;
use tauri::{AppHandle, Manager, State};

use crate::native::NativeError;

/// The app key the app opened with, if this system has a store for one.
pub struct AppKey(pub Option<AppKeys>);

/// Where the app key is kept, and whether to ask where to keep it. Nothing
/// on a Mac, or where the app has no key.
#[tauri::command]
#[specta::specta]
pub fn app_key_home(key: State<'_, AppKey>) -> Option<KeyHomeState> {
    key.0.as_ref().and_then(AppKeys::home)
}

/// Moves the app key to `home` and remembers the choice. Memory stops the
/// asking without moving the key. It runs off the main thread, since the
/// keyring may show an unlock prompt and wait for you.
#[tauri::command]
#[specta::specta]
pub async fn keep_app_key_in(
    app: AppHandle,
    home: KeyHome,
) -> Result<Option<KeyHomeState>, NativeError> {
    tauri::async_runtime::spawn_blocking(move || {
        let key = app.state::<AppKey>();
        let Some(keys) = key.0.as_ref() else {
            return Err(NativeError::Unsupported {
                what: "Choosing where the app key is kept".into(),
            });
        };
        keys.keep_in(home).map_err(NativeError::failed)?;
        // The key moved, so a choice the database didn't take only means
        // the app asks again next time. The key file's being there still
        // holds the file choice.
        let state = app.state::<App>();
        if let Err(error) = app_keys::remember(&state.devices().registry(), home) {
            tracing::warn!(%error, "couldn't remember where the app key is kept");
        }
        Ok(keys.home())
    })
    .await
    .map_err(NativeError::failed)?
}
