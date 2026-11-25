// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Logins through openssh on Unix: one ControlMaster per SSH host, which
//! later connections to it reuse.

use std::collections::HashMap;
use std::io;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use openssh::{ControlPersist, KnownHosts, Session, SessionBuilder, Stdio};
use tracing::debug;

use super::{ssh_reason, RUN_TIMEOUT};
use crate::pinned::PinnedSsh;
use crate::setup::Output;
use crate::ssh_config::{is_safe_destination, SshConfig};
use crate::transport::{Conn, DialError, Rest, StderrTail, CONNECT_TIMEOUT, LOGIN_TIMEOUT};

/// How long a ControlMaster stays up once nothing uses it.
const MASTER_IDLE_SECS: usize = 600;

/// The longest control folder that leaves room for openssh's
/// `.ssh-connectionXXXXXX/master` and ssh's own temporary suffix inside the
/// 104 bytes a socket path can take on macOS.
const MAX_CONTROL_DIR: usize = 56;

/// The SSH masters, one per host.
pub(super) struct Masters {
    /// Where their control sockets go.
    control_dir: Option<PathBuf>,
    /// Masters by the host as the route names it.
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

impl Masters {
    /// Keeps control sockets in `control_dir`, or in openssh's default
    /// folder when it's none or too long for a socket path.
    pub fn new(control_dir: Option<PathBuf>) -> Self {
        Self {
            control_dir: control_dir.filter(|dir| dir.as_os_str().len() <= MAX_CONTROL_DIR),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Runs `command`, which starts `maxd proxy`, on a host over its
    /// master, logging in first if there's no master yet or it died.
    pub async fn dial(
        &self,
        host: &str,
        config: Option<&SshConfig>,
        pin: Option<&PinnedSsh>,
        command: &str,
    ) -> Result<Conn, DialError> {
        if !is_safe_destination(host) {
            return Err(DialError::refused("that isn't an SSH host name"));
        }
        if pin.is_some_and(|pin| !pin.is_plain()) {
            return Err(DialError::refused("that route can't go in an ssh command"));
        }
        let session = self.session(host, config, pin).await?;
        let spawned = session
            .arc_raw_command(command)
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
    pub async fn run(
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
    pub async fn forget(&self, host: &str) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let short = Masters::new(Some(PathBuf::from("/Users/max/Library/MaxHQ/ssh")));
        assert!(short.control_dir.is_some());
        let long = Masters::new(Some(PathBuf::from(format!(
            "/Users/{}/ssh",
            "m".repeat(60)
        ))));
        assert!(long.control_dir.is_none());
    }
}
