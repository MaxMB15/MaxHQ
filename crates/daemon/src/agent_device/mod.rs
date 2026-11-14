// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! agent-device, the CLI that drives a Simulator or emulator, run inside a
//! test host's boundary. `maxd` installs the pinned release into its own
//! tools folder, runs its HTTP daemon with a state folder per host, and
//! runs one allowed command at a time for a lease with the target, config
//! and session filled in. `maxd guest` uses it in a macOS VM, and `maxd`
//! on the device for the Mac's own Simulator and the Android emulator.
//!
//! [`AgentDevice::run`] checks the command against [`allow::ALLOWED`]. The
//! other calls run the commands `maxd` needs itself: `install`,
//! `screenshot` and `close`.

pub mod allow;
mod daemon;
pub mod install;
pub mod process;

#[cfg(test)]
pub(crate) mod tests;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use maxhq_core::leases::{MAX_OUTPUT_BYTES, MAX_RUN_SECONDS};
use maxhq_protocol::ids::LeaseId;
use maxhq_protocol::leases::RunOutput;

pub use allow::{Target, VERSION};
pub use daemon::Endpoint;

/// How long `install` of an app may take.
const INSTALL_APP_WAIT: Duration = Duration::from_secs(10 * 60);

/// How long `close` may take.
const CLOSE_WAIT: Duration = Duration::from_secs(15);

/// How long `screenshot` may take.
const SCREENSHOT_WAIT: Duration = Duration::from_secs(60);

/// The largest screenshot read back, in bytes.
const MAX_SCREENSHOT_BYTES: u64 = 32 * 1024 * 1024;

/// The folder under a state folder that holds each session's files.
const SESSIONS_DIR: &str = "sessions";

/// Only one install runs at a time in this process.
static INSTALLING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Where agent-device lives for one test host, and what it runs with.
#[derive(Clone, Debug)]
pub struct Config {
    /// The tools folder, `<data>/tools`, which every host shares.
    pub tools: PathBuf,
    /// This host's state folder, for its daemon and sessions.
    pub state: PathBuf,
    /// `node`, which runs the CLI.
    pub node: PathBuf,
    /// `npm`, which installs it.
    pub npm: PathBuf,
    /// More environment for every command, such as the Android SDK's
    /// `ANDROID_HOME` and `maxd`'s `ANDROID_AVD_HOME`.
    pub env: Vec<(String, String)>,
    /// A program and its words that every command runs under, such as
    /// `sandbox-exec` with a profile, or none.
    pub wrap: Vec<String>,
}

impl Config {
    /// The folders for the host called `host` under `data`, `maxd`'s data
    /// folder, with `node` and `npm` from `PATH`.
    pub fn under(data: &Path, host: &str) -> Self {
        Self {
            tools: data.join("tools"),
            state: data.join("agent-device").join(host),
            node: PathBuf::from("node"),
            npm: PathBuf::from("npm"),
            env: Vec::new(),
            wrap: Vec::new(),
        }
    }
}

/// agent-device for one test host.
pub struct AgentDevice {
    config: Config,
    /// The installed entry point, once installed.
    entry: Mutex<Option<PathBuf>>,
}

/// The agent-device session a lease runs its commands in.
pub fn session(lease: LeaseId) -> String {
    format!("mhq-{}", lease.0.simple())
}

impl AgentDevice {
    /// agent-device as `config` places it. Nothing runs until a call.
    pub fn new(config: Config) -> Self {
        Self {
            config,
            entry: Mutex::new(None),
        }
    }

