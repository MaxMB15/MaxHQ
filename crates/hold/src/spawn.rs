// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Starting a holder: the session's folder and `launch.json`, then
//! `maxd hold` run so it leaves the caller behind.
//!
//! With setsid the holder detaches itself, as `holder.rs` describes. With
//! a scope, `systemd-run --scope` first moves it into a scope of its own,
//! on the user's manager for `maxd` or the system's for a guest, so the
//! caller's unit doesn't count it and a restart of the caller leaves it
//! running. A scope keeps the caller's environment and pipes, which a
//! service unit wouldn't.
//!
//! The holder's key and the secret variables go to the holder on its
//! stdin, which the first step hands down to the second. Nothing else reads
//! that pipe. The key also goes to `holder.key`, which only this user can
//! read, so a client started later can find it. It never goes in the
//! holder's environment, its arguments or `launch.json`.

use std::env::VarError;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::OnceCell;

use crate::client;
use crate::error::SpawnError;
use crate::files::{self, LaunchFile, KEY, LAUNCH};
use crate::find::Spawning;
use crate::scope::{self, Systemctl, Waits};
use crate::secret::{self, Zeroed};
use crate::{Detach, Holds, Launch};

/// The variable that picks how holders detach when [`Detach::Auto`] is in
/// use: `setsid`, `user` for a user scope, or `system` for a system scope.
/// `systemd` means a user scope too.
pub const DETACH_ENV: &str = "MAXHQ_HOLD_DETACH";

/// How long a holder has to say it's ready.
pub(crate) const READY_WAIT: Duration = Duration::from_secs(20);

/// How long the first step of the holder has to exit once it has started
/// the second.
const EXIT_WAIT: Duration = Duration::from_secs(5);

/// How long `systemctl` has to answer whether it runs.
const CHECK_WAIT: Duration = Duration::from_secs(5);

/// The most of a failed holder's stderr kept for the error.
const STDERR_KEEP: usize = 64 * 1024;

pub(crate) async fn spawn(holds: &Holds, launch: &Launch) -> Result<(), SpawnError> {
    let detach = resolve(holds.detach).await?;
    let key = secret::new_key().map_err(|source| SpawnError::Io {
        path: KEY.into(),
        source,
    })?;
    let pipe =
        secret::encode(Some(&key), &launch.secret_env).map_err(|reason| SpawnError::Secrets {
            reason: reason.to_owned(),
        })?;
    let sessions = absolute(&holds.dirs.sessions)?;
    let sockets = absolute(&holds.dirs.sockets)?;
    let mut launch = launch.clone();
    // The holder runs from `/`, so relative paths would point somewhere
    // else by then. A bare program name is still looked up in `PATH`.
    launch.cwd = absolute(&launch.cwd)?;
    if launch.program.is_relative() && launch.program.components().count() > 1 {
        launch.program = absolute(&launch.program)?;
    }
    files::private_dir_all(&sessions).map_err(|source| SpawnError::Io {
        path: sessions.clone(),
        source,
    })?;
    let dir = files::session_dir(&sessions, &launch.session);
    let _spawning = Spawning::new(&dir);
    match files::private_dir(&dir) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            return Err(SpawnError::Exists {
                session: launch.session,
            });
        }
        Err(source) => return Err(SpawnError::Io { path: dir, source }),
    }
    // This spawn made the folder, so no holder serves the session from a
    // scope with its name. Whatever runs there is left from an older one.
    if let Some(user) = scope_manager(detach) {
        let unit = scope::unit_name(launch.session);
        let holder_alive = || files::holder_alive(&dir).unwrap_or(true);
        let cleared = scope::clear(&Systemctl { user }, &unit, holder_alive, Waits::SPAWN).await;
        if let Err(reason) = cleared {
            let _ = fs::remove_dir_all(&dir);
            return Err(SpawnError::Unit { unit, reason });
        }
    }
    let record = LaunchFile::new(&launch, &sockets, &holds.limits);
    let written = files::write_key(&dir, &key)
        .map_err(|source| (KEY, source))
        .and_then(|()| {
            files::write_json(&dir.join(LAUNCH), &record).map_err(|source| (LAUNCH, source))
        });
    drop(key);
    if let Err((name, source)) = written {
        let _ = fs::remove_dir_all(&dir);
        return Err(SpawnError::Io {
            path: dir.join(name),
            source,
        });
    }
    let started = start(holds, &launch, detach, &dir, pipe).await;
    // A holder that gave up leaves nothing worth keeping. One that is
    // merely slow may still start, so its folder stays. One that reported
    // an error is on its way out, but may not have let go of its lock yet.
    let gone = match &started {
        Ok(()) | Err(SpawnError::Timeout) => false,
        Err(SpawnError::Holder { .. }) => client::wait_gone_within(&dir, EXIT_WAIT).await,
        Err(_) => !files::holder_alive(&dir).unwrap_or(true),
    };
    if gone {
        let _ = fs::remove_dir_all(&dir);
    }
    started
}

