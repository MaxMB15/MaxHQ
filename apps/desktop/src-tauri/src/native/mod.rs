// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The native pieces the composer needs: spell check that follows macOS,
//! the spelling menu, a picker that takes files and folders together, files
//! and images from the clipboard, a description and thumbnail for each
//! attachment, moving a picked file to the Trash, showing a download in
//! Finder, and saving a conversation's export where you pick.
//!
//! The webview can only learn about a path by being handed one: a drop, a
//! paste or the picker. These commands record those in the app's
//! [`Offered`], and the ones that read a file refuse any other path, so a
//! script in the page can't read your disk through them.
//!
//! Linux and Windows share one implementation over cross-platform crates.
//! What they don't have yet answers with nothing to give, or with
//! [`NativeError::Unsupported`] where an empty answer would mislead.

mod describe;
// Only macOS checks spelling sentence by sentence, but the tests run
// everywhere.
#[cfg(any(target_os = "macos", test))]
mod text;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(any(target_os = "linux", windows))]
mod portable;
#[cfg(any(windows, test))]
pub mod windows;
#[cfg(any(target_os = "linux", windows))]
use portable as platform;

use std::path::{Path, PathBuf};

use maxhq_app::offered::{NotOffered, Offered};
use maxhq_app::App;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::ipc::{Invoke, Response};
use tauri::{AppHandle, DragDropEvent, Manager, Runtime, State, Webview, Window, WindowEvent};
use tauri_plugin_dialog::DialogExt;
use thiserror::Error;

use crate::terminal_stream;

pub use describe::PathInfo;

/// The interface labels on the spelling menu.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SpellingMenuLabels {
    pub no_guesses: String,
    pub ignore: String,
    pub learn: String,
    pub cut: String,
    pub copy: String,
    pub paste: String,
}

/// The interface labels on the attachment picker.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[cfg_attr(
    target_os = "macos",
    allow(dead_code, reason = "macOS picks files and folders in one panel")
)]
pub struct AttachmentPickerLabels {
    pub prompt: String,
    pub message: String,
    /// Where the picker can't take files and folders together, a question
    /// asks which first. This is its button for files, in English when
    /// it's left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub files: Option<String>,
    /// The question's button for folders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub folder: Option<String>,
    /// The question's button that cancels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub cancel: Option<String>,
}

/// A word the spell checker doesn't know.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Misspelling {
    /// Where the word starts, in UTF-16 code units, which is how
    /// JavaScript indexes a string.
    pub start: u32,
    /// Where the word ends, in UTF-16 code units.
    pub end: u32,
    /// The word as written.
    pub word: String,
    /// The language the checker took its sentence to be in, such as `en`
    /// or `nl`. Pass it back to [`spelling_menu`] so the guesses come from
    /// the same dictionary.
    pub language: Option<String>,
}

/// A point in the webview, in CSS pixels from its top left corner, such as
/// a `contextmenu` event's `clientX` and `clientY`.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Type)]
pub struct WebviewPoint {
    /// Pixels from the left edge.
    #[specta(type = specta_typescript::Number)]
    pub x: f64,
    /// Pixels from the top edge.
    #[specta(type = specta_typescript::Number)]
    pub y: f64,
}

/// What you picked in the spelling menu. Cut, copy and paste go straight
/// to the focused text through the webview, so they come back as `none`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(
    not(target_os = "macos"),
    allow(dead_code, reason = "only macOS has the spelling menu")
)]
pub enum SpellingChoice {
    /// The menu closed without a spelling choice.
    None,
    /// Put this word in place of the misspelling.
    Replace {
        /// The guess you picked.
        word: String,
    },
    /// The word is now ignored everywhere in the app until it quits, so
    /// check the text again.
    Ignored,
    /// The word is now in your macOS dictionary, so check the text again.
    Learned,
}

/// Why a native command failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type, Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NativeError {
    /// This platform has no such thing.
    #[cfg_attr(
        target_os = "macos",
        allow(dead_code, reason = "macOS has every piece")
    )]
    #[error("{what} isn't available on this platform")]
    Unsupported {
        /// What the command would have done.
        what: String,
    },
    /// The path didn't come from a drop, a paste or the picker.
    #[error("{path} wasn't dropped, pasted or picked")]
    NotOffered {
        /// The path the webview asked about.
        path: String,
    },
    /// Anything else.
    #[error("{message}")]
    Failed {
        /// What went wrong.
        message: String,
    },
}

