// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! `maxd service`, which runs `maxd` in your login session: a LaunchAgent on
//! macOS, a systemd user unit on Linux and a logon task of your own on
//! Windows ([`task`]). On NixOS the module writes the unit, and install
//! leaves it to the module.
//!
//! Every call to `launchctl`, `systemctl`, `loginctl` and `schtasks` goes
//! through a [`Runner`], so tests check what would run without loading
//! anything. [`uninstall`] takes the rest of `maxd` off the device too,
//! and [`app_watch`] has a `maxd` whose app was deleted uninstall itself.

pub mod app_watch;
pub mod task;
pub mod uninstall;

use std::ffi::OsStr;
use std::fmt;
use std::fs;
#[cfg(unix)]
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use maxhq_core::fs::{write_atomic, Atomic};
use maxhq_core::paths::MaxdLayout;
use maxhq_core::service::{launcher_log, launcher_program, restart_command, task_file, task_xml};
use maxhq_os::fsmode;
use thiserror::Error;

pub use maxhq_core::service::{launch_agent_file, Manager, LABEL, UNIT};
pub use uninstall::{uninstall, Clis, SystemHolders, Uninstall};

/// Where the NixOS module puts maxd's user unit. systemd reads it for every
/// user, and a unit of the same name in your own folder shadows it.
pub const MODULE_UNIT: &str = "/etc/systemd/user/maxd.service";

/// How many times `launchctl bootstrap` is tried. Taking an old copy down
/// finishes a moment after `bootout` returns, and bootstrapping before then
/// fails.
const BOOTSTRAP_TRIES: u32 = 10;

/// The wait between bootstrap tries.
const BOOTSTRAP_PAUSE: Duration = Duration::from_millis(200);

/// How many times `launchctl print` is asked whether the old copy has
/// gone after `bootout`. A running `maxd` takes a few seconds to stop, so
/// with [`GONE_PAUSE`] this waits about 20 seconds at most.
const GONE_TRIES: u32 = 100;

/// The wait between those checks.
const GONE_PAUSE: Duration = Duration::from_millis(200);

/// Folders added to the service's `PATH` after your own, so `maxd` finds
/// the agent CLIs even when it's installed from an app that has a short
/// one. `~/.local/bin` comes first among them, from the home folder.
const EXTRA_PATH: &[&str] = &[
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

/// What `maxd service` was asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Write the service file and start it.
    Install,
    /// Stop the service, end what it started and remove its file.
    Uninstall {
        /// Delete `maxd`'s data and logs too.
        delete_data: bool,
    },
    /// Say whether it's installed and running.
    Status,
    /// Start the installed service.
    Start,
    /// Stop the service until you next log in.
    Stop,
    /// Stop the service and start it again.
    Restart,
}

impl Action {
    /// The action a command-line word names.
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "install" => Some(Self::Install),
            "uninstall" => Some(Self::Uninstall { delete_data: false }),
            "status" => Some(Self::Status),
            "start" => Some(Self::Start),
            "stop" => Some(Self::Stop),
            "restart" => Some(Self::Restart),
            _ => None,
        }
    }
}

/// Why a service action failed.
#[derive(Debug, Error)]
pub enum ServiceError {
    /// This system has no service manager `maxd` knows.
    #[error("maxd service supports macOS, Linux and Windows, not {0}")]
    Unsupported(&'static str),
    /// `HOME` isn't set.
    #[error("HOME isn't set, so there's nowhere to put the service file")]
    NoHome,
    /// Running as root would put the service in the wrong session.
    #[error("run maxd service as your own user, not root, since maxd runs in your login session")]
    Root,
    /// The user ID or the path to this program couldn't be found.
    #[error("couldn't find {what}: {source}")]
    Lookup {
        /// What was being looked up.
        what: &'static str,
        /// What went wrong.
        source: io::Error,
    },
    /// A file or folder couldn't be written or removed.
    #[error("couldn't write {}: {source}", path.display())]
    Io {
        /// The file or folder.
        path: PathBuf,
        /// What went wrong.
        source: io::Error,
    },
    /// A service manager command failed.
    #[error("`{command}` failed: {detail}")]
    Command {
        /// The command line.
        command: String,
        /// What it printed, or why it didn't run.
        detail: String,
    },
    /// Uninstall was asked to delete the data of a `maxd` the NixOS module
    /// runs, which would start again without it.
    #[error("the NixOS module runs maxd here, so uninstall leaves its data alone. Take maxd out of your NixOS configuration, then run this again")]
    ModuleKeepsData,
    /// Uninstall did what it could, but these were left.
    #[error("uninstall didn't finish: {}", .0.join("; "))]
    Unfinished(Vec<String>),
    /// Start or restart found no service to run.
    #[error("maxd's service isn't installed, as {} is missing. Run `maxd service install` first", .0.display())]
    NotInstalled(PathBuf),
}

/// How a command ended.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ran {
    /// Whether it exited with status 0.
    pub success: bool,
    /// What it printed.
    pub stdout: String,
    /// What it printed as errors.
    pub stderr: String,
}

/// Runs service manager commands.
pub trait Runner {
    /// Runs `program` with `args` and waits for it.
    fn run(&mut self, program: &str, args: &[&str]) -> io::Result<Ran>;

    /// Starts `program` with `args` and doesn't wait for it, for a command
    /// that stops the process that runs it.
    fn spawn(&mut self, program: &str, args: &[&str]) -> io::Result<()> {
        self.run(program, args).map(drop)
    }
}

/// Runs commands for real.
pub struct System;

