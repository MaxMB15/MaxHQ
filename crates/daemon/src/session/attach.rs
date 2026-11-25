// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Attachments. A client stages each file before it sends a message, which
//! puts a copy in `attachments/staged/<id>/<name>`. Sending moves the copy
//! into `attachments/<agent>/<id>/<name>`, a folder the agent's CLI may
//! read. A message to that agent may name it again, such as a queued one
//! taken back and sent again, and it stays where it is. Staged copies
//! nobody sent are deleted after a day. A picture's preview and a
//! document's text are made as it's staged, and move with it (see
//! [`crate::sidecar`]).
//!
//! Staging can copy a large file, so the manager runs it off its loop.
//! Finding and moving a staged copy are quick renames, which it runs in
//! place.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
#[cfg(windows)]
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use base64::Engine;
use maxhq_core::attached;
use maxhq_protocol::ids::AttachmentId;
use maxhq_protocol::message::{AttachmentSource, MAX_INLINE_ATTACHMENT};
use maxhq_protocol::timeline::{AttachmentKind, AttachmentRef};
use thiserror::Error;

use crate::doctext::Extractor;
use crate::drive::Staged;
use crate::sidecar;

/// The most a staged folder may hold.
pub const MAX_FOLDER: u64 = 200 * 1024 * 1024;

/// How long a staged copy waits to be sent before it's deleted.
pub const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// Why a message can't name an attachment that's neither staged nor in its
/// agent's folder, such as one sent to another agent or staged over a day
/// ago.
pub const GONE: &str =
    "a file attached to the message is no longer ready to send. Remove it and attach it again";