impl NativeError {
    /// A failure described by `message`.
    pub(crate) fn failed(message: impl ToString) -> Self {
        Self::Failed {
            message: message.to_string(),
        }
    }
}

impl From<NotOffered> for NativeError {
    fn from(error: NotOffered) -> Self {
        Self::NotOffered { path: error.path }
    }
}

/// Records the paths of a file drop, for the window's event handler.
/// Tauri tells the webview about the drop in the same main thread turn,
/// before the webview can ask about the paths.
pub fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    if let WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) = event
        && let Some(app) = window.try_state::<App>()
    {
        app.offered().offer(paths.iter().cloned());
    }
}

/// Runs blocking native work on Tauri's blocking pool.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, NativeError> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(NativeError::failed)
}

/// Finds the misspelled words in the composer's text, with the languages
/// and learned words from System Settings. Each sentence gets its own
/// language guess, so Dutch and English can share a message. Off macOS
/// nothing is ever misspelled.
#[tauri::command]
#[specta::specta]
pub async fn check_spelling(text: String) -> Result<Vec<Misspelling>, NativeError> {
    blocking(move || platform::check_spelling(&text)).await
}

/// Shows the spelling menu at a point in the webview. With a misspelled
/// word the menu offers guesses, Ignore Spelling and Learn Spelling, and it
/// always offers Cut, Copy and Paste. Resolves once the menu closes.
/// Ignore and Learn are already done by then.
#[tauri::command]
#[specta::specta]
pub async fn spelling_menu(
    webview: Webview,
    word: Option<String>,
    language: Option<String>,
    at: WebviewPoint,
    labels: SpellingMenuLabels,
) -> Result<SpellingChoice, NativeError> {
    platform::spelling_menu(&webview, word, language, at.x, at.y, labels).await
}

/// Opens the system picker for files and folders together and resolves
/// with the chosen paths, or none if you cancel.
#[tauri::command]
#[specta::specta]
pub async fn pick_attachments(
    webview: Webview,
    app: State<'_, App>,
    labels: AttachmentPickerLabels,
) -> Result<Vec<String>, NativeError> {
    let picked = platform::pick_attachments(&webview, labels).await?;
    Ok(hand_over(app.offered(), picked))
}

/// The files on the clipboard, such as ones copied in Finder. Empty when
/// it holds none.
#[tauri::command]
#[specta::specta]
pub async fn read_clipboard_files(app: State<'_, App>) -> Result<Vec<String>, NativeError> {
    let files = blocking(platform::clipboard_files).await?;
    Ok(hand_over(app.offered(), files))
}

/// Remembers paths and turns them into strings for the webview.
fn hand_over(offered: &Offered, paths: Vec<PathBuf>) -> Vec<String> {
    let strings = paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    offered.offer(paths);
    strings
}

/// Describes dropped, pasted or picked paths for their chips, in the same
/// order.
#[tauri::command]
#[specta::specta]
pub async fn describe_paths(
    app: State<'_, App>,
    paths: Vec<String>,
) -> Result<Vec<PathInfo>, NativeError> {
    for path in &paths {
        app.offered().check(path)?;
    }
    blocking(move || paths.iter().map(|path| describe::describe(path)).collect()).await
}

/// Moves a dropped, pasted or picked file to the Trash, where Finder can
/// put it back, such as a GitHub App key once its devices hold it. Folders,
/// links and paths you didn't hand over are refused.
#[tauri::command]
#[specta::specta]
pub async fn trash_offered(app: State<'_, App>, path: String) -> Result<(), NativeError> {
    let path = app.offered().check(&path)?;
    blocking(move || trash_file(&path, platform::trash)).await?
}

/// Moves `path` to the Trash with `trash` if it's a plain file.
fn trash_file(
    path: &Path,
    trash: impl FnOnce(&Path) -> Result<(), NativeError>,
) -> Result<(), NativeError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => trash(path),
        Ok(_) => Err(NativeError::failed(format!(
            "{} isn't a file",
            path.display()
        ))),
        Err(error) => Err(NativeError::failed(error)),
    }
}

/// The folder under the user's Downloads that files from remote agents land
/// in.
pub const DOWNLOADS_FOLDER: &str = "MaxHQ";