impl Runner for System {
    fn run(&mut self, program: &str, args: &[&str]) -> io::Result<Ran> {
        let out = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .output()?;
        Ok(Ran {
            success: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    fn spawn(&mut self, program: &str, args: &[&str]) -> io::Result<()> {
        // A group of its own, so it isn't stopped with this process.
        maxhq_os::proc::spawn_detached(
            Command::new(program)
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
    }
}

/// Everything an action needs to know about this system. Tests build one
/// around a temp folder.
#[derive(Clone, Debug)]
pub struct Setup {
    /// The service manager.
    pub manager: Manager,
    /// Your home folder.
    pub home: PathBuf,
    /// `$XDG_CONFIG_HOME`, or `~/.config`, where systemd looks for user
    /// units.
    pub config_home: PathBuf,
    /// Your user ID, which names your launchd session.
    pub uid: u32,
    /// The `maxd` the service runs.
    pub exe: PathBuf,
    /// The `PATH` the service runs with.
    pub path: String,
    /// The project folders `path` leaves out of your own `PATH`.
    pub left_out: Vec<String>,
    /// The socket `maxd` serves on, which the status checks.
    pub socket: Option<PathBuf>,
    /// `/` for real, a temp folder in tests. The NixOS module's unit is
    /// looked for under it.
    pub root: PathBuf,
    /// `maxd`'s data folder, which uninstall can delete.
    pub data: Option<PathBuf>,
    /// The folder for `maxd`'s socket and the other files that last until
    /// logout. On macOS it's the data folder.
    pub runtime: Option<PathBuf>,
    /// On Windows, MaxHQ's folder, with the logon task's file, its
    /// launcher and `maxd`'s builds, and the data folder inside. `None`
    /// elsewhere.
    pub app: Option<PathBuf>,
}

impl Setup {
    /// What to tell you about the folders `path` leaves out, if any.
    pub fn left_out_note(&self) -> Option<String> {
        (!self.left_out.is_empty()).then(|| {
            format!(
                "Left out of maxd's PATH, since a project can change what runs from them: {}. Link a tool into ~/.local/bin if maxd needs it.",
                self.left_out.join(", ")
            )
        })
    }

    /// Reads this system: the OS, `HOME`, your user ID, this program's path
    /// and your `PATH`.
    pub fn from_env() -> Result<Self, ServiceError> {
        Self::read(|| std::env::current_exe().and_then(fs::canonicalize))
    }

    /// [`Setup::from_env`] for a `maxd` whose program may have been
    /// deleted while it ran, as it is once its app has gone.
    pub fn from_env_as_running() -> Result<Self, ServiceError> {
        Self::read(std::env::current_exe)
    }

    /// Reads this system, with `exe` giving this program's path.
    fn read(exe: impl FnOnce() -> io::Result<PathBuf>) -> Result<Self, ServiceError> {
        let manager = Manager::here().ok_or(ServiceError::Unsupported(std::env::consts::OS))?;
        if manager == Manager::TaskScheduler {
            return Self::read_windows(exe);
        }
        let home = std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .ok_or(ServiceError::NoHome)?;
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .unwrap_or_else(|| home.join(".config"));
        let uid = user_id()?;
        if uid == 0 {
            return Err(ServiceError::Root);
        }
        let exe = exe().map_err(|source| ServiceError::Lookup {
            what: "the maxd program",
            source,
        })?;
        let exe = stable_exe(&home, exe);
        let (path, left_out) = service_path(std::env::var_os("PATH").as_deref(), &home);
        Ok(Self {
            manager,
            home,
            config_home,
            uid,
            exe,
            path,
            left_out,
            socket: maxhq_core::paths::socket(),
            root: PathBuf::from("/"),
            data: maxhq_core::paths::data_dir(),
            runtime: maxhq_core::paths::runtime_dir(),
            app: None,
        })
    }

    /// Reads Windows: your profile folder, MaxHQ's folder, `maxd`'s data
    /// folder and this program's path. The task runs as you without
    /// elevation, so an admin may install it for themselves, and it needs
    /// no `PATH` of its own: Windows gives a logon task yours.
    fn read_windows(exe: impl FnOnce() -> io::Result<PathBuf>) -> Result<Self, ServiceError> {
        let home = maxhq_os::home::home_dir().ok_or(ServiceError::NoHome)?;
        let missing = || ServiceError::Lookup {
            what: "MaxHQ's folder",
            source: io::Error::other("Windows gave no local app data folder"),
        };
        let app = maxhq_core::paths::windows_app_dir().ok_or_else(missing)?;
        let data = maxhq_core::paths::data_dir().ok_or_else(missing)?;
        let exe = exe().map_err(|source| ServiceError::Lookup {
            what: "the maxd program",
            source,
        })?;
        let exe =
            maxhq_core::fs_ext::plain(stable_program(&MaxdLayout::windows(&app).program(), exe));
        Ok(Self {
            manager: Manager::TaskScheduler,
            config_home: home.join(".config"),
            uid: 0,
            exe,
            path: String::new(),
            left_out: Vec::new(),
            socket: maxhq_core::paths::socket(),
            root: PathBuf::from("/"),
            data: Some(data),
            runtime: maxhq_core::paths::runtime_dir(),
            app: Some(app),
            home,
        })
    }

    /// The service file. On Windows it's the copy of the logon task that
    /// install registered.
    pub fn file(&self) -> PathBuf {
        match self.manager {
            Manager::Launchd => launch_agent_file(&self.home),
            Manager::Systemd => self.config_home.join("systemd/user").join(UNIT),
            Manager::TaskScheduler => task_file(&self.app_folder()),
        }
    }

    /// MaxHQ's folder on Windows, which holds the logon task's file, the
    /// launcher and `maxd`'s builds: the one Windows has when the setup
    /// names none. Only the logon task reads it, so off Windows a setup
    /// without one has an empty path here.
    pub fn app_folder(&self) -> PathBuf {
        self.app
            .clone()
            .or_else(maxhq_core::paths::windows_app_dir)
            .unwrap_or_default()
    }

    /// `maxd`'s data folder, which on Windows holds the launcher's log.
    pub fn data_folder(&self) -> PathBuf {
        self.data
            .clone()
            .unwrap_or_else(|| self.app_folder().join("data"))
    }

    /// True when the NixOS module's unit is there, so the module owns
    /// maxd's unit and install leaves it alone.
    pub fn module_owns_unit(&self) -> bool {
        self.manager == Manager::Systemd
            && self.root.join(MODULE_UNIT.trim_start_matches('/')).exists()
    }

    /// The command that removes the unit install wrote, which shadows the
    /// module's, and starts the module's in its place.
    pub fn unshadow_command(&self) -> String {
        format!(
            "{} service uninstall && systemctl --user restart {UNIT}",
            self.exe.display()
        )
    }

    /// Where launchd, or on Windows the launcher, writes what `maxd` logs.
    /// systemd keeps it in the journal instead.
    pub fn log_file(&self) -> PathBuf {
        match self.manager {
            Manager::TaskScheduler => launcher_log(&self.data_folder()),
            Manager::Launchd | Manager::Systemd => log_file(&self.home),
        }
    }

    /// What goes in the service file. On Windows it names your user, which
    /// is read now.
    pub fn contents(&self) -> String {
        match self.manager {
            Manager::Launchd => plist(&self.exe, &self.path, &self.log_file()),
            Manager::Systemd => systemd_unit(&self.exe, &self.path),
            Manager::TaskScheduler => {
                self.task_contents(&maxhq_os::home::user_sid().unwrap_or_default())
            }
        }
    }

    /// The logon task for the user with the security ID `sid`: the
    /// launcher in MaxHQ's folder, running this setup's `maxd`.
    pub fn task_contents(&self, sid: &str) -> String {
        task_xml(sid, &launcher_program(&self.app_folder()), &self.exe)
    }

    /// Whether launchd knows the service, loaded in your session.
    fn loaded(&self, runner: &mut dyn Runner) -> bool {
        runner
            .run("launchctl", &["print", &self.target()])
            .is_ok_and(|ran| ran.success)
    }

    /// launchd's name for your login session.
    fn domain(&self) -> String {
        format!("gui/{}", self.uid)
    }

    /// launchd's name for the service in your session.
    fn target(&self) -> String {
        format!("gui/{}/{LABEL}", self.uid)
    }
}

/// `~/.local/bin/maxd` when it leads to `exe`, so the service keeps
/// running the newest version after an update flips that link. Any other
/// `exe` is kept as it is.
pub fn stable_exe(home: &Path, exe: PathBuf) -> PathBuf {
    stable_program(&home.join(".local/bin/maxd"), exe)
}

/// `link` when it leads to `exe`, which is canonical, and `exe` otherwise.
/// On Windows `link` is `maxd.exe` through the `bin\current` junction.
fn stable_program(link: &Path, exe: PathBuf) -> PathBuf {
    match fs::canonicalize(link) {
        Ok(target) if target == exe => link.to_owned(),
        _ => exe,
    }
}

/// Your user ID, from `id -u`.
fn user_id() -> Result<u32, ServiceError> {
    let lookup = |source| ServiceError::Lookup {
        what: "your user ID",
        source,
    };
    let ran = System.run("id", &["-u"]).map_err(lookup)?;
    ran.stdout.trim().parse().map_err(|_| {
        lookup(io::Error::other(format!(
            "`id -u` printed {:?}",
            ran.stdout
        )))
    })
}

/// Your `PATH` without its relative, repeated and project folders, followed
/// by the usual places agent CLIs live. Also returns the project folders it
/// left out.
pub fn service_path(current: Option<&OsStr>, home: &Path) -> (String, Vec<String>) {
    service_path_where(current, home, &only_root_changes)
}

/// [`service_path`], with `locked` saying which folders only root can change.
fn service_path_where(
    current: Option<&OsStr>,
    home: &Path,
    locked: &dyn Fn(&Path) -> bool,
) -> (String, Vec<String>) {
    let current = current.map(OsStr::to_string_lossy).unwrap_or_default();
    let local = home.join(".local/bin").to_string_lossy().into_owned();
    let usual: Vec<&str> = std::iter::once(local.as_str())
        .chain(EXTRA_PATH.iter().copied())
        .collect();
    let mut dirs: Vec<&str> = Vec::new();
    let mut left_out: Vec<String> = Vec::new();
    for dir in current.split(':') {
        if !dir.starts_with('/') || dirs.contains(&dir) {
            continue;
        }
        if usual.contains(&dir) || !project_folder(Path::new(dir), home, locked) {
            dirs.push(dir);
        } else if !left_out.iter().any(|out| out == dir) {
            left_out.push(dir.to_owned());
        }
    }
    for dir in usual {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    (dirs.join(":"), left_out)
}

/// Whether a project decides what's in `dir`, because it's a package's
/// `node_modules/.bin`, a virtual environment or a git work tree. Whatever
/// you clone or pull there would then run as `maxd`. A clone that's an
/// installed tool, such as `~/.nvm` or Homebrew, isn't a project, and nor is
/// a folder that only root can change, such as a system nvm.
fn project_folder(dir: &Path, home: &Path, locked: &dyn Fn(&Path) -> bool) -> bool {
    let real = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_owned());
    if locked(dir) && locked(&real) {
        return false;
    }
    let real_home = fs::canonicalize(home).unwrap_or_else(|_| home.to_owned());
    [dir, real.as_path()].into_iter().any(|at| {
        at.ends_with("node_modules/.bin")
            || at.ancestors().any(|up| up.join("pyvenv.cfg").exists())
            || at
                .ancestors()
                .find(|up| up.join(".git").symlink_metadata().is_ok())
                .is_some_and(|top| !tool_clone(top, home) && !tool_clone(top, &real_home))
    })
}

/// Whether only root can change what `dir` names: every folder and link on
/// the way there passes [`root_only`]. A folder above it that you own would
/// let you swap it out.
fn only_root_changes(dir: &Path) -> bool {
    dir.is_absolute()
        && dir.ancestors().all(|up| {
            up.symlink_metadata().is_ok_and(|meta| {
                fsmode::owner_of(&meta).is_some_and(|owner| {
                    root_only(owner, fsmode::mode_of(&meta), meta.file_type().is_symlink())
                })
            })
        })
}

/// Whether root owns this and no group or other user may write to it. A
/// link's own mode means nothing.
fn root_only(owner: u32, mode: u32, link: bool) -> bool {
    owner == 0 && (link || mode & 0o022 == 0)
}

/// Whether the git clone at `top` is an installed tool or your home's own
/// files: `/`, your home, a dot-folder right in it, a folder right in
/// `~/.config` or `~/.local/share`, or Homebrew. A clone any deeper, such as
/// a worktree under `~/.claude`, is a project.
fn tool_clone(top: &Path, home: &Path) -> bool {
    let Some(parent) = top.parent() else {
        return true;
    };
    let dotted = top
        .file_name()
        .is_some_and(|name| name.as_encoded_bytes().starts_with(b"."));
    top == home
        || top == Path::new("/opt/homebrew")
        || (parent == home && dotted)
        || parent == home.join(".config")
        || parent == home.join(".local/share")
}

/// Folders CLIs install to that a LaunchAgent's short `PATH` misses.
const INSTALL_DIRS: [&str; 5] = [
    "~/.local/bin",
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "~/.nix-profile/bin",
    "/run/current-system/sw/bin",
];

/// The executable called `name`, from `PATH` or a usual install folder.
pub fn find_program(name: &str) -> Option<PathBuf> {
    let home = std::env::home_dir();
    let path = std::env::var_os("PATH").unwrap_or_default();
    find_on(&path, home.as_deref(), name)
}

/// The executable called `name`, from an absolute folder on `path` or a
/// usual install folder. A relative folder would depend on where `maxd`
/// started.
fn find_on(path: &OsStr, home: Option<&Path>, name: &str) -> Option<PathBuf> {
    use maxhq_os::which::{self, Env, Rules};

    let usual: Vec<PathBuf> = INSTALL_DIRS
        .iter()
        .filter_map(|dir| match dir.strip_prefix("~/") {
            Some(rest) => home.map(|home| home.join(rest)),
            None => Some(PathBuf::from(dir)),
        })
        .collect();
    let env = Env {
        path: Some(path.to_owned()),
        ..Env::current()
    };
    which::find_in(Rules::HOST, name, &usual, &env, &which::Host)
}

/// Where launchd writes what `maxd` logs for the user at `home`.
pub fn log_file(home: &Path) -> PathBuf {
    home.join("Library/Logs/MaxHQ/maxd.log")
}

/// How big launchd's log of `maxd` grows before a start moves it aside.
pub const LOG_LIMIT: u64 = 16 << 20;

/// Moves the log at `path` to `<path>.1` once it's past `limit`, over the
/// last one there, and gives a fresh file in its place. launchd opens the
/// log once and never trims it, so only the file `writing` points at moves.
#[cfg(unix)]
pub fn rotate_log(path: &Path, writing: &File, limit: u64) -> io::Result<Option<File>> {
    let log = match fs::metadata(path) {
        Ok(log) => log,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    if log.len() <= limit || !fsmode::same_file(writing, path)? {
        return Ok(None);
    }
    let mut last = path.as_os_str().to_owned();
    last.push(".1");
    fs::rename(path, last)?;
    fsmode::set_open_mode(File::options().create(true).append(true), 0o600)
        .open(path)
        .map(Some)
}

/// The LaunchAgent. launchd starts `maxd run` at login and again whenever
/// it exits. It leaves the rest of the job's process group alone when
/// `maxd` stops, so the holders `maxd` started keep their agents running.
pub fn plist(exe: &Path, path: &str, log: &Path) -> String {
    let exe = xml(&exe.to_string_lossy());
    let path = xml(path);
    let log = xml(&log.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{exe}</string>
		<string>run</string>
	</array>
	<key>EnvironmentVariables</key>
	<dict>
		<key>PATH</key>
		<string>{path}</string>
	</dict>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<true/>
	<key>AbandonProcessGroup</key>
	<true/>
	<key>StandardOutPath</key>
	<string>{log}</string>
	<key>StandardErrorPath</key>
	<string>{log}</string>
</dict>
</plist>
"#
    )
}

/// The systemd user unit. systemd starts `maxd run` with your user manager
/// and again whenever it exits.
pub fn systemd_unit(exe: &Path, path: &str) -> String {
    let exe = systemd_quote(&exe.to_string_lossy(), true);
    let path = systemd_quote(&format!("PATH={path}"), false);
    format!(
        "# Written by `maxd service install`. `maxd service uninstall` removes it.
[Unit]
Description=MaxHQ daemon, which watches the AI coding agents on this device

[Service]
ExecStart={exe} run
Environment={path}
Restart=always
RestartSec=2

[Install]
WantedBy=default.target
"
    )
}

/// Escapes text for an XML element.
fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Quotes a word for a systemd unit. `%` starts a specifier everywhere,
/// and in a command line `$` starts a variable too.
fn systemd_quote(text: &str, command: bool) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for c in text.chars() {
        match c {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '%' => quoted.push_str("%%"),
            '$' if command => quoted.push_str("$$"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// Writes the service file and starts the service. Running it again
/// replaces the file and restarts `maxd`. Where the NixOS module owns the
/// unit, it writes and runs nothing. Returns what to tell you.
pub fn install(setup: &Setup, runner: &mut dyn Runner) -> Result<Vec<String>, ServiceError> {
    if setup.module_owns_unit() {
        return Ok(owned_by_module(setup));
    }
    let file = setup.file();
    let mut report = Vec::new();
    match setup.manager {
        Manager::Launchd => {
            make_dir(&setup.home.join("Library/LaunchAgents"), 0o755)?;
            make_dir(&setup.home.join("Library/Logs/MaxHQ"), 0o700)?;
            write_atomically(&file, &setup.contents())?;
            // Take down any copy from an earlier install, so the new file
            // and program are the ones that run. None loaded is fine.
            let _ = runner.run("launchctl", &["bootout", &setup.target()]);
            // Bootstrapping while the old copy is still stopping fails
            // with error 5, so wait for launchd to forget it first.
            wait_unloaded(runner, &setup.target(), GONE_TRIES, GONE_PAUSE)?;
            // A service you once disabled stays disabled until enabled.
            let _ = runner.run("launchctl", &["enable", &setup.target()]);
            let domain = setup.domain();
            let file_arg = file.to_string_lossy();
            retry(runner, "launchctl", &["bootstrap", &domain, &file_arg])?;
            report.push(format!("Installed {}.", file.display()));
            report.push(format!(
                "maxd runs now and at every login, and logs to {}.",
                setup.log_file().display()
            ));
        }
        Manager::Systemd => {
            make_dir(&setup.config_home.join("systemd/user"), 0o755)?;
            write_atomically(&file, &setup.contents())?;
            must(runner, "systemctl", &["--user", "daemon-reload"])?;
            must(runner, "systemctl", &["--user", "enable", UNIT])?;
            // A restart starts a stopped service and picks up a new
            // program in a running one.
            must(runner, "systemctl", &["--user", "restart", UNIT])?;
            report.push(format!("Installed {}.", file.display()));
            report.push(format!(
                "maxd runs now and whenever your user manager starts. See its log with `journalctl --user -u {UNIT}`."
            ));
            let uid = setup.uid.to_string();
            let linger = runner.run(
                "loginctl",
                &["show-user", &uid, "--property=Linger", "--value"],
            );
            if linger.is_ok_and(|ran| ran.success && ran.stdout.trim() == "no") {
                report.push(
                    "Your user manager stops when you log out, and maxd with it. `loginctl enable-linger` keeps it running."
                        .into(),
                );
            }
        }
        Manager::TaskScheduler => report = task::install(setup, runner)?,
    }
    report.extend(setup.left_out_note());
    Ok(report)
}

/// Starts the installed service, which then runs until you log out, and
/// again when you next log in. Where it runs already, that's left alone.
/// Returns what to tell you.
pub fn start(setup: &Setup, runner: &mut dyn Runner) -> Result<Vec<String>, ServiceError> {
    match setup.manager {
        Manager::Launchd => {
            let target = setup.target();
            if setup.loaded(runner) {
                // Without -k, a running maxd is left as it is.
                must(runner, "launchctl", &["kickstart", &target])?;
            } else {
                let file = setup.file();
                if !file.exists() {
                    return Err(ServiceError::NotInstalled(file));
                }
                let _ = runner.run("launchctl", &["enable", &target]);
                let file_arg = file.to_string_lossy();
                retry(
                    runner,
                    "launchctl",
                    &["bootstrap", &setup.domain(), &file_arg],
                )?;
            }
        }
        Manager::Systemd => {
            must(runner, "systemctl", &["--user", "start", UNIT])?;
        }
        Manager::TaskScheduler => {
            if !task::start(setup, runner)? {
                return Ok(vec!["maxd is running already.".into()]);
            }
        }
    }
    Ok(vec!["Started maxd.".into()])
}

/// Stops the service, and with it `maxd`, until you next log in or start
/// it. The agents its holders keep go on running. Returns what to tell
/// you.
pub fn stop(setup: &Setup, runner: &mut dyn Runner) -> Result<Vec<String>, ServiceError> {
    let stopped = match setup.manager {
        Manager::Launchd => {
            let target = setup.target();
            let loaded = setup.loaded(runner);
            if loaded {
                let _ = runner.run("launchctl", &["bootout", &target]);
                wait_unloaded(runner, &target, GONE_TRIES, GONE_PAUSE)?;
            }
            loaded
        }
        Manager::Systemd => {
            must(runner, "systemctl", &["--user", "stop", UNIT])?;
            true
        }
        Manager::TaskScheduler => task::stop(setup, runner)?,
    };
    Ok(vec![if stopped {
        "Stopped maxd. It starts again when you next log in, or with `maxd service start`.".into()
    } else {
        "maxd wasn't running.".into()
    }])
}

/// Stops the service and starts it again, or starts one that's stopped.
/// Returns what to tell you.
pub fn restart(setup: &Setup, runner: &mut dyn Runner) -> Result<Vec<String>, ServiceError> {
    match setup.manager {
        Manager::Launchd if !setup.loaded(runner) => return start(setup, runner),
        // The new launcher hands over to the one that runs maxd.
        Manager::TaskScheduler => task::run(setup, runner)?,
        manager => {
            let (program, args) = restart_command(manager, setup.uid);
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            must(runner, program, &args)?;
        }
    }
    Ok(vec!["Restarted maxd.".into()])
}

/// What install says where the NixOS module owns maxd's unit, and how to
/// remove a unit of its own that shadows the module's, which it leaves in
/// place.
pub fn owned_by_module(setup: &Setup) -> Vec<String> {
    let mut report = vec![format!(
        "The NixOS module owns maxd's unit, so this wrote nothing. Restart maxd with `systemctl --user restart {UNIT}`."
    )];
    let file = setup.file();
    if file.exists() {
        report.push(format!(
            "{} shadows the module's unit. Remove it with `{}`.",
            file.display(),
            setup.unshadow_command()
        ));
    }
    report
}

/// Whether the service is installed and running, and whether the socket
/// answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// The service file.
    pub file: PathBuf,
    /// Whether the file is there.
    pub installed: bool,
    /// What the service manager says, such as "running" or "not loaded".
    pub state: String,
    /// The process ID, while it runs.
    pub pid: Option<u32>,
    /// The socket, and whether it accepts a connection.
    pub socket: Option<(PathBuf, bool)>,
}

impl Status {
    /// Whether `maxd` is running as a service.
    pub fn running(&self) -> bool {
        self.pid.is_some()
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let installed = if self.installed {
            "installed"
        } else {
            "not installed"
        };
        writeln!(f, "file: {} ({installed})", self.file.display())?;
        match self.pid {
            Some(pid) => writeln!(f, "state: {}, process {pid}", self.state)?,
            None => writeln!(f, "state: {}", self.state)?,
        }
        if let Some((path, answers)) = &self.socket {
            let answers = if *answers {
                "answering"
            } else {
                "not answering"
            };
            writeln!(f, "socket: {} ({answers})", path.display())?;
        }
        Ok(())
    }
}

/// Asks the service manager how `maxd` is doing.
pub fn status(setup: &Setup, runner: &mut dyn Runner) -> Result<Status, ServiceError> {
    let file = setup.file();
    let (state, pid) = match setup.manager {
        Manager::Launchd => match runner.run("launchctl", &["print", &setup.target()]) {
            Ok(ran) if ran.success => parse_launchctl_print(&ran.stdout),
            _ => ("not loaded".into(), None),
        },
        Manager::Systemd => {
            let show = [
                "--user",
                "show",
                UNIT,
                "--property=LoadState,ActiveState,SubState,MainPID",
            ];
            let ran = runner
                .run("systemctl", &show)
                .map_err(|err| command_error("systemctl", &show, &err.to_string()))?;
            if !ran.success {
                return Err(command_error("systemctl", &show, &ran.stderr));
            }
            parse_systemctl_show(&ran.stdout)
        }
        Manager::TaskScheduler => task::state(setup, runner),
    };
    let socket = setup.socket.as_ref().map(|path| {
        let answers = maxhq_os::ipc::answers(path);
        (path.clone(), answers)
    });
    Ok(Status {
        installed: file.exists(),
        file,
        state,
        pid,
        socket,
    })
}

/// Reads the state and process ID out of `launchctl print`. The first
/// `state =` and `pid =` lines belong to the service itself, and later ones
/// to its nested parts.
fn parse_launchctl_print(text: &str) -> (String, Option<u32>) {
    let field = |name: &str| {
        text.lines().find_map(|line| {
            let (key, value) = line.split_once(" = ")?;
            (key.trim() == name).then(|| value.trim().to_owned())
        })
    };
    let state = field("state").unwrap_or_else(|| "loaded".into());
    let pid = field("pid").and_then(|pid| pid.parse().ok());
    (state, pid)
}

/// Reads the state and process ID out of `systemctl show`.
fn parse_systemctl_show(text: &str) -> (String, Option<u32>) {
    let field = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
            .map(str::trim)
            .unwrap_or_default()
    };
    if field("LoadState") == "not-found" {
        return ("not loaded".into(), None);
    }
    let state = match (field("ActiveState"), field("SubState")) {
        ("", _) => "unknown".to_owned(),
        (active, "") => active.to_owned(),
        (active, sub) => format!("{active}, {sub}"),
    };
    let pid = field("MainPID").parse().ok().filter(|pid| *pid != 0);
    (state, pid)
}

/// Runs a command that must succeed.
fn must(runner: &mut dyn Runner, program: &str, args: &[&str]) -> Result<Ran, ServiceError> {
    let ran = runner
        .run(program, args)
        .map_err(|err| command_error(program, args, &err.to_string()))?;
    if ran.success {
        Ok(ran)
    } else {
        Err(command_error(program, args, &ran.stderr))
    }
}

/// Runs a command until it succeeds, a few times at most.
fn retry(runner: &mut dyn Runner, program: &str, args: &[&str]) -> Result<Ran, ServiceError> {
    let mut tries = 1;
    loop {
        match must(runner, program, args) {
            Ok(ran) => return Ok(ran),
            Err(err) if tries >= BOOTSTRAP_TRIES => return Err(err),
            Err(_) => {
                tries += 1;
                thread::sleep(BOOTSTRAP_PAUSE);
            }
        }
    }
}

/// Waits until launchd no longer knows the job `target`, asking at most
/// `tries` times with `pause` between. A job that stays is an error.
fn wait_unloaded(
    runner: &mut dyn Runner,
    target: &str,
    tries: u32,
    pause: Duration,
) -> Result<(), ServiceError> {
    for tried in 1..=tries {
        match runner.run("launchctl", &["print", target]) {
            Ok(ran) if ran.success => {}
            // An unknown job, or no launchctl, which bootstrap reports.
            _ => return Ok(()),
        }
        if tried < tries {
            thread::sleep(pause);
        }
    }
    Err(command_error(
        "launchctl",
        &["bootout", target],
        &format!(
            "maxd was still loaded after {} seconds",
            (pause * tries).as_secs()
        ),
    ))
}

/// The error for a command that failed, naming the whole command line.
fn command_error(program: &str, args: &[&str], detail: &str) -> ServiceError {
    let detail = detail.trim();
    ServiceError::Command {
        command: std::iter::once(program)
            .chain(args.iter().copied())
            .collect::<Vec<_>>()
            .join(" "),
        detail: if detail.is_empty() {
            "it printed nothing".into()
        } else {
            detail.into()
        },
    }
}

/// Makes `dir` and any folders above it that are missing.
fn make_dir(dir: &Path, mode: u32) -> Result<(), ServiceError> {
    fsmode::dir_all_with_mode(dir, mode).map_err(|source| ServiceError::Io {
        path: dir.into(),
        source,
    })
}

/// Writes the file whole, so the service manager never reads half of one.
fn write_atomically(path: &Path, contents: &str) -> Result<(), ServiceError> {
    let io = |source| ServiceError::Io {
        path: path.into(),
        source,
    };
    write_atomic(path, contents.as_bytes(), Atomic::PLAIN).map_err(io)
}

/// Removes a file, which may already be gone.
fn remove(path: &Path) -> Result<(), ServiceError> {
    match fs::remove_file(path) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(ServiceError::Io {
            path: path.into(),
            source: err,
        }),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    /// Records every command and answers each with a canned result. The
    /// first answer given whose prefix fits is the one used.
    #[derive(Default)]
    pub(super) struct Fake {
        pub(super) ran: Vec<String>,
        answers: Vec<(&'static str, Ran, bool)>,
    }

    impl Fake {
        /// Answers any command line that starts with `prefix`.
        pub(super) fn answer(mut self, prefix: &'static str, stdout: &str) -> Self {
            self.answers.push((prefix, Self::ok(stdout), false));
            self
        }

        /// Answers the next `times` command lines that start with
        /// `prefix`, then lets them fall through.
        pub(super) fn answer_times(
            mut self,
            prefix: &'static str,
            stdout: &str,
            times: usize,
        ) -> Self {
            for _ in 0..times {
                self.answers.push((prefix, Self::ok(stdout), true));
            }
            self
        }

        /// Fails any command line that starts with `prefix`, printing
        /// `stderr`.
        pub(super) fn fail(mut self, prefix: &'static str, stderr: &str) -> Self {
            let ran = Ran {
                success: false,
                stdout: String::new(),
                stderr: stderr.into(),
            };
            self.answers.push((prefix, ran, false));
            self
        }

        fn ok(stdout: &str) -> Ran {
            Ran {
                success: true,
                stdout: stdout.into(),
                stderr: String::new(),
            }
        }
    }

    impl Runner for Fake {
        fn run(&mut self, program: &str, args: &[&str]) -> io::Result<Ran> {
            let line = std::iter::once(program)
                .chain(args.iter().copied())
                .collect::<Vec<_>>()
                .join(" ");
            self.ran.push(line.clone());
            let found = self
                .answers
                .iter()
                .position(|(prefix, _, _)| line.starts_with(prefix));
            let answer = found.map(|at| {
                if self.answers[at].2 {
                    self.answers.remove(at).1
                } else {
                    self.answers[at].1.clone()
                }
            });
            // Unless told otherwise, launchd knows no job, as before any
            // install.
            Ok(answer.unwrap_or(Ran {
                success: !line.starts_with("launchctl print"),
                ..Ran::default()
            }))
        }
    }

    /// A setup with everything under `dir`.
    pub(super) fn setup(dir: &TempDir, manager: Manager) -> Setup {
        let home = dir.path().join("home");
        Setup {
            manager,
            config_home: home.join(".config"),
            uid: 501,
            exe: "/Applications/MaxHQ.app/Contents/MacOS/maxd".into(),
            path: "/usr/bin:/bin".into(),
            left_out: Vec::new(),
            socket: Some(dir.path().join("maxd.sock")),
            root: dir.path().to_owned(),
            data: Some(home.join("Library/Application Support/MaxHQ")),
            runtime: Some(dir.path().join("run/maxhq")),
            app: None,
            home,
        }
    }

    #[test]
    fn the_plist_runs_maxd_at_login_and_keeps_it_alive() {
        let text = plist(
            Path::new("/Apps/Max & Co/maxd"),
            "/usr/bin:/bin",
            Path::new("/Users/max/Library/Logs/MaxHQ/maxd.log"),
        );
        assert!(text.contains("<string>com.maxagenthq.maxd</string>"));
        assert!(text.contains(
            "<array>\n\t\t<string>/Apps/Max &amp; Co/maxd</string>\n\t\t<string>run</string>\n\t</array>"
        ));
        assert!(text.contains("<key>RunAtLoad</key>\n\t<true/>"));
        assert!(text.contains("<key>KeepAlive</key>\n\t<true/>"));
        assert!(text.contains("<key>AbandonProcessGroup</key>\n\t<true/>"));
        assert!(text.contains("<key>PATH</key>\n\t\t<string>/usr/bin:/bin</string>"));
        assert!(text.contains(
            "<key>StandardErrorPath</key>\n\t<string>/Users/max/Library/Logs/MaxHQ/maxd.log</string>"
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn plutil_accepts_the_plist() {
        let dir = TempDir::new();
        let file = dir.path().join("test.plist");
        let text = plist(
            Path::new("/Apps/<odd> & \"quoted\"/maxd"),
            "/usr/bin",
            Path::new("/tmp/maxd.log"),
        );
        fs::write(&file, text).unwrap();
        let ran = System
            .run("plutil", &["-lint", &file.to_string_lossy()])
            .unwrap();
        assert!(ran.success, "{ran:?}");
    }

    #[test]
    fn the_unit_quotes_what_systemd_would_expand() {
        let text = systemd_unit(
            Path::new("/opt/max hq/100%/$bin/maxd"),
            "/run/wrappers/bin:/usr/bin",
        );
        assert!(text.contains("ExecStart=\"/opt/max hq/100%%/$$bin/maxd\" run\n"));
        assert!(text.contains("Environment=\"PATH=/run/wrappers/bin:/usr/bin\"\n"));
        assert!(text.contains("Restart=always\n"));
        assert!(text.contains("WantedBy=default.target\n"));
    }

    // The service's PATH is a Unix unit's.
    #[cfg(unix)]
    #[test]
    fn the_service_path_keeps_yours_and_adds_the_usual_places() {
        let (path, left_out) = service_path(
            Some(OsStr::new("/nix/bin:.:/usr/bin::/nix/bin")),
            Path::new("/Users/max"),
        );
        assert_eq!(
            path,
            "/nix/bin:/usr/bin:/Users/max/.local/bin:/opt/homebrew/bin:/usr/local/bin:/bin:/usr/sbin:/sbin"
        );
        assert!(left_out.is_empty());
        assert!(service_path(None, Path::new("/h"))
            .0
            .starts_with("/h/.local/bin:"));
    }

    // The service's PATH is a Unix unit's.
    #[cfg(unix)]
    #[test]
    fn the_service_path_keeps_a_clone_only_root_can_change() {
        let dir = TempDir::new();
        let root = fs::canonicalize(dir.path()).unwrap();
        let home = root.join("home");
        let at = |name: &str| {
            let at = root.join(name);
            fs::create_dir_all(&at).unwrap();
            at
        };
        at("home");
        // A system nvm, and a clone with the same shape a user can change.
        at("usr/local/nvm/.git");
        let system = at("usr/local/nvm/versions/node/v22/bin");
        at("srv/app/.git");
        let shared = at("srv/app/bin");
        // A virtual environment and global packages only root can change.
        let venv = at("usr/local/venv");
        fs::write(venv.join("pyvenv.cfg"), "").unwrap();
        let venv_bin = at("usr/local/venv/bin");
        let packages = at("usr/local/lib/node_modules/.bin");
        // A repo's link into there, which a pull can point somewhere else.
        at("home/code/app/.git");
        let link = root.join("home/code/app/bin");
        std::os::unix::fs::symlink(&system, &link).unwrap();

        let path = [&system, &shared, &venv_bin, &packages, &link]
            .map(|dir| dir.to_str().unwrap())
            .join(":");
        let locked = root.join("usr/local");
        let (kept, left_out) = service_path_where(Some(OsStr::new(&path)), &home, &|dir: &Path| {
            dir.starts_with(&locked)
        });

        let kept: Vec<&str> = kept.split(':').collect();
        for dir in [&system, &venv_bin, &packages] {
            assert!(kept.contains(&dir.to_str().unwrap()), "{dir:?} in {kept:?}");
        }
        assert_eq!(left_out, [shared.to_str().unwrap(), link.to_str().unwrap()]);
    }

    // The service's PATH is a Unix unit's.
    #[cfg(unix)]
    #[test]
    fn only_root_changes_a_folder_root_owns_all_the_way_up() {
        use std::os::unix::fs::PermissionsExt;

        assert!(root_only(0, 0o755, false));
        assert!(root_only(0, 0o777, true));
        assert!(!root_only(501, 0o755, false));
        assert!(!root_only(501, 0o777, true));
        assert!(!root_only(0, 0o775, false));
        assert!(!root_only(0, 0o757, false));
        assert!(!root_only(0, 0o1777, false));

        assert!(only_root_changes(Path::new("/usr/bin")));
        assert!(!only_root_changes(Path::new("usr/bin")));
        // Even for a test running as root, others may write to this one.
        let dir = TempDir::new();
        let open = fs::canonicalize(dir.path()).unwrap().join("open");
        let below = open.join("below");
        fs::create_dir_all(&below).unwrap();
        fs::set_permissions(&open, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(!only_root_changes(&below));
        assert!(!only_root_changes(&open));
    }

    // The service's PATH is a Unix unit's.
    #[cfg(unix)]
    #[test]
    fn the_service_path_leaves_out_what_a_project_decides() {
        let dir = TempDir::new();
        let home = fs::canonicalize(dir.path()).unwrap().join("home");
        let folder = |name: &str| {
            let at = home.join(name);
            fs::create_dir_all(&at).unwrap();
            at.to_str().unwrap().to_owned()
        };
        // Git work trees, of a clone and of a linked worktree.
        folder("code/app/.git");
        fs::create_dir_all(home.join("linked")).unwrap();
        fs::write(home.join("linked/.git"), "gitdir: elsewhere\n").unwrap();
        folder(".claude/worktrees/app/.git");
        // Installed tools that are clones, and your home's own files.
        folder(".git");
        folder(".nvm/.git");
        folder(".config/nvm/.git");
        folder(".local/share/nvm/.git");
        folder(".local/bin/.git");
        // A virtual environment.
        folder("venv");
        fs::write(home.join("venv/pyvenv.cfg"), "").unwrap();

        let projects = [
            folder("code/app/bin"),
            home.join("code/app/not-made-yet/bin")
                .to_str()
                .unwrap()
                .to_owned(),
            folder("linked/bin"),
            folder(".claude/worktrees/app/bin"),
            folder("plain/node_modules/.bin"),
            folder("venv/bin"),
        ];
        let tools = [
            folder(".nvm/versions/node/v22/bin"),
            folder(".config/nvm/bin"),
            folder(".local/share/nvm/bin"),
            folder(".cargo/bin"),
            folder("bin"),
            "/usr/bin".to_owned(),
        ];
        let current = [projects.as_slice(), tools.as_slice()].concat();
        let (path, left_out) = service_path(Some(OsStr::new(&current.join(":"))), &home);

        assert_eq!(left_out, projects);
        let kept: Vec<&str> = path.split(':').collect();
        for tool in &tools {
            assert!(kept.contains(&tool.as_str()), "{tool} is kept");
        }
        // A usual place stays, even as a clone of its own.
        assert!(kept.contains(&home.join(".local/bin").to_str().unwrap()));
        assert!(!project_folder(
            Path::new("/opt/homebrew/opt/node/bin"),
            Path::new("/nowhere"),
            &|_| false,
        ));
        assert!(tool_clone(Path::new("/opt/homebrew"), Path::new("/h")));
        assert!(!tool_clone(Path::new("/opt/flutter"), Path::new("/h")));
    }

    // The service's PATH is a Unix unit's.
    #[cfg(unix)]
    #[test]
    fn a_relative_folder_on_path_finds_no_program() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let program = dir.path().join("maxhq-test-program");
        fs::write(&program, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        let relative = crate::testing::from_here(dir.path());
        assert_eq!(
            find_on(relative.as_os_str(), None, "maxhq-test-program"),
            None
        );
        assert_eq!(
            find_on(dir.path().as_os_str(), None, "maxhq-test-program"),
            Some(program)
        );
    }

    #[test]
    fn install_says_which_folders_it_left_out() {
        let dir = TempDir::new();
        let mut setup = setup(&dir, Manager::Launchd);
        let report = install(&setup, &mut Fake::default()).unwrap();
        assert!(!report.iter().any(|line| line.contains("Left out")));

        setup.left_out = vec!["/home/dev/code/app/bin".to_owned()];
        let report = install(&setup, &mut Fake::default()).unwrap();
        assert!(report
            .iter()
            .any(|line| line.contains("Left out") && line.contains("/home/dev/code/app/bin")));
    }

    #[test]
    fn install_writes_the_launch_agent_under_home_and_bootstraps_it() {
        let dir = TempDir::new();
        let setup = setup(&dir, Manager::Launchd);
        let mut fake = Fake::default();
        let report = install(&setup, &mut fake).unwrap();

        let file = dir
            .path()
            .join("home/Library/LaunchAgents/com.maxagenthq.maxd.plist");
        assert_eq!(setup.file(), file);
        assert_eq!(fs::read_to_string(&file).unwrap(), setup.contents());
        assert!(dir.path().join("home/Library/Logs/MaxHQ").is_dir());
        assert_eq!(
            fake.ran,
            [
                "launchctl bootout gui/501/com.maxagenthq.maxd".to_owned(),
                "launchctl print gui/501/com.maxagenthq.maxd".to_owned(),
                "launchctl enable gui/501/com.maxagenthq.maxd".to_owned(),
                // The same file, as the code spells it on this system.
                format!("launchctl bootstrap gui/501 {}", setup.file().display()),
            ]
        );
        assert!(report[0].contains("Installed"));

        // Installing again replaces the file in place.
        install(&setup, &mut Fake::default()).unwrap();
        let names: Vec<_> = fs::read_dir(file.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["com.maxagenthq.maxd.plist"]);
    }

    #[test]
    fn a_failed_bootstrap_is_retried_then_reported() {
        struct Refuses(u32);
        impl Runner for Refuses {
            fn run(&mut self, _: &str, args: &[&str]) -> io::Result<Ran> {
                let bootstrap = args.first() == Some(&"bootstrap");
                let print = args.first() == Some(&"print");
                self.0 += u32::from(bootstrap);
                Ok(Ran {
                    success: !bootstrap && !print,
                    stdout: String::new(),
                    stderr: "Bootstrap failed: 5: Input/output error".into(),
                })
            }
        }
        let dir = TempDir::new();
        let mut refuses = Refuses(0);
        let err = install(&setup(&dir, Manager::Launchd), &mut refuses).unwrap_err();
        assert_eq!(refuses.0, BOOTSTRAP_TRIES);
        assert!(err.to_string().contains("Input/output error"), "{err}");
    }

    #[test]
    fn install_waits_for_the_old_copy_to_go_before_it_bootstraps() {
        let dir = TempDir::new();
        let setup = setup(&dir, Manager::Launchd);
        // launchd still knows the old job for two checks after bootout.
        let mut fake = Fake::default().answer_times("launchctl print", "", 2);
        install(&setup, &mut fake).unwrap();
        let print = "launchctl print gui/501/com.maxagenthq.maxd";
        assert_eq!(
            fake.ran,
            [
                "launchctl bootout gui/501/com.maxagenthq.maxd".to_owned(),
                print.to_owned(),
                print.to_owned(),
                print.to_owned(),
                "launchctl enable gui/501/com.maxagenthq.maxd".to_owned(),
                format!("launchctl bootstrap gui/501 {}", setup.file().display()),
            ]
        );
    }

    #[test]
    fn an_old_copy_that_never_goes_is_reported() {
        let mut fake = Fake::default().answer("launchctl print", "");
        let target = "gui/501/com.maxagenthq.maxd";
        let err = wait_unloaded(&mut fake, target, 3, Duration::ZERO).unwrap_err();
        assert_eq!(fake.ran.len(), 3, "{:?}", fake.ran);
        assert!(err.to_string().contains("still loaded"), "{err}");
    }

    #[test]
    fn install_writes_the_user_unit_and_starts_it() {
        let dir = TempDir::new();
        let setup = setup(&dir, Manager::Systemd);
        let mut fake = Fake::default().answer("loginctl", "no\n");
        let report = install(&setup, &mut fake).unwrap();

        let file = dir.path().join("home/.config/systemd/user/maxd.service");
        assert_eq!(setup.file(), file);
        assert_eq!(fs::read_to_string(&file).unwrap(), setup.contents());
        assert_eq!(
            fake.ran,
            [
                "systemctl --user daemon-reload",
                "systemctl --user enable maxd.service",
                "systemctl --user restart maxd.service",
                "loginctl show-user 501 --property=Linger --value",
            ]
        );
        assert!(report.iter().any(|line| line.contains("enable-linger")));
    }

    /// Puts the NixOS module's unit under the temp root.
    pub(super) fn module_unit(dir: &TempDir) -> PathBuf {
        let unit = dir.path().join("etc/systemd/user/maxd.service");
        fs::create_dir_all(unit.parent().unwrap()).unwrap();
        fs::write(&unit, "made by the module").unwrap();
        unit
    }

    #[test]
    fn install_leaves_the_unit_to_the_nixos_module() {
        let dir = TempDir::new();
        let setup = setup(&dir, Manager::Systemd);
        let module = module_unit(&dir);
        let mut fake = Fake::default();
        let report = install(&setup, &mut fake).unwrap();
        assert!(fake.ran.is_empty(), "{:?}", fake.ran);
        assert!(!setup.file().exists());
        assert_eq!(
            report,
            ["The NixOS module owns maxd's unit, so this wrote nothing. Restart maxd with `systemctl --user restart maxd.service`."]
        );
        assert_eq!(fs::read_to_string(&module).unwrap(), "made by the module");

        // launchd has no such unit, so a Mac installs as ever.
        let mac = Setup {
            manager: Manager::Launchd,
            ..setup
        };
        assert!(!mac.module_owns_unit());
    }

    #[test]
    fn install_says_how_to_remove_a_unit_that_shadows_the_modules() {
        let dir = TempDir::new();
        let setup = setup(&dir, Manager::Systemd);
        module_unit(&dir);
        fs::create_dir_all(setup.file().parent().unwrap()).unwrap();
        fs::write(setup.file(), "an older install").unwrap();
        let mut fake = Fake::default();
        let report = install(&setup, &mut fake).unwrap();
        assert!(fake.ran.is_empty(), "{:?}", fake.ran);
        assert_eq!(
            fs::read_to_string(setup.file()).unwrap(),
            "an older install"
        );
        assert_eq!(report.len(), 2, "{report:?}");
        assert_eq!(
            report[1],
            format!(
                "{} shadows the module's unit. Remove it with `/Applications/MaxHQ.app/Contents/MacOS/maxd service uninstall && systemctl --user restart maxd.service`.",
                setup.file().display()
            )
        );
    }

    #[test]
    fn status_reads_launchd() {
        let dir = TempDir::new();
        let setup = setup(&dir, Manager::Launchd);
        let print = "gui/501/com.maxagenthq.maxd = {\n\tactive count = 1\n\tpath = /x.plist\n\ttype = LaunchAgent\n\tstate = running\n\n\tprogram = /maxd\n\tpid = 4242\n\tendpoints = {\n\t\tstate = active\n\t}\n}\n";
        let mut fake = Fake::default().answer("launchctl print", print);
        let status = status(&setup, &mut fake).unwrap();
        assert_eq!(status.state, "running");
        assert_eq!(status.pid, Some(4242));
        assert!(!status.installed);
        assert_eq!(
            status.socket,
            Some((dir.path().join("maxd.sock"), false)),
            "nothing listens on the temp socket"
        );
        assert!(status.to_string().contains("state: running, process 4242"));
    }

    #[test]
    fn status_reads_systemd() {
        assert_eq!(
            parse_systemctl_show(
                "LoadState=loaded\nActiveState=active\nSubState=running\nMainPID=77\n"
            ),
            ("active, running".into(), Some(77))
        );
        assert_eq!(
            parse_systemctl_show(
                "LoadState=loaded\nActiveState=inactive\nSubState=dead\nMainPID=0\n"
            ),
            ("inactive, dead".into(), None)
        );
        assert_eq!(
            parse_systemctl_show("LoadState=not-found\nActiveState=inactive\nMainPID=0\n"),
            ("not loaded".into(), None)
        );
    }

    #[test]
    fn a_service_launchd_doesnt_know_is_not_loaded() {
        struct Missing;
        impl Runner for Missing {
            fn run(&mut self, _: &str, _: &[&str]) -> io::Result<Ran> {
                Ok(Ran::default())
            }
        }
        let dir = TempDir::new();
        let status = status(&setup(&dir, Manager::Launchd), &mut Missing).unwrap();
        assert_eq!(status.state, "not loaded");
        assert!(!status.running());
    }

    // Unix only: launchd's log is the one rotated.
    #[cfg(unix)]
    #[test]
    fn a_log_moves_aside_once_too_big_and_only_when_maxd_writes_it() {
        use std::io::Write;

        let dir = TempDir::new();
        let (log, last) = (dir.path().join("maxd.log"), dir.path().join("maxd.log.1"));
        fs::write(&log, "first run\n").unwrap();
        let writing = File::options().append(true).open(&log).unwrap();
        let other = File::create(dir.path().join("other.log")).unwrap();

        assert!(rotate_log(&log, &writing, 10).unwrap().is_none());
        assert!(rotate_log(&log, &other, 4).unwrap().is_none());
        assert!(!last.exists());

        let mut fresh = rotate_log(&log, &writing, 4).unwrap().unwrap();
        assert_eq!(fs::read_to_string(&last).unwrap(), "first run\n");
        fresh.write_all(b"second run\n").unwrap();
        assert_eq!(fs::read_to_string(&log).unwrap(), "second run\n");

        rotate_log(&log, &fresh, 4).unwrap().unwrap();
        assert_eq!(fs::read_to_string(&last).unwrap(), "second run\n");
        assert_eq!(fs::read_to_string(&log).unwrap(), "");

        let gone = dir.path().join("never.log");
        assert!(rotate_log(&gone, &writing, 4).unwrap().is_none());
    }
}
