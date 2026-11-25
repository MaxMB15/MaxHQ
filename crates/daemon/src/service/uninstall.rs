// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! `maxd service uninstall`, which takes `maxd` off this device: the
//! service, the agents its holders keep running, what else it started that
//! outlives it and, with `--delete-data`, its data. The MaxHQ app's own
//! files stay.
//!
//! Commands go through a [`Runner`] and holders through [`Holders`], so
//! tests check what would happen without stopping anything real.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use maxhq_core::service::{launcher_program, task_name_here};
use maxhq_hold::{Ended, Found, Holds};
use maxhq_protocol::ids::SessionId;

use super::{
    command_error, find_program, remove, task, wait_unloaded, Manager, Runner, ServiceError, Setup,
    GONE_PAUSE, GONE_TRIES, UNIT,
};
use crate::hooks::{FUNNEL_PORT, FUNNEL_PREFIX};
use crate::incus::MANAGED_KEY;
use crate::leases::{emulator, incus as test_linux, mac_host};

/// How long a holder gets to stop its agent before it's killed.
const HOLDER_GRACE: Duration = Duration::from_secs(10);

/// How many times a process is looked for after it's asked to stop. With
/// [`GONE_PAUSE`] it gets about 10 seconds.
const STOP_TRIES: u32 = 50;

/// The folder in `maxd`'s data folder where the MaxHQ app keeps its SSH
/// files. It stays when the rest of the data goes.
const APP_SSH: &str = "ssh";

/// How [`uninstall`] runs.
#[derive(Clone, Copy, Debug)]
pub struct Uninstall {
    /// Delete `maxd`'s data and logs too.
    pub delete_data: bool,
    /// The `maxd` the service runs is the one uninstalling, after it
    /// stopped serving. Its service is stopped last, without waiting.
    pub itself: bool,
    /// The wait between checks that something stopped.
    pub pause: Duration,
}

impl Uninstall {
    /// An uninstall run from the command line.
    pub fn new(delete_data: bool) -> Self {
        Self {
            delete_data,
            itself: false,
            pause: GONE_PAUSE,
        }
    }
}

/// Ends the agents that holders keep running after `maxd` stops.
pub trait Holders {
    /// Ends every holder and the agent it runs.
    fn end_all(&mut self) -> Ending;
}

/// What [`Holders::end_all`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ending {
    /// How many running agents it ended.
    pub ended: usize,
    /// What may still run, in words for you.
    pub left: Vec<String>,
}

/// The holders in the usual folders.
pub struct SystemHolders;

impl Holders for SystemHolders {
    fn end_all(&mut self) -> Ending {
        let Some(holds) = Holds::from_env() else {
            return Ending::default();
        };
        let left = |what: String| Ending {
            ended: 0,
            left: vec![what],
        };
        let sessions = match holds.list() {
            Ok(sessions) => sessions,
            Err(err) => return left(format!("couldn't list the agents maxd ran: {err}")),
        };
        if sessions.is_empty() {
            return Ending::default();
        }
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(err) => return left(format!("couldn't end the agents maxd ran: {err}")),
        };
        let ends = sessions
            .into_iter()
            .map(|session| end_held(&holds, session));
        let mut ending = Ending::default();
        for (ran, still) in runtime.block_on(futures::future::join_all(ends)) {
            ending.ended += usize::from(ran && still.is_none());
            ending.left.extend(still);
        }
        ending
    }
}

/// Ends one session's holder and agent. Gives whether one was running, and
/// what may still run.
async fn end_held(holds: &Holds, session: SessionId) -> (bool, Option<String>) {
    let (ran, holder_pid) = match holds.find(session).await {
        Ok(Found::Nothing | Found::NotStarted) => return (false, None),
        Ok(Found::Live { holder_pid }) => (true, Some(holder_pid)),
        // The holder has gone, but the agent may have outlived it.
        Ok(Found::Ended) => (false, None),
        // Starting, unconfirmed, or a folder that couldn't be read.
        _ => (true, None),
    };
    match holds.end(session, holder_pid, HOLDER_GRACE).await {
        Ended::Gone => (ran, None),
        Ended::Unconfirmed { pid, reason } => {
            let process = pid.map_or(String::new(), |pid| format!(" as process {pid}"));
            (
                true,
                Some(format!("agent {session} may still run{process}: {reason}")),
            )
        }
    }
}

