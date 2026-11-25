// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! `maxd service` on Windows: a logon task of your own in Task Scheduler,
//! which needs no admin, and the launcher it runs.
//!
//! The task runs `bin\maxdw.exe launch "<maxd.exe>"` each time you sign
//! in. The launcher, a windowless copy of `maxd.exe`, runs `maxd run` and
//! starts it again, as launchd and systemd do (see
//! [`maxhq_os::launcher`]). Running the task again restarts `maxd`.
//!
//! Task Scheduler sends nothing when it ends a task, so stopping asks
//! `maxd` over its stop endpoint, waits for it, then ends the task, which
//! ends the launcher, and kills a `maxd` that still serves.
//!
//! `schtasks` prints in your Windows language, so only its exit codes are
//! read.

use std::io;
use std::path::Path;
use std::time::Duration;

use maxhq_core::fs::{write_atomic, Atomic};
use maxhq_core::service::{
    launcher_log, launcher_program, task_file, task_file_bytes, task_name, task_name_here,
    RESTART_EXIT,
};
use maxhq_os::ipc::LocalStream;
use maxhq_os::launcher::{self, Backoff, Plan};
use maxhq_os::rt::block_on;
use maxhq_os::stop::{self, Request};

use super::{command_error, must, Runner, ServiceError, Setup, GONE_TRIES, LOG_LIMIT};

/// What stopping `maxd` needs of the processes here, so tests can fake
/// them.
pub trait Processes {
    /// Asks the `maxd` serving at `socket` to stop, and gives its process
    /// ID once it has taken the request.
    fn ask_stop(&mut self, socket: &Path) -> io::Result<u32>;
    /// The process ID of the `maxd` serving at `socket`, if one does.
    fn serving(&mut self, socket: &Path) -> Option<u32>;
    /// Whether the process `pid` still runs.
    fn alive(&mut self, pid: u32) -> bool;
    /// Ends the process `pid` at once.
    fn kill(&mut self, pid: u32) -> io::Result<()>;
}

/// The processes here, for real.
pub struct SystemProcesses;

impl Processes for SystemProcesses {
    fn ask_stop(&mut self, socket: &Path) -> io::Result<u32> {
        block_on(stop::ask(socket, Request::Stop))?
    }

    fn serving(&mut self, socket: &Path) -> Option<u32> {
        // The stop endpoint, since a client that connects and says nothing
        // is dropped there without a word. Connecting checks that this
        // user serves it.
        let endpoint = stop::endpoint(socket);
        block_on(async move {
            let stream = LocalStream::connect(&endpoint).await?;
            stream
                .peer()?
                .pid
                .ok_or_else(|| io::Error::other("no process ID"))
        })
        .ok()?
        .ok()
    }

    fn alive(&mut self, pid: u32) -> bool {
        maxhq_os::proc::alive(pid)
    }

    fn kill(&mut self, pid: u32) -> io::Result<()> {
        maxhq_os::proc::stop(pid, maxhq_os::proc::Stop::Hard)
    }
}

/// Your Windows user's security ID, which names the task and its user.
fn sid() -> Result<String, ServiceError> {
    maxhq_os::home::user_sid().map_err(|source| ServiceError::Lookup {
        what: "your Windows user",
        source,
    })
}

/// Registers the logon task and starts it. Running it again replaces the
/// task and the launcher, and restarts `maxd`. Returns what to tell you.
pub fn install(setup: &Setup, runner: &mut dyn Runner) -> Result<Vec<String>, ServiceError> {
    install_as(
        setup,
        &sid()?,
        runner,
        &mut SystemProcesses,
        super::GONE_PAUSE,
    )
}

/// [`install`] for the user with the security ID `sid`, waiting `pause`
/// between checks that the old `maxd` has stopped.
pub(super) fn install_as(
    setup: &Setup,
    sid: &str,
    runner: &mut dyn Runner,
    processes: &mut dyn Processes,
    pause: Duration,
) -> Result<Vec<String>, ServiceError> {
    let app = setup.app_folder();
    let name = task_name(sid);
    let io = |path: &Path| {
        let path = path.to_owned();
        move |source| ServiceError::Io { path, source }
    };
    // What runs now goes first, so the launcher and the program written
    // here are the ones that run, and the launcher's file is free.
    stop_as(setup, &name, runner, processes, pause)?;
    std::fs::create_dir_all(&app).map_err(io(&app))?;
    let launcher = launcher_program(&app);
    launcher::windowless_copy(&setup.exe, &launcher).map_err(io(&launcher))?;
    let file = task_file(&app);
    let bytes = task_file_bytes(&setup.task_contents(sid));
    write_atomic(&file, &bytes, Atomic::PLAIN).map_err(io(&file))?;
    let file_arg = file.to_string_lossy();
    must(
        runner,
        "schtasks",
        &["/Create", "/TN", &name, "/XML", &file_arg, "/F"],
    )?;
    must(runner, "schtasks", &["/Run", "/TN", &name])?;
    Ok(vec![
        format!(
            "Installed the logon task \"{name}\", as {}.",
            file.display()
        ),
        format!(
            "maxd runs now and each time you sign in, and logs to {}.",
            setup.log_file().display()
        ),
    ])
}

/// Whether Task Scheduler has the task called `name`.
pub(super) fn registered(runner: &mut dyn Runner, name: &str) -> bool {
    runner
        .run("schtasks", &["/Query", "/TN", name])
        .is_ok_and(|ran| ran.success)
}