fn absolute(path: &Path) -> Result<PathBuf, SpawnError> {
    std::path::absolute(path).map_err(|source| SpawnError::Io {
        path: path.to_owned(),
        source,
    })
}

/// Whether `detach` runs the holder in a scope, and on the user's manager
/// when it does.
fn scope_manager(detach: Detach) -> Option<bool> {
    match detach {
        Detach::UserScope => Some(true),
        Detach::SystemScope => Some(false),
        Detach::Auto | Detach::Setsid => None,
    }
}

/// Runs the holder, hands it `pipe`, its key and the secret variables, and
/// waits for its `ready`.
async fn start(
    holds: &Holds,
    launch: &Launch,
    detach: Detach,
    dir: &Path,
    pipe: Zeroed,
) -> Result<(), SpawnError> {
    let (program, mut args) = match &holds.holder {
        Some((program, args)) => (program.clone(), args.clone()),
        None => {
            let program = std::env::current_exe().map_err(|source| SpawnError::Start {
                program: "maxd".into(),
                source,
            })?;
            (program, vec![OsString::from("hold")])
        }
    };
    args.push(dir.as_os_str().to_owned());
    let (mut command, shown) = match scope_manager(detach) {
        Some(user) => {
            let mut command = Command::new("systemd-run");
            if user {
                command.arg("--user");
            }
            command
                .args(["--scope", "--collect", "--quiet"])
                .arg(format!("--unit={}", scope::unit_base(launch.session)))
                .arg("--")
                .arg(&program);
            (command, PathBuf::from("systemd-run"))
        }
        None => (Command::new(&program), program),
    };
    command
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|source| SpawnError::Start {
        program: shown,
        source,
    })?;
    // The holder reads the lot before it starts the CLI. A holder that
    // gives up first closes the pipe, which ends the write.
    if let Some(mut stdin) = child.stdin.take() {
        tokio::spawn(async move {
            let _ = stdin.write_all(&pipe.0).await;
            drop(stdin);
            drop(pipe);
        });
    }
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let errors = tokio::spawn(read_capped(stderr));
    let ready = tokio::time::timeout(READY_WAIT, wait_ready(stdout)).await;
    match ready {
        Ok(true) => {
            errors.abort();
            let _ = tokio::time::timeout(EXIT_WAIT, child.wait()).await;
            Ok(())
        }
        Ok(false) => {
            let status = tokio::time::timeout(EXIT_WAIT, child.wait()).await;
            let said = tokio::time::timeout(EXIT_WAIT, errors)
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default();
            let said = said.trim();
            let message = if !said.is_empty() {
                said.to_owned()
            } else if let Ok(Ok(status)) = status {
                format!("it ended with {status} before it was ready")
            } else {
                "it ended before it was ready".to_owned()
            };
            Err(SpawnError::Holder { message })
        }
        Err(_) => {
            errors.abort();
            Err(SpawnError::Timeout)
        }
    }
}