    /// The release it drives with.
    pub fn version(&self) -> &'static str {
        VERSION
    }

    /// Installs the pinned release if it isn't yet, then removes old ones
    /// no process uses. The answer is why it failed, for a person to read.
    pub async fn install(&self) -> Result<PathBuf, String> {
        if let Some(entry) = lock(&self.entry).clone() {
            return Ok(entry);
        }
        let paths = {
            let _held = INSTALLING.lock().await;
            install::ensure(&self.config.tools, &self.config.npm, VERSION).await?
        };
        if let Some(running) = install::running().await {
            install::prune(&self.config.tools, VERSION, &running);
        }
        *lock(&self.entry) = Some(paths.entry.clone());
        Ok(paths.entry)
    }

    /// Installs, then starts the daemon unless it answers its health
    /// check. Every command calls this first, so a daemon that went away
    /// comes back.
    pub async fn start(&self) -> Result<Endpoint, String> {
        let entry = self.install().await?;
        let args = ["devices".to_owned(), "--json".to_owned()];
        let start = async {
            process::run(
                self.command(&entry, &args),
                daemon::READY_WAIT,
                MAX_OUTPUT_BYTES,
            )
            .await
            .map_err(|err| format!("agent-device couldn't run: {err}"))
        };
        daemon::ensure(&self.config.state, start).await
    }

    /// Runs one command the agent gave for `session` on `target`, once
    /// [`allow::check`] passes it. The answer's error is why it couldn't
    /// run; a command that ran and failed answers its output.
    pub async fn run(
        &self,
        target: &Target,
        session: &str,
        command: &[String],
        timeout: Duration,
    ) -> Result<RunOutput, String> {
        allow::check(command)?;
        let timeout = timeout.min(Duration::from_secs(MAX_RUN_SECONDS.into()));
        self.run_unchecked(target, session, command, timeout).await
    }

    /// Installs the app at `app`, a `.app` folder or an `.apk` or `.ipa`
    /// file, for `session` on `target`.
    pub async fn install_app(
        &self,
        target: &Target,
        session: &str,
        app: &Path,
    ) -> Result<(), String> {
        let name = app
            .file_stem()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("{} doesn't name an app.", app.display()))?;
        // A full path starts with a slash, so it can't read as a flag.
        if !app.is_absolute() {
            return Err(format!("{} isn't a full path to an app.", app.display()));
        }
        let command = [
            "install".to_owned(),
            allow::install_name(name).to_owned(),
            app.display().to_string(),
        ];
        let ran = self
            .run_unchecked(target, session, &command, INSTALL_APP_WAIT)
            .await?;
        succeeded("install", &ran)
    }

    /// A PNG of `target`'s screen, taken in `session`.
    pub async fn screenshot(&self, target: &Target, session: &str) -> Result<Vec<u8>, String> {
        let name = format!("screen-{}.png", uuid::Uuid::now_v7().simple());
        let command = ["screenshot".to_owned(), name.clone()];
        let ran = self
            .run_unchecked(target, session, &command, SCREENSHOT_WAIT)
            .await;
        let file = self.session_dir(session).join(name);
        let read = ran
            .and_then(|ran| succeeded("screenshot", &ran))
            .and_then(|()| {
                let size = std::fs::metadata(&file).map_err(|_| no_screenshot())?.len();
                if size > MAX_SCREENSHOT_BYTES {
                    return Err("The screenshot is too large to read.".to_owned());
                }
                std::fs::read(&file).map_err(|_| no_screenshot())
            });
        let _ = std::fs::remove_file(&file);
        read
    }

    /// Ends `session` on `target`, which quits the app it opened and lets
    /// go of the device, then removes the session's files. A session that
    /// had already ended counts as closed.
    pub async fn close(&self, target: &Target, session: &str) -> Result<(), String> {
        let command = ["close".to_owned()];
        let ran = self
            .run_unchecked(target, session, &command, CLOSE_WAIT)
            .await;
        let _ = std::fs::remove_dir_all(self.session_dir(session));
        let ran = ran?;
        if ran.timed_out {
            return Err("agent-device's close didn't finish in time.".to_owned());
        }
        if ran.exit_code == Some(0) || ran.stderr.contains("SESSION_NOT_FOUND") {
            return Ok(());
        }
        succeeded("close", &ran)
    }

    /// Stops the daemon, if one runs. It's best effort.
    pub async fn stop(&self) {
        let Some(entry) = lock(&self.entry).clone() else {
            return;
        };
        let mut args = vec![
            "daemon".to_owned(),
            "stop".to_owned(),
            "--state-dir".to_owned(),
        ];
        args.push(self.config.state.display().to_string());
        let _ = process::run(
            self.command(&entry, &args),
            daemon::STOP_WAIT,
            MAX_OUTPUT_BYTES,
        )
        .await;
    }

    /// Runs `command` for `session` on `target` with the flags `maxd`
    /// adds, in the session's own folder.
    async fn run_unchecked(
        &self,
        target: &Target,
        session: &str,
        command: &[String],
        timeout: Duration,
    ) -> Result<RunOutput, String> {
        let endpoint = self.start().await?;
        let entry = self.install().await?;
        let config = endpoint.config.display().to_string();
        let mut bound: Vec<&str> = target.args().to_vec();
        bound.extend(["--config", &config, "--session", session]);
        let args = allow::bind(command, &bound);
        let dir = self.session_dir(session);
        std::fs::create_dir_all(&dir)
            .map_err(|err| format!("couldn't make the lease's folder: {err}"))?;
        let mut run = self.command(&entry, &args);
        run.current_dir(&dir);
        process::run(run, timeout, MAX_OUTPUT_BYTES)
            .await
            .map_err(|err| format!("agent-device couldn't run: {err}"))
    }

    /// `node <entry> <args>` with agent-device's environment, under the
    /// wrap if there is one.
    fn command(&self, entry: &Path, args: &[String]) -> std::process::Command {
        let mut command = match self.config.wrap.split_first() {
            Some((program, words)) => {
                let mut command = std::process::Command::new(program);
                command.args(words).arg(&self.config.node);
                command
            }
            None => std::process::Command::new(&self.config.node),
        };
        command.arg(entry).args(args);
        for name in daemon::STRIPPED {
            command.env_remove(name);
        }
        command.envs(daemon::env(&self.config.state));
        command.envs(self.config.env.iter().map(|(k, v)| (k, v)));
        command
    }

    /// The folder `session`'s commands run in.
    fn session_dir(&self, session: &str) -> PathBuf {
        self.config.state.join(SESSIONS_DIR).join(session)
    }
}

/// Ok when `ran` exited cleanly, else why `step` failed.
fn succeeded(step: &str, ran: &RunOutput) -> Result<(), String> {
    if ran.timed_out {
        return Err(format!("agent-device's {step} didn't finish in time."));
    }
    if ran.exit_code == Some(0) {
        return Ok(());
    }
    let line = process::last_line(ran).map_or(String::new(), |line| format!(": {line}"));
    Err(format!("agent-device's {step} failed{line}"))
}

fn no_screenshot() -> String {
    "agent-device's screenshot left no picture.".to_owned()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
