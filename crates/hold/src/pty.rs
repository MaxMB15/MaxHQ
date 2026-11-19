// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Pseudo-terminals: the guest's SSH server opens one for a session that
//! asks for it, and a terminal holder opens one for each shell or `!` run.

use std::fs::OpenOptions;
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;

use rustix::process::Pid;
use rustix::termios::{LocalModes, Winsize};
use tokio::io::unix::AsyncFd;

/// Has the forked process lead a session of its own with the pty on its
/// standard input as the session's terminal.
#[allow(unsafe_code)]
pub fn controlling_tty(process: &mut std::process::Command) {
    // SAFETY: `setsid` and the `TIOCSCTTY` ioctl are plain system calls,
    // which are safe in a forked process, and fd 0 is the pty there.
    unsafe {
        process.pre_exec(|| {
            rustix::process::setsid()?;
            let stdin = std::os::fd::BorrowedFd::borrow_raw(0);
            rustix::process::ioctl_tiocsctty(stdin)?;
            Ok(())
        });
    }
}

/// A new pty of `size`: its master and the terminal end.
pub fn open(size: Winsize) -> io::Result<(OwnedFd, OwnedFd)> {
    use rustix::pty::{grantpt, openpt, ptsname, unlockpt, OpenptFlags};

    #[cfg(target_os = "linux")]
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)?;
    // macOS can't open the master closed on exec, so it's marked after.
    #[cfg(not(target_os = "linux"))]
    let master = {
        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)?;
        rustix::io::fcntl_setfd(&master, rustix::io::FdFlags::CLOEXEC)?;
        master
    };
    grantpt(&master)?;
    unlockpt(&master)?;
    let name = ptsname(&master, Vec::new())?;
    let slave = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY)
        .open(Path::new(std::ffi::OsStr::from_bytes(name.as_bytes())))?;
    rustix::termios::tcsetwinsize(&master, size)?;
    Ok((master, slave.into()))
}

/// A window size from a client's numbers, which a pty keeps as 16 bits.
pub fn winsize(cols: u32, rows: u32, width: u32, height: u32) -> Winsize {
    let clamp = |value: u32| u16::try_from(value).unwrap_or(u16::MAX);
    Winsize {
        ws_row: clamp(rows),
        ws_col: clamp(cols),
        ws_xpixel: clamp(width),
        ws_ypixel: clamp(height),
    }
}

/// Whether the program at the pty reads a line without echoing it, as
/// `sudo`, `ssh` and `read -s` do at a password prompt. False when the
/// terminal's modes can't be read.
pub fn password_mode(fd: impl AsFd) -> bool {
    rustix::termios::tcgetattr(fd).is_ok_and(|modes| {
        let local = modes.local_modes;
        local.contains(LocalModes::ICANON) && !local.contains(LocalModes::ECHO)
    })
}

/// Whether the program at the pty reads each key as it comes rather than
/// a line, as a shell's line editor, `tmux` or a full-screen program does.
/// False when the terminal's modes can't be read.
pub fn key_mode(fd: impl AsFd) -> bool {
    rustix::termios::tcgetattr(fd)
        .is_ok_and(|modes| !modes.local_modes.contains(LocalModes::ICANON))
}

/// The process group in the pty's foreground, if any. A shell at its
/// prompt is its own foreground group, and a command it runs takes the
/// foreground while it runs.
pub fn foreground(fd: impl AsFd) -> Option<Pid> {
    rustix::termios::tcgetpgrp(fd).ok()
}

/// A pty's master, read and written without blocking.
pub struct Master(AsyncFd<OwnedFd>);

impl Master {
    /// Takes a master from [`open`] for the async reads and writes.
    #[allow(unsafe_code)]
    pub fn new(fd: OwnedFd) -> io::Result<Self> {
        rustix::io::ioctl_fionbio(&fd, true)?;
        // SAFETY: the master is a file descriptor this owns, which nothing
        // else closes or replaces while it's registered.
        let fd = unsafe { AsyncFd::register(fd) }.map_err(|err| err.into_parts().1)?;
        Ok(Self(fd))
    }

    /// Reads some output. Zero once every terminal end has closed, which
    /// Linux reports as an I/O error.
    pub async fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut ready = self.0.readable().await?;
            let read = ready.try_io(|fd| match rustix::io::read(fd.get_ref(), &mut *buf) {
                Err(rustix::io::Errno::IO) => Ok(0),
                read => read.map_err(io::Error::from),
            });
            if let Ok(read) = read {
                return read;
            }
        }
    }

    /// Writes all of `data` as input.
    pub async fn write_all(&self, mut data: &[u8]) -> io::Result<()> {
        while !data.is_empty() {
            let mut ready = self.0.writable().await?;
            let wrote = ready.try_io(|fd| Ok(rustix::io::write(fd.get_ref(), data)?));
            if let Ok(wrote) = wrote {
                data = &data[wrote?..];
            }
        }
        Ok(())
    }

    /// Sets the window size, which signals the foreground group.
    pub fn resize(&self, size: Winsize) {
        let _ = rustix::termios::tcsetwinsize(self.0.get_ref().as_fd(), size);
    }

    /// Whether the program at the pty waits for a password, as
    /// [`password_mode`] says.
    pub fn password_mode(&self) -> bool {
        password_mode(self.0.get_ref())
    }

    /// Whether the program at the pty reads keys, as [`key_mode`] says.
    pub fn key_mode(&self) -> bool {
        key_mode(self.0.get_ref())
    }

    /// The process group in the pty's foreground, as [`foreground`] says.
    pub fn foreground(&self) -> Option<Pid> {
        foreground(self.0.get_ref())
    }
}

