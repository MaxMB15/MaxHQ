// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What a dropped, pasted or picked path is, for its chip: the name, the
//! kind, the size and a MIME type. The guess goes by the extension first and
//! looks at the first bytes only when the extension says nothing.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use serde::Serialize;
use specta::Type;

/// What a path holds, in the terms the agents care about. Images and PDFs
/// can go to Claude inline, and everything else goes by path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    /// A picture a model can look at, such as a PNG or a JPEG.
    Image,
    /// A PDF.
    Pdf,
    /// Any other file, text or not.
    File,
    /// A folder.
    Folder,
    /// Nothing is there any more, or it can't be read.
    Missing,
}

/// One path, described for its chip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct PathInfo {
    /// The path as the webview passed it.
    pub path: String,
    /// The last part of the path, such as `notes.txt`.
    pub name: String,
    /// What it holds.
    pub kind: FileKind,
    /// The file's size in bytes. Unset for folders and missing paths.
    #[specta(type = Option<specta_typescript::Number>)]
    pub size: Option<u64>,
    /// The MIME type, such as `image/png`. Unset for folders and missing
    /// paths.
    pub mime: Option<String>,
}

/// How many bytes the guess reads when the extension says nothing.
const SNIFF_LEN: usize = 512;

/// Describes one path. A symlink describes what it points to.
pub fn describe(path: &str) -> PathInfo {
    let name = name_of(path);
    let missing = |name: String| PathInfo {
        path: path.to_owned(),
        name,
        kind: FileKind::Missing,
        size: None,
        mime: None,
    };
    let Ok(metadata) = std::fs::metadata(path) else {
        return missing(name);
    };
    if metadata.is_dir() {
        return PathInfo {
            path: path.to_owned(),
            name,
            kind: FileKind::Folder,
            size: None,
            mime: None,
        };
    }
    let (kind, mime) = match guess_by_name(&name) {
        Some(guess) => guess,
        None => match read_head(Path::new(path)) {
            Some(head) => guess_by_content(&head),
            None => return missing(name),
        },
    };
    PathInfo {
        path: path.to_owned(),
        name,
        kind,
        size: Some(metadata.len()),
        mime: Some(mime.to_owned()),
    }
}

/// The last part of a path, or the whole path when it has none, as `/`
/// doesn't.
fn name_of(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned())
}

/// The first bytes of a file, or none when it can't be opened.
fn read_head(path: &Path) -> Option<Vec<u8>> {
    let mut head = Vec::with_capacity(SNIFF_LEN);
    File::open(path)
        .ok()?
        .take(SNIFF_LEN as u64)
        .read_to_end(&mut head)
        .ok()?;
    Some(head)
}

/// The kind and MIME type from a file name's extension, or none when the
/// extension is missing or unknown.
pub fn guess_by_name(name: &str) -> Option<(FileKind, &'static str)> {
    let extension = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    let image = |mime| Some((FileKind::Image, mime));
    let file = |mime| Some((FileKind::File, mime));
    match extension.as_str() {
        "png" => image("image/png"),
        "jpg" | "jpeg" => image("image/jpeg"),
        "gif" => image("image/gif"),
        "webp" => image("image/webp"),
        "heic" => image("image/heic"),
        "heif" => image("image/heif"),
        "tif" | "tiff" => image("image/tiff"),
        "bmp" => image("image/bmp"),
        "pdf" => Some((FileKind::Pdf, "application/pdf")),
        // A model reads SVG as text, so it isn't an image here.
        "svg" => file("image/svg+xml"),
        "txt" | "text" | "log" => file("text/plain"),
        "md" | "markdown" => file("text/markdown"),
        "csv" => file("text/csv"),
        "tsv" => file("text/tab-separated-values"),
        "html" | "htm" => file("text/html"),
        "css" => file("text/css"),
        "js" | "mjs" | "cjs" | "jsx" => file("text/javascript"),
        "ts" | "mts" | "cts" | "tsx" => file("text/typescript"),
        "json" => file("application/json"),
        "xml" => file("application/xml"),
        "yaml" | "yml" => file("application/yaml"),
        "toml" => file("application/toml"),
        "sh" | "bash" | "zsh" => file("application/x-sh"),
        "py" => file("text/x-python"),
        "rs" => file("text/x-rust"),
        "go" => file("text/x-go"),
        "swift" => file("text/x-swift"),
        "c" | "h" => file("text/x-c"),
        "cc" | "cpp" | "cxx" | "hpp" => file("text/x-c++"),
        "java" => file("text/x-java"),
        "rb" => file("text/x-ruby"),
        "sql" => file("application/sql"),
        "zip" => file("application/zip"),
        "gz" | "tgz" => file("application/gzip"),
        "tar" => file("application/x-tar"),
        "mp4" => file("video/mp4"),
        "mov" => file("video/quicktime"),
        "mp3" => file("audio/mpeg"),
        "wav" => file("audio/wav"),
        _ => None,
    }
}

