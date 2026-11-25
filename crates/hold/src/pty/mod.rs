// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The terminal a holder's shell or `!` run gets: a pty on Unix, a
//! pseudoconsole (ConPTY) on Windows, behind [`spawn`] and [`Term`].
//!
//! A pty says how the program reads its input and which process group has
//! the foreground. A pseudoconsole says neither: it keeps no termios and
//! names no foreground process. So on Windows [`Term::password_mode`] and
//! [`Term::key_mode`] are false and [`Term::foreground`] is `None`, and the
//! holder learns about a password prompt or a busy shell only from the
//! screen and the shell's own marks.
//!
//! On Unix the pty's own calls stay public too, for the guest's SSH server.

use std::io;
use std::process::Command;

use crate::tree::{Group, Tree};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(unix)]
pub use unix::{controlling_tty, foreground, key_mode, open, password_mode, winsize, Master};
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

/// Why a program didn't start on a terminal of its own.
#[derive(Debug)]
pub(crate) enum Failed {
    /// The terminal couldn't be opened.
    Pty(io::Error),
    /// The program didn't start.
    Start(io::Error),
}

/// Starts `command` on a new terminal of `cols` by `rows`, in a tree of its
/// own. Only the program, its arguments, folder and environment count: the
/// terminal is its stdin, stdout and stderr.
pub(crate) fn spawn(command: Command, cols: u16, rows: u16) -> Result<(Tree, Term), Failed> {
    imp::spawn(command, cols, rows).map(|(tree, term)| (tree, Term(term)))
}

/// The holder's end of a program's terminal.
pub(crate) struct Term(imp::Term);

impl Term {
    /// Reads some output. Zero once the terminal has closed.
    pub(crate) async fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf).await
    }

    /// Writes all of `data` as input.
    pub(crate) async fn write_all(&self, data: &[u8]) -> io::Result<()> {
        self.0.write_all(data).await
    }

    /// Sets the window size.
    pub(crate) fn resize(&self, cols: u16, rows: u16) {
        imp::resize(&self.0, cols, rows);
    }

    /// Whether the program reads a line without echoing it, as at a
    /// password prompt.
    pub(crate) fn password_mode(&self) -> bool {
        self.0.password_mode()
    }

    /// Whether the program reads each key as it comes rather than a line.
    pub(crate) fn key_mode(&self) -> bool {
        self.0.key_mode()
    }

    /// The process group in the terminal's foreground, if it says.
    pub(crate) fn foreground(&self) -> Option<Group> {
        self.0.foreground()
    }

    /// Tells the terminal its program has gone, or should. A pseudoconsole
    /// keeps its output open after the program ends, until it's closed, so
    /// the holder calls this once the program is reaped and when it kills
    /// it. A pty ends by itself, so on Unix it does nothing. Calling it
    /// again does nothing either.
    pub(crate) fn hang_up(&self) {
        imp::hang_up(&self.0);
    }
}
