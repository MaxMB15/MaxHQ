// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Plain `tailscaled` on macOS and Linux, on a Unix socket.

use std::io;
use std::path::PathBuf;

use tokio::net::UnixStream;

use super::MAC_APP;

/// `tailscaled` sockets: plain `tailscaled` on macOS, then Linux.
const SOCKETS: &[&str] = &[
    "/var/run/tailscaled.socket",
    "/var/run/tailscale/tailscaled.sock",
    "/run/tailscale/tailscaled.sock",
];

/// The CLI by absolute path, since an app opened from Finder doesn't get
/// `/usr/local/bin` on its `PATH`.
pub(super) const CLIS: &[&str] = &[
    "/usr/local/bin/tailscale",
    "/opt/homebrew/bin/tailscale",
    "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
    "/usr/bin/tailscale",
];

/// A Unix socket, which needs no auth.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Local(PathBuf);

/// Paths whose presence means Tailscale is installed, even when it isn't
/// running: the apps and state folders, then the sockets and the CLIs.
pub(super) fn install_markers() -> impl Iterator<Item = &'static str> {
    [MAC_APP, "/Library/Tailscale", "/var/lib/tailscale"]
        .into_iter()
        .chain(SOCKETS.iter().copied())
        .chain(CLIS.iter().copied())
}

/// `tailscaled`'s own socket.
pub(super) async fn local_endpoint() -> Option<Local> {
    for path in SOCKETS {
        if tokio::fs::metadata(path).await.is_ok() {
            return Some(Local(PathBuf::from(path)));
        }
    }
    None
}

pub(super) async fn connect(socket: &Local) -> io::Result<UnixStream> {
    UnixStream::connect(&socket.0).await
}
