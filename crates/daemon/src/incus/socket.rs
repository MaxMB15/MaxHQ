// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Connecting to a socket Incus serves on the host: its API socket, and
//! each guest's proxy device. Incus runs on Linux, so on Windows every
//! connect fails with `Unsupported`, and maxd reports no Incus there as it
//! does on a Mac without it.

use std::io;
use std::path::Path;

/// A connection to a socket Incus serves.
#[cfg(unix)]
pub type Stream = tokio::net::UnixStream;

/// Windows has no Incus, so no connection is ever made. The type only
/// keeps the callers compiled.
#[cfg(not(unix))]
pub type Stream = tokio::io::DuplexStream;

/// The half of a [`Stream`] that reads.
#[cfg(unix)]
pub type ReadHalf = tokio::net::unix::OwnedReadHalf;

/// The half of a [`Stream`] that reads.
#[cfg(not(unix))]
pub type ReadHalf = tokio::io::ReadHalf<Stream>;

/// The half of a [`Stream`] that writes.
#[cfg(unix)]
pub type WriteHalf = tokio::net::unix::OwnedWriteHalf;

/// The half of a [`Stream`] that writes.
#[cfg(not(unix))]
pub type WriteHalf = tokio::io::WriteHalf<Stream>;

/// Connects to the socket at `path`.
#[cfg(unix)]
pub async fn connect(path: &Path) -> io::Result<Stream> {
    Stream::connect(path).await
}

/// Windows has no Incus, so this always fails.
#[cfg(not(unix))]
pub async fn connect(_path: &Path) -> io::Result<Stream> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Incus runs on Linux, not on Windows",
    ))
}

/// Splits `stream` into halves that read and write on their own.
#[cfg(unix)]
pub fn split(stream: Stream) -> (ReadHalf, WriteHalf) {
    stream.into_split()
}

/// Splits `stream` into halves that read and write on their own.
#[cfg(not(unix))]
pub fn split(stream: Stream) -> (ReadHalf, WriteHalf) {
    tokio::io::split(stream)
}
