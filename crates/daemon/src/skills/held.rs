// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A skill folder read once, through an open folder, into memory. Its tree
//! hash and every copy written from it are of the same bytes, so a file
//! changed, swapped for a link or swapped for a pipe after the read can't
//! be what's copied. Each file opens once without following a link, and
//! only folders and regular files are taken, as [`tree::read`] takes them.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use maxhq_core::skills::ignored;
use maxhq_core::skills::tree::{self, TreeError, MAX_BYTES, MAX_ENTRIES};
use maxhq_protocol::message::{EntryKind, ManifestEntry};
use maxhq_protocol::skills::TreeHash;
use maxhq_transfer::beneath::{Dir, FileType};

/// A skill folder's manifest and the bytes of each of its files.
#[derive(Debug)]
pub struct Held {
    /// The files and folders, sorted by path, as [`tree::read`] gives them.
    pub entries: Vec<ManifestEntry>,
    data: BTreeMap<String, Vec<u8>>,
}

impl Held {
    /// Reads the skill folder at `path`. `path` itself may be a link, as a
    /// linked skill is; nothing inside it may.
    pub fn at(path: &Path) -> Result<Self, TreeError> {
        let opened = fs::canonicalize(path).and_then(|real| Dir::open(&real));
        Self::read(&opened.map_err(|err| failed("", &err))?)
    }

    /// Reads the skill folder open at `dir`.
    pub fn read(dir: &Dir) -> Result<Self, TreeError> {
        let mut entries = Vec::new();
        let mut data = BTreeMap::new();
        let mut bytes = 0u64;
        let mut todo = vec![String::new()];
        while let Some(folder) = todo.pop() {
            // Each folder opens from `dir` again, refusing a link on the way.
            let parts: Vec<String> = folder
                .split('/')
                .filter(|part| !part.is_empty())
                .map(str::to_owned)
                .collect();
            let here = dir.dir(&parts).map_err(|err| failed(&folder, &err))?;
            let mut names = Vec::new();
            for name in here.raw_names().map_err(|err| failed(&folder, &err))? {
                match name.into_string() {
                    Ok(name) => names.push(name),
                    Err(name) => {
                        let shown = String::from_utf8_lossy(name.as_bytes()).into_owned();
                        return Err(TreeError::BadPath(within(&folder, &shown)));
                    }
                }
            }
            names.sort_unstable();
            for name in names {
                if ignored(&name) {
                    continue;
                }
                let path = within(&folder, &name);
                if entries.len() >= MAX_ENTRIES {
                    return Err(TreeError::TooLarge);
                }
                match here.kind(&name).map_err(|err| failed(&path, &err))? {
                    // Gone since the folder was listed.
                    None => {}
                    Some(FileType::Symlink) => return Err(TreeError::Link(path)),
                    Some(FileType::Directory) => {
                        entries.push(entry(path.clone(), EntryKind::Dir, false, 0, None));
                        todo.push(path);
                    }
                    Some(FileType::RegularFile) => {
                        let file = here
                            .read_regular(&name)
                            .map_err(|err| failed(&path, &err))?;
                        let meta = file.metadata().map_err(|err| failed(&path, &err))?;
                        let size = meta.len();
                        bytes = bytes.saturating_add(size);
                        if bytes > MAX_BYTES {
                            return Err(TreeError::TooLarge);
                        }
                        let mut content = Vec::new();
                        file.take(size.saturating_add(1))
                            .read_to_end(&mut content)
                            .map_err(|err| failed(&path, &err))?;
                        if content.len() as u64 != size {
                            return Err(TreeError::Read {
                                path,
                                reason: "it changed while it was read".into(),
                            });
                        }
                        let executable = meta.permissions().mode() & 0o111 != 0;
                        let sha = sha256_hex(&content);
                        entries.push(entry(
                            path.clone(),
                            EntryKind::File,
                            executable,
                            size,
                            Some(sha),
                        ));
                        data.insert(path, content);
                    }
                    Some(_) => return Err(TreeError::Special(path)),
                }
            }
        }
        entries.sort_unstable_by(|a, b| a.path.cmp(&b.path));
        Ok(Self { entries, data })
    }

    /// The tree hash of what was read.
    pub fn tree(&self) -> Result<TreeHash, TreeError> {
        tree::hash(&self.entries)
    }

    /// The bytes of the file at `path` in the skill, when it has one.
    pub fn file(&self, path: &str) -> Option<&[u8]> {
        self.data.get(path).map(Vec::as_slice)
    }

    /// Writes what was read into the folder `to`, made if it's missing.
    /// Files are 0755 when executable and 0644 otherwise.
    pub fn write(&self, to: &Path) -> io::Result<()> {
        fs::create_dir_all(to)?;
        for entry in &self.entries {
            let target = to.join(&entry.path);
            match entry.kind {
                EntryKind::Dir => fs::create_dir_all(&target)?,
                EntryKind::File => {
                    fs::write(&target, self.file(&entry.path).unwrap_or_default())?;
                    let mode = if entry.executable { 0o755 } else { 0o644 };
                    fs::set_permissions(&target, fs::Permissions::from_mode(mode))?;
                }
                // A link is refused when it's read.
                EntryKind::Symlink { .. } => {}
            }
        }
        Ok(())
    }
}

