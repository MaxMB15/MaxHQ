// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Skills: reading `SKILL.md`, hashing a skill folder, a project's lock,
//! the links a checkout needs, what each CLI sees from a folder, and how
//! much of each CLI's listing a set takes. All of it reads files through
//! [`SkillFs`], which [`disk::DiskFs`] implements over the real disk and
//! [`mem::MemFs`] fakes, so the rest never touches the disk itself.

use std::io;
use std::path::{Component, Path, PathBuf};

pub mod always;
pub mod budget;
pub mod disk;
pub mod frontmatter;
pub mod globals;
pub mod lock;
pub mod mem;
pub mod pack;
pub mod plan;
pub mod scan;
pub mod store;
pub mod tree;

/// Where a checkout keeps its skills, from its root.
pub const SKILLS_DIR: &str = ".maxhq/skills";

/// Where agents write the skills they change, from a checkout's root. No
/// CLI scans it.
pub const PROPOSALS_DIR: &str = ".maxhq/proposals";

/// Where `maxd` mirrors the lock for you and for tools to read, from a
/// checkout's root. It's never read back.
pub const LOCK_MIRROR: &str = ".maxhq/skills.lock.json";

/// The set an Incus workspace mounts read only, as the guest sees it.
pub const MOUNT: &str = "/opt/maxhq/skills/current";

/// The folders MaxHQ links each skill into, from a checkout's root: the
/// one Claude reads and the one Codex, Cursor and most ACP agents read.
pub const LINK_DIRS: [&str; 2] = [".claude/skills", ".agents/skills"];

/// The file that makes a folder a skill.
pub const SKILL_FILE: &str = "SKILL.md";

/// What's at a path, without following a link there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    /// A folder.
    Dir,
    /// A regular file.
    File {
        /// Its size in bytes.
        size: u64,
        /// Whether any execute bit is set.
        executable: bool,
    },
    /// A symbolic link.
    Link {
        /// What it points at, as written.
        target: String,
    },
    /// Anything else, such as a device, a pipe or a socket. Nothing reads it.
    Other,
}

/// The files skills are read from. `maxd` implements it over the disk;
/// [`mem::MemFs`] is the fake tests use. A path that isn't there is
/// `Ok(None)`, so an error always means the read failed.
pub trait SkillFs {
    /// What's at `path`, not following a link at the last part.
    fn node(&self, path: &Path) -> io::Result<Option<Node>>;

    /// `path` with every link resolved, when it's there.
    fn real(&self, path: &Path) -> io::Result<Option<PathBuf>>;

    /// The names in the folder at `path`, following links, in any order.
    fn list(&self, path: &Path) -> io::Result<Vec<String>>;

    /// Up to `limit + 1` bytes of the file at `path`, following links, so
    /// the caller can tell a file over `limit` from one at it.
    fn read(&self, path: &Path, limit: u64) -> io::Result<Vec<u8>>;
}

/// `path` with `.` dropped and each `..` taking off the part before it,
/// without asking the disk.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// `path` as text with `/` between its parts on every OS, as plans, the
/// picks you send back and a link's target are written, so they read the
/// same whichever device made them. A Unix path is its own text.
pub fn path_text(path: &Path) -> String {
    if cfg!(windows) {
        slashed(path)
    } else {
        path.to_string_lossy().into_owned()
    }
}

/// `path`'s parts joined by `/`, after its prefix when it has one.
fn slashed(path: &Path) -> String {
    let mut text = String::new();
    // After a prefix such as `C:` the next part follows on directly, so a
    // path relative to a drive stays one.
    let mut joined = true;
    for part in path.components() {
        match part {
            Component::Prefix(prefix) => {
                text.push_str(&prefix.as_os_str().to_string_lossy());
                joined = true;
            }
            Component::RootDir => {
                text.push('/');
                joined = true;
            }
            other => {
                if !joined {
                    text.push('/');
                }
                text.push_str(&other.as_os_str().to_string_lossy());
                joined = false;
            }
        }
    }
    text
}

/// Where a link at `link` pointing at `target` leads, read as text.
pub fn link_points_at(link: &Path, target: &str) -> PathBuf {
    let base = link.parent().unwrap_or(Path::new("/"));
    normalize(&base.join(target))
}

/// Whether a file or folder name is one a skill never carries: Finder's
/// `.DS_Store` and `._*` files, and `.git`, in every spelling a disk may take
/// for the same name.
pub fn ignored(name: &str) -> bool {
    maxhq_transfer::name::skipped(name) || name.starts_with("._")
}

/// The hex SHA-256 of `bytes`.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let hash = ring::digest::digest(&ring::digest::SHA256, bytes);
    hash.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizing_drops_dots_and_climbs() {
        assert_eq!(
            normalize(Path::new(
                "/work/repo/.claude/skills/../../.maxhq/./skills/x"
            )),
            PathBuf::from("/work/repo/.maxhq/skills/x")
        );
    }

    #[test]
    fn path_text_is_slashed_on_every_os() {
        let joined = Path::new("/home/dev/.claude").join("skills").join("x");
        assert_eq!(path_text(&joined), "/home/dev/.claude/skills/x");
        assert_eq!(
            path_text(Path::new("../../.maxhq/skills/x")),
            "../../.maxhq/skills/x"
        );
        assert_eq!(
            path_text(&Path::new("packages").join("web")),
            "packages/web"
        );
        assert_eq!(slashed(Path::new("/a//b/./c/")), "/a/b/c");
        assert_eq!(slashed(Path::new("a/../b")), "a/../b");
        if cfg!(windows) {
            assert_eq!(slashed(Path::new(r"C:\Users\dev")), "C:/Users/dev");
            assert_eq!(slashed(Path::new(r"C:drive\relative")), "C:drive/relative");
            assert_eq!(slashed(Path::new(r"..\skills\x")), "../skills/x");
            assert_eq!(slashed(Path::new(r"\\server\share\x")), r"\\server\share/x");
        }
    }

    #[test]
    fn a_relative_link_resolves_from_its_folder() {
        assert_eq!(
            link_points_at(
                Path::new("/work/repo/.claude/skills/unslop"),
                "../../.maxhq/skills/unslop"
            ),
            PathBuf::from("/work/repo/.maxhq/skills/unslop")
        );
        assert_eq!(
            link_points_at(Path::new("/work/repo/.maxhq/skills"), MOUNT),
            PathBuf::from(MOUNT)
        );
    }

    #[test]
    fn finder_files_and_git_are_ignored() {
        for name in [
            ".DS_Store",
            "._SKILL.md",
            ".git",
            ".GIT",
            ".Git",
            ".g\u{200c}it",
            ".DS_STORE",
        ] {
            assert!(ignored(name), "{name}");
        }
        for name in ["SKILL.md", ".gitignore", "scripts"] {
            assert!(!ignored(name), "{name}");
        }
    }
}
