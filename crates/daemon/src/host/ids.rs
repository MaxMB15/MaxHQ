// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The Unix users and groups that host setup, Incus and the guest work
//! with. They run on Linux and macOS, so on Windows [`own`] says so, which
//! stops [`super::Env::this_device`] before anything reads a file's owner,
//! and Incus and the guest role never get as far as asking.

use std::fs::Metadata;
use std::io;
use std::path::Path;

/// Why host setup doesn't run on Windows.
#[cfg(not(unix))]
pub const NOT_HERE: &str = "Host setup runs on Linux and macOS, not on Windows";

/// This process's user and group IDs.
#[cfg(unix)]
pub fn own() -> Result<(u32, u32), String> {
    Ok((
        rustix::process::getuid().as_raw(),
        rustix::process::getgid().as_raw(),
    ))
}

/// Windows has no user and group IDs for host setup.
#[cfg(not(unix))]
pub fn own() -> Result<(u32, u32), String> {
    Err(NOT_HERE.to_owned())
}

/// Whether this process runs as root.
pub fn root() -> bool {
    cfg!(unix) && maxhq_os::proc::is_root()
}

/// The user ID that owns the file `meta` is of.
#[cfg(unix)]
pub fn owner(meta: &Metadata) -> u32 {
    std::os::unix::fs::MetadataExt::uid(meta)
}

/// Windows files have no owner ID, and nothing that asks runs there.
#[cfg(not(unix))]
pub fn owner(_meta: &Metadata) -> u32 {
    0
}

/// The group ID of the file `meta` is of.
#[cfg(unix)]
pub fn group(meta: &Metadata) -> u32 {
    std::os::unix::fs::MetadataExt::gid(meta)
}

/// Windows files have no group ID, and nothing that asks runs there.
#[cfg(not(unix))]
pub fn group(_meta: &Metadata) -> u32 {
    0
}

/// Gives `path` to the user `uid` and group `gid`.
#[cfg(unix)]
pub fn give(path: &Path, uid: u32, gid: u32) -> io::Result<()> {
    std::os::unix::fs::chown(path, Some(uid), Some(gid))
}

/// Windows has no user and group IDs to give a file to.
#[cfg(not(unix))]
pub fn give(_path: &Path, _uid: u32, _gid: u32) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, NOT_HERE))
}

/// Gives `path` itself to the user `uid` and group `gid`, never following
/// a link.
#[cfg(unix)]
pub fn give_link(path: &Path, uid: u32, gid: u32) -> io::Result<()> {
    std::os::unix::fs::lchown(path, Some(uid), Some(gid))
}

/// Windows has no user and group IDs to give a file to.
#[cfg(not(unix))]
pub fn give_link(_path: &Path, _uid: u32, _gid: u32) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, NOT_HERE))
}
