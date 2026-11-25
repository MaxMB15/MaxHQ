// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::fs;
use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::Path;

use super::{Found, Peer};

pub type Listener = tokio::net::UnixListener;
pub type Stream = tokio::net::UnixStream;
pub type ReadHalf = tokio::net::unix::OwnedReadHalf;
pub type WriteHalf = tokio::net::unix::OwnedWriteHalf;

pub fn bind(path: &Path) -> io::Result<Listener> {
    let std = std::os::unix::net::UnixListener::bind(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    std.set_nonblocking(true)?;
    Listener::from_std(std)
}

pub async fn accept(listener: &Listener) -> io::Result<Stream> {
    listener.accept().await.map(|(stream, _)| stream)
}

pub async fn connect(path: &Path) -> io::Result<Stream> {
    Stream::connect(path).await
}

pub fn peer(stream: &Stream) -> io::Result<Peer> {
    let cred = stream.peer_cred()?;
    Ok(Peer {
        pid: cred.pid().and_then(|pid| u32::try_from(pid).ok()),
        same_user: cred.uid() == rustix::process::geteuid().as_raw(),
        elevated: cred.uid() == 0,
    })
}

pub fn into_split(stream: Stream) -> (ReadHalf, WriteHalf) {
    stream.into_split()
}

pub fn pair() -> io::Result<(Stream, Stream)> {
    Stream::pair()
}

pub fn probe(path: &Path) -> io::Result<Found> {
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.file_type().is_socket() => Ok(Found::Other),
        Ok(_) => {
            if std::os::unix::net::UnixStream::connect(path).is_ok() {
                Ok(Found::Listening)
            } else {
                Ok(Found::Stale)
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Found::Nothing),
        Err(err) => Err(err),
    }
}

pub fn answers(path: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

/// The socket, for calls that read its peer's credentials, such as the
/// audit token on macOS.
impl AsFd for super::LocalStream {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

/// The read half's socket, for the same calls.
impl AsFd for super::ReadHalf {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_ref().as_fd()
    }
}
