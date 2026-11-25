// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The system's SSH, with your own `ssh_config`: one login per SSH host,
//! which later connections to it reuse. On Unix that's a ControlMaster
//! through openssh, and on Windows, where openssh can't run, a russh login
//! that reads its options from `ssh -G`. Only the `system-ssh` feature
//! has it.

use std::collections::HashSet;
use std::io;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use futures::future::BoxFuture;
use maxhq_protocol::ids::DeviceId;
use maxhq_protocol::link::Route;

use super::local::dial_local;
use super::{shorten, Conn, DialError, Transport, PROXY_COMMAND, WINDOWS_PROXY_COMMAND};
use crate::pinned::Pins;
use crate::setup::Output;
use crate::ssh_config::{self, is_safe_destination, SshConfig};

#[cfg(unix)]
mod mux;
#[cfg_attr(not(windows), allow(dead_code))]
#[cfg(any(windows, feature = "russh"))]
mod sessions;

#[cfg(unix)]
use mux::Masters;
#[cfg(windows)]
use sessions::Masters;

/// How long a name lookup gets.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);

/// How long a command run over a login may take.
const RUN_TIMEOUT: Duration = Duration::from_secs(60);

/// The real thing.
pub(crate) struct System {
    /// The local `maxd`'s socket, if this machine has a place for it.
    socket: Option<PathBuf>,
    /// One login per SSH host.
    masters: Masters,
    /// The hosts whose shell has no `sh`.
    windows: Shells,
}

impl System {
    /// Reaches the local `maxd` at `socket` and keeps SSH masters in
    /// `control_dir`, or in openssh's default folder when it's none or too
    /// long for a socket path. Windows keeps no masters, so it has no use
    /// for the folder.
    pub fn new(socket: Option<PathBuf>, control_dir: Option<PathBuf>) -> Self {
        Self {
            socket,
            masters: Masters::new(control_dir),
            windows: Shells::default(),
        }
    }
}

impl Transport for System {
    fn resolve(&self, destination: &str) -> BoxFuture<'static, io::Result<SshConfig>> {
        let destination = destination.to_owned();
        Box::pin(async move { ssh_config::resolve(&destination).await })
    }

    fn lookup(&self, host: &str) -> BoxFuture<'static, Vec<IpAddr>> {
        let host = host.to_owned();
        Box::pin(async move {
            let lookup = tokio::net::lookup_host((host.as_str(), 22));
            match tokio::time::timeout(LOOKUP_TIMEOUT, lookup).await {
                Ok(Ok(addresses)) => addresses.map(|address| address.ip()).collect(),
                _ => Vec::new(),
            }
        })
    }

    fn dial<'a>(
        &'a self,
        route: &'a Route,
        config: Option<&'a SshConfig>,
        _device: Option<DeviceId>,
    ) -> BoxFuture<'a, Result<Conn, DialError>> {
        Box::pin(async move {
            match route {
                Route::Local => {
                    let Some(socket) = self.socket.as_deref() else {
                        return Err(DialError::refused("this machine has no place for maxd"));
                    };
                    dial_local(socket).await
                }
                Route::Ssh { host } => {
                    let command = self.windows.command(host);
                    self.masters.dial(host, config, None, command).await
                }
            }
        })
    }

    fn forget<'a>(&'a self, host: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(self.masters.forget(host))
    }

    fn shell_is_windows(&self, host: &str) {
        self.windows.note(host);
    }

    fn run<'a>(&'a self, host: &'a str, command: &'a str) -> BoxFuture<'a, io::Result<Output>> {
        Box::pin(async move {
            if !is_safe_destination(host) {
                return Err(io::Error::other("that isn't an SSH host name"));
            }
            let config = ssh_config::resolve(host).await.ok();
            self.masters.run(host, config.as_ref(), command).await
        })
    }
}

/// Pinned SSH only, for `maxd`'s links to other devices. A route's host
/// names its pin, and nothing is looked up.
pub(crate) struct Pinned {
    pins: Pins,
    masters: Masters,
    windows: Shells,
}

impl Pinned {
    /// Dials the pins in `pins`, keeping SSH masters in `control_dir`.
    pub fn new(pins: Pins, control_dir: Option<PathBuf>) -> Self {
        Self {
            pins,
            masters: Masters::new(control_dir),
            windows: Shells::default(),
        }
    }
}

