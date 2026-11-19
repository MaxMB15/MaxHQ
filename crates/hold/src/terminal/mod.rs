// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Terminals: a shell in the drawer, or a command run with `!`, kept alive
//! by a holder of its own, `maxd hold --pty`.
//!
//! A terminal's holder starts the way an agent's does, as `holder.rs`
//! describes, and outlives `maxd` the same way. It owns the pty's master
//! and the shell's process group. It keeps the last 2 MiB of output by
//! absolute offset, so `maxd` can come back after a restart and carry on
//! from the last byte it had. Its wire is its own, in `wire.rs`.
//!
//! What someone types goes to the pty and nowhere else. The holder never
//! logs it or keeps it, not even its length. Output is kept: for a `!` run
//! the holder also feeds a headless terminal, so the run's exit carries
//! the lines it showed, with a password sent at a prompt as ` 🔒 sent`.
//! What a run's shell prints before its command, from its startup files,
//! goes nowhere (`marker.rs`).
//!
//! Each terminal has a folder named by its ID, under a folder of its own
//! rather than the agents' one:
//!
//! - `launch.json` says what to run, in what size and where the socket
//!   goes.
//! - `holder.key`, `holder.lock`, `holder.json` and `holder.log` are as for
//!   an agent's holder.
//! - `exit.json` says how the program ended, with the run's transcript.
//!   It stays when the holder ends without a goodbye.
//! - `note.json` is for `maxd`, which keeps what it knows about the
//!   terminal there, so it goes with the folder.
//!
//! A drawer shell that's zsh, bash or fish can mark its prompts and
//! commands (`shell.rs`), with startup files in a `shell-*` folder beside
//! the terminals' ones. The holder passes on the marks it trusts and drops
//! the rest (`marks.rs`).

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use maxhq_core::paths;
use maxhq_protocol::ids::TerminalId;
use maxhq_protocol::time::Timestamp;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::files::{self, OsBytes, LAUNCH};
use crate::spawn::{self, Run};
use crate::{client as agent_client, secret, Detach, Dirs, SpawnError};

pub use self::client::{Reached, TermControl, TermEvent, TermLink};
pub use self::commands::{Commands, ShellCommand};
pub use self::marks::Mark;

mod client;
mod commands;
mod holder;
mod marker;
mod marks;
mod osc;
mod ring;
mod shell;
mod wire;

/// `maxd`'s own file in a terminal's folder.
const NOTE: &str = "note.json";

/// How long a holder stays after its program ends when `maxd` doesn't say
/// goodbye.
const LINGER: Duration = Duration::from_secs(10 * 60);

/// The flag that makes `maxd hold` a terminal's holder.
const PTY: &str = crate::holder::PTY;

/// What a terminal's holder runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TermLaunch {
    /// The terminal. It names the folder.
    pub terminal: TerminalId,
    /// The program, usually the user's shell.
    pub program: PathBuf,
    /// Its arguments.
    pub args: Vec<OsString>,
    /// Variables to set, on top of the environment the holder inherits.
    pub env: Vec<(OsString, OsString)>,
    /// The folder it starts in.
    pub cwd: PathBuf,
    /// The window's width in cells.
    pub cols: u16,
    /// The window's height in cells.
    pub rows: u16,
    /// Whether it's a `!` run, whose exit carries what it showed.
    pub run: bool,
    /// What the shell's marks carry to show they're its own, once
    /// [`Terminals::integrate`] has set it up. Others are dropped.
    pub nonce: Option<String>,
}

impl TermLaunch {
    /// A login shell for the terminal drawer, `$SHELL -l`.
    pub fn shell(terminal: TerminalId, cwd: PathBuf, cols: u16, rows: u16) -> Self {
        Self::new(terminal, vec!["-l".into()], cwd, cols, rows, false)
    }

    /// A `!` run: `command` in a fresh interactive login shell,
    /// `$SHELL -ilc <command>`, so it finds what the user's own terminal
    /// would. A marker goes first, so the holder drops what the shell's
    /// startup printed, as `marker.rs` says.
    pub fn run(terminal: TerminalId, cwd: PathBuf, command: &str, cols: u16, rows: u16) -> Self {
        let args = vec!["-ilc".into(), marker::after_marker(command).into()];
        Self::new(terminal, args, cwd, cols, rows, true)
    }

