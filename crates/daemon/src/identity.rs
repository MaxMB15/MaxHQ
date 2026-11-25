// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Who this device is: an ID that lasts across restarts, the boot it's on
//! and the name the app shows for it.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use maxhq_core::paths::BUILD_LEN;
use maxhq_protocol::ids::{BootId, DeviceId};
use maxhq_protocol::model::Device;
use thiserror::Error;

/// The file in the data dir that holds the device ID.
pub const DEVICE_ID_FILE: &str = "device-id";

/// What a device shows when its boot ID or name can't be read.
const UNKNOWN: &str = "unknown";

/// Why the device ID couldn't be read or made.
#[derive(Debug, Error)]
pub enum IdentityError {
    /// The file couldn't be read or written.
    #[error("couldn't read or write {}: {source}", path.display())]
    Io {
        /// The device ID file.
        path: PathBuf,
        /// What went wrong.
        source: io::Error,
    },
    /// The file holds something other than an ID.
    #[error(
        "{} doesn't hold a device ID. Delete it to make a new one, which the app shows as a new device",
        path.display()
    )]
    Corrupt {
        /// The device ID file.
        path: PathBuf,
    },
}

/// Describes this device, keeping its ID in `data_dir`.
pub fn device(data_dir: &Path) -> Result<Device, IdentityError> {
    Ok(Device {
        id: device_id(data_dir)?,
        name: device_name(),
        os: std::env::consts::OS.into(),
        boot_id: boot_id(),
        maxd_version: env!("CARGO_PKG_VERSION").into(),
        winding_down_until: None,
        workspace_host: None,
        build: build().map(str::to_owned),
        target: Some(TARGET.to_owned()),
        wsl: None,
    })
}

/// The Rust target this `maxd` was built for, such as
/// `x86_64-unknown-linux-musl`.
pub const TARGET: &str = env!("MAXD_TARGET");

/// This build: the first 16 hex characters of the sha256 of the running
/// executable. Two binaries from the same version differ here, so the app
/// can tell when a device runs an old one. It's worked out once, the first
/// time it's asked for, and is none when the executable can't be read.
pub fn build() -> Option<&'static str> {
    static BUILD: OnceLock<Option<String>> = OnceLock::new();
    BUILD
        .get_or_init(|| {
            let exe = std::env::current_exe().ok()?;
            build_of(&exe).ok()
        })
        .as_deref()
}

/// The build of the binary at `path`, as [`build`] works it out.
pub fn build_of(path: &Path) -> io::Result<String> {
    let mut sha = sha256_of(path)?;
    sha.truncate(BUILD_LEN);
    Ok(sha)
}

/// The full sha256 of the file at `path`, in lowercase hex.
pub fn sha256_of(path: &Path) -> io::Result<String> {
    use std::io::Read;
    let mut file = fs::File::open(path)?;
    let mut context = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buf = vec![0; 64 * 1024];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        context.update(&buf[..read]);
    }
    Ok(hex(context.finish().as_ref()))
}

/// `bytes` in lowercase hex.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// The device ID kept in `data_dir`, made the first time it's asked for.
/// The folder must exist.
pub fn device_id(data_dir: &Path) -> Result<DeviceId, IdentityError> {
    let path = data_dir.join(DEVICE_ID_FILE);
    if let Some(id) = read_id(&path)? {
        return Ok(id);
    }
    let id = DeviceId::generate();
    // Write the whole ID to a file of its own, then link it into place. The
    // link fails if the file appeared meanwhile, so the first ID written
    // wins and nobody reads half of one.
    let draft = data_dir.join(format!("{DEVICE_ID_FILE}.{}", std::process::id()));
    let io = |source| IdentityError::Io {
        path: path.clone(),
        source,
    };
    write_file(&draft, &format!("{id}\n")).map_err(io)?;
    let linked = fs::hard_link(&draft, &path);
    let _ = fs::remove_file(&draft);
    match linked {
        Ok(()) => Ok(id),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            read_id(&path)?.ok_or_else(|| IdentityError::Corrupt { path: path.clone() })
        }
        Err(err) => Err(io(err)),
    }
}

