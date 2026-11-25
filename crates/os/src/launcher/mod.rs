// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The launcher a Windows logon task starts `maxd` through, which plays
//! the part launchd and systemd play on Unix: it starts `maxd run`, sends
//! its output to the log, and starts it again when it stops.
//!
//! - It's a windowless copy of `maxd.exe` ([`windowless_copy`]), so the
//!   task opens no console window, and it starts `maxd` with a console
//!   that has none ([`supervise`]), so neither `maxd` nor what it runs
//!   flashes one.
//! - It ties what it starts to itself ([`tie_children`]), so ending the
//!   task ends `maxd` too. What may outlive `maxd`, as a held agent does,
//!   leaves on purpose.
//! - `maxd` exiting with [`Plan::restart_code`] is started again at once,
//!   with 0 it stays stopped, and anything else is a crash, started again
//!   after a pause that grows while it keeps crashing.
//! - Running the task while `maxd` serves restarts it: a second launcher
//!   asks the running `maxd` to restart, leaves it to the launcher that
//!   runs it, and takes over should none.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::ipc::{self, Found};
use crate::stop::{self, Request};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

/// What the launcher runs, and how.
#[derive(Clone, Debug)]
pub struct Plan {
    /// The socket of the `maxd` it runs, which tells whether one serves.
    pub socket: PathBuf,
    /// The program, `maxd.exe`.
    pub program: PathBuf,
    /// Its arguments, `run`.
    pub args: Vec<String>,
    /// The log its output goes to.
    pub log: PathBuf,
    /// How big the log grows before it's moved aside, to the same name
    /// with `.1` after it, when the program next starts.
    pub log_limit: u64,
    /// The exit code that asks to be started again at once.
    pub restart_code: i32,
    /// How long it waits before starting a program that crashed.
    pub backoff: Backoff,
}

/// The pause before starting a program that crashed: `first`, doubling
/// each crash up to `max`, and back to `first` once a run lasts
/// `reset_after`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backoff {
    /// The first pause.
    pub first: Duration,
    /// The longest pause.
    pub max: Duration,
    /// How long a run lasts to count as a good one.
    pub reset_after: Duration,
}

impl Backoff {
    /// The service's: 2 seconds, up to a minute.
    pub const SERVICE: Self = Self {
        first: Duration::from_secs(2),
        max: Duration::from_secs(60),
        reset_after: Duration::from_secs(60),
    };
}

/// How long a second launcher waits for the `maxd` it asked to restart to
/// stop.
const STOP_WAIT: Duration = Duration::from_secs(30);

/// How long it then waits for that `maxd`'s launcher to start it again.
const HANDOVER_WAIT: Duration = Duration::from_secs(10);

/// How often it looks while it waits.
const POLL: Duration = Duration::from_millis(100);

/// How many times in a row the program may fail to start before the
/// launcher gives up, as it does when the program is gone.
const START_TRIES: u32 = 10;

/// Puts this process in a job that ends with it, so all it starts ends
/// when it does, unless it leaves the job on purpose. Windows only.
pub fn tie_children() -> io::Result<()> {
    imp::tie_children()
}

/// Starts `command` with a console that has no window, so a console
/// program a windowed app starts flashes none. Nothing on Unix.
pub fn without_window(command: &mut Command) {
    imp::without_window(command);
}

/// Runs the plan's program until it stops for good, or another launcher
/// runs it. See the module's notes.
pub fn supervise(plan: &Plan) -> io::Result<()> {
    if ipc::probe(&plan.socket)? == Found::Listening && hand_over(plan) {
        return Ok(());
    }
    let mut pause = plan.backoff.first;
    let mut failed_starts = 0;
    loop {
        // A `maxd` that serves now is one another launcher started, and
        // a second would find its socket taken.
        if ipc::probe(&plan.socket)? == Found::Listening {
            note(
                plan,
                "another maxd serves already, so this launcher leaves it be",
            );
            return Ok(());
        }
        let started = Instant::now();
        let exit = match start(plan).and_then(|mut child| child.wait()) {
            Ok(status) => {
                failed_starts = 0;
                status.code()
            }
            Err(err) => {
                failed_starts += 1;
                note(
                    plan,
                    &format!("couldn't start {}: {err}", plan.program.display()),
                );
                if failed_starts >= START_TRIES {
                    return Err(err);
                }
                None
            }
        };
        match after(exit, started.elapsed(), pause, plan) {
            Next::Stop => return Ok(()),
            Next::Again => pause = plan.backoff.first,
            Next::Wait(wait) => {
                note(
                    plan,
                    &format!(
                        "maxd stopped ({}), so it starts again in {} s",
                        exit.map_or("no exit code".to_owned(), |code| format!(
                            "exit code {code}"
                        )),
                        wait.as_secs()
                    ),
                );
                std::thread::sleep(wait);
                pause = (wait * 2).min(plan.backoff.max);
            }
        }
    }
}

/// What the launcher does once the program has stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Next {
    /// Nothing more: it was asked to stop.
    Stop,
    /// Start it again now.
    Again,
    /// Start it again after this long.
    Wait(Duration),
}

/// What follows the program stopping with `exit` after running for `ran`,
/// when the pause would be `pause`.
fn after(exit: Option<i32>, ran: Duration, pause: Duration, plan: &Plan) -> Next {
    match exit {
        Some(0) => Next::Stop,
        Some(code) if code == plan.restart_code => Next::Again,
        _ if ran >= plan.backoff.reset_after => Next::Wait(plan.backoff.first),
        _ => Next::Wait(pause),
    }
}

