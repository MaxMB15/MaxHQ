// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The native pieces on Linux and Windows, where one cross-platform crate
//! covers both: the Trash through `trash`, showing a file through the
//! opener plugin, the picker through the dialog plugin and the clipboard
//! through `arboard`. There's no spell checker or thumbnailer here yet, so
//! the composer hides spelling and shows icons in place of thumbnails.

use std::path::{Path, PathBuf};

use tauri::{Runtime, Webview};
use tauri_plugin_dialog::{DialogExt, FilePath, MessageDialogButtons, MessageDialogResult};
use tokio::sync::oneshot;

use super::{AttachmentPickerLabels, Misspelling, NativeError, SpellingChoice, SpellingMenuLabels};

/// Nothing is misspelled while there's no spell checker here.
pub fn check_spelling(_text: &str) -> Vec<Misspelling> {
    Vec::new()
}

/// There's no spelling menu here yet.
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

/// What the picker picks. GTK's and Windows' pickers take files or folders
/// but not both, so you choose first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pick {
    Files,
    Folders,
}

/// Asks whether you want files or folders, then opens the picker for
/// those, both on the webview's window. Cancelling either gives no paths.
pub async fn pick_attachments<R: Runtime>(
    webview: &Webview<R>,
    labels: AttachmentPickerLabels,
) -> Result<Vec<PathBuf>, NativeError> {
    let window = webview.window();
    // The page sends these in its language. English covers a page that
    // doesn't yet.
    let files = labels.files.unwrap_or_else(|| "Files…".to_owned());
    let folders = labels.folder.unwrap_or_else(|| "Folder…".to_owned());
    let cancel = labels.cancel.unwrap_or_else(|| "Cancel".to_owned());

    let (send, answer) = oneshot::channel();
    window
        .dialog()
        .message(labels.message)
        .title(labels.prompt.clone())
        .parent(&window)
        .buttons(MessageDialogButtons::YesNoCancelCustom(
            files.clone(),
            folders.clone(),
            cancel,
        ))
        .show_with_result(move |result| {
            let _ = send.send(result);
        });
    let answer = answer.await.map_err(|_| closed())?;
    let Some(pick) = pick_of(&answer, &files, &folders) else {
        return Ok(Vec::new());
    };

    let (send, chosen) = oneshot::channel();
    let dialog = window
        .dialog()
        .file()
        .set_parent(&window)
        .set_title(labels.prompt);
    let picked = move |paths: Option<Vec<FilePath>>| {
        let _ = send.send(paths.unwrap_or_default());
    };
    match pick {
        Pick::Files => dialog.pick_files(picked),
        Pick::Folders => dialog.pick_folders(picked),
    }
    chosen
        .await
        .map_err(|_| closed())?
        .into_iter()
        .map(|path| path.into_path().map_err(NativeError::failed))
        .collect()
}

/// The picker the button you pressed asks for, or none for a cancel.
fn pick_of(answer: &MessageDialogResult, files: &str, folders: &str) -> Option<Pick> {
    match answer {
        MessageDialogResult::Custom(label) if label == files => Some(Pick::Files),
        MessageDialogResult::Custom(label) if label == folders => Some(Pick::Folders),
        // A custom button comes back by its label, but in case a backend
        // names it by the stock button it stands in for.
        MessageDialogResult::Yes => Some(Pick::Files),
        MessageDialogResult::No => Some(Pick::Folders),
        _ => None,
    }
}

/// The error for a picker that went away without answering.
fn closed() -> NativeError {
    NativeError::failed("the picker closed without an answer")
}

/// The files on the clipboard: its `text/uri-list` on Linux, or its file
/// drop list on Windows.
pub fn clipboard_files() -> Vec<PathBuf> {
    read_clipboard(|clipboard| clipboard.get().file_list()).unwrap_or_default()
}

/// The image on the clipboard as PNG bytes, unless it holds files, which
/// the composer attaches instead.
pub fn clipboard_image() -> Option<Vec<u8>> {
    read_clipboard(|clipboard| {
        if clipboard
            .get()
            .file_list()
            .is_ok_and(|files| !files.is_empty())
        {
            return Ok(None);
        }
        clipboard.get().image().map(|image| png(&image))
    })
    .flatten()
}