    fn new(
        terminal: TerminalId,
        args: Vec<OsString>,
        cwd: PathBuf,
        cols: u16,
        rows: u16,
        run: bool,
    ) -> Self {
        Self {
            terminal,
            program: login_shell(),
            args,
            env: terminal_env(),
            cwd,
            cols,
            rows,
            run,
            nonce: None,
        }
    }
}

/// The user's shell: `$SHELL` when it's an absolute path, or zsh on macOS
/// and bash elsewhere.
pub fn login_shell() -> PathBuf {
    std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|shell| shell.is_absolute())
        .unwrap_or_else(|| {
            PathBuf::from(if cfg!(target_os = "macos") {
                "/bin/zsh"
            } else {
                "/bin/bash"
            })
        })
}

/// What a terminal's program gets on top of the user's environment, so it
/// knows what it talks to.
fn terminal_env() -> Vec<(OsString, OsString)> {
    [
        ("TERM", "xterm-256color"),
        ("COLORTERM", "truecolor"),
        ("TERM_PROGRAM", "MaxHQ"),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.into()))
    .collect()
}

/// What a `!` run showed, as its card keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunOutput {
    /// The lines, with colors and styles as SGR escapes and no others.
    pub text: String,
    /// How many lines it printed, counting those left out.
    pub lines: u32,
    /// Whether `text` holds only the last part.
    pub truncated: bool,
}

/// How a terminal's program ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermExit {
    /// The exit code, when it exited by itself.
    pub code: Option<i32>,
    /// The signal that ended it, when one did. Both are empty when the
    /// holder went before it.
    pub signal: Option<i32>,
    /// When the holder saw it end.
    pub at: Timestamp,
    /// What a `!` run showed. A shell has none.
    pub output: Option<RunOutput>,
}

/// What a terminal's holder says when `maxd` connects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermHello {
    /// The holder's process ID.
    pub holder_pid: u32,
    /// The program's process ID, which is also its process group.
    pub child_pid: u32,
    /// When the program started.
    pub started_at: Timestamp,
    /// Where the output that follows starts.
    pub from: u64,
    /// Whether the bytes after the offset `maxd` asked for are gone, so it
    /// starts over at `from`, the start of a whole line.
    pub reset: bool,
    /// The window's width in cells.
    pub cols: u16,
    /// The window's height in cells.
    pub rows: u16,
    /// Whether the program waits for a password.
    pub password: bool,
    /// Whether the shell runs something rather than waiting at its prompt.
    /// A `!` run is busy until it ends.
    pub busy: bool,
    /// The window title the shell set last.
    pub title: Option<String>,
    /// The working folder the shell said last.
    pub cwd: Option<String>,
    /// Whether the holder takes out every OSC 133 but the shell's own
    /// marks, as one since protocol 47 does. One from before doesn't say,
    /// and its output passes on any 133 a program prints.
    #[serde(default)]
    pub marks: bool,
}

/// `launch.json` for a terminal.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct LaunchFile {
    terminal: TerminalId,
    program: OsBytes,
    args: Vec<OsBytes>,
    env: Vec<(OsBytes, OsBytes)>,
    cwd: OsBytes,
    cols: u16,
    rows: u16,
    run: bool,
    #[serde(default)]
    nonce: Option<String>,
    sockets: OsBytes,
    linger_ms: u64,
}

/// Starts, lists and reaches terminal holders under one pair of folders.
#[derive(Clone, Debug)]
pub struct Terminals {
    dirs: Dirs,
    holder: Option<(PathBuf, Vec<OsString>)>,
    detach: Detach,
    linger: Duration,
}

impl Terminals {
    /// Terminals under `dirs`, held by `<this program> hold --pty`.
    pub fn new(dirs: Dirs) -> Self {
        Self {
            dirs,
            holder: None,
            detach: Detach::Auto,
            linger: LINGER,
        }
    }

    /// Terminals in the usual folders, or `None` without a home folder:
    /// `terminals` beside the agents' `hold` folder, with the sockets among
    /// the agents' ones.
    pub fn from_env() -> Option<Self> {
        Some(Self::new(Dirs {
            sessions: paths::data_dir()?.join("terminals"),
            sockets: paths::hold_runtime_dir()?,
        }))
    }

