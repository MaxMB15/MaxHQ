// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Holders keep agent CLIs alive while `maxd` and the app come and go.
//!
//! `maxd` starts an agent with [`spawn`]. That runs `maxd hold`, which
//! detaches from `maxd`, starts the CLI with pipes on its stdin, stdout and
//! stderr, and records every line the CLI prints. `maxd` then calls
//! [`connect`] to reach the holder over its Unix socket, read the lines and
//! write the CLI's input. After a restart, `maxd` connects again with the
//! last line it saw and gets every line after it with no gap, along with the
//! prompts the CLI still waits on.
//!
//! Each session has a folder under `hold_dir()` from `maxhq_core::paths`,
//! named by its session ID:
//!
//! - `launch.json` says what to run. [`spawn`] writes it before the holder
//!   starts. It names the secret variables but never holds their values,
//!   which reach the holder on a pipe.
//! - `holder.json` has the holder's and the CLI's process IDs and the
//!   holder's socket.
//! - `holder.lock` stays locked for as long as the holder runs.
//! - `events.log` has one JSON line per line of output, `{seq, at, line}`.
//!   At 64 MiB it becomes `events.log.1`, replacing the one before, so a
//!   session keeps at most two.
//! - `pending.json` lists the prompts the CLI waits on. Every change is
//!   written to a new file, synced and renamed into place.
//! - `stderr.log` has the last 1 MiB the CLI wrote to stderr.
//! - `exit.json` says how the CLI exited.
//! - `exit.seen` appears once a client has seen the exit.
//! - `holder.log` has what the holder reports about itself.
//! - `holder.key` has the key a client shows in its hello. [`spawn`] makes
//!   a new one for each holder and hands it over on the same pipe as the
//!   secret variables. The holder refuses a client without it, so the CLI
//!   can't reach its own holder's socket and answer its own prompts. A
//!   holder started before keys has no such file and takes any client.
//!
//! Sockets live in `hold_runtime_dir()`, named with the last 12 hex digits
//! of the session ID, because macOS caps a socket path at 104 bytes and
//! Linux at 108.
//!
//! In a guest, the guest service runs as root and starts each holder in a
//! systemd scope of its own, [`Detach::SystemScope`]. The holder starts the
//! CLI as the guest's agent user, [`RunAs`]. A guest that restarts finds
//! its holders again with [`Holds::find`] or [`Holds::ensure`] rather than
//! starting new ones.

#![warn(missing_docs)]

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use maxhq_core::paths;
use maxhq_protocol::ids::SessionId;
use maxhq_protocol::time::Timestamp;
use serde::{Deserialize, Serialize};

pub use crate::client::{Connection, Events, Writer};
pub use crate::end::Ended;
pub use crate::error::{ConnectError, SpawnError, WriteError};
pub use crate::find::{Ensured, Found};
pub use crate::holder::run_holder;
pub use crate::scope::unit_name;
pub use crate::secret::Secret;
pub use crate::spawn::DETACH_ENV;
pub use crate::terminal::{
    login_shell, Reached, RunOutput, TermControl, TermEvent, TermExit, TermHello, TermLaunch,
    TermLink, Terminals,
};

mod client;
mod end;
mod error;
mod files;
mod find;
mod holder;
mod log;
mod pending;
pub mod pty;
mod scope;
mod secret;
mod socket;
mod spawn;
mod stderr;
mod terminal;
mod user;
mod wire;

/// How a CLI frames its messages, which tells the holder how to spot the
/// prompts it waits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Framing {
    /// Claude Code's stream-json. A `control_request` with the subtype
    /// `can_use_tool`, `elicitation` or `request_user_dialog` waits under
    /// its `request_id` until a `control_response` names it or the CLI sends
    /// `control_cancel_request`.
    ClaudeStreamJson,
    /// JSON-RPC 2.0, as `codex app-server` speaks it. A message from the CLI
    /// with both `id` and `method` waits under its `id` until a response
    /// with that `id` goes back or the CLI sends `serverRequest/resolved`
    /// naming it.
    JsonRpc,
}