fn within(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_owned()
    } else {
        format!("{folder}/{name}")
    }
}

fn failed(path: &str, err: &io::Error) -> TreeError {
    TreeError::Read {
        path: path.to_owned(),
        reason: err.to_string(),
    }
}

fn entry(
    path: String,
    kind: EntryKind,
    executable: bool,
    size: u64,
    sha256: Option<String>,
) -> ManifestEntry {
    ManifestEntry {
        path,
        kind,
        executable,
        size,
        sha256,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let hash = ring::digest::digest(&ring::digest::SHA256, bytes);
    hash.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;
    use std::sync::mpsc;
    use std::time::Duration;

    use maxhq_core::skills::disk::DiskFs;

    use super::*;
    use crate::testing::TempDir;

    fn skill(dir: &Path) {
        fs::create_dir_all(dir.join("scripts/inner")).unwrap();
        fs::write(dir.join("SKILL.md"), "---\nname: a\n---\nSteps.\n").unwrap();
        fs::write(dir.join("scripts/run.sh"), "echo hi\n").unwrap();
        fs::set_permissions(
            dir.join("scripts/run.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        fs::write(dir.join(".DS_Store"), "finder").unwrap();
    }

    #[test]
    fn it_reads_what_tree_read_reads() {
        let dir = TempDir::new();
        let folder = dir.path().join("a");
        skill(&folder);
        let held = Held::at(&folder).unwrap();
        assert_eq!(held.entries, tree::read(&DiskFs, &folder).unwrap());
        assert_eq!(held.tree().unwrap(), tree::of(&DiskFs, &folder).unwrap());
        assert_eq!(held.file("scripts/run.sh"), Some(&b"echo hi\n"[..]));
    }

    #[test]
    fn the_copy_is_what_was_read_whatever_the_folder_holds_by_then() {
        let dir = TempDir::new();
        let folder = dir.path().join("a");
        skill(&folder);
        let held = Held::at(&folder).unwrap();
        let read = held.tree().unwrap();

        // The file becomes a link to somewhere else after the read.
        let secret = dir.path().join("secret");
        fs::write(&secret, "not the skill's").unwrap();
        fs::remove_file(folder.join("SKILL.md")).unwrap();
        symlink(&secret, folder.join("SKILL.md")).unwrap();

        let to = dir.path().join("copy");
        held.write(&to).unwrap();
        assert_eq!(tree::of(&DiskFs, &to).unwrap(), read);
        let copied = fs::read_to_string(to.join("SKILL.md")).unwrap();
        assert_eq!(copied, "---\nname: a\n---\nSteps.\n");
        let mode = fs::metadata(to.join("scripts/run.sh"))
            .unwrap()
            .permissions();
        assert_eq!(mode.mode() & 0o777, 0o755);
    }

    #[test]
    fn a_link_anywhere_inside_is_refused() {
        let dir = TempDir::new();
        let outside = dir.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("file"), "outside").unwrap();
        for at in ["SKILL.md", "scripts", "scripts/inner/x"] {
            let folder = dir.path().join(format!("skill-{}", at.replace('/', "-")));
            skill(&folder);
            let _ = fs::remove_file(folder.join(at));
            let _ = fs::remove_dir_all(folder.join(at));
            symlink(&outside, folder.join(at)).unwrap();
            let refused = Held::at(&folder).unwrap_err();
            assert!(matches!(refused, TreeError::Link(_)), "{at}: {refused}");
        }
    }

    #[test]
    fn a_pipe_is_refused_without_waiting_for_a_writer() {
        let dir = TempDir::new();
        let folder = dir.path().join("a");
        skill(&folder);
        let made = std::process::Command::new("mkfifo")
            .arg(folder.join("notes"))
            .status();
        assert!(made.unwrap().success());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(Held::at(&folder).map(drop));
        });
        let read = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the read is still waiting on the pipe");
        assert!(matches!(read, Err(TreeError::Special(_))), "{read:?}");
    }

    #[test]
    fn a_folder_that_is_a_link_itself_is_read_through_it() {
        let dir = TempDir::new();
        let folder = dir.path().join("a");
        skill(&folder);
        let linked = dir.path().join("linked");
        symlink(&folder, &linked).unwrap();
        assert_eq!(
            Held::at(&linked).unwrap().tree().unwrap(),
            tree::of(&DiskFs, &folder).unwrap()
        );
    }

    #[test]
    fn too_much_is_refused() {
        let dir = TempDir::new();
        let folder = dir.path().join("a");
        skill(&folder);
        let big = fs::File::create(folder.join("big")).unwrap();
        big.set_len(MAX_BYTES + 1).unwrap();
        assert_eq!(Held::at(&folder).unwrap_err(), TreeError::TooLarge);
    }
}
