// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Stand-ins for platforms without the macOS pieces. Spell check finds
//! nothing and the clipboard holds nothing, which the composer handles
//! anyway, while the menu and the picker say they're missing.

use std::path::{Path, PathBuf};

use tauri::{Runtime, Webview};

use super::{AttachmentPickerLabels, Misspelling, NativeError, SpellingChoice, SpellingMenuLabels};

/// Nothing is misspelled here.
pub fn check_spelling(_text: &str) -> Vec<Misspelling> {
    Vec::new()
}

/// There's no native spelling menu here.
pub async fn spelling_menu<R: Runtime>(
    _webview: &Webview<R>,
    _word: Option<String>,
    _language: Option<String>,
    _x: f64,
    _y: f64,
    _labels: SpellingMenuLabels,
) -> Result<SpellingChoice, NativeError> {
    Err(unsupported("the spelling menu"))
}

/// There's no native picker for files and folders together here.
pub async fn pick_attachments<R: Runtime>(
    _webview: &Webview<R>,
    _labels: AttachmentPickerLabels,
) -> Result<Vec<PathBuf>, NativeError> {
    Err(unsupported("the file and folder picker"))
}

/// The clipboard isn't read for files here.
pub fn clipboard_files() -> Vec<PathBuf> {
    Vec::new()
}

/// The clipboard isn't read for images here.
pub fn clipboard_image() -> Option<Vec<u8>> {
    None
}

/// There are no thumbnails here.
pub async fn thumbnail(
    _path: &Path,
    _size: f64,
    _scale: f64,
) -> Result<Option<Vec<u8>>, NativeError> {
    Ok(None)
}

/// There's no Trash to move files to here.
pub fn trash(_path: &Path) -> Result<(), NativeError> {
    Err(unsupported("the Trash"))
}

/// The error for something this platform lacks.
fn unsupported(what: &str) -> NativeError {
    NativeError::Unsupported {
        what: what.to_owned(),
    }
}

/// There's no Finder here.
pub fn reveal(_path: &Path) -> Result<(), NativeError> {
    Err(unsupported("showing a file"))
}