/// What a holder runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    /// The session this agent belongs to. It names the session's folder.
    pub session: SessionId,
    /// The CLI to run.
    pub program: PathBuf,
    /// Its arguments.
    pub args: Vec<OsString>,
    /// Variables to set, on top of the environment the holder inherits.
    pub env: Vec<(OsString, OsString)>,
    /// Variables to remove from that environment before `env` is applied.
    pub env_remove: Vec<OsString>,
    /// Variables whose values must never be written down, such as tokens.
    /// The holder gets the values on a pipe and sets them in the CLI's
    /// environment alone, after `env`. `launch.json` has only the names.
    pub secret_env: Vec<(OsString, Secret)>,
    /// The user the CLI runs as. Only a holder running as root takes it.
    /// `None` runs the CLI as the holder's own user.
    pub run_as: Option<RunAs>,
    /// The folder the CLI starts in.
    pub cwd: PathBuf,
    /// How the CLI frames its messages.
    pub framing: Framing,
}

/// A user for the CLI to run as.
///
/// A holder running as root gives the CLI this user's groups, then its
/// group, then its ID, and starts it in the launch's folder, which the user
/// must be able to enter. The CLI gets `HOME`, `USER`, `LOGNAME` and
/// `SHELL` for the user, under the launch's own variables.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunAs {
    /// The user's name, which picks its groups.
    pub user: String,
    /// Its user ID.
    pub uid: u32,
    /// Its main group ID.
    pub gid: u32,
    /// Its home folder, for `HOME`.
    pub home: PathBuf,
}

impl RunAs {
    /// The user called `user`, as the user database has it.
    pub fn lookup(user: &str) -> io::Result<Self> {
        let entry = user::lookup(user)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("there's no user called {user}"),
            )
        })?;
        Ok(Self {
            user: user.to_owned(),
            uid: entry.uid,
            gid: entry.gid,
            home: entry.home,
        })
    }

    /// Gets ready to start a process as this user in `cwd`, as a holder
    /// starts a CLI. Only a process running as root can. This reads the
    /// user database, so it blocks.
    pub fn login(&self, cwd: &Path) -> Result<Login, String> {
        user::prepare(self, cwd).map(Login)
    }
}

/// A process start as a [`RunAs`] user, worked out before the fork.
pub struct Login(user::Prepared);

impl Login {
    /// `HOME`, `USER`, `LOGNAME` and `SHELL` for the user.
    pub fn env(&self) -> &[(OsString, OsString)] {
        &self.0.env
    }

    /// The shell the user's record names, if it names one.
    pub fn shell(&self) -> Option<&Path> {
        self.0.shell.as_deref()
    }

    /// Has `command` take the user's groups, group and ID in the forked
    /// process, then enter the folder. Steps the command already has run
    /// first, so one that must happen as root goes in before this. The
    /// command mustn't have a folder of its own.
    pub fn apply(self, command: &mut Command) -> Result<(), String> {
        self.0.apply(command)
    }
}

/// One line the CLI printed on stdout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    /// Its place in the session's output, counting from 1 with no gaps.
    pub seq: u64,
    /// When the holder read it.
    pub at: Timestamp,
    /// The line without its newline. Bytes that aren't UTF-8 become U+FFFD.
    pub text: String,
    /// Whether the CLI printed it before this connection was made, so the
    /// client is catching up rather than seeing it live.
    pub replayed: bool,
}

/// How the CLI ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exit {
    /// The exit code, when the CLI exited by itself.
    pub code: Option<i32>,
    /// The signal that ended it, when one did.
    ///
    /// Both this and `code` are empty when the holder stopped before the
    /// CLI did, so nobody saw how it ended.
    pub signal: Option<i32>,
    /// When the holder saw it end.
    pub at: Timestamp,
    /// The end of what the CLI wrote to stderr, at most 64 KiB by default.
    pub stderr_tail: String,
}

/// Whether the CLI is still running.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ChildState {
    /// It's running.
    Running {
        /// Its process ID, which is also its process group.
        pid: u32,
    },
    /// It has ended.
    Exited(Exit),
}

