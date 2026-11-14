// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Pictures and videos the timeline shows, and the files the file viewer
//! opens, code and plain text among them. Each comes from its device the
//! way a download does, so the device's own limits on what an agent can
//! read are the only gate, and lands in a cache folder of the client's
//! own. A file there is named for its SHA-256 and an extension from a short
//! list, so nothing in the path an agent wrote steers where it lands, and
//! whatever serves the folder can tell a cached file from anything else.

use std::fs;
use std::io;
use std::path::Path;
use std::time::SystemTime;

use tracing::debug;

/// The most the cache keeps. Past it, what was shown longest ago goes.
pub const CACHE_LIMIT: u64 = 2 * 1024 * 1024 * 1024;

/// The largest file fetched at all, whatever the limit asked.
pub const MOST: u64 = 1024 * 1024 * 1024;

/// What fetching a picture or video came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Media {
    /// It's in the cache under `name`.
    Cached {
        /// The file's name in the cache folder.
        name: String,
        /// Its size in bytes.
        size: u64,
    },
    /// It's larger than the limit, so it stayed on the device.
    TooLarge {
        /// Its size in bytes.
        size: u64,
    },
    /// The device refused its path, as one outside the agent's folders
    /// or one that's bad some other way.
    Refused {
        /// Whether it's outside the agent's folders, which Show anyway
        /// can still fetch. A `maxd` from before Show anyway says a bad
        /// path for that too.
        outside: bool,
        /// Why, for a person to read.
        message: String,
    },
}

/// The media type for a cached file's extension, or none for one the
/// cache doesn't keep.
pub fn mime(extension: &str) -> Option<&'static str> {
    Some(match extension {
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "gif" => "image/gif",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "ico" => "image/x-icon",
        "jpeg" | "jpg" => "image/jpeg",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "tif" | "tiff" => "image/tiff",
        "webp" => "image/webp",
        "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "mp4" => "video/mp4",
        "ogv" => "video/ogg",
        "webm" => "video/webm",
        "md" | "markdown" => "text/markdown; charset=utf-8",
        "pdf" => "application/pdf",
        "csv" => "text/csv; charset=utf-8",
        "tsv" => "text/tab-separated-values; charset=utf-8",
        "glb" => "model/gltf-binary",
        "gltf" => "model/gltf+json",
        "stl" => "model/stl",
        "obj" => "model/obj",
        "3mf" => "model/3mf",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        // Code and text go as plain text whatever they hold, so HTML, XML
        // and the like never render as a page.
        _ if CODE.contains(&extension) => "text/plain; charset=utf-8",
        _ => return None,
    })
}

/// The extension a file at `path` on a device keeps in the cache, from the
/// end of its name, or none when the cache doesn't keep that kind.
pub fn extension(path: &str) -> Option<&'static str> {
    let name = path.rsplit(['/', '\\']).next()?;
    let (_, extension) = name.rsplit_once('.')?;
    let extension = extension.to_ascii_lowercase();
    kept().find(|known| *known == extension)
}

/// Every extension the cache keeps.
fn kept() -> impl Iterator<Item = &'static str> {
    EXTENSIONS.iter().chain(CODE.iter()).copied()
}

/// Each extension the cache keeps, as [`mime`] knows them, besides code:
/// pictures and videos, then what the file viewer opens, which the app's
/// list of viewer files (packages/shared/src/agent-view/viewer-files.ts)
/// names too.
const EXTENSIONS: [&str; 34] = [
    "avif", "bmp", "gif", "heic", "heif", "ico", "jpeg", "jpg", "png", "svg", "tif", "tiff",
    "webp", "m4v", "mov", "mp4", "ogv", "webm", "md", "markdown", "pdf", "csv", "tsv", "glb",
    "gltf", "stl", "obj", "3mf", "docx", "xlsx", "pptx", "odt", "ods", "odp",
];

/// The code and plain text extensions the file viewer shows as text, which
/// the app's list names as code too. Files that hold secrets, such as
/// .env, .pem and .key, aren't here.
const CODE: [&str; 48] = [
    "bash",
    "c",
    "cc",
    "cjs",
    "cpp",
    "css",
    "cts",
    "cxx",
    "diff",
    "dockerfile",
    "go",
    "h",
    "hh",
    "hpp",
    "htm",
    "html",
    "hxx",
    "java",
    "js",
    "json",
    "jsonc",
    "jsx",
    "kt",
    "kts",
    "mjs",
    "mts",
    "nix",
    "patch",
    "py",
    "pyi",
    "rb",
    "rs",
    "sh",
    "sql",
    "swift",
    "toml",
    "ts",
    "tsx",
    "xml",
    "yaml",
    "yml",
    "zsh",
    "cfg",
    "conf",
    "ini",
    "log",
    "text",
    "txt",
];

/// The name a file with SHA-256 `sha` keeps in the cache, or none when
/// `sha` isn't one.
pub fn cached_name(sha: &str, extension: &str) -> Option<String> {
    let name = format!("{sha}.{extension}");
    is_cached_name(&name).then_some(name)
}

/// Whether `name` is one the cache gives a file: 64 lowercase hex digits,
/// a dot and an extension it keeps.
pub fn is_cached_name(name: &str) -> bool {
    let Some((sha, extension)) = name.split_once('.') else {
        return false;
    };
    sha.len() == 64
        && sha
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && kept().any(|known| known == extension)
}