#[cfg(test)]
mod tests {
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    use super::*;

    /// Starts `script` under `sh` on a new pty and returns its master. The
    /// shell gets the tests' own `PATH`, since NixOS has no `stty` in
    /// `/usr/bin` or `/bin`.
    fn start(script: &str) -> (Master, tokio::process::Child) {
        let (master, slave) = open(winsize(80, 24, 0, 0)).unwrap();
        let path = std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
        let mut process = std::process::Command::new("/bin/sh");
        process
            .arg("-c")
            .arg(script)
            .env_clear()
            .env("PATH", path)
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave));
        controlling_tty(&mut process);
        let child = tokio::process::Command::from(process).spawn().unwrap();
        (Master::new(master).unwrap(), child)
    }

    /// Reads from `master` until `want` shows up in what it printed.
    async fn read_until(master: &Master, want: &str) -> String {
        let mut seen = Vec::new();
        let mut buf = [0; 4096];
        let deadline = Instant::now() + Duration::from_secs(10);
        while !String::from_utf8_lossy(&seen).contains(want) {
            assert!(Instant::now() < deadline, "no {want:?} in {seen:?}");
            let read = tokio::time::timeout(Duration::from_secs(10), master.read(&mut buf))
                .await
                .expect("output in time")
                .unwrap();
            assert_ne!(read, 0, "the pty closed before {want:?}: {seen:?}");
            seen.extend_from_slice(&buf[..read]);
        }
        String::from_utf8_lossy(&seen).into_owned()
    }

    /// Waits for `child` to end, reading what it prints meanwhile: on macOS
    /// a process can't finish closing its terminal while output is unread.
    /// Reading stops at the end of the output. Past it a read returns at
    /// once, and a loop that never waits gives the runtime no turn to see
    /// the child end, which on Linux it learns from a pidfd.
    async fn finish(master: &Master, child: &mut tokio::process::Child) {
        let mut buf = [0; 4096];
        let mut open = true;
        let wait = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                tokio::select! {
                    status = child.wait() => return status.unwrap(),
                    read = master.read(&mut buf), if open => {
                        open = matches!(read, Ok(read) if read > 0);
                    }
                }
            }
        });
        wait.await.expect("the child ended in time");
    }

    #[tokio::test]
    async fn a_prompt_that_hides_input_is_password_mode() {
        let (master, mut child) =
            start("echo plain; read a; stty -echo; echo hidden; read x; stty echo; echo done");
        read_until(&master, "plain").await;
        // A shell's `read` takes a line with echo on: not a password.
        assert!(!master.password_mode());
        master.write_all(b"one\n").await.unwrap();
        read_until(&master, "hidden").await;
        assert!(master.password_mode(), "stty -echo then read hides input");
        master.write_all(b"two\n").await.unwrap();
        read_until(&master, "done").await;
        assert!(!master.password_mode());
        finish(&master, &mut child).await;
    }

    #[tokio::test]
    async fn a_program_reading_keys_is_key_mode() {
        let (master, mut child) =
            start("echo lines; read a; stty -icanon; echo keys; read b; stty icanon");
        read_until(&master, "lines").await;
        assert!(!master.key_mode());
        master.write_all(b"one\n").await.unwrap();
        read_until(&master, "keys").await;
        assert!(master.key_mode());
        assert!(!master.password_mode());
        master.write_all(b"two\n").await.unwrap();
        finish(&master, &mut child).await;
    }

    #[tokio::test]
    async fn a_command_takes_the_foreground_from_its_shell() {
        let (master, mut child) = start("echo up; read a");
        read_until(&master, "up").await;
        let shell = Pid::from_raw(child.id().unwrap().cast_signed()).unwrap();
        assert_eq!(master.foreground(), Some(shell));
        master.write_all(b"\n").await.unwrap();
        finish(&master, &mut child).await;
    }

    #[tokio::test]
    async fn resizing_reaches_the_terminal() {
        let (master, mut child) = start("read a; stty size");
        master.resize(winsize(132, 50, 0, 0));
        master.write_all(b"\n").await.unwrap();
        read_until(&master, "50 132").await;
        finish(&master, &mut child).await;
    }

    #[test]
    fn sizes_clamp_to_sixteen_bits() {
        let size = winsize(70_000, 40, 0, 1 << 20);
        assert_eq!(
            (size.ws_col, size.ws_row, size.ws_xpixel, size.ws_ypixel),
            (u16::MAX, 40, 0, u16::MAX)
        );
    }
}