/// Shows a file from a remote agent in Finder, selected. Only a path inside
/// `~/Downloads/MaxHQ` is shown, so the page can't point Finder elsewhere.
#[tauri::command]
#[specta::specta]
pub async fn reveal_download(app: AppHandle, path: String) -> Result<(), NativeError> {
    let downloads = app
        .path()
        .download_dir()
        .map_err(NativeError::failed)?
        .join(DOWNLOADS_FOLDER);
    blocking(move || platform::reveal(&inside(&downloads, Path::new(&path))?)).await?
}

/// `path` with its links resolved, if it exists and lies inside `folder`.
fn inside(folder: &Path, path: &Path) -> Result<PathBuf, NativeError> {
    let outside = || NativeError::failed(format!("{} isn't a download", path.display()));
    let folder = std::fs::canonicalize(folder).map_err(|_| outside())?;
    let path = std::fs::canonicalize(path).map_err(|_| outside())?;
    if path.starts_with(&folder) && path != folder {
        Ok(path)
    } else {
        Err(outside())
    }
}

/// The file an export went to, for the menu to name it and the next export
/// to start in the same folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct SavedExport {
    /// The file's name, such as `Fix the login bug 2026-11-08.md`.
    pub name: String,
    /// The folder it's in.
    pub folder: Option<String>,
}

/// Asks where to save a conversation's export in the system save dialog,
/// suggesting `name` in `folder`, and writes `text` there. Resolves with
/// the file written, or none if you cancel. Only the file you chose in the
/// dialog is ever written, so the page can't write anywhere else.
#[tauri::command]
#[specta::specta]
pub async fn save_export(
    window: Window,
    text: String,
    name: String,
    folder: Option<String>,
) -> Result<Option<SavedExport>, NativeError> {
    let (send, answer) = tokio::sync::oneshot::channel();
    // A visual saves as a web page, and everything else as Markdown.
    let (kind, extension) = if name.ends_with(".html") {
        ("Web page", "html")
    } else {
        ("Markdown", "md")
    };
    let dialog = window
        .dialog()
        .file()
        .set_parent(&window)
        .set_file_name(name)
        .add_filter(kind, &[extension]);
    let dialog = match folder {
        Some(folder) => dialog.set_directory(folder),
        None => dialog,
    };
    dialog.save_file(move |path| {
        let _ = send.send(path);
    });
    let chosen = answer
        .await
        .map_err(|_| NativeError::failed("the save dialog closed without an answer"))?;
    let Some(chosen) = chosen else {
        return Ok(None);
    };
    let path = chosen.into_path().map_err(NativeError::failed)?;
    blocking(move || write_export(&path, &text))
        .await?
        .map(Some)
}

/// Writes an export's text to `path`, replacing what was there, which the
/// save dialog already asked about.
fn write_export(path: &Path, text: &str) -> Result<SavedExport, NativeError> {
    std::fs::write(path, text).map_err(NativeError::failed)?;
    Ok(SavedExport {
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        folder: path
            .parent()
            .map(|folder| folder.to_string_lossy().into_owned()),
    })
}

/// Asks for a folder, or a file when `file` is set, on this Mac in the
/// system picker, titled `title` and opening at `start` when it's given.
/// Resolves with the chosen path, or none if you cancel. The path only
/// fills a field, so it isn't offered for attaching.
#[tauri::command]
#[specta::specta]
pub async fn pick_path(
    window: Window,
    title: String,
    start: Option<String>,
    file: bool,
) -> Result<Option<String>, NativeError> {
    let (send, answer) = tokio::sync::oneshot::channel();
    let dialog = window.dialog().file().set_parent(&window).set_title(title);
    let dialog = match start {
        Some(start) => dialog.set_directory(start),
        None => dialog,
    };
    let picked = move |path| {
        let _ = send.send(path);
    };
    if file {
        dialog.pick_file(picked);
    } else {
        dialog.pick_folder(picked);
    }
    let chosen = answer
        .await
        .map_err(|_| NativeError::failed("the picker closed without an answer"))?;
    chosen
        .map(|chosen| {
            let path = chosen.into_path().map_err(NativeError::failed)?;
            Ok(path.to_string_lossy().into_owned())
        })
        .transpose()
}

/// The image on the clipboard as PNG bytes, or an empty body when there's
/// none. A clipboard holding files counts as having no image, since Finder
/// puts the files' icons there too.
///
/// It answers with raw bytes, which the generated bindings can't describe,
/// so `src/native/files.ts` wraps it by hand.
#[tauri::command]
async fn read_clipboard_image() -> Result<Response, NativeError> {
    let png = blocking(platform::clipboard_image).await?;
    Ok(Response::new(png.unwrap_or_default()))
}

