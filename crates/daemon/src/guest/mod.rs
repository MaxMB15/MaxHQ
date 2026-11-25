// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! `maxd guest`, the role that runs as root inside a container workspace
//! under `maxhq-guest.service`. It listens on
//! [`HOST_SOCKET`](maxhq_protocol::guest::HOST_SOCKET) for one host link
//! at a time, spawns holders as user `agent`, and serves the token and
//! credential helpers on
//! [`AGENT_SOCKET`](maxhq_protocol::guest::AGENT_SOCKET).
//!
//! The helpers are the other `maxd guest` subcommands. Each one asks the
//! guest over the agent socket, and the guest asks the host.
//!
//! Transfers go to [`files::Files`], and ports and tunnels to the
//! container's port watch in `ports.rs`, through the [`Files`] and
//! [`Tunnels`] traits the link calls. SSH sessions come as tunnels, and go
//! to the server in `ssh.rs`, whose host key the guest makes at first
//! start.

pub mod files;
// The guest role serves as root inside a Linux container or a macOS VM,
// over Unix sockets, ptys and user switches, so it builds on Unix alone.
// On Windows `maxd guest` says it can't run there, and only the parts the
// host side shares compile: the files side, which the transfer tests
// drive, and the checkout's git.
#[cfg(unix)]
mod accounts;
#[cfg(unix)]
mod agent;
#[cfg(unix)]
mod catalog;
#[cfg(unix)]
mod helper;
#[cfg(unix)]
mod holds;
#[cfg(unix)]
pub(crate) mod lease;
#[cfg(unix)]
mod link;
#[cfg(unix)]
mod login_env;
#[cfg(unix)]
mod ports;
#[cfg(unix)]
mod role;
#[cfg(unix)]
mod ssh;
#[cfg(unix)]
mod tunnel;
#[cfg(unix)]
mod unsaved;
#[cfg(target_os = "macos")]
pub mod vm;
pub(crate) mod worktree;

#[cfg(all(test, unix))]
mod tests;

#[cfg(unix)]
pub(crate) use crate::guestlink::Outbox;
pub use role::{codex, git_credential, github_token, run, token_codex};
#[cfg(unix)]
pub(crate) use role::{Files, Tunnels};

/// `maxd guest` and its helpers on Windows, where the guest role can't run.
#[cfg(not(unix))]
mod role {
    use std::process::ExitCode;

    /// Says the guest role can't run here.
    fn not_here() -> ExitCode {
        eprintln!("maxd guest runs inside a Linux container, not on Windows");
        ExitCode::FAILURE
    }

    pub fn run(_test_host: bool) -> ExitCode {
        not_here()
    }

    pub fn token_codex() -> ExitCode {
        not_here()
    }

    pub fn codex(_command: &[&str]) -> ExitCode {
        not_here()
    }

    pub fn git_credential(_action: &str) -> ExitCode {
        not_here()
    }

    pub fn github_token() -> ExitCode {
        not_here()
    }
}
