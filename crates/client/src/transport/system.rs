// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The system `ssh`, through openssh: one ControlMaster per SSH host, which
//! later connections to it reuse. Only the `system-ssh` feature has it.

use std::collections::HashMap;
use std::io;
use std::net::IpAddr;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use maxhq_protocol::ids::DeviceId;
use maxhq_protocol::link::Route;
use openssh::{ControlPersist, KnownHosts, Session, SessionBuilder, Stdio};
use tracing::debug;

use super::local::dial_local;
use super::{
    shorten, Conn, DialError, Rest, StderrTail, Transport, CONNECT_TIMEOUT, LOGIN_TIMEOUT,
    PROXY_COMMAND,
};
use crate::pinned::{PinnedSsh, Pins};
use crate::setup::Output;
use crate::ssh_config::{self, is_safe_destination, SshConfig};

/// How long a ControlMaster stays up once nothing uses it.
const MASTER_IDLE_SECS: usize = 600;

/// How long a name lookup gets.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);

/// How long a command run over a master may take.
const RUN_TIMEOUT: Duration = Duration::from_secs(60);

/// The longest control folder that leaves room for openssh's
/// `.ssh-connectionXXXXXX/master` and ssh's own temporary suffix inside the
/// 104 bytes a socket path can take on macOS.
const MAX_CONTROL_DIR: usize = 56;

/// The real thing.
pub(crate) struct System {
    /// The local `maxd`'s socket, if this machine has a place for it.
    socket: Option<PathBuf>,
    /// One master per SSH host.
    masters: Masters,
}

impl System {
    /// Reaches the local `maxd` at `socket` and keeps SSH masters in
    /// `control_dir`, or in openssh's default folder when it's none or too
    /// long for a socket path.
    pub fn new(socket: Option<PathBuf>, control_dir: Option<PathBuf>) -> Self {
        let control_dir = control_dir.filter(|dir| dir.as_os_str().len() <= MAX_CONTROL_DIR);
        Self {
            socket,
            masters: Masters {
                control_dir,
                sessions: Mutex::new(HashMap::new()),
            },
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
                Route::Ssh { host } => self.masters.dial(host, config, None).await,
            }
        })
    }

    fn forget<'a>(&'a self, host: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(self.masters.forget(host))
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
}

impl Pinned {
    /// Dials the pins in `pins`, keeping SSH masters in `control_dir`.
    pub fn new(pins: Pins, control_dir: Option<PathBuf>) -> Self {
        Self {
            pins,
            masters: Masters {
                control_dir: control_dir.filter(|dir| dir.as_os_str().len() <= MAX_CONTROL_DIR),
                sessions: Mutex::new(HashMap::new()),
            },
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
            match self.pins.get(host) {
                Some(pin) if pin.is_plain() => self.masters.dial(host, None, Some(&pin)).await,
                Some(_) => Err(DialError::refused("that route can't go in an ssh command")),
                None => Err(DialError::refused("that device isn't in the peer book")),
            }
        })
    }

    fn forget<'a>(&'a self, host: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(self.masters.forget(host))
    }
}

/// The SSH masters, one per host.
struct Masters {
    /// Where their control sockets go.
    control_dir: Option<PathBuf>,
    /// Masters by the host as the route names it.
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

impl Masters {
    /// Runs `maxd proxy` on a host over its master, logging in first if
    /// there's no master yet or it died.
    async fn dial(
        &self,
        host: &str,
        config: Option<&SshConfig>,
        pin: Option<&PinnedSsh>,
    ) -> Result<Conn, DialError> {
        if !is_safe_destination(host) {
            return Err(DialError::refused("that isn't an SSH host name"));
        }
        let session = self.session(host, config, pin).await?;
        let spawned = session
            .arc_raw_command(PROXY_COMMAND)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .await;
        let mut child = match spawned {
            Ok(child) => child,
            Err(err) => {
                debug!(%err, "couldn't start maxd proxy");
                self.forget(host).await;
                return Err(DialError::down("the SSH connection dropped"));
            }
        };
        let (Some(stdin), Some(stdout), Some(stderr)) = (
            child.stdin().take(),
            child.stdout().take(),
            child.stderr().take(),
        ) else {
            return Err(DialError::refused("ssh gave no pipes"));
        };
        Ok(Conn {
            reader: Box::new(stdout),
            writer: Box::new(stdin),
            rest: Rest {
                stderr: Some(StderrTail::read(stderr)),
                _keep: Some(Box::new(child)),
            },
        })
    }