/// A thumbnail of a dropped, pasted or picked path as PNG bytes, at most
/// `size` CSS pixels on a side and `scale` pixels per CSS pixel. Files
/// without a preview get their Finder icon. An empty body means there's
/// nothing to show, such as for a file that's gone.
///
/// It answers with raw bytes, which the generated bindings can't describe,
/// so `src/native/files.ts` wraps it by hand.
#[tauri::command]
async fn thumbnail(
    app: State<'_, App>,
    path: String,
    size: f64,
    scale: f64,
) -> Result<Response, NativeError> {
    let path = app.offered().check(&path)?;
    if !(size.is_finite() && scale.is_finite() && size > 0.0 && scale > 0.0) {
        return Err(NativeError::failed(
            "a thumbnail needs a positive size and scale",
        ));
    }
    let png = platform::thumbnail(&path, size, scale).await?;
    Ok(Response::new(png.unwrap_or_default()))
}

/// The commands that answer with raw bytes, by name.
const RAW_COMMANDS: [&str; 2] = ["read_clipboard_image", "thumbnail"];

/// Combines the generated handler with the commands that answer with raw
/// bytes and the terminal ones, which the generated bindings can't list.
pub fn invoke_handler(
    typed: impl Fn(Invoke) -> bool + Send + Sync + 'static,
) -> impl Fn(Invoke) -> bool + Send + Sync + 'static {
    let raw = raw_handler();
    move |invoke| {
        let command = invoke.message.command();
        if RAW_COMMANDS.contains(&command) || terminal_stream::COMMANDS.contains(&command) {
            raw(invoke)
        } else {
            typed(invoke)
        }
    }
}

/// The handler for the commands that answer with raw bytes, and the
/// terminal ones.
fn raw_handler() -> impl Fn(Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        read_clipboard_image,
        thumbnail,
        terminal_stream::terminal_attach,
        terminal_stream::terminal_detach,
        terminal_stream::terminal_input,
    ]
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[test]
    fn only_plain_files_go_to_the_trash() {
        let dir = std::env::temp_dir().join(format!("maxhq-trash-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("folder")).unwrap();
        let key = dir.join("app.private-key.pem");
        std::fs::write(&key, "test-not-a-real-key").unwrap();
        let trashed = Mutex::new(Vec::new());
        let trash = |path: &Path| {
            trashed.lock().unwrap().push(path.to_owned());
            Ok(())
        };
        assert_eq!(trash_file(&key, trash), Ok(()));
        assert!(trash_file(&dir.join("folder"), trash).is_err());
        assert!(trash_file(&dir.join("gone.pem"), trash).is_err());
        #[cfg(unix)]
        {
            let link = dir.join("link.pem");
            std::os::unix::fs::symlink(&key, &link).unwrap();
            assert!(trash_file(&link, trash).is_err());
        }
        assert_eq!(*trashed.lock().unwrap(), [key]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_export_is_written_where_you_chose() {
        let dir = std::env::temp_dir().join(format!("maxhq-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Fix the login 2026-11-08.md");
        std::fs::write(&path, "an older export").unwrap();

        let saved = write_export(&path, "# Fix the login\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Fix the login\n");
        assert_eq!(saved.name, "Fix the login 2026-11-08.md");
        assert_eq!(saved.folder, Some(dir.to_string_lossy().into_owned()));
        assert!(write_export(&dir.join("gone").join("x.md"), "text").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_downloads_are_shown() {
        let dir = std::env::temp_dir().join(format!("maxhq-reveal-{}", std::process::id()));
        let downloads = dir.join(DOWNLOADS_FOLDER);
        std::fs::create_dir_all(downloads.join("site")).unwrap();
        let file = downloads.join("site").join("notes.txt");
        std::fs::write(&file, "notes").unwrap();
        let elsewhere = dir.join("elsewhere.txt");
        std::fs::write(&elsewhere, "elsewhere").unwrap();
        let real = std::fs::canonicalize(&file).unwrap();

        assert_eq!(inside(&downloads, &file), Ok(real));
        assert!(inside(&downloads, &downloads).is_err());
        assert!(inside(&downloads, &elsewhere).is_err());
        assert!(inside(&downloads, &downloads.join("..").join("elsewhere.txt")).is_err());
        assert!(inside(&downloads, &downloads.join("gone.txt")).is_err());
        #[cfg(unix)]
        {
            let link = downloads.join("link.txt");
            std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
            assert!(inside(&downloads, &link).is_err());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