    /// Runs holders as `program` with `args` before `--pty` and the folder.
    /// The program must end up in [`crate::run_holder`].
    pub fn with_holder(mut self, program: impl Into<PathBuf>, args: Vec<OsString>) -> Self {
        self.holder = Some((program.into(), args));
        self
    }

    /// Detaches holders started from now on this way.
    pub fn with_detach(mut self, detach: Detach) -> Self {
        self.detach = detach;
        self
    }

    /// How long a holder started from now on stays once its program ends
    /// when nobody says goodbye.
    pub fn with_linger(mut self, linger: Duration) -> Self {
        self.linger = linger;
        self
    }

    /// Where these terminals keep their folders and sockets.
    pub fn dirs(&self) -> &Dirs {
        &self.dirs
    }

    fn dir(&self, terminal: TerminalId) -> PathBuf {
        self.dirs.sessions.join(terminal.to_string())
    }

    /// Makes `launch`, a drawer shell, mark its prompts and commands when
    /// it's zsh, bash or fish, as `shell.rs` says. Returns whether it will.
    pub fn integrate(&self, launch: &mut TermLaunch) -> io::Result<bool> {
        shell::integrate(launch, &self.dirs.sessions)
    }

    /// Deletes what writing the shell integration files left when a `maxd`
    /// stopped part way, as `maxd` does when it starts.
    pub fn sweep(&self) -> io::Result<()> {
        shell::sweep(&self.dirs.sessions)
    }

    /// Starts a detached holder for `launch` and returns once its socket
    /// takes connections. It fails when the terminal already has a folder,
    /// or with what the holder said when the program didn't start.
    pub async fn spawn(&self, launch: &TermLaunch) -> Result<(), SpawnError> {
        let detach = spawn::resolve(self.detach).await?;
        let io_error = |path: &Path| {
            let path = path.to_owned();
            move |source| SpawnError::Io { path, source }
        };
        let key = secret::new_key().map_err(io_error(Path::new(files::KEY)))?;
        let pipe = secret::encode(Some(&key), &[]).map_err(|reason| SpawnError::Secrets {
            reason: reason.to_owned(),
        })?;
        let sessions = spawn::absolute(&self.dirs.sessions)?;
        let sockets = spawn::absolute(&self.dirs.sockets)?;
        files::private_dir_all(&sessions).map_err(io_error(&sessions))?;
        let dir = sessions.join(launch.terminal.to_string());
        files::private_dir(&dir).map_err(io_error(&dir))?;
        let program = if launch.program.is_relative() && launch.program.components().count() > 1 {
            spawn::absolute(&launch.program)?
        } else {
            launch.program.clone()
        };
        let record = LaunchFile {
            terminal: launch.terminal,
            program: program.as_path().into(),
            args: launch.args.iter().cloned().map(OsBytes).collect(),
            env: (launch.env.iter())
                .map(|(key, value)| (OsBytes(key.clone()), OsBytes(value.clone())))
                .collect(),
            cwd: spawn::absolute(&launch.cwd)?.as_path().into(),
            cols: launch.cols,
            rows: launch.rows,
            run: launch.run,
            nonce: launch.nonce.clone(),
            sockets: sockets.as_path().into(),
            linger_ms: u64::try_from(self.linger.as_millis()).unwrap_or(u64::MAX),
        };
        let written = files::write_key(&dir, &key)
            .map_err(|source| (files::KEY, source))
            .and_then(|()| {
                files::write_json(&dir.join(LAUNCH), &record).map_err(|source| (LAUNCH, source))
            });
        drop(key);
        if let Err((name, source)) = written {
            let _ = fs::remove_dir_all(&dir);
            return Err(io_error(&dir.join(name))(source));
        }
        let run = Run {
            holder: self.holder.as_ref(),
            flags: &[PTY],
            unit: format!("maxhq-term-{}", launch.terminal),
            detach,
        };
        let started = spawn::start(run, &dir, pipe).await;
        // As for an agent's holder: a slow one may still start, and one
        // that reported an error may not have let go of its lock yet.
        let gone = match &started {
            Ok(()) | Err(SpawnError::Timeout) => false,
            Err(SpawnError::Holder { .. }) => {
                agent_client::wait_gone_within(&dir, spawn::EXIT_WAIT).await
            }
            Err(_) => !files::holder_alive(&dir).unwrap_or(true),
        };
        if gone {
            let _ = fs::remove_dir_all(&dir);
        }
        started
    }