/// Why an attachment couldn't be staged.
#[derive(Debug, Error)]
pub enum StageError {
    /// Paths must be absolute.
    #[error("{0} isn't an absolute path")]
    NotAbsolute(String),
    /// The source couldn't be read.
    #[error("can't read {path}: {source}")]
    Read {
        /// The source.
        path: String,
        /// Why.
        #[source]
        source: io::Error,
    },
    /// The folder holds more than [`MAX_FOLDER`].
    #[error("the folder holds more than 200 MB")]
    TooBig,
    /// The pasted bytes weren't base64.
    #[error("the pasted bytes aren't valid base64")]
    BadBase64,
    /// The pasted bytes are over the limit.
    #[error("the pasted bytes are over 8 MiB")]
    TooLarge,
    /// The name can't be a file name.
    #[error("{0:?} can't be a file name")]
    BadName(String),
    /// The copy couldn't be written.
    #[error("can't write the staged copy: {0}")]
    Write(#[source] io::Error),
}

/// The attachments folder.
#[derive(Clone, Debug)]
pub struct Attachments {
    root: PathBuf,
    /// Reads the text out of the documents staged.
    extractor: Extractor,
}

impl Attachments {
    /// Attachments kept under `root`.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            extractor: Extractor::default(),
        }
    }

    /// The same, reading documents' text with `extractor`.
    #[cfg(test)]
    pub fn with_extractor(self, extractor: Extractor) -> Self {
        Self { extractor, ..self }
    }

    /// The folder an agent's attachments move into.
    pub fn agent_dir(&self, agent: &str) -> PathBuf {
        self.root.join(agent)
    }

    fn staged(&self) -> PathBuf {
        self.root.join("staged")
    }

    /// Copies or writes the source into the staging folder and describes
    /// it. A failure leaves nothing behind.
    pub fn stage(
        &self,
        id: AttachmentId,
        source: &AttachmentSource,
    ) -> Result<AttachmentRef, StageError> {
        let dir = self.staged().join(id.to_string());
        let staged = match source {
            AttachmentSource::Path { path } => self.stage_path(&dir, path),
            AttachmentSource::Bytes { name, mime, data } => self
                .stage_bytes(&dir, name, data)
                .map(|path| (path, Some(mime.clone()))),
            // The manager finds the agent's folder first, and stages what
            // it finds there by its path.
            AttachmentSource::Found { path, .. } => Err(StageError::NotAbsolute(path.clone())),
        };
        match staged.and_then(|(path, mime)| describe(id, &path, mime).map_err(StageError::Write)) {
            Ok(mut attachment) => {
                attachment.thumbnail = self.make_sidecars(&attachment).thumbnail;
                Ok(attachment)
            }
            Err(err) => {
                let _ = maxhq_transfer::remove_tree(&dir);
                Err(err)
            }
        }
    }

    fn stage_path(&self, dir: &Path, path: &str) -> Result<(PathBuf, Option<String>), StageError> {
        let source = Path::new(path);
        if !source.is_absolute() {
            return Err(StageError::NotAbsolute(path.to_owned()));
        }
        let read = |source: io::Error| StageError::Read {
            path: path.to_owned(),
            source,
        };
        let meta = fs::metadata(source).map_err(read)?;
        let name = source
            .file_name()
            .map_or_else(|| "folder".into(), clean_file_name);
        fs::create_dir_all(dir).map_err(StageError::Write)?;
        let target = dir.join(name);
        if meta.is_dir() {
            let mut budget = MAX_FOLDER;
            copy_folder(source, &target, &mut budget)?;
        } else {
            fs::copy(source, &target).map_err(read)?;
        }
        Ok((target, None))
    }

    fn stage_bytes(&self, dir: &Path, name: &str, data: &str) -> Result<PathBuf, StageError> {
        if data.len() / 4 * 3 > MAX_INLINE_ATTACHMENT + 3 {
            return Err(StageError::TooLarge);
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|_| StageError::BadBase64)?;
        if bytes.len() > MAX_INLINE_ATTACHMENT {
            return Err(StageError::TooLarge);
        }
        let file = Path::new(name)
            .file_name()
            .filter(|file| !file.is_empty())
            .map(clean_file_name)
            .ok_or_else(|| StageError::BadName(name.to_owned()))?;
        fs::create_dir_all(dir).map_err(StageError::Write)?;
        let target = dir.join(file);
        fs::write(&target, bytes).map_err(StageError::Write)?;
        Ok(target)
    }

    /// Makes the preview and text kept beside a staged attachment.
    pub fn make_sidecars(&self, attachment: &AttachmentRef) -> sidecar::Sidecars {
        sidecar::make(
            Path::new(&attachment.path),
            attachment.mime.as_deref(),
            &self.extractor,
        )
    }

    /// Describes a staged attachment from its folder, for one staged before
    /// `maxd` restarted.
    pub fn find(&self, id: AttachmentId) -> Option<AttachmentRef> {
        found_in(id, &self.staged())
    }

    /// Describes an attachment a message already moved into `into`, an
    /// agent's folder, so another message to that agent can name it.
    pub fn sent(&self, id: AttachmentId, into: &Path) -> Option<AttachmentRef> {
        found_in(id, into)
    }

    /// Moves a staged attachment into `into`, under its ID. One already
    /// there stays.
    pub fn take(&self, attachment: &AttachmentRef, into: &Path) -> io::Result<Staged> {
        let id = attachment.id.to_string();
        let from = PathBuf::from(&attachment.path);
        let name = from
            .file_name()
            .ok_or_else(|| io::Error::other("a staged attachment has no name"))?;
        let dir = into.join(&id);
        let to = dir.join(name);
        if from != to {
            fs::create_dir_all(&dir)?;
            fs::rename(&from, &to)?;
            sidecar::carry(&from, &to);
            let _ = maxhq_transfer::remove_tree(self.staged().join(&id));
        }
        let mut moved = attachment.clone();
        moved.path = to.to_string_lossy().into_owned();
        Ok(Staged {
            attachment: moved,
            text: sidecar::text_of(&to),
            path: to,
        })
    }

    /// Deletes staged copies older than [`STALE_AFTER`] at `now`. Returns
    /// how many went.
    pub fn clean(&self, now: SystemTime) -> usize {
        let Ok(entries) = fs::read_dir(self.staged()) else {
            return 0;
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let stale = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .is_ok_and(|at| now.duration_since(at).is_ok_and(|age| age > STALE_AFTER));
            if stale && maxhq_transfer::remove_tree(entry.path()).is_ok() {
                removed += 1;
            }
        }
        removed
    }
}