/// The kind and MIME type from a file's first bytes: a known image or PDF
/// signature, then text if the bytes are UTF-8 without NULs, and otherwise
/// opaque bytes.
pub fn guess_by_content(head: &[u8]) -> (FileKind, &'static str) {
    const SIGNATURES: &[(&[u8], FileKind, &str)] = &[
        (b"\x89PNG\r\n\x1a\n", FileKind::Image, "image/png"),
        (b"\xff\xd8\xff", FileKind::Image, "image/jpeg"),
        (b"GIF87a", FileKind::Image, "image/gif"),
        (b"GIF89a", FileKind::Image, "image/gif"),
        (b"%PDF-", FileKind::Pdf, "application/pdf"),
    ];
    for (signature, kind, mime) in SIGNATURES {
        if head.starts_with(signature) {
            return (*kind, mime);
        }
    }
    if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        return (FileKind::Image, "image/webp");
    }
    if is_text(head) {
        (FileKind::File, "text/plain")
    } else {
        (FileKind::File, "application/octet-stream")
    }
}

/// Whether bytes read as text. The sample may end inside a character, so
/// an incomplete sequence at the very end still counts.
fn is_text(head: &[u8]) -> bool {
    if head.contains(&0) {
        return false;
    }
    match std::str::from_utf8(head) {
        Ok(_) => true,
        Err(error) => error.error_len().is_none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_give_kind_and_mime_in_any_case() {
        assert_eq!(
            guess_by_name("Shot.PNG"),
            Some((FileKind::Image, "image/png"))
        );
        assert_eq!(
            guess_by_name("a.jpeg"),
            Some((FileKind::Image, "image/jpeg"))
        );
        assert_eq!(
            guess_by_name("spec.pdf"),
            Some((FileKind::Pdf, "application/pdf"))
        );
        assert_eq!(
            guess_by_name("notes.md"),
            Some((FileKind::File, "text/markdown"))
        );
        assert_eq!(
            guess_by_name("logo.svg"),
            Some((FileKind::File, "image/svg+xml"))
        );
        assert_eq!(
            guess_by_name("archive.tar.gz"),
            Some((FileKind::File, "application/gzip"))
        );
    }

    #[test]
    fn unknown_or_missing_extensions_give_no_guess() {
        assert_eq!(guess_by_name("Makefile"), None);
        assert_eq!(guess_by_name("data.qqq"), None);
        assert_eq!(guess_by_name(".gitignore"), None);
    }

    #[test]
    fn content_signatures_win_over_text() {
        assert_eq!(
            guess_by_content(b"\x89PNG\r\n\x1a\n rest"),
            (FileKind::Image, "image/png")
        );
        assert_eq!(
            guess_by_content(b"%PDF-1.7\n"),
            (FileKind::Pdf, "application/pdf")
        );
        assert_eq!(
            guess_by_content(b"RIFF\x10\0\0\0WEBPVP8 "),
            (FileKind::Image, "image/webp")
        );
    }

    #[test]
    fn utf8_without_nuls_is_text() {
        assert_eq!(
            guess_by_content(b"all: build\n"),
            (FileKind::File, "text/plain")
        );
        assert_eq!(guess_by_content(b""), (FileKind::File, "text/plain"));
        // The sample ends halfway through "é".
        assert_eq!(guess_by_content(b"caf\xc3"), (FileKind::File, "text/plain"));
        assert_eq!(
            guess_by_content(b"\x00\x01\x02"),
            (FileKind::File, "application/octet-stream")
        );
        assert_eq!(
            guess_by_content(b"\xff\xfe bad"),
            (FileKind::File, "application/octet-stream")
        );
    }

    #[test]
    fn describes_files_folders_and_missing_paths() {
        let dir = std::env::temp_dir().join(format!("maxhq-describe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = dir.join("Makefile");
        std::fs::write(&text, "all:\n").unwrap();
        let image = dir.join("shot");
        std::fs::write(&image, b"\x89PNG\r\n\x1a\nxxxx").unwrap();

        let described = describe(text.to_str().unwrap());
        assert_eq!(described.name, "Makefile");
        assert_eq!(described.kind, FileKind::File);
        assert_eq!(described.size, Some(5));
        assert_eq!(described.mime.as_deref(), Some("text/plain"));

        let described = describe(image.to_str().unwrap());
        assert_eq!(described.kind, FileKind::Image);
        assert_eq!(described.mime.as_deref(), Some("image/png"));

        let described = describe(dir.to_str().unwrap());
        assert_eq!(described.kind, FileKind::Folder);
        assert_eq!(described.size, None);
        assert_eq!(described.mime, None);

        let gone = dir.join("gone.png");
        let described = describe(gone.to_str().unwrap());
        assert_eq!(described.name, "gone.png");
        assert_eq!(described.kind, FileKind::Missing);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_root_is_named_by_its_path() {
        assert_eq!(name_of("/"), "/");
        assert_eq!(name_of("/Users/max/notes.txt"), "notes.txt");
        assert_eq!(name_of("/Users/max/src/"), "src");
    }
}