impl Transport for Pinned {
    fn resolve(&self, _destination: &str) -> BoxFuture<'static, io::Result<SshConfig>> {
        Box::pin(async { Err(io::Error::other("a pinned route reads no ssh config")) })
    }

    fn lookup(&self, _host: &str) -> BoxFuture<'static, Vec<IpAddr>> {
        Box::pin(async { Vec::new() })
    }

    fn dial<'a>(
        &'a self,
        route: &'a Route,
        _config: Option<&'a SshConfig>,
        _device: Option<DeviceId>,
    ) -> BoxFuture<'a, Result<Conn, DialError>> {
        Box::pin(async move {
            let Route::Ssh { host } = route else {
                return Err(DialError::refused("a peer link goes over SSH"));
            };
            let Some(pin) = self.pins.get(host) else {
                return Err(DialError::refused("that device isn't in the peer book"));
            };
            let command = self.windows.command(host);
            self.masters.dial(host, None, Some(&pin), command).await
        })
    }

    fn forget<'a>(&'a self, host: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(self.masters.forget(host))
    }

    fn shell_is_windows(&self, host: &str) {
        self.windows.note(host);
    }
}

/// The SSH hosts whose shell said it has no `sh`, which get
/// [`WINDOWS_PROXY_COMMAND`] in place of [`PROXY_COMMAND`]. A forced
/// command runs in place of either, so a pinned key never needs it.
#[derive(Default)]
struct Shells(Mutex<HashSet<String>>);

impl Shells {
    /// Starts `maxd` on `host` the Windows way from now on.
    fn note(&self, host: &str) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(host.to_owned());
    }

    /// What starts `maxd proxy` on `host`.
    fn command(&self, host: &str) -> &'static str {
        if self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(host)
        {
            WINDOWS_PROXY_COMMAND
        } else {
            PROXY_COMMAND
        }
    }
}

/// Reads ssh's stderr for why a login failed.
pub(crate) fn ssh_reason(text: &str) -> DialError {
    let lower = text.to_ascii_lowercase();
    let has = |needle: &str| lower.contains(needle);
    if has("tailscale ssh requires an additional check") {
        DialError::refused("Tailscale SSH wants you to sign in in a browser")
    } else if has("permission denied (") {
        DialError::refused("SSH sign-in was refused")
    } else if has("host key verification failed") || has("remote host identification has changed") {
        DialError::refused("the host key isn't trusted, so ssh to it once by hand")
    } else if has("could not resolve hostname") {
        DialError::down("can't find the host by name")
    } else if has("connection refused") {
        DialError::down("SSH connection refused")
    } else if has("timed out") {
        DialError::down("SSH timed out")
    } else if has("no route to host") || has("network is unreachable") || has("host is down") {
        DialError::down("no route to the host")
    } else if has("connection closed") || has("connection reset") || has("kex_exchange") {
        DialError::down("the SSH connection closed")
    } else {
        let line = text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty() && !line.starts_with("Warning:"))
            .unwrap_or("SSH failed");
        DialError::refused(shorten(line))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_errors_become_reasons() {
        let cases = [
            (
                "dev@devbox: Permission denied (publickey,password).",
                "SSH sign-in was refused",
                false,
            ),
            (
                "ssh: Could not resolve hostname nope: nodename nor servname provided, or not known",
                "can't find the host by name",
                true,
            ),
            (
                "ssh: connect to host 100.101.1.2 port 22: Operation timed out",
                "SSH timed out",
                true,
            ),
            (
                "ssh: connect to host 10.0.0.1 port 22: Connection refused",
                "SSH connection refused",
                true,
            ),
            (
                "kex_exchange_identification: Connection closed by remote host",
                "the SSH connection closed",
                true,
            ),
            (
                "Host key verification failed.",
                "the host key isn't trusted, so ssh to it once by hand",
                false,
            ),
            (
                "# Tailscale SSH requires an additional check.\n# To authenticate, visit: https://login.tailscale.com/a/x",
                "Tailscale SSH wants you to sign in in a browser",
                false,
            ),
            ("something new", "something new", false),
        ];
        for (text, reason, host_down) in cases {
            assert_eq!(
                ssh_reason(text),
                DialError {
                    reason: reason.into(),
                    host_down
                },
                "{text}"
            );
        }
    }

    #[test]
    fn hosts_whose_shell_has_no_sh_start_maxd_the_windows_way() {
        let shells = Shells::default();
        assert_eq!(shells.command("dev@devbox"), PROXY_COMMAND);
        shells.note("dev@pc");
        assert_eq!(shells.command("dev@pc"), WINDOWS_PROXY_COMMAND);
        assert_eq!(shells.command("dev@devbox"), PROXY_COMMAND);
    }
}