/// `image`, which `arboard` gives as 8-bit RGBA pixels, as PNG bytes.
/// Fast compression, since the bytes only go on to the composer.
fn png(image: &arboard::ImageData<'_>) -> Option<Vec<u8>> {
    let width = u32::try_from(image.width).ok()?;
    let height = u32::try_from(image.height).ok()?;
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let written = encoder.write_header().and_then(|mut writer| {
        writer.write_image_data(&image.bytes)?;
        writer.finish()
    });
    match written {
        Ok(()) => Some(bytes),
        Err(error) => {
            tracing::debug!(%error, "couldn't make the clipboard image a PNG");
            None
        }
    }
}

/// Reads the clipboard with `read`. An empty clipboard, or one that holds
/// something else, reads as nothing, and so does a clipboard that can't be
/// reached, such as with no display.
fn read_clipboard<T>(
    read: impl FnOnce(&mut arboard::Clipboard) -> Result<T, arboard::Error>,
) -> Option<T> {
    match arboard::Clipboard::new().and_then(|mut clipboard| read(&mut clipboard)) {
        Ok(value) => Some(value),
        Err(arboard::Error::ContentNotAvailable) => None,
        Err(error) => {
            tracing::debug!(%error, "couldn't read the clipboard");
            None
        }
    }
}

/// There are no thumbnails here yet, so every file shows its icon.
pub async fn thumbnail(
    _path: &Path,
    _size: f64,
    _scale: f64,
) -> Result<Option<Vec<u8>>, NativeError> {
    Ok(None)
}

/// Moves a file to the Trash (the Recycle Bin on Windows), where the file
/// manager can put it back.
pub fn trash(path: &Path) -> Result<(), NativeError> {
    trash::delete(path).map_err(NativeError::failed)
}

/// Opens a file manager window with `path` selected, through the file
/// manager's own D-Bus service on Linux, or the folder alone where there's
/// none, and through Explorer on Windows.
pub fn reveal(path: &Path) -> Result<(), NativeError> {
    tauri_plugin_opener::reveal_item_in_dir(path).map_err(NativeError::failed)
}

/// The error for something this platform doesn't have.
fn unsupported(what: &str) -> NativeError {
    NativeError::Unsupported {
        what: what.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILES: &str = "Bestanden…";
    const FOLDERS: &str = "Map…";

    fn custom(label: &str) -> MessageDialogResult {
        MessageDialogResult::Custom(label.to_owned())
    }

    #[test]
    fn each_button_opens_its_own_picker() {
        assert_eq!(pick_of(&custom(FILES), FILES, FOLDERS), Some(Pick::Files));
        assert_eq!(
            pick_of(&custom(FOLDERS), FILES, FOLDERS),
            Some(Pick::Folders)
        );
        assert_eq!(
            pick_of(&MessageDialogResult::Yes, FILES, FOLDERS),
            Some(Pick::Files)
        );
        assert_eq!(
            pick_of(&MessageDialogResult::No, FILES, FOLDERS),
            Some(Pick::Folders)
        );
    }

    #[test]
    fn a_clipboard_image_becomes_a_png_of_the_same_size() {
        let pixels = [255, 0, 0, 255, 0, 255, 0, 128];
        let image = arboard::ImageData {
            width: 2,
            height: 1,
            bytes: pixels.as_slice().into(),
        };
        let bytes = png(&image).unwrap();
        let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
            .read_info()
            .unwrap();
        let mut decoded = vec![0; reader.output_buffer_size().unwrap()];
        let frame = reader.next_frame(&mut decoded).unwrap();
        assert_eq!((frame.width, frame.height), (2, 1));
        assert_eq!(&decoded[..frame.buffer_size()], pixels);
    }

    #[test]
    fn cancelling_or_closing_the_question_picks_nothing() {
        assert_eq!(pick_of(&custom("Annuleren"), FILES, FOLDERS), None);
        assert_eq!(pick_of(&MessageDialogResult::Cancel, FILES, FOLDERS), None);
        assert_eq!(pick_of(&MessageDialogResult::Ok, FILES, FOLDERS), None);
    }
}
