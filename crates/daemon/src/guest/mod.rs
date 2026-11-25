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

// Outside the tests, Windows uses only the checkout's git from these.
#[cfg_attr(not(unix), allow(dead_code))]
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
mod vm;
#[cfg(unix)]
mod wire;
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) mod worktree;

#[cfg(all(test, unix))]
mod tests;

#[cfg(unix)]
pub(crate) use crate::guestlink::wire::Outbox;
#[cfg(unix)]
pub(crate) use role::{main, Files, Tunnels};

/// `maxd guest` on Windows, where the guest role can't run, so every
/// subcommand says so.
#[cfg(not(unix))]
pub fn main(_args: &[&str]) -> Option<std::process::ExitCode> {
    eprintln!("maxd guest runs inside a Linux container, not on Windows");
    Some(std::process::ExitCode::FAILURE)
}
