// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What can go wrong when starting, reaching or writing to a holder.

use std::io;
use std::path::PathBuf;

use maxhq_protocol::ids::SessionId;
use thiserror::Error;

/// Why a holder didn't start.
#[derive(Debug, Error)]
pub enum SpawnError {
    /// There's no home folder to keep holders in.
    #[error("there's no home folder to keep holders in")]
    NoHome,
    /// The session already has a folder, so it has been started before.
    #[error("session {session} has already been started")]
    Exists {
        /// The session.
        session: SessionId,
    },
    /// `MAXHQ_HOLD_DETACH` names a way to detach that doesn't exist.
    #[error("MAXHQ_HOLD_DETACH is {value:?}, but it can only be setsid, systemd, user or system")]
    BadDetach {
        /// What it's set to.
        value: String,
    },
    /// A file or folder for the session couldn't be made.
    #[error("couldn't set up {}: {source}", path.display())]
    Io {
        /// The file or folder.
        path: PathBuf,
        /// What went wrong.
        source: io::Error,
    },
    /// The holder program didn't run.
    #[error("couldn't run the holder {}: {source}", program.display())]
    Start {
        /// The program.
        program: PathBuf,
        /// What went wrong.
        source: io::Error,
    },
    /// The holder ran but gave up, usually because it couldn't start the
    /// CLI. It reads as what the holder said, which is the reason a person
    /// needs, and a guest's holder failing on the host stays one reason.
    #[error("{message}")]
    Holder {
        /// What the holder wrote to stderr.
        message: String,
    },
    /// The holder didn't say it was ready in time.
    #[error("the holder didn't start within {} seconds", crate::spawn::READY_WAIT.as_secs())]
    Timeout,
    /// The secret variables can't go to the holder, because there's too
    /// much of them. The reason never repeats a value.
    #[error("couldn't pass the secret variables on: {reason}")]
    Secrets {
        /// Why not.
        reason: String,
    },
    /// The scope for the holder couldn't be readied, because a unit with
    /// its name is still there and wouldn't go.
    #[error("couldn't clear the unit {unit}: {reason}")]
    Unit {
        /// The unit, such as `maxhq-hold-<session>.scope`.
        unit: String,
        /// Why not.
        reason: String,
    },
}

/// Why a client couldn't reach a holder.
#[derive(Debug, Error)]
pub enum ConnectError {
    /// The session has no folder.
    #[error("there's no held session {session}")]
    NotFound {
        /// The session.
        session: SessionId,
    },
    /// The session has a folder but its holder never got going.
    #[error("the holder for session {session} never started")]
    NotStarted {
        /// The session.
        session: SessionId,
    },
    /// The holder turned the client away.
    #[error("the holder refused the connection: {message}")]
    Refused {
        /// What it said.
        message: String,
    },
    /// The holder said something this build can't read.
    #[error("the holder sent something unexpected: {0}")]
    Protocol(String),
    /// A file or the socket failed.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Why a write, stop or forget didn't happen.
#[derive(Debug, Error)]
pub enum WriteError {
    /// The CLI has ended or its stdin is closed, so there's nothing to
    /// write to.
    #[error("the agent has ended")]
    Finished,
    /// The CLI is still running, so its record can't be forgotten.
    #[error("the agent is still running")]
    StillRunning,
    /// The line has a newline in it, which would make it two lines.
    #[error("a line can't contain a newline")]
    BadLine,
    /// The connection to the holder is gone. Connect again.
    #[error("lost the connection to the holder")]
    Disconnected,
    /// Writing to the CLI or removing the files failed.
    #[error("{0}")]
    Io(String),
}

/// The error for a missing home folder.
pub(crate) fn no_home() -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        "there's no home folder to keep holders in",
    )
}
