// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Linux's holds, and the kernel's list of power supplies for the
//! charger.
//!
//! The system's hold is a logind inhibitor that a `systemd-inhibit` child
//! keeps for as long as the hold lasts, so `maxd` never links D-Bus. The
//! child runs `cat` on a pipe from this process, so if the process dies
//! without letting go, `cat` reads the end of its input, exits, and the
//! inhibitor goes with it. logind's default `LidSwitchIgnoreInhibited=yes`
//! means closing the lid still sleeps the machine, as on a Mac.
//!
//! The display's hold, with the `display` feature, asks over D-Bus, since
//! the app's switch must know at once whether it was taken.

use std::ffi::OsStr;
use std::io::{self, Read};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use rustix::process::{waitid, Pid, WaitId, WaitIdOptions};
use tracing::{debug, warn};

use super::supplies::{on_charger_in, SUPPLIES};
use super::Keep;

#[cfg(feature = "display")]
mod display;

/// Whether a hold that failed after it started was logged as a warning
/// already. Later ones only show in the debug log, since they'd fail the
/// same way each time.
static WARNED: AtomicBool = AtomicBool::new(false);

/// A held hold. Dropping it lets go.
pub enum Hold {
    System(#[allow(dead_code)] Inhibitor),
    #[cfg(feature = "display")]
    Display(#[allow(dead_code)] display::Inhibitors),
}

pub const fn supports(keep: Keep) -> bool {
    matches!(keep, Keep::System) || cfg!(feature = "display")
}

pub fn take(keep: Keep, reason: &str) -> io::Result<Hold> {
    match keep {
        Keep::System => {
            let why = format!("--why={reason}");
            inhibit(
                "systemd-inhibit",
                [
                    "--what=idle:sleep",
                    "--who=MaxHQ",
                    &why,
                    "--mode=block",
                    "cat",
                ],
            )
            .map(Hold::System)
        }
        #[cfg(feature = "display")]
        Keep::Display => display::take(reason).map(Hold::Display),
        #[cfg(not(feature = "display"))]
        Keep::Display => Err(crate::unsupported("keeping the display on")),
    }
}

pub fn on_charger() -> io::Result<bool> {
    Ok(on_charger_in(Path::new(SUPPLIES)))
}

/// Starts `program` with `args` to hold the inhibitor. A child that starts
/// and then fails, such as on a machine without logind, is logged when it
/// ends.
fn inhibit<I, S>(program: &str, args: I) -> io::Result<Inhibitor>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            io::Error::new(error.kind(), format!("couldn't run {program}: {error}"))
        })?;
    Ok(Inhibitor::watch(child))
}

/// The running `systemd-inhibit`, which holds the inhibitor until it's
/// dropped.
pub struct Inhibitor {
    /// Shared with the thread that reads its errors, which reaps it if it
    /// fails by itself.
    child: Arc<Mutex<Child>>,
    /// Its input. Closing it ends `cat`, and so the inhibitor.
    stdin: Option<ChildStdin>,
}

impl Inhibitor {
    /// Keeps `child`, with a thread that logs what it writes to stderr if
    /// it fails.
    fn watch(mut child: Child) -> Self {
        let stdin = child.stdin.take();
        let stderr = child.stderr.take();
        let pid = Pid::from_child(&child);
        let child = Arc::new(Mutex::new(child));
        let shared = child.clone();
        let reader = std::thread::Builder::new()
            .name("systemd-inhibit errors".into())
            .spawn(move || {
                let mut written = String::new();
                if let Some(mut stderr) = stderr {
                    // It only ends once the child and its `cat` are gone.
                    let _ = stderr.read_to_string(&mut written);
                }
                // Its stderr closes a moment before it can be reaped, so
                // this waits for that moment. It leaves the child for
                // `try_wait` and takes no lock, so a hold let go meanwhile
                // reaps it instead.
                let _ = waitid(
                    WaitId::Pid(pid),
                    WaitIdOptions::EXITED | WaitIdOptions::NOWAIT,
                );
                let status = shared
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .try_wait();
                if let Ok(Some(status)) = status
                    && !status.success()
                    && !written.trim().is_empty()
                {
                    refused(&format!("systemd-inhibit failed: {}", written.trim()));
                }
            });
        if let Err(error) = reader {
            debug!(%error, "couldn't watch systemd-inhibit for errors");
        }
        Self { child, stdin }
    }

    /// The child's process ID.
    #[cfg(test)]
    fn id(&self) -> u32 {
        self.child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .id()
    }
}

impl Drop for Inhibitor {
    fn drop(&mut self) {
        // Closing the input first ends `cat` even if the kill fails.
        drop(self.stdin.take());
        let mut child = self.child.lock().unwrap_or_else(PoisonError::into_inner);
        // Killing `systemd-inhibit` closes the inhibitor at once. It may
        // have ended already, which is fine.
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Logs that the system let go of a hold it took, as a warning the first
/// time.
fn refused(reason: &str) {
    if WARNED.swap(true, Ordering::Relaxed) {
        debug!(%reason, "Linux refused to keep this computer awake");
    } else {
        warn!(%reason, "Linux refused to keep this computer awake");
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    /// Whether a process with `pid` is still around, zombies included.
    fn exists(pid: u32) -> bool {
        Path::new(&format!("/proc/{pid}")).exists()
    }

    /// Waits up to five seconds for `done`.
    fn eventually(mut done: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        done()
    }

    #[test]
    fn the_child_is_reaped_when_let_go() {
        let _forks = crate::forks();
        let held = inhibit("cat", Vec::<&str>::new()).unwrap();
        let pid = held.id();
        assert!(exists(pid));
        drop(held);
        // Dropping it waited for it, so not even a zombie is left.
        assert!(!exists(pid));
    }

    #[test]
    fn a_missing_program_holds_nothing() {
        let _forks = crate::forks();
        let err = inhibit("/nonexistent/systemd-inhibit", Vec::<&str>::new())
            .err()
            .unwrap();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(
            err.to_string()
                .starts_with("couldn't run /nonexistent/systemd-inhibit: "),
            "{err}"
        );
    }

    #[test]
    fn a_child_that_fails_by_itself_is_reaped() {
        let _forks = crate::forks();
        let held = inhibit(
            "sh",
            ["-c", "echo 'Failed to inhibit: Access denied' >&2; exit 1"],
        )
        .unwrap();
        let pid = held.id();
        assert!(eventually(|| !exists(pid)), "{pid} wasn't reaped");
        drop(held);
    }
}
