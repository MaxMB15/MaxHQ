// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What `maxd` keeps beside an attachment: a small preview of a picture
//! and the text read out of a document. Every attachment has a folder of
//! its own, `<id>/<name>`, and its sidecars sit beside it there, the
//! preview as `.<name>.thumbnail` and the text as `<name>.txt`. They're
//! made where the attachment lands, on this device or in a container's
//! guest, and move with it, so a message read back from a transcript later
//! finds them again by the attachment's path.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use maxhq_os::fsmode;
use maxhq_protocol::timeline::{AttachmentKind, AttachmentRef};

use crate::doctext::{self, Extractor};
use crate::thumbnail;

/// How much of a preview's file is read back, a little past the longest
/// preview there is.
const PREVIEW_READ_CAP: u64 = 64 * 1024;

/// What sits beside an attachment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sidecars {
    /// A picture's preview, as a `data:` URL.
    pub thumbnail: Option<String>,
    /// Where a document's text is saved.
    pub text: Option<PathBuf>,
}

/// Makes the sidecars of the file that just landed at `path`, keeping any
/// it has already. `mime` is the file's media type, when it's known. A
/// folder gets none.
pub fn make(path: &Path, mime: Option<&str>, extractor: &Extractor) -> Sidecars {
    if !fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file()) {
        return Sidecars::default();
    }
    let mime = mime.or_else(|| mime_for(path));
    let thumbnail = thumbnail_of(path).or_else(|| {
        let url = thumbnail::thumbnail(path, mime)?;
        if let Some(preview) = preview_path(path)
            && let Err(err) = write_readable(&preview, url.as_bytes())
        {
            tracing::debug!(path = %path.display(), "couldn't keep a picture's preview: {err}");
        }
        Some(url)
    });
    Sidecars {
        thumbnail,
        text: extractor.save(path),
    }
}

/// The sidecars beside the file at `path`.
pub fn read(path: &Path) -> Sidecars {
    Sidecars {
        thumbnail: thumbnail_of(path),
        text: text_of(path),
    }
}

/// The preview kept beside the picture at `path`, if there's one.
pub fn thumbnail_of(path: &Path) -> Option<String> {
    let preview = preview_path(path)?;
    let meta = fs::symlink_metadata(&preview).ok()?;
    if !meta.is_file() || meta.len() > PREVIEW_READ_CAP {
        return None;
    }
    let url = fs::read_to_string(&preview).ok()?;
    thumbnail::is_preview(&url).then_some(url)
}

/// Gives the pictures read back from a transcript the previews kept
/// beside them.
pub fn add_previews(attachments: &mut [AttachmentRef]) {
    for attachment in attachments {
        let path = Path::new(&attachment.path);
        if attachment.kind == AttachmentKind::Image
            && attachment.thumbnail.is_none()
            && path.is_absolute()
        {
            attachment.thumbnail = thumbnail_of(path);
        }
    }
}

/// Where the text of the document at `path` is saved, if it is.
pub fn text_of(path: &Path) -> Option<PathBuf> {
    if !doctext::wants(path) {
        return None;
    }
    let text = doctext::text_path(path);
    fs::symlink_metadata(&text)
        .is_ok_and(|meta| meta.is_file())
        .then_some(text)
}

/// The attachment in a folder of its own, passing over its sidecars.
pub fn attachment_in(dir: &Path) -> Option<PathBuf> {
    let mut names: Vec<OsString> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    names.sort();
    let sidecars: HashSet<OsString> = names.iter().flat_map(|name| names_of(name)).collect();
    names
        .into_iter()
        .find(|name| !sidecars.contains(name))
        .map(|name| dir.join(name))
}

/// Moves the sidecars of the file that was at `from` beside the file now
/// at `to`.
pub fn carry(from: &Path, to: &Path) {
    let (Some(from_name), Some(to_name)) = (from.file_name(), to.file_name()) else {
        return;
    };
    let (Some(from_dir), Some(to_dir)) = (from.parent(), to.parent()) else {
        return;
    };
    for (old, new) in names_of(from_name).into_iter().zip(names_of(to_name)) {
        let old = from_dir.join(old);
        if fs::symlink_metadata(&old).is_err() {
            continue;
        }
        if let Err(err) = fs::rename(&old, to_dir.join(new)) {
            tracing::debug!(path = %old.display(), "couldn't move an attachment's sidecar: {err}");
        }
    }
}

/// Writes a sidecar that anyone may read, as an agent's CLI must, even
/// when a guest running as root makes it.
pub fn write_readable(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = fsmode::set_open_mode(
        OpenOptions::new().write(true).create(true).truncate(true),
        0o644,
    )
    .open(path)?;
    file.write_all(bytes)
}

