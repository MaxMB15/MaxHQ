// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The engine's [`Files`] on the real disk, read only.

use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use maxhq_core::fs::{open_regular, read_regular};
use maxhq_protocol::time::Timestamp;
use maxhq_workflow::{Entry, Files, Stat, MAX_FILE};

/// The disk, as the engine reads it. It opens files for reading only.
#[derive(Clone, Copy, Debug, Default)]
pub struct Disk;

impl Files for Disk {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        read_regular(path, MAX_FILE)
    }

    fn read_from(&self, path: &Path, offset: u64, max: usize) -> io::Result<Vec<u8>> {
        let mut file = open_regular(path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = Vec::with_capacity(max.min(64 * 1024));
        file.take(max as u64).read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    fn stat(&self, path: &Path) -> io::Result<Stat> {
        let meta = fs::metadata(path)?;
        Ok(Stat {
            size: meta.len(),
            modified: Timestamp::from(meta.modified()?),
        })
    }

    fn list(&self, dir: &Path) -> io::Result<Vec<Entry>> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            // A link counts as what it points at.
            let dir = match entry.file_type()? {
                kind if kind.is_symlink() => entry.path().is_dir(),
                kind => kind.is_dir(),
            };
            entries.push(Entry { name, dir });
        }
        Ok(entries)
    }

    fn alive(&self, pid: u32) -> bool {
        maxhq_os::proc::alive(pid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Links, devices and pipes are Unix's.
    #[cfg(unix)]
    use std::{os::unix::fs::symlink, path::PathBuf, time::Duration};

    #[cfg(unix)]
    use maxhq_workflow::Catalog;

    /// A project whose one workflow file is a link to `target`, and how many
    /// workflows a scan of it finds. The scan has to end soon.
    #[cfg(unix)]
    fn scan_with_link(dir: &Path, target: &Path) -> usize {
        let project: PathBuf = dir.join("project");
        fs::create_dir_all(project.join(".maxhq/workflows")).unwrap();
        symlink(target, project.join(".maxhq/workflows/a.yaml")).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(Catalog::default().scan(&Disk, &project).workflows.len());
        });
        rx.recv_timeout(Duration::from_secs(10))
            .expect("the scan is still waiting on the file")
    }

    /// What a scan finds in a project with no workflows of its own.
    #[cfg(unix)]
    fn built_in(dir: &Path) -> usize {
        Catalog::default().scan(&Disk, dir).workflows.len()
    }

    #[test]
    #[cfg(unix)]
    fn a_workflow_linked_to_zeroes_ends_the_scan_and_stays_small() {
        let dir = crate::testing::TempDir::new();
        assert_eq!(
            scan_with_link(dir.path(), Path::new("/dev/zero")),
            built_in(dir.path())
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_workflow_linked_to_a_terminal_ends_the_scan() {
        let dir = crate::testing::TempDir::new();
        assert_eq!(
            scan_with_link(dir.path(), Path::new("/dev/ptmx")),
            built_in(dir.path())
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_workflow_linked_to_a_pipe_ends_the_scan() {
        let dir = crate::testing::TempDir::new();
        let fifo = dir.path().join("fifo");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status();
        assert!(made.unwrap().success());
        assert_eq!(scan_with_link(dir.path(), &fifo), built_in(dir.path()));
    }

    #[test]
    #[cfg(unix)]
    fn a_workflow_linked_to_a_regular_file_keeps_working() {
        let dir = crate::testing::TempDir::new();
        let real = dir.path().join("real.yaml");
        fs::write(&real, "id: real\nname: Real\nnodes: {}\n").unwrap();
        let got = Disk.read(&real).unwrap();
        let link = dir.path().join("link.yaml");
        symlink(&real, &link).unwrap();
        assert_eq!(Disk.read(&link).unwrap(), got);
        assert_eq!(Disk.read_from(&link, 4, 4).unwrap(), b"real");
    }

    #[test]
    fn a_read_stops_just_past_the_cap() {
        let dir = crate::testing::TempDir::new();
        let large = dir.path().join("large");
        fs::File::create(&large)
            .unwrap()
            .set_len(MAX_FILE + 4096)
            .unwrap();
        assert_eq!(Disk.read(&large).unwrap().len() as u64, MAX_FILE + 1);
    }

    #[test]
    #[cfg(unix)]
    fn a_read_from_a_device_or_a_pipe_is_refused() {
        let dir = crate::testing::TempDir::new();
        let fifo = dir.path().join("fifo");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status();
        assert!(made.unwrap().success());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send((
                Disk.read_from(Path::new("/dev/zero"), 0, 8).is_err(),
                Disk.read_from(&fifo, 0, 8).is_err(),
            ));
        });
        let got = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("still waiting");
        assert_eq!(got, (true, true));
    }

    #[test]
    fn reads_lists_and_stats_a_real_folder() {
        let dir = crate::testing::TempDir::new();
        fs::create_dir(dir.path().join("run")).unwrap();
        fs::write(dir.path().join("run/task.yaml"), "id: one\n").unwrap();
        let disk = Disk;
        let file = dir.path().join("run/task.yaml");
        assert_eq!(disk.read(&file).unwrap(), b"id: one\n");
        assert_eq!(disk.read_from(&file, 4, 3).unwrap(), b"one");
        assert_eq!(disk.stat(&file).unwrap().size, 8);
        let entries = disk.list(dir.path()).unwrap();
        assert_eq!(
            entries,
            vec![Entry {
                name: "run".into(),
                dir: true
            }]
        );
        assert!(disk.alive(std::process::id()));
        assert!(!disk.alive(0));
    }
}
