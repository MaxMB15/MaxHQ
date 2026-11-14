// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The macOS side, through AppKit and Quick Look.
//!
//! The spell checker, the pasteboard and Quick Look all work from any
//! thread, so their calls run on Tauri's blocking pool and never hold up
//! the window. The menu and the picker are windows, which AppKit keeps on
//! the main thread, so they run there through the webview and send their
//! answer back.

// Tauri hands over its webview and window as raw pointers.
#![allow(unsafe_code)]

mod menu;
mod panel;
mod pasteboard;
mod spell;
mod thumbnail;

use std::path::{Path, PathBuf};

use objc2_app_kit::{NSView, NSWindow, NSWorkspace};
use objc2_foundation::{NSArray, NSFileManager, NSURL};
use tauri::{Runtime, Webview};
use tokio::sync::oneshot;

use super::{blocking, AttachmentPickerLabels, NativeError, SpellingChoice, SpellingMenuLabels};

pub use pasteboard::{clipboard_files, clipboard_image};
pub use spell::check as check_spelling;

/// Runs `work` on the main thread with the webview's view and window, and
/// waits for its answer.
async fn on_main_thread<R: Runtime, T: Send + 'static>(
    webview: &Webview<R>,
    work: impl FnOnce(&NSView, Option<&NSWindow>) -> T + Send + 'static,
) -> Result<T, NativeError> {
    let (send, answer) = oneshot::channel();
    webview
        .with_webview(move |platform| {
            // SAFETY: Tauri passes its live WKWebView, which is an NSView,
            // and the window holding it. This closure runs on the main
            // thread while both are alive, and nothing keeps the
            // references past it.
            let view = unsafe { platform.inner().cast::<NSView>().as_ref() };
            let window = unsafe { platform.ns_window().cast::<NSWindow>().as_ref() };
            if let Some(view) = view {
                let _ = send.send(work(view, window));
            }
        })
        .map_err(NativeError::failed)?;
    answer
        .await
        .map_err(|_| NativeError::failed("the webview closed"))
}

/// Shows the spelling menu and carries out Ignore or Learn.
pub async fn spelling_menu<R: Runtime>(
    webview: &Webview<R>,
    word: Option<String>,
    language: Option<String>,
    x: f64,
    y: f64,
    labels: SpellingMenuLabels,
) -> Result<SpellingChoice, NativeError> {
    let guesses = match word.clone() {
        Some(word) => Some(blocking(move || spell::guesses(&word, language.as_deref())).await?),
        None => None,
    };
    let shown = guesses.clone();
    let picked = on_main_thread(webview, move |view, _window| {
        menu::pop_up(view, shown.as_deref(), x, y, &labels)
    })
    .await?;
    let choice = match (picked, word) {
        (menu::Picked::Guess(index), _) => guesses
            .and_then(|guesses| guesses.into_iter().nth(index))
            .map_or(SpellingChoice::None, |word| SpellingChoice::Replace {
                word,
            }),
        (menu::Picked::Ignore, Some(word)) => {
            blocking(move || spell::ignore(&word)).await?;
            SpellingChoice::Ignored
        }
        (menu::Picked::Learn, Some(word)) => {
            blocking(move || spell::learn(&word)).await?;
            SpellingChoice::Learned
        }
        _ => SpellingChoice::None,
    };
    Ok(choice)
}

/// Opens the picker as a sheet on the webview's window.
pub async fn pick_attachments<R: Runtime>(
    webview: &Webview<R>,
    labels: AttachmentPickerLabels,
) -> Result<Vec<PathBuf>, NativeError> {
    let chosen = on_main_thread(webview, move |view, window| {
        panel::begin(view, window, &labels)
    })
    .await?;
    chosen
        .await
        .map_err(|_| NativeError::failed("the picker closed without an answer"))
}

/// Asks Quick Look for a thumbnail.
pub async fn thumbnail(path: &Path, size: f64, scale: f64) -> Result<Option<Vec<u8>>, NativeError> {
    let Some(made) = thumbnail::request(path, size, scale) else {
        return Ok(None);
    };
    made.await
        .map_err(|_| NativeError::failed("Quick Look didn't answer"))
}

/// Moves a file to the Trash, where Finder can put it back.
pub fn trash(path: &Path) -> Result<(), NativeError> {
    let url = NSURL::from_file_path(path)
        .ok_or_else(|| NativeError::failed(format!("{} isn't a file path", path.display())))?;
    NSFileManager::defaultManager()
        .trashItemAtURL_resultingItemURL_error(&url, None)
        .map_err(|error| NativeError::failed(error.localizedDescription()))
}

/// Opens a Finder window with `path` selected.
pub fn reveal(path: &Path) -> Result<(), NativeError> {
    let url = NSURL::from_file_path(path)
        .ok_or_else(|| NativeError::failed(format!("{} isn't a file path", path.display())))?;
    NSWorkspace::sharedWorkspace()
        .activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
    Ok(())
}