/// The CLIs uninstall asks about what `maxd` set up outside itself.
#[derive(Clone, Debug, Default)]
pub struct Clis {
    /// Tailscale, which may serve webhooks through Funnel.
    pub tailscale: Option<PathBuf>,
    /// Incus, which may run the Linux test container.
    pub incus: Option<PathBuf>,
}

impl Clis {
    /// The CLIs this device has.
    pub fn find() -> Self {
        Self {
            tailscale: maxhq_client::tailscale_cli()
                .map(PathBuf::from)
                .or_else(|| find_program("tailscale")),
            incus: find_program("incus"),
        }
    }
}

/// Takes `maxd` off this device and tells `say` each thing it did. Running
/// it again finds nothing left to do.
///
/// It stops the update check and the service, ends the agents holders keep
/// running, stops what `maxd` started that outlives it and removes the
/// service file. With `delete_data` it then deletes `maxd`'s data and logs,
/// but keeps what the MaxHQ app uses. Where the NixOS module owns the unit,
/// it only stops the service and removes a unit of your own, as it always
/// has, and refuses to delete the data.
///
/// A service that won't stop is an error before anything else is touched.
/// Anything else left is reported together at the end, as
/// [`ServiceError::Unfinished`].
pub fn uninstall(
    setup: &Setup,
    how: Uninstall,
    clis: &Clis,
    runner: &mut dyn Runner,
    holders: &mut dyn Holders,
    say: &mut dyn FnMut(String),
) -> Result<(), ServiceError> {
    if setup.module_owns_unit() {
        if how.delete_data {
            return Err(ServiceError::ModuleKeepsData);
        }
        let file = setup.file();
        let _ = runner.run("systemctl", &["--user", "disable", "--now", UNIT]);
        remove(&file)?;
        let _ = runner.run("systemctl", &["--user", "daemon-reload"]);
        say(format!("Stopped maxd and removed {}.", file.display()));
        return Ok(());
    }
    let mut steps = Steps {
        setup,
        how,
        runner,
        say,
        said: false,
        problems: Vec::new(),
    };
    // The check on a fresh update could roll back and start maxd again.
    steps.stop_left(
        "maxd update watch --build ",
        &["-f"],
        false,
        "Stopped the check on a recent update.",
        "the check on a recent update",
    );
    if !how.itself {
        steps.stop_service()?;
    } else if setup.manager == Manager::Systemd {
        // Disabling needs the unit file, which goes before the service stops.
        let _ = steps.runner.run("systemctl", &["--user", "disable", UNIT]);
    }
    let ending = holders.end_all();
    if ending.ended > 0 {
        steps.tell(format!("Ended {}.", count(ending.ended, "running agent")));
    }
    let agents_left = !ending.left.is_empty();
    steps.problems.extend(ending.left);
    steps.stop_leftovers(clis);
    steps.remove_file();
    if how.delete_data {
        if agents_left {
            steps
                .problems
                .push("kept maxd's data, since an agent may still use it".into());
        } else {
            steps.delete_data();
        }
    }
    if how.itself {
        steps.stop_itself();
    }
    if !steps.said {
        (steps.say)("maxd isn't installed, so there was nothing to stop or remove.".into());
    }
    if steps.problems.is_empty() {
        Ok(())
    } else {
        Err(ServiceError::Unfinished(steps.problems))
    }
}

/// What became of the processes [`Steps::stop`] looked for.
enum Stop {
    /// None ran.
    NotRunning,
    /// They ran and have stopped.
    Stopped,
    /// They're still there after being killed.
    Left,
}

/// An uninstall under way.
struct Steps<'a> {
    setup: &'a Setup,
    how: Uninstall,
    runner: &'a mut dyn Runner,
    say: &'a mut dyn FnMut(String),
    /// Whether anything was done.
    said: bool,
    /// What was left, in words for you.
    problems: Vec<String>,
}

