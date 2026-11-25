// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Finds the program a name runs, as a shell would: through `PATH`, then
//! folders the caller adds. Only absolute folders count, since a relative
//! one would depend on where the process started.
//!
//! On Windows a name runs through `PATHEXT`, a `.exe` first, and [`launch`]
//! looks through an npm `.cmd` shim to the program it starts. A `.cmd`
//! runs through `cmd.exe`, which refuses or mangles arguments it can't
//! quote, such as JSON or a prompt.
//!
//! The rules are pure functions over an [`Env`] and an [`Fs`] view, so both
//! systems' rules are tested on either. `unix.rs` and `windows.rs` here are
//! those rules, not system calls, and both build everywhere.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

mod unix;
mod windows;

/// Whose rules find a program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rules {
    /// `PATH` split at `:`, and a file with an execute bit.
    Unix,
    /// `PATH` split at `;`, `PATHEXT`, and npm shims looked through.
    Windows,
}

impl Rules {
    /// The rules of the system this runs on.
    pub const HOST: Self = if cfg!(windows) {
        Self::Windows
    } else {
        Self::Unix
    };
}

/// The environment variables the search reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Env {
    /// `PATH`.
    pub path: Option<OsString>,
    /// `PATHEXT`, which only Windows reads. Without it, Windows's default.
    pub pathext: Option<OsString>,
}

impl Env {
    /// This process's environment.
    pub fn current() -> Self {
        Self {
            path: std::env::var_os("PATH"),
            pathext: std::env::var_os("PATHEXT"),
        }
    }
}

/// The files the search may look at.
pub trait Fs {
    /// The mode bits of the file at `path`, or `None` when no file is
    /// there. A folder is no file. Windows rules ignore the bits.
    fn file_mode(&self, path: &Path) -> Option<u32>;

    /// The text of the small file at `path`, such as a shim, or `None`
    /// when it can't be read or is too large to be one.
    fn read_text(&self, path: &Path) -> Option<String>;
}

/// The real file system.
#[derive(Clone, Copy, Debug, Default)]
pub struct Host;

/// A shim is a few lines. Anything larger isn't one.
const SHIM_MAX: u64 = 64 * 1024;

impl Fs for Host {
    fn file_mode(&self, path: &Path) -> Option<u32> {
        let meta = fs::metadata(path).ok()?;
        meta.is_file().then(|| crate::fsmode::mode_of(&meta))
    }

    fn read_text(&self, path: &Path) -> Option<String> {
        let mut text = String::new();
        fs::File::open(path)
            .ok()?
            .take(SHIM_MAX + 1)
            .read_to_string(&mut text)
            .ok()?;
        (text.len() as u64 <= SHIM_MAX).then_some(text)
    }
}

/// How to start a program [`find`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    /// The program to start.
    pub program: PathBuf,
    /// Arguments that go before the caller's own, such as the script an
    /// npm shim hands to `node.exe`.
    pub args: Vec<OsString>,
}

impl Launch {
    /// Starts `program` as it is.
    pub fn direct(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
        }
    }

    /// A command that starts it, for the caller to add its own arguments
    /// to.
    pub fn command(&self) -> std::process::Command {
        let mut command = std::process::Command::new(&self.program);
        command.args(&self.args);
        command
    }
}

/// The program called `name`, from an absolute folder on `PATH`, then from
/// `extra` in order.
pub fn find(name: &str, extra: &[PathBuf]) -> Option<PathBuf> {
    find_in(Rules::HOST, name, extra, &Env::current(), &Host)
}

/// [`find`] under `rules`, over `env` and `fs`.
pub fn find_in(
    rules: Rules,
    name: &str,
    extra: &[PathBuf],
    env: &Env,
    fs: &impl Fs,
) -> Option<PathBuf> {
    let on_path = env
        .path
        .as_deref()
        .map(|path| split_paths(rules, path))
        .unwrap_or_default();
    on_path
        .into_iter()
        .chain(extra.iter().cloned())
        .filter(|dir| is_absolute(rules, dir))
        .flat_map(|dir| candidates(rules, &dir, name, env))
        .find(|file| runnable_in(rules, file, env, fs))
}

/// Whether `path` is a program you may run.
pub fn runnable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| crate::fsmode::is_executable(path, &meta))
}

/// Whether the text `path` is absolute by either system's rules: Unix's
/// `/srv/app`, or Windows' `C:\work` or `\\server\share`. For a path that
/// names a place on another device, whose system isn't known here; that
/// device reads it again by its own rules.
pub fn absolute_anywhere(path: &str) -> bool {
    path.starts_with('/') || windows::is_absolute(Path::new(path))
}

/// [`runnable`] under `rules`, over `env` and `fs`.
pub fn runnable_in(rules: Rules, path: &Path, env: &Env, fs: &impl Fs) -> bool {
    fs.file_mode(path)
        .is_some_and(|mode| runs(rules, path, mode, env.pathext.as_deref()))
}

/// How to start `found`, a program [`find`] returned. On Windows an npm
/// shim becomes the program it names: a native `.exe` as it is, or a
/// script through `node.exe`, from beside the shim or else from `PATH`.
/// Anything else, or a shim that can't be read, starts as it is.
pub fn launch(found: &Path) -> Launch {
    launch_in(Rules::HOST, found, &Env::current(), &Host)
}

/// [`launch`] under `rules`, over `env` and `fs`.
pub fn launch_in(rules: Rules, found: &Path, env: &Env, fs: &impl Fs) -> Launch {
    match rules {
        Rules::Unix => Launch::direct(found),
        Rules::Windows => windows::launch(found, env, fs),
    }
}

/// Whether a file at `path` with mode `mode` runs here. What
/// [`crate::fsmode::is_executable`] asks.
pub(crate) fn host_runs(path: &Path, mode: u32) -> bool {
    let pathext = if cfg!(windows) {
        std::env::var_os("PATHEXT")
    } else {
        None
    };
    runs(Rules::HOST, path, mode, pathext.as_deref())
}

fn runs(rules: Rules, path: &Path, mode: u32, pathext: Option<&OsStr>) -> bool {
    match rules {
        Rules::Unix => unix::runs(mode),
        Rules::Windows => windows::runs(path, pathext),
    }
}

fn split_paths(rules: Rules, path: &OsStr) -> Vec<PathBuf> {
    match rules {
        Rules::Unix => unix::split_paths(path),
        Rules::Windows => windows::split_paths(path),
    }
}

fn is_absolute(rules: Rules, dir: &Path) -> bool {
    match rules {
        Rules::Unix => unix::is_absolute(dir),
        Rules::Windows => windows::is_absolute(dir),
    }
}

fn candidates(rules: Rules, dir: &Path, name: &str, env: &Env) -> Vec<PathBuf> {
    match rules {
        Rules::Unix => vec![dir.join(name)],
        Rules::Windows => windows::candidates(dir, name, env.pathext.as_deref()),
    }
}

#[cfg(test)]
mod tests;
