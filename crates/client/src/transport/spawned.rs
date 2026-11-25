// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A stream through a program this machine starts, which runs
//! `maxd proxy` somewhere: its stdin and stdout are the stream, as `ssh`'s
//! are for an SSH route, and its stderr says why it ended. `wsl.exe` is
//! the one for a WSL distro.

use std::io;
use std::process::Stdio;

use super::{Conn, DialError, Rest, StderrTail};
use crate::wsl;

/// Starts `program` with `args` and opens a stream through it. The
/// program ends when the stream does. It starts with no window, since the
/// app has none to share.
pub(crate) fn dial_spawned(program: &str, args: &[String]) -> Result<Conn, DialError> {
    let mut command = std::process::Command::new(program);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    maxhq_os::launcher::without_window(&mut command);
    let mut command = tokio::process::Command::from(command);
    command.kill_on_drop(true);
    let mut child = command.spawn().map_err(|err| {
        DialError::refused(match err.kind() {
            io::ErrorKind::NotFound => format!("{program} isn't on this machine"),
            _ => format!("{program} didn't start: {err}"),
        })
    })?;
    let (Some(stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return Err(DialError::refused(format!("{program} gave no pipes")));
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

/// Opens a stream to the `maxd` in WSL distro `distro` on this machine.
pub(crate) fn dial_wsl(distro: &str) -> Result<Conn, DialError> {
    if !cfg!(windows) {
        return Err(DialError::refused("WSL runs only on Windows"));
    }
    if !wsl::is_distro_name(distro) {
        return Err(DialError::refused("that isn't a WSL distro's name"));
    }
    dial_spawned(wsl::PROGRAM, &wsl::proxy_args(distro)).map_err(|err| {
        if err.reason.contains("isn't on this machine") {
            DialError::refused("WSL isn't installed on this PC")
        } else {
            err
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_program_that_isnt_there_is_refused() {
        let err = dial_spawned("maxhq-test-no-such-program", &[])
            .err()
            .unwrap();
        assert_eq!(
            err,
            DialError::refused("maxhq-test-no-such-program isn't on this machine")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn only_windows_dials_wsl() {
        assert_eq!(
            dial_wsl("Ubuntu").err().unwrap(),
            DialError::refused("WSL runs only on Windows")
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_distro_name_that_could_be_a_flag_is_refused() {
        assert_eq!(
            dial_wsl("--help").err().unwrap(),
            DialError::refused("that isn't a WSL distro's name")
        );
    }

    /// The stream is the program's stdin and stdout, and its stderr is
    /// why it ended.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_stream_runs_through_the_program() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let script = "read line; echo \"got $line\"; echo bye >&2";
        let mut conn = dial_spawned("sh", &["-c".to_owned(), script.to_owned()]).unwrap();
        conn.writer.write_all(b"hello\n").await.unwrap();
        let mut out = String::new();
        conn.reader.read_to_string(&mut out).await.unwrap();
        assert_eq!(out, "got hello\n");
        assert_eq!(conn.rest.closed_reason().await, "bye");
    }
}