/// Reads the ID at `path`, or none when there's no file yet.
fn read_id(path: &Path) -> Result<Option<DeviceId>, IdentityError> {
    match fs::read_to_string(path) {
        Ok(text) => text
            .trim()
            .parse()
            .map(Some)
            .map_err(|_| IdentityError::Corrupt { path: path.into() }),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(IdentityError::Io {
            path: path.into(),
            source,
        }),
    }
}

/// Writes `text` to a new file and waits until it's on disk.
fn write_file(path: &Path, text: &str) -> io::Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()
}

/// The boot this device is on. macOS reads `kern.bootsessionuuid`, Linux
/// reads the kernel's boot ID and Windows its count of boots. When none
/// works the value is "unknown", and the app can't tell when the device
/// restarts.
pub fn boot_id() -> BootId {
    match maxhq_os::boot::id().as_deref().and_then(clean) {
        Some(id) => BootId(id),
        None => {
            tracing::warn!(
                "couldn't read the boot ID, so clients can't tell when this device restarts"
            );
            BootId(UNKNOWN.into())
        }
    }
}

/// The name the app shows: the computer name on macOS, which is the one
/// you set in System Settings, and the host name elsewhere.
pub fn device_name() -> String {
    let computer = if cfg!(target_os = "macos") {
        output("/usr/sbin/scutil", &["--get", "ComputerName"])
    } else {
        None
    };
    computer
        .as_deref()
        .and_then(clean)
        .or_else(host_name)
        .unwrap_or_else(|| UNKNOWN.into())
}

/// The host name, from the kernel or the `hostname` command.
fn host_name() -> Option<String> {
    fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .or_else(|| output("hostname", &[]))
        .as_deref()
        .and_then(clean)
}

/// Trims what a command printed, or none when it printed nothing.
fn clean(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// What a command printed when it succeeded.
fn output(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    #[test]
    fn a_build_is_the_start_of_the_binarys_sha256() {
        let dir = TempDir::new();
        let path = dir.path().join("maxd");
        fs::write(&path, "abc").unwrap();
        assert_eq!(
            sha256_of(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(build_of(&path).unwrap(), "ba7816bf8f01cfea");
        assert_eq!(build().map(str::len), Some(BUILD_LEN));
    }

    #[test]
    fn the_device_id_is_made_once_and_kept() {
        let dir = TempDir::new();
        let first = device_id(dir.path()).unwrap();
        assert_eq!(device_id(dir.path()).unwrap(), first);
        let stored = fs::read_to_string(dir.path().join(DEVICE_ID_FILE)).unwrap();
        assert_eq!(stored.trim(), first.to_string());
        let leftovers = fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(leftovers, 1, "only the ID file is left behind");
    }

    #[test]
    fn a_damaged_id_file_is_reported_not_replaced() {
        let dir = TempDir::new();
        let path = dir.path().join(DEVICE_ID_FILE);
        fs::write(&path, "not an id").unwrap();
        assert!(matches!(
            device_id(dir.path()),
            Err(IdentityError::Corrupt { .. })
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "not an id");
    }

    #[test]
    fn this_device_has_a_boot_and_a_name() {
        let boot = boot_id();
        assert!(!boot.0.is_empty());
        assert_ne!(boot.0, UNKNOWN, "the boot ID is readable here");
        assert_eq!(boot_id(), boot, "the boot ID stays put");
        assert!(!device_name().is_empty());
    }

    #[test]
    fn command_output_is_trimmed() {
        assert_eq!(
            clean(" Dev's MacBook Pro\n").as_deref(),
            Some("Dev's MacBook Pro")
        );
        assert_eq!(clean(" \n"), None);
    }
}