/// What a holder says when a client connects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hello {
    /// The holder's process ID. For a finished session it's the ID the
    /// holder had.
    pub holder_pid: u32,
    /// Whether the CLI is running.
    pub child: ChildState,
    /// The oldest line still on disk. Lines before it went with an old log.
    pub first_seq: u64,
    /// The newest line so far, or 0 before the first. A client that asked
    /// for lines after `after` gets each line up to this one marked
    /// replayed.
    pub last_seq: u64,
    /// The prompts the CLI waits on, oldest first, as the lines that asked
    /// them. They also come again in the replay when they're after `after`.
    pub pending: Vec<Line>,
}

/// Something that happened in a held session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The CLI printed a line.
    Line(Line),
    /// The CLI ended. Nothing comes after it.
    Exited(Exit),
}

/// What happened to a write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteOutcome {
    /// The CLI got the line.
    Written,
    /// The line answers a prompt that's no longer pending, because another
    /// client answered it first or the CLI withdrew it. The CLI never got
    /// the line.
    AlreadyAnswered,
}

/// Where holders keep their records and sockets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dirs {
    /// The folder with one folder per session.
    pub sessions: PathBuf,
    /// The folder for the holders' sockets.
    pub sockets: PathBuf,
}

impl Dirs {
    /// The folders from `maxhq_core::paths`, or `None` without a home
    /// folder. Setting `HOME`, `XDG_DATA_HOME` or `MAXD_SOCKET` moves them,
    /// and `MAXHQ_ROLE=guest` puts them under `/var/lib/maxhq` and
    /// `/run/maxhq`.
    pub fn from_env() -> Option<Self> {
        Some(Self {
            sessions: paths::hold_dir()?,
            sockets: paths::hold_runtime_dir()?,
        })
    }
}

/// How much a holder keeps and for how long.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The size at which `events.log` becomes `events.log.1`.
    pub rotate_at: u64,
    /// The most of the CLI's stderr that `stderr.log` keeps.
    pub stderr_cap: u64,
    /// How much of that stderr goes in [`Exit::stderr_tail`].
    pub stderr_tail: usize,
    /// How many bytes of recent output the holder keeps in memory, so a
    /// client catching up rarely reads the disk.
    pub memory_tail: usize,
    /// How long a holder stays up after the CLI ends when no client has
    /// seen the end.
    pub linger: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            rotate_at: 64 * 1024 * 1024,
            stderr_cap: 1024 * 1024,
            stderr_tail: 64 * 1024,
            memory_tail: 4 * 1024 * 1024,
            linger: Duration::from_secs(10 * 60),
        }
    }
}

/// How a holder leaves the process that spawns it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Detach {
    /// [`DETACH_ENV`] decides when it's set. Otherwise, on Linux, it's a
    /// system scope when the caller is root and systemd answers, a user
    /// scope when this user's systemd answers, and setsid everywhere else.
    #[default]
    Auto,
    /// A double fork and a new session. On macOS the holder then outlives
    /// the LaunchAgent that runs `maxd`, since the plist abandons its
    /// process group.
    Setsid,
    /// `systemd-run --user --scope`, which moves the holder out of the
    /// `maxd` unit, so restarting or stopping `maxd` leaves it running.
    UserScope,
    /// `systemd-run --scope` on the system manager, for a guest service
    /// that runs as root. Restarting the service leaves the holder running,
    /// as long as the holder is a child of the service and never its main
    /// process.
    SystemScope,
}

/// Starts, finds and reaches holders under one pair of folders.
#[derive(Clone, Debug)]
pub struct Holds {
    dirs: Dirs,
    holder: Option<(PathBuf, Vec<OsString>)>,
    limits: Limits,
    detach: Detach,
}

impl Holds {
    /// Holders under `dirs`, run as `<this program> hold`.
    pub fn new(dirs: Dirs) -> Self {
        Self {
            dirs,
            holder: None,
            limits: Limits::default(),
            detach: Detach::Auto,
        }
    }

    /// Holders in the usual folders, or `None` without a home folder.
    pub fn from_env() -> Option<Self> {
        Dirs::from_env().map(Self::new)
    }

    /// Runs holders as `program` with `args` before the session folder,
    /// rather than as `<this program> hold`. The program must end up in
    /// [`run_holder`].
    pub fn with_holder(mut self, program: impl Into<PathBuf>, args: Vec<OsString>) -> Self {
        self.holder = Some((program.into(), args));
        self
    }