/// Reads the holder's stdout until it says `ready`. Returns false when it
/// closes first.
async fn wait_ready(stdout: Option<impl AsyncRead + Unpin>) -> bool {
    let Some(stdout) = stdout else {
        return false;
    };
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim() == "ready" {
            return true;
        }
    }
    false
}

/// Reads the holder's stderr, keeping the first [`STDERR_KEEP`] bytes.
async fn read_capped(stderr: Option<impl AsyncRead + Unpin>) -> String {
    let Some(mut stderr) = stderr else {
        return String::new();
    };
    let mut kept = Vec::new();
    let mut buf = [0; 4096];
    loop {
        match stderr.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = STDERR_KEEP.saturating_sub(kept.len());
                kept.extend_from_slice(&buf[..n.min(room)]);
            }
        }
    }
    String::from_utf8_lossy(&kept).into_owned()
}

/// Settles [`Detach::Auto`].
async fn resolve(detach: Detach) -> Result<Detach, SpawnError> {
    if detach != Detach::Auto {
        return Ok(detach);
    }
    let value = match std::env::var(DETACH_ENV) {
        Ok(value) => value,
        Err(VarError::NotUnicode(value)) => value.to_string_lossy().into_owned(),
        Err(VarError::NotPresent) => String::new(),
    };
    if let Some(detach) = parse_detach(&value)? {
        return Ok(detach);
    }
    if !cfg!(target_os = "linux") {
        return Ok(Detach::Setsid);
    }
    if rustix::process::geteuid().is_root() && systemd_reachable(false).await {
        Ok(Detach::SystemScope)
    } else if systemd_reachable(true).await {
        Ok(Detach::UserScope)
    } else {
        Ok(Detach::Setsid)
    }
}

/// The way to detach that [`DETACH_ENV`] names, or `None` when it's empty.
fn parse_detach(value: &str) -> Result<Option<Detach>, SpawnError> {
    match value {
        "" => Ok(None),
        "setsid" => Ok(Some(Detach::Setsid)),
        "user" | "systemd" => Ok(Some(Detach::UserScope)),
        "system" => Ok(Some(Detach::SystemScope)),
        _ => Err(SpawnError::BadDetach {
            value: value.to_owned(),
        }),
    }
}

/// Whether systemd answers, the user's manager or the system's, asked once
/// per process.
async fn systemd_reachable(user: bool) -> bool {
    static USER: OnceCell<bool> = OnceCell::const_new();
    static SYSTEM: OnceCell<bool> = OnceCell::const_new();
    let cell = if user { &USER } else { &SYSTEM };
    *cell
        .get_or_init(|| async {
            let mut check = Command::new("systemctl");
            if user {
                check.arg("--user");
            }
            let check = check
                .args(["show", "--property=Version"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .status();
            matches!(
                tokio::time::timeout(CHECK_WAIT, check).await,
                Ok(Ok(status)) if status.success()
            )
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_detach_variable_names_each_way() {
        assert_eq!(parse_detach("").unwrap(), None);
        assert_eq!(parse_detach("setsid").unwrap(), Some(Detach::Setsid));
        assert_eq!(parse_detach("user").unwrap(), Some(Detach::UserScope));
        assert_eq!(parse_detach("systemd").unwrap(), Some(Detach::UserScope));
        assert_eq!(parse_detach("system").unwrap(), Some(Detach::SystemScope));
        let err = parse_detach("System").unwrap_err();
        assert!(matches!(err, SpawnError::BadDetach { ref value } if value == "System"));
        assert!(err.to_string().contains("user or system"), "{err}");
    }

    #[test]
    fn only_scopes_have_a_manager() {
        assert_eq!(scope_manager(Detach::UserScope), Some(true));
        assert_eq!(scope_manager(Detach::SystemScope), Some(false));
        assert_eq!(scope_manager(Detach::Setsid), None);
    }
}
