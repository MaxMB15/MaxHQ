// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The header's sun, which keeps this Mac's screen on. [`ScreenOn`] holds
//! it, and these commands let the webview read and flip it.

use maxhq_app::screen::{ScreenError, ScreenOn};
use maxhq_app::App;
use tauri::State;

use crate::native::NativeError;

/// Whether the sun is on, or nothing where the system can't keep its
/// screen on, so the header leaves it out.
#[tauri::command]
#[specta::specta]
pub fn screen_on(screen: State<'_, ScreenOn>) -> Option<bool> {
    ScreenOn::SUPPORTED.then(|| screen.is_on())
}

/// Turns the sun on or off. It lasts across restarts.
#[tauri::command]
#[specta::specta]
pub fn set_screen_on(
    app: State<'_, App>,
    screen: State<'_, ScreenOn>,
    on: bool,
) -> Result<(), NativeError> {
    screen
        .set(on, &app.devices().registry())
        .map_err(|error| match error {
            ScreenError::Unsupported => NativeError::Unsupported {
                what: "Keeping the screen on".into(),
            },
            error => NativeError::failed(error),
        })
}
