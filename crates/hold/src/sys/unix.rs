// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::fs::{self, File};
use std::io;
use std::os::fd::AsFd;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;

/// The file that reads as empty and takes any write.
pub(crate) const NULL: &str = "/dev/null";

/// Starts a new session with setsid, which takes the holder out of the
/// spawner's process group and away from any terminal. On Linux it also
/// becomes a child subreaper, so processes the CLI leaves behind come back
/// to it rather than to init. It outlives whatever folder it was started
/// in, so it keeps none of them busy.
pub(crate) fn leave_session() -> Result<(), String> {
    if let Err(err) = rustix::process::setsid() {
        return Err(format!("couldn't start a new session: {err}"));
    }
    #[cfg(target_os = "linux")]
    if let Err(err) = rustix::process::set_child_subreaper(Some(rustix::process::getpid())) {
        eprintln!("couldn't become a child subreaper: {err}");
    }
    let _ = std::env::set_current_dir("/");
    Ok(())
}

/// Runs the second step. Init or the nearest subreaper waits for it once
/// the first exits.
pub(crate) fn spawn_second(command: &mut Command) -> io::Result<()> {
    command.spawn().map(drop)
}

/// Whether the holder is still in its spawner's job, which Unix has none
/// of.
pub(crate) fn in_spawner_job() -> bool {
    false
}

/// Keeps a console window from showing for the holder, which Unix never
/// shows.
pub(crate) fn hide_window(_command: &mut tokio::process::Command) {}

/// Whether `path` names the file `file` has open.
pub(crate) fn same_file(file: &File, path: &Path) -> io::Result<bool> {
    let open = file.metadata()?;
    let there = fs::metadata(path)?;
    Ok(there.ino() == open.ino() && there.dev() == open.dev())
}

/// A copy of the holder's stdin, to read from.
pub(crate) fn stdin_file() -> io::Result<File> {
    io::stdin().as_fd().try_clone_to_owned().map(File::from)
}

/// Points stdin at `file`.
pub(crate) fn set_stdin(file: &File) {
    let _ = rustix::stdio::dup2_stdin(file);
}

/// Points stdout and stderr at `file`.
pub(crate) fn set_output(file: &File) {
    let _ = rustix::stdio::dup2_stdout(file);
    let _ = rustix::stdio::dup2_stderr(file);
}

/// What tells the holder to go: a SIGTERM or a SIGINT. The holder has no
/// terminal of its own, so a hangup means nothing and is let pass.
pub(crate) struct Quit {
    term: tokio::signal::unix::Signal,
    int: tokio::signal::unix::Signal,
    hup: tokio::signal::unix::Signal,
}

impl Quit {
    /// Starts watching. Must run inside the runtime.
    pub(crate) fn watch() -> io::Result<Self> {
        use tokio::signal::unix::{signal, SignalKind};
        Ok(Self {
            term: signal(SignalKind::terminate())?,
            int: signal(SignalKind::interrupt())?,
            hup: signal(SignalKind::hangup())?,
        })
    }

    /// Waits for the next signal to go.
    pub(crate) async fn next(&mut self) {
        loop {
            tokio::select! {
                _ = self.term.recv() => return,
                _ = self.int.recv() => return,
                _ = self.hup.recv() => {}
            }
        }
    }
}

/// An OS string's bytes, which on Unix are whatever the OS holds.
pub(crate) fn os_bytes(text: &std::ffi::OsStr) -> std::borrow::Cow<'_, [u8]> {
    use std::os::unix::ffi::OsStrExt;
    text.as_bytes().into()
}

/// The OS string [`os_bytes`] gave `bytes` for. Any bytes make one.
pub(crate) fn os_from_bytes(bytes: Vec<u8>) -> Option<std::ffi::OsString> {
    use std::os::unix::ffi::OsStringExt;
    Some(std::ffi::OsString::from_vec(bytes))
}