impl Steps<'_> {
    fn tell(&mut self, line: String) {
        self.said = true;
        (self.say)(line);
    }

    /// Stops the service and waits until it's gone.
    fn stop_service(&mut self) -> Result<(), ServiceError> {
        match self.setup.manager {
            Manager::Launchd => {
                let target = self.setup.target();
                if !self.succeeds("launchctl", &["print", &target]) {
                    return Ok(());
                }
                let _ = self.runner.run("launchctl", &["bootout", &target]);
                wait_unloaded(self.runner, &target, GONE_TRIES, self.how.pause)?;
            }
            Manager::Systemd => {
                let active = ["--user", "is-active", "--quiet", UNIT];
                let was_active = self.succeeds("systemctl", &active);
                if !was_active && !self.setup.file().exists() {
                    return Ok(());
                }
                // Disabling it too, so no link to the file is left.
                let stop = ["--user", "disable", "--now", UNIT];
                let _ = self.runner.run("systemctl", &stop);
                if !was_active {
                    return Ok(());
                }
                if self.succeeds("systemctl", &active) {
                    return Err(command_error(
                        "systemctl",
                        &stop,
                        "maxd was still running after it",
                    ));
                }
            }
            Manager::TaskScheduler => {
                let ran = task::stop_as(
                    self.setup,
                    &task_name_here(),
                    self.runner,
                    &mut task::SystemProcesses,
                    self.how.pause,
                )?;
                if !ran {
                    return Ok(());
                }
            }
        }
        self.tell("Stopped the maxd service.".into());
        Ok(())
    }

    /// Stops what `maxd` started that would outlive it.
    fn stop_leftovers(&mut self, clis: &Clis) {
        let launchd = self.setup.manager == Manager::Launchd;
        // A VM helper stops its VM and exits once maxd has gone, so it gets
        // a moment first. Those of the maxd that's uninstalling itself are
        // still its own and end with it.
        if launchd && !self.how.itself {
            self.stop_left(
                "maxd-vm",
                &["-P", "1", "-x"],
                true,
                "Stopped maxd's VM helpers.",
                "a VM helper",
            );
        }
        if let Some(data) = self.setup.data.clone() {
            let tools = data
                .join("tools")
                .join(crate::agent_device::install::PACKAGE);
            self.stop_left(
                &literal(&format!("{}/", tools.display())),
                &["-f"],
                false,
                "Stopped agent-device, which drove your test devices.",
                "agent-device",
            );
        }
        self.stop_left(
            &format!("-avd {}( |$)", literal(emulator::AVD)),
            &["-f"],
            false,
            "Stopped the Android test emulator.",
            "the Android test emulator",
        );
        if launchd {
            self.shut_down_simulator();
        }
        self.close_ssh_masters();
        if let Some(cli) = &clis.tailscale {
            self.close_funnel(&cli.to_string_lossy());
        }
        if let (Manager::Systemd, Some(cli)) = (self.setup.manager, &clis.incus) {
            self.stop_test_container(&cli.to_string_lossy());
        }
    }

    /// Stops the processes of yours that match `pattern`, and says `done`
    /// if any ran, or reports `what` as left if they don't stop.
    ///
    /// They're asked to stop, then killed. With `wait_first` they get a
    /// while to end on their own before that.
    fn stop_left(
        &mut self,
        pattern: &str,
        flags: &[&str],
        wait_first: bool,
        done: &str,
        what: &str,
    ) {
        match self.stop(pattern, flags, wait_first) {
            Stop::NotRunning => {}
            Stop::Stopped => self.tell(done.into()),
            Stop::Left => self.problems.push(format!("{what} is still running")),
        }
    }

    /// Stops the processes of yours that `pgrep` finds with `flags` and
    /// `pattern`.
    fn stop(&mut self, pattern: &str, flags: &[&str], wait_first: bool) -> Stop {
        let uid = self.setup.uid.to_string();
        let mut filter = vec!["-U", &uid];
        filter.extend_from_slice(flags);
        filter.extend(["--", pattern]);
        if !self.succeeds("pgrep", &filter) {
            return Stop::NotRunning;
        }
        if wait_first && self.gone(&filter) {
            return Stop::Stopped;
        }
        for signal in [None, Some("-KILL")] {
            let args: Vec<&str> = signal.into_iter().chain(filter.iter().copied()).collect();
            let _ = self.runner.run("pkill", &args);
            if self.gone(&filter) {
                return Stop::Stopped;
            }
        }
        Stop::Left
    }

    /// Waits a while for `pgrep` to find nothing.
    fn gone(&mut self, filter: &[&str]) -> bool {
        for tried in 1..=STOP_TRIES {
            if !self.succeeds("pgrep", filter) {
                return true;
            }
            if tried < STOP_TRIES {
                thread::sleep(self.how.pause);
            }
        }
        false
    }

    fn succeeds(&mut self, program: &str, args: &[&str]) -> bool {
        self.runner.run(program, args).is_ok_and(|ran| ran.success)
    }

    /// Shuts down the Simulator `maxd` made for the iOS test host, and with
    /// `delete_data` deletes it.
    fn shut_down_simulator(&mut self) {
        let Some(data) = &self.setup.data else {
            return;
        };
        let Ok(udid) = fs::read_to_string(mac_host::udid_file(data)) else {
            return;
        };
        let udid = udid.trim().to_owned();
        if udid.is_empty() {
            return;
        }
        // Without xcrun there's no Simulator to stop.
        let Ok(ran) = self.runner.run("xcrun", &["simctl", "shutdown", &udid]) else {
            return;
        };
        let gone = |stderr: &str| stderr.contains("Invalid device");
        if ran.success {
            self.tell("Shut down the iOS test Simulator.".into());
        } else if !gone(&ran.stderr) && !ran.stderr.contains("current state: Shutdown") {
            self.problems.push(format!(
                "couldn't shut down the iOS test Simulator: {}",
                ran.stderr.trim()
            ));
        }
        if !self.how.delete_data {
            return;
        }
        match self.runner.run("xcrun", &["simctl", "delete", &udid]) {
            Ok(ran) if ran.success => self.tell("Deleted the iOS test Simulator.".into()),
            Ok(ran) if gone(&ran.stderr) => {}
            Ok(ran) => self.problems.push(format!(
                "couldn't delete the iOS test Simulator: {}",
                ran.stderr.trim()
            )),
            Err(err) => self
                .problems
                .push(format!("couldn't delete the iOS test Simulator: {err}")),
        }
    }

    /// Closes the SSH connections `maxd` kept open to your other devices.
    /// They'd close on their own once idle for a while.
    fn close_ssh_masters(&mut self) {
        let Some(runtime) = &self.setup.runtime else {
            return;
        };
        let Ok(entries) = fs::read_dir(runtime) else {
            return;
        };
        let mut masters: Vec<PathBuf> = entries
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".ssh-connection")
            })
            .map(|entry| entry.path().join("master"))
            .filter(|master| master.exists())
            .collect();
        masters.sort();
        let mut closed = 0;
        for master in masters {
            let master = master.to_string_lossy();
            closed += usize::from(self.succeeds("ssh", &["-S", &master, "-O", "exit", "maxhq"]));
        }
        if closed > 0 {
            self.tell(format!(
                "Closed {} to your other devices.",
                count(closed, "SSH connection")
            ));
        }
    }

    /// Closes the Funnel paths `maxd` opened for webhooks, and leaves the
    /// rest of what Tailscale serves alone.
    fn close_funnel(&mut self, cli: &str) {
        let Ok(ran) = self.runner.run(cli, &["serve", "status", "--json"]) else {
            return;
        };
        if !ran.success {
            return;
        }
        let paths = funnel_paths(&ran.stdout);
        let mut closed = 0;
        for path in &paths {
            let set_path = format!("--set-path={path}");
            match self
                .runner
                .run(cli, &["funnel", FUNNEL_PORT, &set_path, "off"])
            {
                Ok(ran) if ran.success => closed += 1,
                Ok(ran) => self.problems.push(format!(
                    "couldn't close the Tailscale Funnel path {path}: {}",
                    ran.stderr.trim()
                )),
                Err(err) => self.problems.push(format!(
                    "couldn't close the Tailscale Funnel path {path}: {err}"
                )),
            }
        }
        if closed > 0 {
            self.tell(format!(
                "Closed {} for webhooks.",
                count(closed, "Tailscale Funnel path")
            ));
        }
    }

    /// Stops the Linux test container, or with `delete_data` deletes it.
    /// Workspace containers hold your work, so they stay.
    fn stop_test_container(&mut self, cli: &str) {
        let filter = format!("{MANAGED_KEY}=true");
        let Ok(ran) = self.runner.run(
            cli,
            &["list", &filter, "--format", "csv", "--columns", "ns"],
        ) else {
            return;
        };
        if !ran.success {
            return;
        }
        let mut workspaces = Vec::new();
        for line in ran.stdout.lines() {
            let Some((name, state)) = line.split_once(',') else {
                continue;
            };
            if name != test_linux::NAME {
                workspaces.push(name.to_owned());
                continue;
            }
            let (args, done, verb) = if self.how.delete_data {
                (["delete", "--force", name], "Deleted", "delete")
            } else if state.eq_ignore_ascii_case("RUNNING") {
                (["stop", "--force", name], "Stopped", "stop")
            } else {
                continue;
            };
            match self.runner.run(cli, &args) {
                Ok(ran) if ran.success => {
                    self.tell(format!("{done} the Linux test container."));
                }
                Ok(ran) => self.problems.push(format!(
                    "couldn't {verb} the Linux test container: {}",
                    ran.stderr.trim()
                )),
                Err(err) => self
                    .problems
                    .push(format!("couldn't {verb} the Linux test container: {err}")),
            }
        }
        if !workspaces.is_empty() {
            self.tell(format!(
                "Left {} in Incus, since they hold your work: {}.",
                count(workspaces.len(), "workspace container"),
                workspaces.join(", ")
            ));
        }
    }

    /// Removes the service file, and on Windows the logon task and its
    /// launcher first.
    fn remove_file(&mut self) {
        if self.setup.manager == Manager::TaskScheduler {
            self.remove_task();
        }
        let file = self.setup.file();
        if !file.exists() {
            return;
        }
        if let Err(err) = fs::remove_file(&file) {
            self.problems
                .push(format!("couldn't remove {}: {err}", file.display()));
            return;
        }
        if self.setup.manager == Manager::Systemd {
            let _ = self.runner.run("systemctl", &["--user", "daemon-reload"]);
        }
        self.tell(format!("Removed {}.", file.display()));
    }

    /// Takes the logon task out of Task Scheduler and deletes the launcher
    /// it ran.
    fn remove_task(&mut self) {
        let name = task_name_here();
        if task::registered(self.runner, &name) {
            let failed = match self
                .runner
                .run("schtasks", &["/Delete", "/TN", &name, "/F"])
            {
                Ok(ran) if ran.success => None,
                Ok(ran) => Some(ran.stderr.trim().to_owned()),
                Err(err) => Some(err.to_string()),
            };
            match failed {
                None => self.tell(format!("Removed the logon task \"{name}\".")),
                Some(why) => self
                    .problems
                    .push(format!("couldn't remove the logon task \"{name}\": {why}")),
            }
        }
        let launcher = launcher_program(&self.setup.app_folder());
        let mut old = launcher.as_os_str().to_owned();
        old.push(".old");
        let _ = fs::remove_file(PathBuf::from(old));
        match delete(&launcher) {
            Ok(true) => self.tell(format!("Removed {}.", launcher.display())),
            Ok(false) => {}
            Err(err) => self
                .problems
                .push(format!("couldn't remove {}: {err}", launcher.display())),
        }
    }

    /// Deletes `maxd`'s data, its logs and its keys for your other devices,
    /// but not the files the MaxHQ app keeps in the data folder.
    fn delete_data(&mut self) {
        if let Some(data) = self.setup.data.clone() {
            self.delete_data_folder(&data);
        }
        let mut others: Vec<PathBuf> = Vec::new();
        others.extend(
            self.setup
                .runtime
                .clone()
                .filter(|runtime| Some(runtime) != self.setup.data.as_ref()),
        );
        if self.setup.manager == Manager::Launchd {
            others.push(self.setup.home.join("Library/Logs/MaxHQ"));
        }
        // The key this device reaches your other devices with, and their
        // host keys. maxhq-host.toml beside it stays, since you wrote it.
        others.push(self.setup.home.join(".config/maxhq/ssh"));
        for path in others {
            match delete(&path) {
                Ok(true) => self.tell(format!("Deleted {}.", path.display())),
                Ok(false) => {}
                Err(err) => self
                    .problems
                    .push(format!("couldn't delete {}: {err}", path.display())),
            }
        }
    }

    /// Deletes the data folder, all but the app's SSH files.
    fn delete_data_folder(&mut self, data: &Path) {
        let entries = match fs::read_dir(data) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return,
            Err(err) => {
                self.problems
                    .push(format!("couldn't delete {}: {err}", data.display()));
                return;
            }
        };
        let (mut deleted, mut kept, mut failed) = (false, false, false);
        for entry in entries {
            let path = match entry {
                Ok(entry) if entry.file_name() == APP_SSH => {
                    kept = true;
                    continue;
                }
                Ok(entry) => entry.path(),
                Err(err) => {
                    failed = true;
                    self.problems
                        .push(format!("couldn't delete {}: {err}", data.display()));
                    continue;
                }
            };
            match delete(&path) {
                Ok(gone) => deleted |= gone,
                Err(err) => {
                    failed = true;
                    self.problems
                        .push(format!("couldn't delete {}: {err}", path.display()));
                }
            }
        }
        if kept {
            if deleted && !failed {
                self.tell(format!(
                    "Deleted maxd's data in {}, but kept {}, which the MaxHQ app uses.",
                    data.display(),
                    data.join(APP_SSH).display()
                ));
            }
            return;
        }
        if failed {
            return;
        }
        match fs::remove_dir(data) {
            Ok(()) => self.tell(format!("Deleted {}.", data.display())),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => self
                .problems
                .push(format!("couldn't delete {}: {err}", data.display())),
        }
    }

    /// Stops the service of the `maxd` that's uninstalling itself. That
    /// stops this process, so it's started and not waited for.
    fn stop_itself(&mut self) {
        let target = self.setup.target();
        let started = match self.setup.manager {
            Manager::Launchd => self.runner.spawn("launchctl", &["bootout", &target]),
            Manager::Systemd => self.runner.spawn("systemctl", &["--user", "stop", UNIT]),
            // This maxd exits 0 when it's done, and its launcher stops.
            Manager::TaskScheduler => Ok(()),
        };
        match started {
            Ok(()) => self.tell("Stopping the maxd service.".into()),
            Err(err) => self
                .problems
                .push(format!("couldn't stop the maxd service: {err}")),
        }
    }
}