/// The media type a file's name says it has, for the pictures a preview
/// can be made of.
fn mime_for(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    })
}

/// Where the preview of the picture at `path` is kept.
fn preview_path(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?;
    Some(path.with_file_name(&names_of(name)[0]))
}

/// The names of the sidecars a file named `name` can have: its preview,
/// its text, and the text while it's being written.
fn names_of(name: &OsStr) -> [OsString; 3] {
    let mut preview = OsString::from(".");
    preview.push(name);
    preview.push(".thumbnail");
    let mut text = name.to_owned();
    text.push(".txt");
    let mut part = text.clone();
    part.push(".part");
    [preview, text, part]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{fake_program, TempDir};

    fn picture(path: &Path) {
        image::RgbImage::from_pixel(40, 30, image::Rgb([200, 10, 10]))
            .save(path)
            .expect("a picture");
    }

    #[test]
    fn a_picture_keeps_its_preview_beside_it() {
        let dir = TempDir::new();
        let path = dir.path().join("red.png");
        picture(&path);
        let made = make(&path, None, &Extractor::default());
        let url = made.thumbnail.expect("a preview");
        assert!(url.starts_with("data:image/jpeg;base64,"));
        assert_eq!(made.text, None);
        assert!(dir.path().join(".red.png.thumbnail").is_file());
        assert_eq!(read(&path).thumbnail.as_deref(), Some(url.as_str()));
        assert_eq!(attachment_in(dir.path()), Some(path));
    }

    #[test]
    fn a_document_keeps_its_text_beside_it() {
        let dir = TempDir::new();
        let path = dir.path().join("a.docx");
        fs::write(&path, "words").unwrap();
        let bin = TempDir::new();
        let extract = fake_program(bin.path(), "extract", "#!/bin/sh\nprintf read\n");
        let made = make(&path, None, &Extractor::with(extract, &[]));
        assert_eq!(made.thumbnail, None);
        let text = dir.path().join("a.docx.txt");
        assert_eq!(made.text.as_deref(), Some(text.as_path()));
        assert_eq!(read(&path).text, Some(text));
        assert_eq!(attachment_in(dir.path()), Some(path));
    }

    #[test]
    fn pictures_read_back_get_their_previews() {
        let dir = TempDir::new();
        let path = dir.path().join("red.png");
        picture(&path);
        let url = make(&path, None, &Extractor::default()).thumbnail;
        let mut read_back = vec![AttachmentRef {
            id: maxhq_protocol::ids::AttachmentId::derive("red"),
            name: "red.png".to_owned(),
            kind: AttachmentKind::Image,
            bytes: 1,
            mime: Some("image/png".to_owned()),
            path: path.display().to_string(),
            thumbnail: None,
        }];
        read_back.push(AttachmentRef {
            kind: AttachmentKind::File,
            ..read_back[0].clone()
        });
        add_previews(&mut read_back);
        assert!(url.is_some());
        assert_eq!(read_back[0].thumbnail, url);
        assert_eq!(read_back[1].thumbnail, None);
    }

    #[test]
    fn a_file_named_like_a_sidecar_is_still_the_attachment() {
        let dir = TempDir::new();
        let path = dir.path().join("a.docx.txt");
        fs::write(&path, "words").unwrap();
        assert_eq!(attachment_in(dir.path()), Some(path));
    }

    #[test]
    fn sidecars_move_with_their_file() {
        let dir = TempDir::new();
        let from_dir = dir.path().join("staged/1");
        let to_dir = dir.path().join("agent/1");
        fs::create_dir_all(&from_dir).unwrap();
        fs::create_dir_all(&to_dir).unwrap();
        let from = from_dir.join("shot.png");
        picture(&from);
        make(&from, None, &Extractor::default());
        fs::write(from_dir.join("shot.png.txt"), "not a document's").unwrap();
        let to = to_dir.join("shot.png");
        fs::rename(&from, &to).unwrap();
        carry(&from, &to);
        assert!(read(&to).thumbnail.is_some());
        assert!(to_dir.join("shot.png.txt").is_file());
        assert_eq!(fs::read_dir(&from_dir).unwrap().count(), 0);
    }

    #[test]
    fn a_preview_that_isnt_one_is_left_unread() {
        let dir = TempDir::new();
        let path = dir.path().join("a.png");
        fs::write(&path, "not a picture").unwrap();
        fs::write(dir.path().join(".a.png.thumbnail"), "javascript:alert(1)").unwrap();
        assert_eq!(thumbnail_of(&path), None);
        assert_eq!(
            make(&path, Some("image/png"), &Extractor::default()),
            Sidecars::default()
        );
        assert_eq!(
            make(dir.path(), None, &Extractor::default()),
            Sidecars::default()
        );
    }
}