/// A staged file's name with each control character, such as a newline,
/// made a space, so the line that names it stays one line. A name that
/// isn't text has the same done to its bytes.
fn clean_file_name(name: &OsStr) -> OsString {
    match name.to_str() {
        Some(text) => attached::clean_name(text).into_owned().into(),
        None => clean_raw_name(name),
    }
}

/// A name that isn't text with each control byte made a space.
#[cfg(unix)]
fn clean_raw_name(name: &OsStr) -> OsString {
    OsString::from_vec(
        name.as_bytes()
            .iter()
            .map(|&byte| {
                if byte < 0x20 || byte == 0x7f {
                    b' '
                } else {
                    byte
                }
            })
            .collect(),
    )
}

/// A name that isn't text, which on Windows is one with a lone surrogate,
/// with each control unit made a space.
#[cfg(windows)]
fn clean_raw_name(name: &OsStr) -> OsString {
    let units: Vec<u16> = name
        .encode_wide()
        .map(|unit| {
            if unit < 0x20 || unit == 0x7f {
                u16::from(b' ')
            } else {
                unit
            }
        })
        .collect();
    OsString::from_wide(&units)
}

/// Copies a folder, skipping links inside it, within `budget` bytes.
fn copy_folder(from: &Path, to: &Path, budget: &mut u64) -> Result<(), StageError> {
    let read = |source: io::Error| StageError::Read {
        path: from.to_string_lossy().into_owned(),
        source,
    };
    fs::create_dir_all(to).map_err(StageError::Write)?;
    for entry in fs::read_dir(from).map_err(read)? {
        let entry = entry.map_err(read)?;
        let meta = entry.path().symlink_metadata().map_err(read)?;
        let target = to.join(entry.file_name());
        if meta.is_dir() {
            copy_folder(&entry.path(), &target, budget)?;
        } else if meta.is_file() {
            *budget = budget.checked_sub(meta.len()).ok_or(StageError::TooBig)?;
            fs::copy(entry.path(), &target).map_err(read)?;
        }
    }
    Ok(())
}

/// The attachment `id` in its folder under `root`, with its preview.
fn found_in(id: AttachmentId, root: &Path) -> Option<AttachmentRef> {
    let path = sidecar::attachment_in(&root.join(id.to_string()))?;
    let mut attachment = describe(id, &path, None).ok()?;
    attachment.thumbnail = sidecar::thumbnail_of(&path);
    Some(attachment)
}

/// What a staged file or folder is.
fn describe(id: AttachmentId, path: &Path, mime: Option<String>) -> io::Result<AttachmentRef> {
    let meta = fs::metadata(path)?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mime = mime.or_else(|| mime_for(&name).map(str::to_owned));
    let (kind, bytes) = if meta.is_dir() {
        (AttachmentKind::Folder, folder_size(path))
    } else {
        let kind = match mime.as_deref() {
            Some(mime) if mime.starts_with("image/") => AttachmentKind::Image,
            Some("application/pdf") => AttachmentKind::Pdf,
            _ => AttachmentKind::File,
        };
        (kind, meta.len())
    };
    Ok(AttachmentRef {
        id,
        name,
        kind,
        bytes,
        mime,
        path: path.to_string_lossy().into_owned(),
        thumbnail: None,
    })
}

fn folder_size(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.path().symlink_metadata() {
            Ok(meta) if meta.is_dir() => folder_size(&entry.path()),
            Ok(meta) => meta.len(),
            Err(_) => 0,
        })
        .sum()
}

