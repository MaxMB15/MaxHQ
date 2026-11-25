// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Paths without the `\\?\` Windows puts in front of every path it gives
//! back from a handle, `canonicalize` or a link, where the path means the
//! same without it.

use std::path::PathBuf;

/// `path` without Windows' `\\?\` prefix, when it means the same without
/// it: a drive or share path short enough for the old limit, with no
/// part that the plain form would read differently. Any other path,
/// and every path on Unix, comes back as it is.
pub fn plain(path: PathBuf) -> PathBuf {
    if !cfg!(windows) {
        return path;
    }
    match path.to_str().and_then(unverbatim) {
        Some(text) => PathBuf::from(text),
        None => path,
    }
}

/// Windows' path text `text` without its `\\?\`, or none when it has no
/// such prefix or means something else without it. The plain form reads
/// `.`, `..`, a trailing dot or space, a stream's `:` and device names
/// such as `NUL` its own way, and stops at 260 characters.
fn unverbatim(text: &str) -> Option<String> {
    let (plain, names) = if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        let mut parts = rest.splitn(3, '\\');
        let (server, share) = (parts.next()?, parts.next()?);
        if server.is_empty() || share.is_empty() {
            return None;
        }
        (format!(r"\\{rest}"), parts.next().unwrap_or(""))
    } else {
        let rest = text.strip_prefix(r"\\?\")?;
        let names = match rest.as_bytes() {
            [letter, b':', b'\\', ..] if letter.is_ascii_alphabetic() => &rest[3..],
            _ => return None,
        };
        (rest.to_owned(), names)
    };
    if plain.encode_utf16().count() >= 260 || plain.contains('/') {
        return None;
    }
    let names: Vec<&str> = names.split('\\').collect();
    let last = names.len() - 1;
    for (n, name) in names.iter().enumerate() {
        let odd = if name.is_empty() {
            n != last
        } else {
            *name == "."
                || *name == ".."
                || name.ends_with(['.', ' '])
                || name.contains(':')
                || is_device(name)
        };
        if odd {
            return None;
        }
    }
    Some(plain)
}

/// Whether Windows reads the name `name` as a device in a plain path,
/// with or without an extension: `CON`, `NUL`, `COM1` and the like.
fn is_device(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    let upper = stem.to_ascii_uppercase();
    match upper.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" => true,
        _ => {
            let mut chars = upper.chars();
            let head: String = chars.by_ref().take(3).collect();
            let digit = chars.next();
            (head == "COM" || head == "LPT")
                && chars.next().is_none()
                && digit.is_some_and(|c| c.is_ascii_digit() || matches!(c, '¹' | '²' | '³'))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_windows_drops_the_prefix() {
        let odd = PathBuf::from(r"\\?\C:\Users\dev");
        let want = if cfg!(windows) {
            PathBuf::from(r"C:\Users\dev")
        } else {
            odd.clone()
        };
        assert_eq!(plain(odd), want);
    }

    #[test]
    fn a_verbatim_path_loses_its_prefix_only_when_it_means_the_same() {
        let plain = |text: &str| unverbatim(text);
        assert_eq!(plain(r"\\?\C:\Users\dev"), Some(r"C:\Users\dev".into()));
        assert_eq!(plain(r"\\?\C:\"), Some(r"C:\".into()));
        assert_eq!(plain(r"\\?\d:\work\x.txt"), Some(r"d:\work\x.txt".into()));
        assert_eq!(
            plain(r"\\?\UNC\server\share\folder"),
            Some(r"\\server\share\folder".into())
        );
        assert_eq!(
            plain(r"\\?\UNC\server\share"),
            Some(r"\\server\share".into())
        );
        // Not verbatim, or not a drive or share.
        assert_eq!(plain(r"C:\Users"), None);
        assert_eq!(plain(r"\\?\Volume{0123}\x"), None);
        assert_eq!(plain(r"\\?\GLOBALROOT\Device\x"), None);
        assert_eq!(plain(r"\\?\UNC\server"), None);
        assert_eq!(plain(r"\\?\UNC\\share"), None);
        // Parts the plain form reads another way.
        for odd in [
            r"\\?\C:\a\.\b",
            r"\\?\C:\a\..\b",
            r"\\?\C:\trailing.",
            r"\\?\C:\space \x",
            r"\\?\C:\file:stream",
            r"\\?\C:\NUL",
            r"\\?\C:\dir\con.txt",
            r"\\?\C:\COM1",
            r"\\?\C:\lpt9.log",
            r"\\?\C:\COM¹",
            r"\\?\C:\a\\b",
            r"\\?\C:\a/b",
        ] {
            assert_eq!(plain(odd), None, "{odd}");
        }
        for fine in [
            r"\\?\C:\CONFIG",
            r"\\?\C:\COM10",
            r"\\?\C:\nully",
            r"\\?\C:\.git",
        ] {
            assert!(plain(fine).is_some(), "{fine}");
        }
        let long = format!(r"\\?\C:\{}", "a".repeat(300));
        assert_eq!(plain(&long), None);
    }
}