/// Asks the `maxd` that serves to restart, and waits for its launcher to
/// start it again. False when there's none to, or it never asked.
fn hand_over(plan: &Plan) -> bool {
    let asked = crate::rt::block_on(stop::ask(&plan.socket, Request::Restart)).and_then(|pid| pid);
    let pid = match asked {
        Ok(pid) => pid,
        Err(err) => {
            note(
                plan,
                &format!("couldn't ask the maxd that serves to restart: {err}"),
            );
            return false;
        }
    };
    note(plan, &format!("asked maxd (process {pid}) to restart"));
    if !wait_for(STOP_WAIT, || !crate::proc::alive(pid)) {
        note(plan, &format!("maxd (process {pid}) didn't stop"));
        return false;
    }
    wait_for(HANDOVER_WAIT, || {
        ipc::probe(&plan.socket).is_ok_and(|found| found == Found::Listening)
    })
}

/// Waits up to `limit` for `done`, and says whether it came.
fn wait_for(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL);
    }
}

/// Starts the program, with the log as its output, once the log is moved
/// aside if it's grown too big.
fn start(plan: &Plan) -> io::Result<std::process::Child> {
    let log = open_log(plan)?;
    let mut command = Command::new(&plan.program);
    command
        .args(&plan.args)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    without_window(&mut command);
    command.spawn()
}

/// The log, open to add to, after moving it aside if it's past the
/// limit. A log that can't be moved is written to as it is.
fn open_log(plan: &Plan) -> io::Result<File> {
    if let Some(dir) = plan.log.parent() {
        fs::create_dir_all(dir)?;
    }
    if fs::metadata(&plan.log).is_ok_and(|meta| meta.len() > plan.log_limit) {
        let mut old = plan.log.as_os_str().to_owned();
        old.push(".1");
        let _ = fs::rename(&plan.log, PathBuf::from(old));
    }
    OpenOptions::new().create(true).append(true).open(&plan.log)
}

/// Adds a line of the launcher's own to the log.
fn note(plan: &Plan, line: &str) {
    if let Ok(mut log) = open_log(plan) {
        let _ = writeln!(log, "maxd launcher: {line}");
    }
}

/// The subsystem a console program has.
const CONSOLE: u16 = 3;

/// The subsystem a windowed program has, which Windows starts without a
/// console.
const WINDOWED: u16 = 2;

/// Marks the Windows program in `image` as windowed, as `editbin
/// /subsystem:windows` does: the subsystem field of its optional header
/// becomes 2. It fails for what isn't a console or windowed program. The
/// rest is left alone, so a signature no longer matches.
pub fn windowless(image: &mut [u8]) -> io::Result<()> {
    let bad = |why: &str| io::Error::new(io::ErrorKind::InvalidData, why.to_owned());
    let u16_at = |image: &[u8], at: usize| {
        image
            .get(at..at + 2)
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
    };
    if image.get(..2) != Some(b"MZ") {
        return Err(bad("this isn't a Windows program"));
    }
    let header = image
        .get(0x3c..0x40)
        .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        .and_then(|at| usize::try_from(at).ok())
        .ok_or_else(|| bad("this Windows program has no PE header"))?;
    if image.get(header..header + 4) != Some(b"PE\0\0") {
        return Err(bad("this Windows program has no PE header"));
    }
    // The signature, then the 20-byte file header.
    let optional = header + 24;
    match u16_at(image, optional) {
        Some(0x10b | 0x20b) => {}
        _ => return Err(bad("this Windows program has no optional header")),
    }
    let subsystem = optional + 68;
    match u16_at(image, subsystem) {
        Some(CONSOLE | WINDOWED) => {
            image[subsystem..subsystem + 2].copy_from_slice(&WINDOWED.to_le_bytes());
            Ok(())
        }
        Some(other) => Err(bad(&format!(
            "this Windows program is for subsystem {other}, not the console"
        ))),
        None => Err(bad("this Windows program's header is cut short")),
    }
}

/// Writes a [`windowless`] copy of `program` to `to`, unless one is there
/// already. The copy goes beside `to` and is renamed into place. Windows
/// won't replace a program that runs, but lets it be renamed, so a `to`
/// that runs is moved aside to the same name with `.old` after it, which
/// the next copy removes once it's stopped.
pub fn windowless_copy(program: &Path, to: &Path) -> io::Result<()> {
    let mut image = fs::read(program)?;
    windowless(&mut image)?;
    let beside = |suffix: &str| {
        let mut path = to.as_os_str().to_owned();
        path.push(suffix);
        PathBuf::from(path)
    };
    let (new, old) = (beside(".maxhq-new"), beside(".old"));
    // Gone once the launcher that ran it stopped.
    let _ = fs::remove_file(&old);
    if fs::read(to).is_ok_and(|there| there == image) {
        return Ok(());
    }
    if let Some(dir) = to.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&new, &image)?;
    let placed = fs::rename(&new, to).or_else(|err| {
        if err.kind() != io::ErrorKind::PermissionDenied {
            return Err(err);
        }
        fs::rename(to, &old)?;
        fs::rename(&new, to)
    });
    placed.inspect_err(|_| {
        let _ = fs::remove_file(&new);
    })
}

#[cfg(test)]
mod tests;
