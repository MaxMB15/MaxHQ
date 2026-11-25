// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Paths on another machine that reads them as Linux and macOS do: a
//! container's, a guest's or a VM's. This device's own rules never judge
//! them, so on Windows a `\` stays part of a name and `/work` is a full
//! path though it has no drive. On Unix each gives what `std::path` gives.

/// A path's parts. `/` alone splits them, and an empty or `.` part says
/// nothing.
pub fn parts(path: &str) -> impl Iterator<Item = &str> {
    path.split('/')
        .filter(|part| !part.is_empty() && *part != ".")
}

/// Whether the path starts at the root.
pub fn is_absolute(path: &str) -> bool {
    path.starts_with('/')
}

/// Whether a part of the path is `..`.
pub fn climbs(path: &str) -> bool {
    parts(path).any(|part| part == "..")
}

/// `path` read from the folder `base`. An absolute path stands alone.
pub fn join(base: &str, path: &str) -> String {
    if is_absolute(path) || base.is_empty() {
        path.to_owned()
    } else if base.ends_with('/') {
        format!("{base}{path}")
    } else {
        format!("{base}/{path}")
    }
}

/// The parts of `path` below `root`, or none when it isn't `root` or inside
/// it. Parts are compared whole, so `/work` doesn't hold `/workshop`.
pub fn below<'a>(path: &'a str, root: &str) -> Option<Vec<&'a str>> {
    if is_absolute(path) != is_absolute(root) {
        return None;
    }
    let mut rest = parts(path);
    for want in parts(root) {
        (rest.next()? == want).then_some(())?;
    }
    Some(rest.collect())
}

/// Whether `path` is `root` or inside it.
pub fn starts_with(path: &str, root: &str) -> bool {
    below(path, root).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_reads_the_same_on_every_device() {
        assert!(is_absolute("/work/ws"));
        assert!(!is_absolute("work/ws"));
        assert_eq!(join("/work/ws", "notes.txt"), "/work/ws/notes.txt");
        assert_eq!(join("/work/ws/", "notes.txt"), "/work/ws/notes.txt");
        assert_eq!(join("/work/ws", "/etc/hosts"), "/etc/hosts");
        assert!(climbs("/work/ws/../etc"));
        // A backslash is part of a name, not a step up or a separator.
        assert!(!climbs(r"/work/ws/..\etc"));
        assert_eq!(
            below(r"/work//ws/./a\b/c", "/work/ws"),
            Some(vec![r"a\b", "c"])
        );
        assert!(starts_with("/work/ws", "/work/ws/"));
        assert!(!starts_with("/workshop/ws", "/work"));
        assert!(!starts_with("work/ws", "/work"));
        assert!(!starts_with(r"/work\ws", "/work"));
    }

    /// Each answer is the one `std::path` gives on Unix.
    #[cfg(unix)]
    #[test]
    fn a_path_reads_as_std_reads_it_on_unix() {
        use std::path::{Component, Path};
        let paths = [
            "/work/ws",
            "/work/ws/",
            "/work//ws/./a/b",
            "/work/ws/../etc",
            "/workshop/ws",
            "/",
            "",
            "work/ws",
            "notes.txt",
            "/etc/hosts",
        ];
        for path in paths {
            assert_eq!(is_absolute(path), Path::new(path).is_absolute(), "{path}");
            assert_eq!(
                climbs(path),
                Path::new(path)
                    .components()
                    .any(|part| part == Component::ParentDir),
                "{path}"
            );
            for base in paths {
                assert_eq!(
                    Some(join(base, path).as_str()),
                    Path::new(base).join(path).to_str(),
                    "{base} {path}"
                );
                // An empty root is no folder, so it holds nothing here,
                // where std finds every path in it.
                if !base.is_empty() {
                    assert_eq!(
                        starts_with(path, base),
                        Path::new(path).starts_with(base),
                        "{path} {base}"
                    );
                }
            }
        }
    }
}
