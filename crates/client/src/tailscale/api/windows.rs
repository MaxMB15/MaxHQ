// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The Windows service, on a named pipe.

use std::io;
use std::time::Duration;

use tokio::net::windows::named_pipe::NamedPipeClient;

/// The pipe `tailscaled` serves the LocalAPI on.
const PIPE: &str = r"\\.\pipe\ProtectedPrefix\Administrators\Tailscale\tailscaled";

/// How long to wait for a free instance of the service's pipe.
const PIPE_BUSY_WAIT: Duration = Duration::from_secs(2);

/// The CLI where Tailscale's installer puts it.
pub(super) const CLIS: &[&str] = &[r"C:\Program Files\Tailscale\tailscale.exe"];

/// The service's named pipe, which checks who connects itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Local(&'static str);

/// The install folder and the service's state folder, then the CLI.
pub(super) fn install_markers() -> impl Iterator<Item = &'static str> {
    [r"C:\Program Files\Tailscale", r"C:\ProgramData\Tailscale"]
        .into_iter()
        .chain(CLIS.iter().copied())
}

/// The service's pipe, once Tailscale is installed. Looking for a pipe
/// opens it, which takes one of the service's instances, so it's only
/// opened to call it, and a call that finds nothing falls back to the CLI.
pub(super) async fn local_endpoint() -> Option<Local> {
    super::installed().await.then_some(Local(PIPE))
}

pub(super) async fn connect(pipe: &Local) -> io::Result<NamedPipeClient> {
    maxhq_os::ipc::open_pipe(pipe.0, PIPE_BUSY_WAIT).await
}