    /// Gives holders started from now on these limits.
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Detaches holders started from now on this way.
    pub fn with_detach(mut self, detach: Detach) -> Self {
        self.detach = detach;
        self
    }

    /// Where these holders keep their records and sockets.
    pub fn dirs(&self) -> &Dirs {
        &self.dirs
    }

    /// Starts a detached holder for `launch` and returns once its socket
    /// takes connections.
    ///
    /// The holder outlives the caller. It fails when the session already
    /// has a folder, or when the holder can't start the CLI, in which case
    /// the error carries what the holder said. With a scope, a unit left
    /// over from an earlier holder of the session is cleared first.
    pub async fn spawn(&self, launch: &Launch) -> Result<(), SpawnError> {
        spawn::spawn(self, launch).await
    }

    /// What's there for a session: nothing, a holder that's starting, one
    /// that runs and proved it's the session's own, or one that has gone.
    ///
    /// A running holder proves itself the way [`Holds::connect`] checks it.
    /// It holds the session's lock and answers a hello for the session
    /// with the process ID in `holder.json`.
    pub async fn find(&self, session: SessionId) -> io::Result<Found> {
        find::find(self, session).await
    }

    /// Starts a holder for `launch` unless the session already has one,
    /// for a caller that restarted and doesn't know what survived it.
    ///
    /// It waits out a holder that's starting, and clears a folder whose
    /// holder never started. Anything else already there comes back as
    /// [`Ensured::Existing`], and nothing new starts.
    pub async fn ensure(&self, launch: &Launch) -> Result<Ensured, SpawnError> {
        find::ensure(self, launch).await
    }

    /// The sessions with a holder still running, or finished with nobody
    /// having seen the end yet.
    pub fn list(&self) -> io::Result<Vec<SessionId>> {
        files::list(&self.dirs.sessions)
    }

    /// Deletes the files of each session `keep` turns down whose holder has
    /// gone, for a caller that forgot the session without its end. Gives
    /// how many went.
    pub fn sweep(&self, mut keep: impl FnMut(SessionId) -> bool) -> io::Result<usize> {
        files::sweep(&self.dirs.sessions, &mut keep)
    }

    /// Reaches a session's holder.
    ///
    /// The connection replays each line after `after`, then carries on live
    /// with no gap. With `after` empty it starts at the newest line. A
    /// finished session whose holder has gone is served from its files.
    ///
    /// The one exception is lines the logs no longer hold. A client that
    /// asks for them, or reads so slowly that both logs fill behind it,
    /// carries on from the oldest line kept, which shows as a jump in `seq`.
    pub async fn connect(
        &self,
        session: SessionId,
        after: Option<u64>,
    ) -> Result<Connection, ConnectError> {
        client::connect(self, session, after).await
    }

    /// Makes sure nothing still runs for a session whose holder can't be
    /// reached, so a new holder never runs beside the old one.
    ///
    /// A running holder gets SIGTERM, which has it stop its CLI and record
    /// the exit, then SIGKILL after `grace`. A CLI that a dead holder left
    /// behind gets the same. `holder_pid` is the holder's process ID as
    /// its hello gave it, when one came. A process is only signalled once
    /// it's confirmed as the session's own, so one that can't be is left
    /// alone and reported as [`Ended::Unconfirmed`].
    pub async fn end(&self, session: SessionId, holder_pid: Option<u32>, grace: Duration) -> Ended {
        end::end(self, session, holder_pid, grace).await
    }
}

/// Starts a detached holder in the usual folders. See [`Holds::spawn`].
pub async fn spawn(launch: &Launch) -> Result<(), SpawnError> {
    let holds = Holds::from_env().ok_or(SpawnError::NoHome)?;
    holds.spawn(launch).await
}

/// The sessions in the usual folders that are worth connecting to. See
/// [`Holds::list`].
pub fn list() -> io::Result<Vec<SessionId>> {
    let holds = Holds::from_env().ok_or_else(error::no_home)?;
    holds.list()
}

/// Reaches a session's holder in the usual folders. See [`Holds::connect`].
pub async fn connect(session: SessionId, after: Option<u64>) -> Result<Connection, ConnectError> {
    let holds = Holds::from_env().ok_or_else(|| ConnectError::Io(error::no_home()))?;
    holds.connect(session, after).await
}