/// Marks a cached file as shown now, so the cache keeps it longest.
pub(crate) fn touch(path: &Path) {
    let touched = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|file| file.set_modified(SystemTime::now()));
    if let Err(err) = touched {
        debug!(%err, path = %path.display(), "couldn't mark a cached file as shown");
    }
}

/// Removes what was shown longest ago from `folder` until what's left fits
/// in `limit`, keeping `keep` whatever its age.
pub(crate) fn prune(folder: &Path, limit: u64, keep: &Path) -> io::Result<()> {
    let mut cached = Vec::new();
    let mut total = 0;
    for item in fs::read_dir(folder)?.flatten() {
        let name = item.file_name();
        if !name.to_str().is_some_and(is_cached_name) {
            continue;
        }
        let Ok(meta) = item.metadata() else {
            continue;
        };
        total += meta.len();
        let shown = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        cached.push((shown, meta.len(), item.path()));
    }
    cached.sort();
    for (_, size, path) in cached {
        if total <= limit {
            break;
        }
        if path == keep {
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => total -= size,
            Err(err) => debug!(%err, path = %path.display(), "couldn't drop a cached file"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn a_name_comes_from_the_hash_and_a_known_extension() {
        assert_eq!(extension("/w/shots/Frame One.PNG"), Some("png"));
        assert_eq!(extension("clip.webm"), Some("webm"));
        assert_eq!(extension("C:\\w\\pic.jpeg"), Some("jpeg"));
        assert_eq!(extension("/w/notes.txt"), Some("txt"));
        assert_eq!(extension("/w/src/main.RS"), Some("rs"));
        assert_eq!(extension("/w/.env"), None);
        assert_eq!(extension("/w/app.env"), None);
        assert_eq!(extension("/w/server.pem"), None);
        assert_eq!(extension("/w/server.key"), None);
        assert_eq!(extension("/w/docs/Plan.MD"), Some("md"));
        assert_eq!(extension("/w/part.3mf"), Some("3mf"));
        assert_eq!(extension("/w/part.step"), None);
        assert_eq!(extension("/w/png"), None);
        assert_eq!(extension("/w.png/file"), None);
        assert_eq!(
            cached_name(SHA, "png").as_deref(),
            Some(&*format!("{SHA}.png"))
        );
        assert_eq!(cached_name("../../etc", "png"), None);
        assert_eq!(cached_name(&SHA.to_uppercase(), "png"), None);
        assert_eq!(cached_name(SHA, "exe"), None);
        assert_eq!(
            cached_name(SHA, "html").as_deref(),
            Some(&*format!("{SHA}.html"))
        );
    }

    #[test]
    fn every_extension_kept_has_a_media_type() {
        for extension in kept() {
            assert!(mime(extension).is_some(), "{extension}");
        }
        assert_eq!(kept().count(), EXTENSIONS.len() + CODE.len());
        let mut sorted: Vec<_> = kept().collect();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), kept().count(), "an extension is listed twice");
    }

    #[test]
    fn code_and_text_go_as_plain_text() {
        for extension in ["rs", "ts", "html", "htm", "xml", "js", "txt", "log", "conf"] {
            assert_eq!(
                mime(extension),
                Some("text/plain; charset=utf-8"),
                "{extension}"
            );
        }
        assert_eq!(mime("svg"), Some("image/svg+xml"));
        assert_eq!(mime("env"), None);
        assert_eq!(mime("pem"), None);
        assert_eq!(mime("key"), None);
    }

    #[test]
    fn only_a_cache_name_is_one() {
        assert!(is_cached_name(&format!("{SHA}.mp4")));
        assert!(is_cached_name(&format!("{SHA}.rs")));
        assert!(!is_cached_name(&format!("{SHA}.env")));
        assert!(!is_cached_name(&format!("{SHA}.mp4.part")));
        assert!(!is_cached_name(&format!("{SHA}/x.png")));
        assert!(!is_cached_name(&format!("{}.png", &SHA[1..])));
        assert!(!is_cached_name(".incoming"));
        assert!(!is_cached_name(&format!("../{SHA}.png")));
    }

    #[test]
    fn pruning_drops_what_was_shown_longest_ago_first() {
        let dir = std::env::temp_dir().join(format!(
            "maxhq-media-{}",
            maxhq_protocol::ids::IdempotencyKey::generate()
        ));
        fs::create_dir_all(dir.join(".incoming")).unwrap();
        let file = |digit: char, shown: u64| {
            let path = dir.join(format!("{}.png", digit.to_string().repeat(64)));
            fs::write(&path, [0; 100]).unwrap();
            let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 + shown);
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(at)
                .unwrap();
            path
        };
        let oldest = file('1', 0);
        let older = file('2', 10);
        let newer = file('3', 20);
        let newest = file('4', 30);
        fs::write(dir.join("notes.txt"), [0; 500]).unwrap();

        // The oldest is the one just fetched, so it stays.
        prune(&dir, 250, &oldest).unwrap();
        assert!(oldest.exists());
        assert!(!older.exists());
        assert!(!newer.exists());
        assert!(newest.exists());
        assert!(dir.join("notes.txt").exists());
        assert!(dir.join(".incoming").exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