    /// Every terminal with a folder: running, or ended with nobody having
    /// said goodbye.
    pub fn list(&self) -> io::Result<Vec<TerminalId>> {
        let entries = match fs::read_dir(&self.dirs.sessions) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err),
        };
        let mut found = Vec::new();
        for entry in entries {
            let entry = entry?;
            if let Some(terminal) = entry.file_name().to_str().and_then(|n| n.parse().ok()) {
                found.push(terminal);
            }
        }
        found.sort();
        Ok(found)
    }

    /// Reaches a terminal's holder. The link replays the output from
    /// `after`, the offset just past the last byte the caller has, then
    /// carries on live with no gap. With `after` empty it replays all the
    /// holder keeps. A holder that has gone gives the exit it left.
    pub async fn connect(&self, terminal: TerminalId, after: Option<u64>) -> io::Result<Reached> {
        client::connect(&self.dir(terminal), terminal, after, false).await
    }

    /// Reaches a terminal's holder for everything but the output: whether
    /// it waits for a password or runs something, its title and folder, and
    /// its exit. It costs nothing however much the terminal prints.
    pub async fn watch(&self, terminal: TerminalId) -> io::Result<Reached> {
        client::connect(&self.dir(terminal), terminal, None, true).await
    }

    /// Deletes the folder of a terminal whose holder has gone. Returns
    /// whether it went.
    pub fn remove(&self, terminal: TerminalId) -> io::Result<bool> {
        let dir = self.dir(terminal);
        if files::holder_alive(&dir)? {
            return Ok(false);
        }
        files::remove_session(&dir)?;
        Ok(true)
    }

    /// Keeps `note` in the terminal's folder for `maxd`.
    pub fn save_note<T: Serialize>(&self, terminal: TerminalId, note: &T) -> io::Result<()> {
        files::write_json(&self.dir(terminal).join(NOTE), note)
    }

    /// The note kept with [`Terminals::save_note`], if any.
    pub fn load_note<T: DeserializeOwned>(&self, terminal: TerminalId) -> io::Result<Option<T>> {
        files::read_json(&self.dir(terminal).join(NOTE))
    }
}

/// The socket's name for `terminal`: `t` and the last 11 hex digits of its
/// ID, which can't be an agent holder's name, all hex.
fn socket_name(terminal: TerminalId) -> String {
    let hex = terminal.0.simple().to_string();
    format!("t{}", &hex[hex.len() - 11..])
}

/// The holder itself, `maxd hold --pty <folder> --detached`.
pub(crate) fn hold(dir: &Path) -> ExitCode {
    crate::holder::detached(dir, |dir| async move {
        match holder::Started::start(&dir) {
            Ok(started) => {
                started.run().await;
                ExitCode::SUCCESS
            }
            Err(message) => {
                eprintln!("{message}");
                ExitCode::FAILURE
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_is_an_interactive_login_shell_with_the_command() {
        let terminal = TerminalId::generate();
        let launch = TermLaunch::run(terminal, "/tmp".into(), "ls -la", 80, 24);
        assert_eq!(
            launch.args,
            [
                OsString::from("-ilc"),
                r"printf '\033]7346;maxhq-run\007'; ls -la".into()
            ]
        );
        assert!(launch.run);
        assert!(launch.program.is_absolute());
        let shell = TermLaunch::shell(terminal, "/tmp".into(), 80, 24);
        assert_eq!(shell.args, [OsString::from("-l")]);
        assert!(!shell.run);
        for launch in [&launch, &shell] {
            let env: Vec<_> = launch.env.iter().map(|(key, _)| key.clone()).collect();
            assert_eq!(env, ["TERM", "COLORTERM", "TERM_PROGRAM"]);
        }
    }

    #[test]
    fn socket_names_cant_meet_an_agents() {
        let terminal: TerminalId = "0199f0a2-7c3e-7a10-9b4d-1234567890ab".parse().unwrap();
        assert_eq!(socket_name(terminal), "t234567890ab");
    }
}
