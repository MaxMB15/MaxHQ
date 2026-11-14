// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Names inside a transfer: the checks a receiver makes on every path, and
//! the clean-up a download's name gets before it lands on the Mac.

use unicode_normalization::UnicodeNormalization;

/// The longest file name Linux and APFS take, in bytes.
pub const MAX_NAME: usize = 255;

/// The longest symlink target a receiver recreates, in bytes.
pub const MAX_TARGET: usize = 4095;

/// The most parts a path in a transfer has. A real tree, pnpm's store
/// included, stays under 20.
pub const MAX_DEPTH: usize = 128;

/// The longest path in a transfer, in bytes, the most Linux opens at once.
pub const MAX_PATH: usize = 4096;

/// Why a path was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BadPath {
    /// The path is empty or has an empty part, such as `a//b`.
    #[error("the path has an empty part")]
    Empty,
    /// The path starts at `/`.
    #[error("the path is absolute")]
    Absolute,
    /// A part is `.` or `..`.
    #[error("the path has a `.` or `..` part")]
    Dots,
    /// A part has a zero byte or another control character.
    #[error("the path has a control character")]
    Control,
    /// A part is longer than [`MAX_NAME`] bytes.
    #[error("a part of the path is over {MAX_NAME} bytes")]
    TooLong,
    /// The path is over [`MAX_DEPTH`] parts or [`MAX_PATH`] bytes.
    #[error("the path is over {MAX_DEPTH} folders deep or {MAX_PATH} bytes long")]
    TooDeep,
}

/// Refuses a path over [`MAX_DEPTH`] parts or [`MAX_PATH`] bytes, before
/// anything is spent on its parts.
pub fn check_depth(path: &str) -> Result<(), BadPath> {
    if path.len() > MAX_PATH || path.split('/').count() > MAX_DEPTH {
        return Err(BadPath::TooDeep);
    }
    Ok(())
}

/// Checks a manifest path and gives its parts in NFC.
///
/// A path is relative and `/` separated, no deeper than [`check_depth`]
/// allows, and no part is empty, `.`, `..`, longer than [`MAX_NAME`] bytes
/// or holds a control character.
pub fn check(path: &str) -> Result<Vec<String>, BadPath> {
    if path.starts_with('/') {
        return Err(BadPath::Absolute);
    }
    check_depth(path)?;
    path.split('/').map(part).collect()
}

/// Checks one part of a path, giving it in NFC.
pub fn part(name: &str) -> Result<String, BadPath> {
    if name.is_empty() {
        return Err(BadPath::Empty);
    }
    if name.chars().any(char::is_control) {
        return Err(BadPath::Control);
    }
    let name = nfc(name);
    if name == "." || name == ".." {
        return Err(BadPath::Dots);
    }
    if name.len() > MAX_NAME {
        return Err(BadPath::TooLong);
    }
    Ok(name)
}

/// The text in NFC, the form Linux compares and the Mac usually types.
pub fn nfc(text: &str) -> String {
    text.nfc().collect()
}

/// Whether a sender leaves this file or folder out and a receiver refuses
/// it: `.git` folders and Finder's `.DS_Store`, in every spelling a disk may
/// take for the same name. On APFS and HFS+, `.GIT` opens the real `.git`.
pub fn skipped(name: &str) -> bool {
    let folded = folded(name);
    folded == ".git" || folded == ".ds_store"
}

/// The name as a Mac disk compares it: APFS and HFS+ ignore case, APFS folds
/// `\u{17f}` to `s` and the two `st` ligatures to `st`, and HFS+ ignores the
/// invisible characters listed here. The result is only good for comparing.
pub(crate) fn folded(name: &str) -> String {
    let mut folded = String::with_capacity(name.len());
    for c in name.chars() {
        match c {
            '\u{200c}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{206a}'..='\u{206f}'
            | '\u{feff}' => {}
            '\u{17f}' => folded.push('s'),
            '\u{fb05}' | '\u{fb06}' => folded.push_str("st"),
            c => folded.extend(c.to_lowercase()),
        }
    }
    nfc(&folded)
}