    /// Runs `command` on a host over its master and waits up to
    /// [`RUN_TIMEOUT`] for it. The host must be one [`is_safe_destination`]
    /// passes.
    async fn run(
        &self,
        host: &str,
        config: Option<&SshConfig>,
        command: &str,
    ) -> io::Result<Output> {
        let session = self
            .session(host, config, None)
            .await
            .map_err(|err| io::Error::other(err.reason))?;
        let mut ran = session.raw_command(command);
        ran.stdin(Stdio::null());
        let output = match tokio::time::timeout(RUN_TIMEOUT, ran.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => {
                debug!(%err, "couldn't run a command over ssh");
                self.forget(host).await;
                return Err(io::Error::other("the SSH connection dropped"));
            }
            Err(_) => return Err(io::Error::new(io::ErrorKind::TimedOut, "it took too long")),
        };
        Ok(Output {
            ok: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    /// The host's master, if it still answers, or a new one.
    async fn session(
        &self,
        host: &str,
        config: Option<&SshConfig>,
        pin: Option<&PinnedSsh>,
    ) -> Result<Arc<Session>, DialError> {
        let existing = self
            .sessions
            .lock()
            .expect("masters lock")
            .get(host)
            .cloned();
        if let Some(session) = existing {
            if session.check().await.is_ok() {
                return Ok(session);
            }
            self.forget(host).await;
        }

        let mut builder = SessionBuilder::default();
        builder
            .known_hosts_check(known_hosts(config))
            .connect_timeout(
                config
                    .and_then(|config| config.connect_timeout)
                    .map_or(CONNECT_TIMEOUT, Duration::from_secs),
            )
            .control_persist(ControlPersist::IdleFor(
                NonZeroUsize::new(MASTER_IDLE_SECS).expect("not zero"),
            ));
        if let Some(dir) = &self.control_dir {
            if let Err(err) = create_private_dir(dir) {
                debug!(%err, "couldn't make the ssh control folder");
            } else {
                builder.control_directory(dir);
            }
        }
        let mut destination = host;
        if let Some(pin) = pin {
            // A config file in place of yours, since the builder takes no
            // other options. It holds no secret.
            let file = pin
                .known_hosts
                .with_file_name(format!("config-{}", pin.alias));
            if let Err(err) = std::fs::write(&file, pin.config()) {
                return Err(DialError::refused(format!(
                    "can't write the pinned ssh config: {err}"
                )));
            }
            builder
                .config_file(&file)
                .known_hosts_check(KnownHosts::Strict);
            destination = &pin.host;
        }
        let session = match tokio::time::timeout(LOGIN_TIMEOUT, builder.connect(destination)).await
        {
            Ok(Ok(session)) => Arc::new(session),
            Ok(Err(err)) => return Err(ssh_failure(&err)),
            Err(_) => return Err(DialError::refused("SSH login didn't finish")),
        };
        self.sessions
            .lock()
            .expect("masters lock")
            .insert(host.to_owned(), Arc::clone(&session));
        Ok(session)
    }

    /// Drops a host's master, and closes it if nothing else holds it.
    async fn forget(&self, host: &str) {
        let removed = self.sessions.lock().expect("masters lock").remove(host);
        if let Some(session) = removed.and_then(|session| Arc::try_unwrap(session).ok()) {
            let _ = session.close().await;
        }
    }
}

/// Makes a folder only this user can open.
fn create_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

/// The host key policy the host's own config asks for. ssh's default,
/// `ask`, can't ask in the background, so it refuses unknown keys.
fn known_hosts(config: Option<&SshConfig>) -> KnownHosts {
    match config.and_then(|config| config.strict_host_key_checking.as_deref()) {
        Some("accept-new") => KnownHosts::Add,
        Some("no" | "off") => KnownHosts::Accept,
        _ => KnownHosts::Strict,
    }
}

/// What an SSH login failure means for the app.
fn ssh_failure(err: &openssh::Error) -> DialError {
    let text = match err {
        openssh::Error::Connect(err) | openssh::Error::Master(err) => err.to_string(),
        openssh::Error::Ssh(err) => return DialError::refused(format!("can't run ssh: {err}")),
        other => other.to_string(),
    };
    ssh_reason(&text)
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
    fn host_key_policy_follows_the_config() {
        let config = |value: &str| SshConfig {
            strict_host_key_checking: Some(value.into()),
            ..SshConfig::default()
        };
        assert!(matches!(
            known_hosts(Some(&config("accept-new"))),
            KnownHosts::Add
        ));
        assert!(matches!(
            known_hosts(Some(&config("no"))),
            KnownHosts::Accept
        ));
        assert!(matches!(
            known_hosts(Some(&config("ask"))),
            KnownHosts::Strict
        ));
        assert!(matches!(
            known_hosts(Some(&config("yes"))),
            KnownHosts::Strict
        ));
        assert!(matches!(known_hosts(None), KnownHosts::Strict));
    }

    #[test]
    fn long_control_folders_fall_back_to_the_default() {
        let short = System::new(None, Some(PathBuf::from("/Users/max/Library/MaxHQ/ssh")));
        assert!(short.masters.control_dir.is_some());
        let long = System::new(
            None,
            Some(PathBuf::from(format!("/Users/{}/ssh", "m".repeat(60)))),
        );
        assert!(long.masters.control_dir.is_none());
    }
}