/// Deletes a file or a whole folder. A link is removed, not followed. Gives
/// whether there was anything.
fn delete(path: &Path) -> io::Result<bool> {
    let removed = match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(err) => Err(err),
    };
    match removed {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

/// The Funnel paths `maxd` opened, from `tailscale serve status --json`.
fn funnel_paths(status: &str) -> Vec<String> {
    let Ok(status) = serde_json::from_str::<serde_json::Value>(status) else {
        return Vec::new();
    };
    let port = format!(":{}", FUNNEL_PORT.trim_start_matches("--https="));
    let mut paths: Vec<String> = status
        .get("Web")
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(host, _)| host.ends_with(&port))
        .filter_map(|(_, web)| web.get("Handlers")?.as_object())
        .flat_map(|handlers| handlers.keys())
        .filter(|path| path.starts_with(FUNNEL_PREFIX))
        .cloned()
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// `text` as an extended regular expression that matches only itself.
fn literal(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for char in text.chars() {
        if r"\^$.|?*+()[]{}".contains(char) {
            escaped.push('\\');
        }
        escaped.push(char);
    }
    escaped
}

/// "1 thing" or "2 things".
fn count(n: usize, thing: &str) -> String {
    if n == 1 {
        format!("1 {thing}")
    } else {
        format!("{n} {thing}s")
    }
}

#[cfg(test)]
mod tests;