/// Whether a symlink at `parent` with this target resolves inside the
/// transfer, read as text. `link` says whether a path inside the transfer
/// is itself a symlink. A target may not step through one, since that
/// link could point somewhere shallower and let `..` climb out.
pub fn link_stays_inside(
    parent: &[String],
    target: &str,
    link: impl Fn(&[String]) -> bool,
) -> bool {
    if target.is_empty()
        || target.len() > MAX_TARGET
        || target.starts_with('/')
        || target.contains('\0')
    {
        return false;
    }
    let mut at: Vec<String> = parent.to_vec();
    let parts: Vec<&str> = target.split('/').filter(|part| !part.is_empty()).collect();
    for (index, part) in parts.iter().enumerate() {
        match *part {
            "." => {}
            ".." => {
                if at.pop().is_none() {
                    return false;
                }
            }
            name => {
                at.push(nfc(name));
                let last = index + 1 == parts.len();
                if !last && link(&at) {
                    return false;
                }
            }
        }
    }
    true
}

/// A name a download can land under in a Mac folder: the last part of the
/// path, with slashes, colons and control characters swapped for `_`, no
/// leading dot, so Finder shows it, and at most [`MAX_NAME`] bytes.
pub fn download_name(path: &str) -> String {
    let last = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default();
    let mut name: String = nfc(last)
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = name.trim_start_matches('.').trim();
    name = if trimmed.is_empty() {
        "download".to_owned()
    } else {
        trimmed.to_owned()
    };
    truncate(&mut name, MAX_NAME);
    name
}

/// The first of `name`, `name 2`, `name 3` and so on that `taken` says is
/// free, with the number before the extension. The caller checks the
/// disk, which on APFS ignores case, so `Notes.txt` and `notes.txt` clash.
pub fn free_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_owned();
    }
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, ""),
    };
    let mut number = 2u32;
    loop {
        let suffix = format!(" {number}{extension}");
        let mut stem = stem.to_owned();
        truncate(&mut stem, MAX_NAME.saturating_sub(suffix.len()));
        let candidate = format!("{stem}{suffix}");
        if !taken(&candidate) || number == u32::MAX {
            return candidate;
        }
        number += 1;
    }
}

