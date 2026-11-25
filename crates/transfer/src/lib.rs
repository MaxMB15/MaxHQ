// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Files and folders over a MaxHQ link.
//!
//! An upload or download is one file or folder. The sender walks it into a
//! manifest of entries, each with a path, kind, size, executable bit and
//! SHA-256, and the entries' bytes travel as one stream laid end to end in
//! manifest order, so a single offset says how far a transfer got.
//!
//! The receiver checks every name before anything touches its disk, writes
//! each file to a partial copy, checks its hash when the last byte lands
//! and only then renames it into place. It grants credit as it writes, and
//! everything below the credit's `committed` offset is on disk and fsynced,
//! so a transfer picks up from there after a dropped link.
//!
//! The crate is plain blocking code. The callers in `maxd` and the app run
//! it on blocking threads.

#![warn(missing_docs)]

// The disk side works through Unix calls, so it builds on Unix alone
// until it has a Windows side. Names and manifests build everywhere.
#[cfg(unix)]
pub mod beneath;
pub mod manifest;
pub mod name;
#[cfg(unix)]
pub mod receive;
#[cfg(unix)]
pub mod remove;
#[cfg(unix)]
pub mod send;

#[cfg(all(test, unix))]
mod testing;

use std::io;

use maxhq_protocol::message::ErrorCode;

#[cfg(unix)]
pub use receive::{Grant, Receiver};
#[cfg(unix)]
pub use remove::remove_tree;
#[cfg(unix)]
pub use send::{walk, Outgoing, Source, Walk};

/// A gibibyte.
pub const GIB: u64 = 1024 * 1024 * 1024;

/// An attachment over this gets a warning in the composer, with its
/// largest folders, since `node_modules` is the likely accident.
pub const WARN_BYTES: u64 = 100 * 1000 * 1000;

/// The most one upload may hold, in bytes.
pub const MAX_BYTES: u64 = 2 * GIB;

/// The most files, folders and links one upload may hold.
pub const MAX_ENTRIES: usize = 20_000;

/// How much attachments may take on one device, by default.
pub const DEVICE_QUOTA: u64 = 5 * GIB;

/// A receiver fsyncs after this many bytes, or [`SYNC_EVERY`], whichever
/// comes first.
pub const SYNC_BYTES: u64 = 8 * 1024 * 1024;

/// A receiver fsyncs at least this often while bytes arrive.
pub const SYNC_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// Why a transfer stopped, in the shape `TransferFailed` sends.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct Failure {
    /// What went wrong, for code to act on.
    pub code: ErrorCode,
    /// The manifest entry it stopped at, if it was one.
    pub entry: Option<u32>,
    /// What went wrong, for a person to read.
    pub message: String,
}

impl Failure {
    /// A failure with this code.
    pub fn new(code: ErrorCode, entry: Option<u32>, message: impl Into<String>) -> Self {
        Self {
            code,
            entry,
            message: message.into(),
        }
    }

    /// A failed disk operation, with a code that says whether the disk was
    /// full, a path broke a rule or something else went wrong.
    #[cfg(unix)]
    pub fn io(err: &io::Error, entry: Option<u32>) -> Self {
        use rustix::io::Errno;
        let code = match err.raw_os_error().map(Errno::from_raw_os_error) {
            Some(Errno::NOSPC | Errno::DQUOT) => ErrorCode::NoSpace,
            Some(Errno::LOOP | Errno::XDEV | Errno::NOTDIR | Errno::EXIST) => ErrorCode::BadPath,
            _ => ErrorCode::Internal,
        };
        Self::new(code, entry, err.to_string())
    }
}

/// How much room a receiver leaves on its disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Room {
    /// Bytes that stay free whatever the disk's size.
    pub keep_bytes: u64,
    /// The share of the disk that stays free, in percent.
    pub keep_percent: u64,
    /// The most that share keeps free, however big the disk.
    pub keep_at_most: u64,
}

impl Room {
    /// 5% of the disk, but at least 1 GiB and at most 10 GiB.
    pub const DEFAULT: Room = Room {
        keep_bytes: GIB,
        keep_percent: 5,
        keep_at_most: 10 * GIB,
    };

    /// The bytes that stay free on a disk of `size` bytes.
    fn keep(&self, size: u64) -> u64 {
        (size / 100 * self.keep_percent.min(100))
            .min(self.keep_at_most)
            .max(self.keep_bytes)
    }

    /// Refuses `incoming` more bytes if they'd leave less than this free on
    /// the disk under `dir`.
    #[cfg(unix)]
    pub fn check(&self, dir: &beneath::Dir, incoming: u64) -> Result<(), Failure> {
        let (free, size) = dir.space().map_err(|err| Failure::io(&err, None))?;
        self.check_space(free, size, incoming)
    }

    /// Refuses `incoming` more bytes on a disk of `size` bytes with `free`
    /// left.
    pub fn check_space(&self, free: u64, size: u64, incoming: u64) -> Result<(), Failure> {
        let keep = self.keep(size);
        if free.saturating_sub(incoming) < keep || free < incoming {
            return Err(Failure::new(
                ErrorCode::NoSpace,
                None,
                format!(
                    "the device has {} free and MaxHQ keeps {} free, so {} more won't fit",
                    human(free),
                    human(keep),
                    human(incoming)
                ),
            ));
        }
        Ok(())
    }
}