/// The media type for the file name's extension, for the kinds a CLI reads
/// differently.
pub(crate) fn mime_for(name: &str) -> Option<&'static str> {
    let ext = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{fake_program, TempDir};
    use std::path::MAIN_SEPARATOR as SEP;

    fn store(dir: &TempDir) -> Attachments {
        Attachments::new(dir.path().join("attachments"))
    }

    #[test]
    fn paths_are_copied_and_described() {
        let dir = TempDir::new();
        let source = dir.path().join("shot.png");
        fs::write(&source, b"png bytes").unwrap();
        let id = AttachmentId::generate();
        let staged = store(&dir)
            .stage(
                id,
                &AttachmentSource::Path {
                    path: source.to_string_lossy().into_owned(),
                },
            )
            .unwrap();
        assert_eq!(staged.kind, AttachmentKind::Image);
        assert_eq!(staged.bytes, 9);
        assert_eq!(staged.name, "shot.png");
        assert!(source.exists(), "the original stays");
        assert_eq!(fs::read(&staged.path).unwrap(), b"png bytes");
        assert_eq!(store(&dir).find(id).unwrap(), staged);
    }

    // Windows makes links only with Developer Mode or as an administrator.
    #[cfg(unix)]
    #[test]
    fn folders_are_copied_whole_without_their_links() {
        let dir = TempDir::new();
        let source = dir.path().join("notes");
        fs::create_dir_all(source.join("deep")).unwrap();
        fs::write(source.join("a.txt"), b"aaaa").unwrap();
        fs::write(source.join("deep/b.txt"), b"bb").unwrap();
        std::os::unix::fs::symlink("/etc/hosts", source.join("link")).unwrap();
        let staged = store(&dir)
            .stage(
                AttachmentId::generate(),
                &AttachmentSource::Path {
                    path: source.to_string_lossy().into_owned(),
                },
            )
            .unwrap();
        assert_eq!(staged.kind, AttachmentKind::Folder);
        assert_eq!(staged.bytes, 6);
        assert!(Path::new(&staged.path).join("deep/b.txt").exists());
        assert!(!Path::new(&staged.path).join("link").exists());
    }

    #[test]
    fn relative_paths_and_bad_bytes_are_refused_and_leave_nothing() {
        let dir = TempDir::new();
        let store = store(&dir);
        let relative = AttachmentSource::Path {
            path: "notes.txt".into(),
        };
        assert!(matches!(
            store.stage(AttachmentId::generate(), &relative),
            Err(StageError::NotAbsolute(_))
        ));
        let id = AttachmentId::generate();
        let bad = AttachmentSource::Bytes {
            name: "x.png".into(),
            mime: "image/png".into(),
            data: "not base64!".into(),
        };
        assert!(matches!(store.stage(id, &bad), Err(StageError::BadBase64)));
        assert!(store.find(id).is_none());
        let huge = AttachmentSource::Bytes {
            name: "x.bin".into(),
            mime: "application/octet-stream".into(),
            data: "A".repeat(MAX_INLINE_ATTACHMENT / 3 * 4 + 400),
        };
        assert!(matches!(
            store.stage(AttachmentId::generate(), &huge),
            Err(StageError::TooLarge)
        ));
    }

    #[test]
    fn bytes_are_written_under_a_safe_name_and_moved_on_send() {
        let dir = TempDir::new();
        let store = store(&dir);
        let id = AttachmentId::generate();
        let pasted = AttachmentSource::Bytes {
            name: "../../evil.pdf".into(),
            mime: "application/pdf".into(),
            data: base64::engine::general_purpose::STANDARD.encode(b"%PDF"),
        };
        let staged = store.stage(id, &pasted).unwrap();
        assert_eq!(staged.name, "evil.pdf");
        assert_eq!(staged.kind, AttachmentKind::Pdf);
        assert!(staged.path.contains(&format!("{SEP}staged{SEP}")));

        let agent = store.agent_dir("agent-1");
        let moved = store.take(&staged, &agent).unwrap();
        assert_eq!(moved.path, agent.join(id.to_string()).join("evil.pdf"));
        assert_eq!(fs::read(&moved.path).unwrap(), b"%PDF");
        assert_eq!(moved.attachment.path, moved.path.to_string_lossy());
        assert!(store.find(id).is_none(), "the staged copy is gone");
    }

    #[test]
    fn control_characters_in_a_name_become_spaces() {
        let dir = TempDir::new();
        let store = store(&dir);
        let pasted = AttachmentSource::Bytes {
            name: "two\nlines\t.txt".into(),
            mime: "text/plain".into(),
            data: base64::engine::general_purpose::STANDARD.encode(b"hi"),
        };
        let staged = store.stage(AttachmentId::generate(), &pasted).unwrap();
        assert_eq!(staged.name, "two lines .txt");
        let name = format!("{SEP}two lines .txt");
        assert!(staged.path.ends_with(&name), "{}", staged.path);

        // Windows file names can't hold a `\r`, so there it's DEL alone.
        let name = if cfg!(windows) {
            "a\u{7f}b .png"
        } else {
            "a\u{7f}b\r.png"
        };
        let source = dir.path().join(name);
        fs::write(&source, b"png bytes").unwrap();
        let path = AttachmentSource::Path {
            path: source.to_string_lossy().into_owned(),
        };
        let staged = store.stage(AttachmentId::generate(), &path).unwrap();
        assert_eq!(staged.name, "a b .png");
        assert_eq!(fs::read(&staged.path).unwrap(), b"png bytes");
    }

    #[cfg(unix)]
    #[test]
    fn a_name_that_isnt_text_is_cleaned_byte_by_byte() {
        assert_eq!(
            clean_file_name(OsStr::from_bytes(b"\xff\n.bin")),
            OsStr::from_bytes(b"\xff .bin")
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_name_that_isnt_text_is_cleaned_unit_by_unit() {
        let name = |units: &[u16]| OsString::from_wide(units);
        assert_eq!(
            clean_file_name(&name(&[0xd800, 0x0a, 0x2e])),
            name(&[0xd800, 0x20, 0x2e])
        );
    }

    #[test]
    fn a_documents_text_is_read_as_its_staged_and_moves_with_it() {
        let dir = TempDir::new();
        let read = fake_program(
            &dir.path().join("bin"),
            "extract",
            "#!/bin/sh\nprintf read\n",
        );
        let store = store(&dir).with_extractor(Extractor::with(read, &[]));
        let source = dir.path().join("notes.docx");
        fs::write(&source, b"words").unwrap();
        let id = AttachmentId::generate();
        let staged = store
            .stage(
                id,
                &AttachmentSource::Path {
                    path: source.to_string_lossy().into_owned(),
                },
            )
            .unwrap();
        assert_eq!(staged.kind, AttachmentKind::File);
        let text = crate::doctext::text_path(Path::new(&staged.path));
        assert_eq!(fs::read_to_string(&text).unwrap(), "read");
        assert_eq!(
            store.find(id).unwrap(),
            staged,
            "the text isn't the attachment"
        );

        let moved = store.take(&staged, &store.agent_dir("agent-1")).unwrap();
        let text = moved.text.expect("the text moved with it");
        assert_eq!(text, crate::doctext::text_path(&moved.path));
        assert_eq!(fs::read_to_string(text).unwrap(), "read");
    }

    #[test]
    fn a_pictures_preview_is_made_as_its_staged_and_found_again() {
        let dir = TempDir::new();
        let store = store(&dir);
        let mut png = io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(40, 30, image::Rgb([200, 10, 10]))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let id = AttachmentId::generate();
        let pasted = AttachmentSource::Bytes {
            name: "shot.png".into(),
            mime: "image/png".into(),
            data: base64::engine::general_purpose::STANDARD.encode(png.into_inner()),
        };
        let staged = store.stage(id, &pasted).unwrap();
        assert!(staged.thumbnail.is_some());
        assert_eq!(store.find(id).unwrap(), staged);

        let moved = store.take(&staged, &store.agent_dir("agent-1")).unwrap();
        assert_eq!(moved.attachment.thumbnail, staged.thumbnail);
        assert_eq!(sidecar::thumbnail_of(&moved.path), staged.thumbnail);
        assert_eq!(moved.text, None);
    }

    #[test]
    fn stale_staged_copies_are_cleaned() {
        let dir = TempDir::new();
        let store = store(&dir);
        let pasted = AttachmentSource::Bytes {
            name: "a.txt".into(),
            mime: "text/plain".into(),
            data: base64::engine::general_purpose::STANDARD.encode(b"a"),
        };
        let id = AttachmentId::generate();
        store.stage(id, &pasted).unwrap();
        assert_eq!(store.clean(SystemTime::now()), 0);
        assert!(store.find(id).is_some());
        let tomorrow = SystemTime::now() + STALE_AFTER + Duration::from_secs(60);
        assert_eq!(store.clean(tomorrow), 1);
        assert!(store.find(id).is_none());
    }
}
