// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! [`SkillFs`] over the real disk, for `maxd`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::{path_text, Node, SkillFs};
use crate::fs::read_regular;
use crate::fs_ext;

/// The real disk.
#[derive(Clone, Copy, Debug, Default)]
pub struct DiskFs;

/// `result` with a missing path as `Ok(None)`, so only a failed read errs.
fn there<T>(result: io::Result<T>) -> io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

impl SkillFs for DiskFs {
    fn node(&self, path: &Path) -> io::Result<Option<Node>> {
        let Some(meta) = there(fs::symlink_metadata(path))? else {
            return Ok(None);
        };
        let kind = meta.file_type();
        Ok(Some(if kind.is_symlink() {
            // Written with `/`, as plans write a link's target.
            Node::Link {
                target: path_text(&fs_ext::read_link(path)?),
            }
        } else if kind.is_dir() {
            Node::Dir
        } else if kind.is_file() {
            Node::File {
                size: meta.len(),
                executable: maxhq_os::fsmode::is_executable(path, &meta),
            }
        } else {
            Node::Other
        }))
    }

    fn real(&self, path: &Path) -> io::Result<Option<PathBuf>> {
        there(fs_ext::canonicalize(path))
    }

    fn list(&self, path: &Path) -> io::Result<Vec<String>> {
        fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
            .collect()
    }

    fn read(&self, path: &Path, limit: u64) -> io::Result<Vec<u8>> {
        read_regular(path, limit)
    }
}

// Pipes, devices and links, which these tests make, are Unix's.
#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;
    use crate::skills::mem::MemFs;
    use crate::skills::tree;
    use crate::test_dir::TempDir;

    /// Runs `work` on its own thread and fails if it doesn't end soon.
    fn within<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(work());
        });
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .expect("it's still waiting on the file")
    }

    fn pipe(path: &Path) {
        let made = std::process::Command::new("mkfifo").arg(path).status();
        assert!(made.unwrap().success());
    }

    #[test]
    fn a_device_or_a_pipe_is_no_file() {
        let dir = TempDir::new("skill-disk-odd");
        let fifo = dir.0.join("fifo");
        pipe(&fifo);
        assert_eq!(
            DiskFs.node(Path::new("/dev/zero")).unwrap(),
            Some(Node::Other)
        );
        assert_eq!(DiskFs.node(&fifo).unwrap(), Some(Node::Other));
        assert!(within(|| DiskFs.read(Path::new("/dev/zero"), u64::MAX - 1)).is_err());
        assert!(within(move || DiskFs.read(&fifo, 10)).is_err());
    }

    #[test]
    fn a_pipe_in_a_skill_is_refused_and_doesnt_hold_up_the_read() {
        let dir = TempDir::new("skill-disk-pipe");
        let skill = dir.0.join("unslop");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "---\nname: unslop\n---\n").unwrap();
        pipe(&skill.join("fifo"));
        let read = within(move || tree::of(&DiskFs, &skill));
        assert!(matches!(read, Err(tree::TreeError::Special(path)) if path == "fifo"));
    }

    #[test]
    fn a_skill_file_linked_to_a_device_or_a_pipe_doesnt_hold_up_a_scan() {
        for target in ["/dev/zero", "/dev/ptmx", "fifo"] {
            let dir = TempDir::new("skill-disk-scan");
            let repo = dir.0.join("repo");
            let skill = repo.join(".claude/skills/x");
            fs::create_dir_all(&skill).unwrap();
            pipe(&skill.join("fifo"));
            symlink(target, skill.join("SKILL.md")).unwrap();
            let home = dir.0.join("home");
            fs::create_dir_all(&home).unwrap();
            within(move || {
                let input = crate::skills::scan::ScanInput::new(&repo, &home);
                drop(crate::skills::scan::scan(&DiskFs, &input));
            });
        }
    }

    #[test]
    fn the_disk_reads_like_the_fake() {
        let dir = TempDir::new("skill-disk");
        let skill = dir.0.join("unslop");
        fs::create_dir_all(skill.join("scripts")).unwrap();
        fs::write(skill.join("SKILL.md"), "---\nname: unslop\n---\n").unwrap();
        let run = skill.join("scripts").join("run.sh");
        fs::write(&run, "#!/bin/sh\n").unwrap();
        maxhq_os::fsmode::set_mode(&run, 0o755).unwrap();
        symlink("unslop", dir.0.join("link")).unwrap();
        symlink("gone", dir.0.join("dangling")).unwrap();

        let mut mem = MemFs::new();
        mem.file("/s/unslop/SKILL.md", "---\nname: unslop\n---\n")
            .exec("/s/unslop/scripts/run.sh", "#!/bin/sh\n");
        assert_eq!(
            tree::of(&DiskFs, &skill).unwrap(),
            tree::of(&mem, Path::new("/s/unslop")).unwrap()
        );

        assert_eq!(
            DiskFs.node(&run).unwrap(),
            Some(Node::File {
                size: 10,
                executable: true
            })
        );
        assert_eq!(
            DiskFs.node(&dir.0.join("link")).unwrap(),
            Some(Node::Link {
                target: "unslop".into()
            })
        );
        assert_eq!(DiskFs.node(&dir.0.join("missing")).unwrap(), None);
        assert_eq!(DiskFs.real(&dir.0.join("dangling")).unwrap(), None);
        assert_eq!(
            DiskFs.real(&dir.0.join("link")).unwrap(),
            Some(fs::canonicalize(&skill).unwrap())
        );
        let mut names = DiskFs.list(&dir.0.join("link")).unwrap();
        names.sort();
        assert_eq!(names, ["SKILL.md", "scripts"]);
        assert_eq!(DiskFs.read(&run, 3).unwrap(), b"#!/b");
        assert!(DiskFs.list(&dir.0.join("missing")).is_err());
    }
}