/// Stops `maxd` and its launcher until you next sign in, and says whether
/// anything ran.
pub fn stop(setup: &Setup, runner: &mut dyn Runner) -> Result<bool, ServiceError> {
    stop_as(
        setup,
        &task_name_here(),
        runner,
        &mut SystemProcesses,
        super::GONE_PAUSE,
    )
}

/// [`stop`] for the task called `name`, waiting `pause` between checks.
///
/// `maxd` is asked first, so it ends its links and leaves its holders
/// running, as it does when launchd or systemd stops it. One that doesn't
/// stop in time is killed. Ending the task then ends a launcher that's
/// left, as one waiting to start `maxd` again, and a `maxd` that still
/// serves is killed.
pub(super) fn stop_as(
    setup: &Setup,
    name: &str,
    runner: &mut dyn Runner,
    processes: &mut dyn Processes,
    pause: Duration,
) -> Result<bool, ServiceError> {
    let mut ran = false;
    if let Some(socket) = &setup.socket
        && let Ok(pid) = processes.ask_stop(socket)
    {
        ran = true;
        if !gone(processes, pid, pause) {
            let _ = processes.kill(pid);
        }
    }
    // Ending a task that doesn't run, or isn't there, fails, which is fine.
    let end = ["/End", "/TN", name];
    let _ = runner.run("schtasks", &end);
    if let Some(socket) = &setup.socket
        && let Some(pid) = processes.serving(socket)
    {
        ran = true;
        let _ = processes.kill(pid);
        if !gone(processes, pid, pause) {
            return Err(command_error(
                "schtasks",
                &end,
                &format!("maxd (process {pid}) was still running after it"),
            ));
        }
    }
    Ok(ran)
}

/// Waits a while for the process `pid` to end, and says whether it did.
fn gone(processes: &mut dyn Processes, pid: u32, pause: Duration) -> bool {
    for tried in 1..=GONE_TRIES {
        if !processes.alive(pid) {
            return true;
        }
        if tried < GONE_TRIES {
            std::thread::sleep(pause);
        }
    }
    false
}

/// Runs the task, which starts `maxd`, or restarts it when it runs: the
/// new launcher hands over to the one that runs it.
pub fn run(setup: &Setup, runner: &mut dyn Runner) -> Result<(), ServiceError> {
    run_named(setup, &task_name_here(), runner)
}

/// [`run`] for the task called `name`.
pub(super) fn run_named(
    setup: &Setup,
    name: &str,
    runner: &mut dyn Runner,
) -> Result<(), ServiceError> {
    if !registered(runner, name) {
        return Err(ServiceError::NotInstalled(setup.file()));
    }
    must(runner, "schtasks", &["/Run", "/TN", name]).map(drop)
}

/// Starts `maxd` unless it serves already, and says whether it started it.
pub fn start(setup: &Setup, runner: &mut dyn Runner) -> Result<bool, ServiceError> {
    start_named(setup, &task_name_here(), runner, &mut SystemProcesses)
}

/// [`start`] for the task called `name`.
pub(super) fn start_named(
    setup: &Setup,
    name: &str,
    runner: &mut dyn Runner,
    processes: &mut dyn Processes,
) -> Result<bool, ServiceError> {
    if !registered(runner, name) {
        return Err(ServiceError::NotInstalled(setup.file()));
    }
    if setup
        .socket
        .as_deref()
        .and_then(|socket| processes.serving(socket))
        .is_some()
    {
        return Ok(false);
    }
    must(runner, "schtasks", &["/Run", "/TN", name])?;
    Ok(true)
}

/// What Task Scheduler and the socket say: the state, and the process ID
/// of the `maxd` that serves while the task is there.
pub fn state(setup: &Setup, runner: &mut dyn Runner) -> (String, Option<u32>) {
    state_of(setup, &task_name_here(), runner, &mut SystemProcesses)
}

/// [`state`] for the task called `name`.
pub(super) fn state_of(
    setup: &Setup,
    name: &str,
    runner: &mut dyn Runner,
    processes: &mut dyn Processes,
) -> (String, Option<u32>) {
    if !registered(runner, name) {
        return ("not loaded".into(), None);
    }
    match setup
        .socket
        .as_deref()
        .and_then(|socket| processes.serving(socket))
    {
        Some(pid) => ("running".into(), Some(pid)),
        None => ("stopped".into(), None),
    }
}

/// `maxd launch <program>`, which the logon task runs: runs `program run`
/// until it stops for good, with its output in the log.
pub fn launch(program: &Path) -> Result<(), String> {
    let socket = maxhq_core::paths::socket()
        .ok_or("couldn't work out where maxd's socket goes. Set MAXD_SOCKET to a path")?;
    let data = maxhq_core::paths::data_dir().ok_or("couldn't find a folder for maxd's data")?;
    let plan = Plan {
        socket,
        program: program.to_owned(),
        args: vec!["run".to_owned()],
        log: launcher_log(&data),
        log_limit: LOG_LIMIT,
        restart_code: RESTART_EXIT,
        backoff: Backoff::SERVICE,
    };
    match launcher::tie_children() {
        Ok(()) => {}
        // launchd and systemd do this on Unix.
        Err(err) if err.kind() == io::ErrorKind::Unsupported => {
            return Err(format!(
                "{err}. maxd launch runs maxd for Windows's logon task, and `maxd service install` sets it up"
            ));
        }
        // Ending the task would leave maxd running, but it runs.
        Err(_) => {}
    }
    launcher::supervise(&plan).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests;