impl Default for Room {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Refuses an upload of `incoming` bytes when attachments already take
/// `used` of a `quota`.
pub fn check_quota(used: u64, incoming: u64, quota: u64) -> Result<(), Failure> {
    if used.saturating_add(incoming) > quota {
        return Err(Failure::new(
            ErrorCode::QuotaExceeded,
            None,
            format!(
                "attachments would take {} of the {} this device allows",
                human(used.saturating_add(incoming)),
                human(quota)
            ),
        ));
    }
    Ok(())
}

/// Bytes in every regular file under `path`, never following a link. What
/// can't be read below `path` is left out of the count, so an unreadable
/// folder can't stop a quota working.
pub fn folder_bytes(path: &std::path::Path) -> io::Result<u64> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_file() {
        return Ok(meta.len());
    }
    if !meta.is_dir() {
        return Ok(0);
    }
    let mut total = 0u64;
    let mut todo = vec![path.to_path_buf()];
    while let Some(folder) = todo.pop() {
        let items = match std::fs::read_dir(&folder) {
            Ok(items) => items,
            Err(err) if folder == path => return Err(err),
            Err(_) => continue,
        };
        for item in items.flatten() {
            let Ok(meta) = item.metadata() else {
                continue;
            };
            if meta.is_file() {
                total = total.saturating_add(meta.len());
            } else if meta.is_dir() {
                todo.push(item.path());
            }
        }
    }
    Ok(total)
}

/// A size for a person, such as `1.5 GiB`.
pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::beneath::Dir;
    use crate::testing::TempDir;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_folder_count_passes_over_what_it_cant_read() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("kept"), [0; 10]).unwrap();
        let shut = dir.path().join("shut");
        std::fs::create_dir(&shut).unwrap();
        std::fs::write(shut.join("hidden"), [0; 5]).unwrap();
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o000)).unwrap();
        let counted = folder_bytes(dir.path());
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(counted.unwrap(), 10);
    }

    #[test]
    fn a_folder_count_gets_through_a_tree_past_what_a_path_can_name() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("kept"), [0; 10]).unwrap();
        let mut at = Dir::open(dir.path()).unwrap();
        for _ in 0..600 {
            at.make_dir("d", 0o755).unwrap();
            at = at.dir(&["d".to_owned()]).unwrap();
        }
        let counted = folder_bytes(dir.path());
        remove_tree(dir.path().join("d")).unwrap();
        assert_eq!(counted.unwrap(), 10);
    }

    #[test]
    fn a_folder_count_says_when_the_folder_itself_is_gone() {
        let dir = TempDir::new();
        let missing = folder_bytes(&dir.path().join("missing")).unwrap_err();
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);
    }

    /// A 2 TB disk, as its maker counts.
    const TWO_TB: u64 = 2_000_000_000_000;

    #[test]
    fn the_reserve_is_five_percent_between_one_and_ten_gibibytes() {
        let room = Room::DEFAULT;
        // On a small disk the floor is what counts.
        assert_eq!(room.keep(4 * GIB), GIB);
        // A mid-size disk keeps 5%.
        assert_eq!(room.keep(100 * GIB), 5 * GIB);
        // A big one keeps no more than the ceiling.
        assert_eq!(room.keep(TWO_TB), 10 * GIB);
    }

    #[test]
    fn space_runs_short_below_the_reserve() {
        let room = Room::DEFAULT;
        let disk = 100 * GIB;
        assert!(room.check_space(10 * GIB, disk, 4 * GIB).is_ok());
        // 5% of 100 GiB is 5 GiB, more than the 1 GiB floor.
        let short = room.check_space(10 * GIB, disk, 6 * GIB).unwrap_err();
        assert_eq!(short.code, ErrorCode::NoSpace);
        assert_eq!(
            short.message,
            "the device has 10.0 GiB free and MaxHQ keeps 5.0 GiB free, so 6.0 GiB more won't fit"
        );
        let small = 4 * GIB;
        assert!(room.check_space(2 * GIB, small, GIB / 2).is_ok());
        assert!(room.check_space(2 * GIB, small, GIB + 1).is_err());
        assert!(room.check_space(GIB, small, 2 * GIB).is_err());
        // A small picture fits on a 2 TB disk with 80 GiB free, and runs
        // short only once less than 10 GiB would be left.
        let picture = 60 * 1024;
        assert!(room.check_space(80 * GIB, TWO_TB, picture).is_ok());
        assert!(room
            .check_space(10 * GIB + picture, TWO_TB, picture)
            .is_ok());
        assert!(room.check_space(10 * GIB, TWO_TB, picture).is_err());
    }

    #[test]
    fn the_quota_counts_what_is_there_and_what_comes() {
        assert!(check_quota(GIB, GIB, DEVICE_QUOTA).is_ok());
        assert!(check_quota(4 * GIB, GIB, DEVICE_QUOTA).is_ok());
        let over = check_quota(4 * GIB, GIB + 1, DEVICE_QUOTA).unwrap_err();
        assert_eq!(over.code, ErrorCode::QuotaExceeded);
    }

    #[test]
    fn disk_errors_map_to_codes() {
        let full = io::Error::from_raw_os_error(rustix::io::Errno::NOSPC.raw_os_error());
        assert_eq!(Failure::io(&full, Some(2)).code, ErrorCode::NoSpace);
        let link = io::Error::from_raw_os_error(rustix::io::Errno::LOOP.raw_os_error());
        assert_eq!(Failure::io(&link, None).code, ErrorCode::BadPath);
        let other = io::Error::other("odd");
        assert_eq!(Failure::io(&other, None).code, ErrorCode::Internal);
    }

    #[test]
    fn sizes_read_as_people_say_them() {
        assert_eq!(human(12), "12 bytes");
        assert_eq!(human(1536), "1.5 KiB");
        assert_eq!(human(5 * GIB), "5.0 GiB");
    }
}