fn truncate(text: &mut String, bytes: usize) {
    if text.len() <= bytes {
        return;
    }
    let mut end = bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_goes_so_deep_and_no_deeper() {
        let deep = |parts: usize| vec!["d"; parts].join("/");
        assert_eq!(check(&deep(MAX_DEPTH)).unwrap().len(), MAX_DEPTH);
        assert_eq!(check(&deep(MAX_DEPTH + 1)), Err(BadPath::TooDeep));
        let long = "a".repeat(MAX_NAME);
        assert_eq!(
            check(&vec![long.as_str(); 20].join("/")),
            Err(BadPath::TooDeep)
        );
    }

    #[test]
    fn plain_paths_pass_and_come_back_in_nfc() {
        assert_eq!(check("notes/run.sh").unwrap(), ["notes", "run.sh"]);
        // An e and a combining acute accent, as APFS may keep it.
        let decomposed = "cafe\u{301}.txt";
        assert_eq!(check(decomposed).unwrap(), ["caf\u{e9}.txt"]);
        assert_eq!(check(".env").unwrap(), [".env"]);
        assert_eq!(check("a..b").unwrap(), ["a..b"]);
    }

    #[test]
    fn paths_that_escape_or_hide_are_refused() {
        assert_eq!(check("/etc/passwd"), Err(BadPath::Absolute));
        assert_eq!(check("../up"), Err(BadPath::Dots));
        assert_eq!(check("a/../../up"), Err(BadPath::Dots));
        assert_eq!(check("a/./b"), Err(BadPath::Dots));
        assert_eq!(check(""), Err(BadPath::Empty));
        assert_eq!(check("a//b"), Err(BadPath::Empty));
        assert_eq!(check("a/"), Err(BadPath::Empty));
        assert_eq!(check("a\0b"), Err(BadPath::Control));
        assert_eq!(check("line\nbreak"), Err(BadPath::Control));
        assert_eq!(check("a\tb"), Err(BadPath::Control));
        assert_eq!(check("a\u{7f}b"), Err(BadPath::Control));
        assert_eq!(check(&"x".repeat(256)), Err(BadPath::TooLong));
        assert!(check(&"x".repeat(255)).is_ok());
    }

    #[test]
    fn git_folders_and_finder_files_are_skipped() {
        assert!(skipped(".git"));
        assert!(skipped(".DS_Store"));
        assert!(!skipped(".github"));
        assert!(!skipped(".gitignore"));
    }

    #[test]
    fn other_spellings_of_those_names_are_skipped_too() {
        for name in [
            ".GIT",
            ".Git",
            ".g\u{200c}it",
            ".gi\u{200e}t",
            "\u{202a}.git",
            ".\u{206a}GIT",
            ".git\u{feff}",
            ".DS_STORE",
            ".ds_store",
            ".DS_\u{17f}tore",
            ".DS_\u{fb05}ore",
            ".DS_\u{fb06}ORE",
            ".ds\u{200d}_store",
        ] {
            assert!(skipped(name), "{name:?}");
        }
        // Accents count on every disk, so these are other names.
        for name in [
            "git",
            ".gi\u{301}t",
            ".g\u{131}t",
            ".GITHUB",
            ".git.",
            ".ds_stores",
        ] {
            assert!(!skipped(name), "{name:?}");
        }
    }

    #[test]
    fn links_must_stay_inside_and_not_step_through_links() {
        let parent = vec!["notes".to_owned()];
        let none = |_: &[String]| false;
        assert!(link_stays_inside(&parent, "run.sh", none));
        assert!(link_stays_inside(&parent, "../notes/run.sh", none));
        assert!(link_stays_inside(&parent, "..", none));
        assert!(!link_stays_inside(&parent, "../..", none));
        assert!(!link_stays_inside(&parent, "../../etc/passwd", none));
        assert!(!link_stays_inside(&parent, "/etc/passwd", none));
        assert!(!link_stays_inside(&parent, "", none));
        // `here` points at the folder it's in, so `here/..` would climb out.
        let here = |path: &[String]| path == ["notes", "here"];
        assert!(!link_stays_inside(&parent, "here/../..", here));
        assert!(link_stays_inside(&parent, "here", here));
    }

    #[test]
    fn download_names_are_safe_in_finder() {
        assert_eq!(download_name("/home/agent/work/dist/app.zip"), "app.zip");
        assert_eq!(download_name("dist/"), "dist");
        assert_eq!(download_name(".bashrc"), "bashrc");
        assert_eq!(download_name("a:b\u{7}c"), "a_b_c");
        assert_eq!(download_name(".."), "download");
        assert_eq!(download_name(""), "download");
        let long = "\u{e9}".repeat(200);
        let name = download_name(&long);
        assert!(name.len() <= MAX_NAME);
        assert!(name.chars().all(|c| c == '\u{e9}'));
    }

    #[test]
    fn a_taken_name_gets_a_number_before_its_extension() {
        let taken = ["notes.txt", "notes 2.txt", "dist"];
        let is_taken = |name: &str| taken.iter().any(|taken| taken.eq_ignore_ascii_case(name));
        assert_eq!(free_name("new.txt", is_taken), "new.txt");
        assert_eq!(free_name("Notes.txt", is_taken), "Notes 3.txt");
        assert_eq!(free_name("dist", is_taken), "dist 2");
        assert_eq!(free_name(".txt", |name| name == ".txt"), ".txt 2");
    }
}
