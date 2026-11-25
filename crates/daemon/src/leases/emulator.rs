// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The Android emulator on this device, the Mac or a Linux device. `maxd`
//! keeps its own AVD, `mhq-test`, in `<data>/test-hosts/avd`, never your
//! `~/.android/avd`. A build creates it from the system image detection
//! found, boots it headless once and saves a `clean` snapshot. Each lease
//! boots from that snapshot without saving, so nothing a lease does is
//! kept, and ends by stopping the emulator by its serial. agent-device and
//! `adb` run on the device as `maxd`; the app under test runs inside the
//! emulator's own VM.
//!
//! `maxd` only stops an emulator it started: the one it holds, or after a
//! restart the serial it wrote down, once that emulator says it runs
//! `mhq-test`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures::future::BoxFuture;
use futures::FutureExt;
use maxhq_protocol::ids::LeaseId;
use maxhq_protocol::leases::{RunOutput, TestHostBackend, TestKind};
use tracing::warn;

use super::backend::{Built, LeaseTarget, TestBackend};
use super::detect::{self, AndroidSdk, Ready};
use crate::agent_device::{self, allow::Platform, AgentDevice, Config, Target};
use crate::session::clock::Clock;

/// The AVD `maxd` creates and drives.
pub const AVD: &str = "mhq-test";

/// The snapshot each lease boots from.
const SNAPSHOT: &str = "clean";

/// The console ports `maxd` boots on, highest first, to keep clear of the
/// 5554 the emulator picks for you. Each takes the odd port above it for
/// adb, and the serial is `emulator-<port>`.
const PORTS: std::ops::RangeInclusive<u16> = 5600..=5680;

/// How long the first, cold boot may take.
const COLD_BOOT: Duration = Duration::from_secs(10 * 60);

/// How long a boot from the snapshot may take.
const SNAPSHOT_BOOT: Duration = Duration::from_secs(3 * 60);

/// How often to ask whether it has booted.
const BOOT_POLL: Duration = Duration::from_secs(2);

/// How long an emulator has to quit once asked.
const STOP_WAIT: Duration = Duration::from_secs(30);

/// How often to look whether it has quit.
const STOP_POLL: Duration = Duration::from_secs(1);

/// How long `avdmanager` may take.
const AVDMANAGER_WAIT: Duration = Duration::from_secs(2 * 60);

/// How long one `adb` command may take.
const ADB_WAIT: Duration = Duration::from_secs(15);

/// How long saving the snapshot may take.
const SAVE_WAIT: Duration = Duration::from_secs(2 * 60);

/// One SDK program to run, with the environment it runs in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SdkCommand {
    /// The program.
    pub program: PathBuf,
    /// Its arguments.
    pub args: Vec<String>,
    /// More environment, on top of `maxd`'s own.
    pub env: Vec<(String, String)>,
}

/// An emulator `maxd` started.
pub trait Process: Send + Sync {
    /// Whether it has quit.
    fn exited(&self) -> bool;
    /// Stops it and everything it started.
    fn kill(&self);
}

/// What drives the emulator's screen: agent-device, or a fake in tests.
pub trait Drive: Send + Sync {
    /// Installs agent-device if it isn't yet.
    fn install(&self) -> BoxFuture<'_, Result<(), String>>;
    /// The release it drives with.
    fn version(&self) -> &'static str;
    /// Runs one allowed command.
    fn run<'a>(
        &'a self,
        target: &'a Target,
        session: &'a str,
        command: &'a [String],
        timeout: Duration,
    ) -> BoxFuture<'a, Result<RunOutput, String>>;
    /// A PNG of the screen.
    fn screenshot<'a>(
        &'a self,
        target: &'a Target,
        session: &'a str,
    ) -> BoxFuture<'a, Result<Vec<u8>, String>>;
    /// Installs the app at `app`.
    fn install_app<'a>(
        &'a self,
        target: &'a Target,
        session: &'a str,
        app: &'a Path,
    ) -> BoxFuture<'a, Result<(), String>>;
    /// Ends the session.
    fn close<'a>(
        &'a self,
        target: &'a Target,
        session: &'a str,
    ) -> BoxFuture<'a, Result<(), String>>;
    /// Stops its daemon.
    fn stop(&self) -> BoxFuture<'_, ()>;
}

impl Drive for AgentDevice {
    fn install(&self) -> BoxFuture<'_, Result<(), String>> {
        async move { AgentDevice::install(self).await.map(drop) }.boxed()
    }

    fn version(&self) -> &'static str {
        AgentDevice::version(self)
    }

    fn run<'a>(
        &'a self,
        target: &'a Target,
        session: &'a str,
        command: &'a [String],
        timeout: Duration,
    ) -> BoxFuture<'a, Result<RunOutput, String>> {
        AgentDevice::run(self, target, session, command, timeout).boxed()
    }

    fn screenshot<'a>(
        &'a self,
        target: &'a Target,
        session: &'a str,
    ) -> BoxFuture<'a, Result<Vec<u8>, String>> {
        AgentDevice::screenshot(self, target, session).boxed()
    }

    fn install_app<'a>(
        &'a self,
        target: &'a Target,
        session: &'a str,
        app: &'a Path,
    ) -> BoxFuture<'a, Result<(), String>> {
        AgentDevice::install_app(self, target, session, app).boxed()
    }

    fn close<'a>(
        &'a self,
        target: &'a Target,
        session: &'a str,
    ) -> BoxFuture<'a, Result<(), String>> {
        AgentDevice::close(self, target, session).boxed()
    }

    fn stop(&self) -> BoxFuture<'_, ()> {
        AgentDevice::stop(self).boxed()
    }
}

/// What the backend runs on the device. Tests give it a fake SDK that
/// answers by script, so no test runs `avdmanager`, `adb` or an emulator.
pub trait Android: Send + Sync + 'static {
    /// What detection found for the emulator, or the setup text.
    fn ready(&self) -> BoxFuture<'_, Result<Ready, String>>;
    /// A Java for `avdmanager` when the device has none of its own, such
    /// as the one Android Studio bundles.
    fn java_home(&self) -> Option<PathBuf>;
    /// Runs `command` to its end, writing `input` to it, and answers its
    /// stdout, or why it failed.
    fn run<'a>(
        &'a self,
        command: &'a SdkCommand,
        input: Option<&'a str>,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<String, String>>;
    /// Starts the emulator `command` names, its output going to `log`.
    fn boot(&self, command: &SdkCommand, log: &Path) -> Result<Arc<dyn Process>, String>;
    /// Whether nothing listens on the loopback `port`.
    fn port_free(&self, port: u16) -> bool;
    /// agent-device as `config` places it.
    fn agent_device(&self, config: Config) -> Arc<dyn Drive>;
}

/// The device `maxd` runs on.
pub struct HostAndroid;

impl Android for HostAndroid {
    fn ready(&self) -> BoxFuture<'_, Result<Ready, String>> {
        detect::ready(TestKind::AndroidEmulator, TestHostBackend::HostEmulator).boxed()
    }

    fn java_home(&self) -> Option<PathBuf> {
        if std::env::var_os("JAVA_HOME").is_some() || crate::service::find_program("java").is_some()
        {
            return None;
        }
        let home = std::env::home_dir().unwrap_or_default();
        crate::tools::android::bundled_javas(&home)
            .into_iter()
            .find(|java| java.join("bin").join("java").exists())
    }

    fn run<'a>(
        &'a self,
        command: &'a SdkCommand,
        input: Option<&'a str>,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<String, String>> {
        use std::process::Stdio;
        use tokio::io::AsyncWriteExt;

        async move {
            let name = program_name(&command.program);
            let mut child = tokio::process::Command::new(&command.program)
                .args(&command.args)
                .envs(command.env.iter().map(|(k, v)| (k, v)))
                .stdin(if input.is_some() {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .map_err(|err| format!("{name} couldn't run: {err}"))?;
            if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
                let _ = stdin.write_all(input.as_bytes()).await;
            }
            let output = tokio::time::timeout(timeout, child.wait_with_output())
                .await
                .map_err(|_| format!("{name} didn't finish in time."))?
                .map_err(|err| format!("{name} couldn't run: {err}"))?;
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if output.status.success() {
                return Ok(stdout);
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(failed(name, last_line(&stderr).or(last_line(&stdout))))
        }
        .boxed()
    }

    fn boot(&self, command: &SdkCommand, log: &Path) -> Result<Arc<dyn Process>, String> {
        use std::process::Stdio;

        use maxhq_os::proc::GroupExt;

        let out = std::fs::File::create(log)
            .map_err(|err| format!("couldn't make the emulator's log: {err}"))?;
        let err = out
            .try_clone()
            .map_err(|err| format!("couldn't make the emulator's log: {err}"))?;
        let mut run = std::process::Command::new(&command.program);
        run.args(&command.args)
            .envs(command.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err)
            // Its own group, so stopping it stops the qemu it starts.
            .own_group();
        let mut child = tokio::process::Command::from(run)
            .spawn()
            .map_err(|err| format!("The emulator couldn't start: {err}"))?;
        let group = match child.id().map(maxhq_os::proc::adopt).transpose() {
            Ok(group) => group,
            Err(err) => {
                let _ = child.start_kill();
                return Err(format!("The emulator couldn't start: {err}"));
            }
        };
        Ok(Arc::new(HostProcess {
            child: Mutex::new(child),
            _group: group,
        }))
    }

    fn port_free(&self, port: u16) -> bool {
        std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
    }

    fn agent_device(&self, config: Config) -> Arc<dyn Drive> {
        Arc::new(AgentDevice::new(config))
    }
}

/// An emulator process on this device, and the group that stops the qemu
/// it starts with it.
struct HostProcess {
    child: Mutex<tokio::process::Child>,
    _group: Option<maxhq_os::proc::Group>,
}

impl Process for HostProcess {
    fn exited(&self) -> bool {
        !matches!(lock(&self.child).try_wait(), Ok(None))
    }

    fn kill(&self) {
        let mut child = lock(&self.child);
        if let Some(pid) = child.id() {
            let _ = maxhq_os::proc::stop_group(pid, maxhq_os::proc::Stop::Hard);
        }
        let _ = child.start_kill();
    }
}

/// What a build or the first need after a restart set up.
#[derive(Clone)]
struct Setup {
    sdk: AndroidSdk,
    env: Vec<(String, String)>,
    drive: Arc<dyn Drive>,
}

/// The emulator that runs now.
struct Running {
    serial: String,
    process: Arc<dyn Process>,
}

#[derive(Default)]
struct State {
    setup: Option<Setup>,
    running: Option<Running>,
}

/// The Android emulator test host.
pub struct Emulator<A = HostAndroid> {
    android: A,
    clock: Arc<dyn Clock>,
    /// `maxd`'s data folder.
    data: PathBuf,
    state: Mutex<State>,
    /// Builds, starts, closes and resets take turns.
    turn: tokio::sync::Mutex<()>,
}

impl Emulator {
    /// The emulator on this device, with `maxd`'s data folder `data`.
    pub fn on_host(data: PathBuf, clock: Arc<dyn Clock>) -> Self {
        Self::new(HostAndroid, data, clock)
    }
}

impl<A: Android> Emulator<A> {
    /// The emulator `android` runs, keeping its files under `data`.
    pub fn new(android: A, data: PathBuf, clock: Arc<dyn Clock>) -> Self {
        Self {
            android,
            clock,
            data,
            state: Mutex::default(),
            turn: tokio::sync::Mutex::const_new(()),
        }
    }

    /// `maxd`'s own AVD folder, for ANDROID_AVD_HOME.
    fn avd_home(&self) -> PathBuf {
        self.data.join("test-hosts").join("avd")
    }

    fn avd_dir(&self) -> PathBuf {
        self.avd_home().join(format!("{AVD}.avd"))
    }

    /// Where the serial of the emulator that runs is written down, so a
    /// `maxd` that restarts can stop it.
    fn serial_file(&self) -> PathBuf {
        self.data.join("test-hosts").join("emulator-serial")
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    /// What detection found, made ready to run.
    async fn set_up(&self) -> Result<Setup, String> {
        let Ready::HostEmulator { sdk, node, .. } = self.android.ready().await? else {
            return Err("The Android emulator wasn't checked.".into());
        };
        Ok(self.setup_for(sdk, node))
    }

    fn setup_for(&self, sdk: AndroidSdk, node: PathBuf) -> Setup {
        let root = sdk.root.display().to_string();
        let mut env = vec![
            ("ANDROID_HOME".to_owned(), root.clone()),
            ("ANDROID_SDK_ROOT".to_owned(), root),
            (
                "ANDROID_AVD_HOME".to_owned(),
                self.avd_home().display().to_string(),
            ),
        ];
        if let Some(java) = self.android.java_home() {
            env.push(("JAVA_HOME".to_owned(), java.display().to_string()));
        }
        let mut config = Config::under(&self.data, "android-emulator");
        let npm = node.with_file_name("npm");
        if npm.exists() {
            config.npm = npm;
        }
        config.node = node;
        config.env = env
            .iter()
            .filter(|(name, _)| name == "ANDROID_HOME" || name == "ANDROID_AVD_HOME")
            .cloned()
            .collect();
        let setup = Setup {
            drive: self.android.agent_device(config),
            sdk,
            env,
        };
        self.lock().setup = Some(setup.clone());
        setup
    }

    /// The setup so far, or a new one.
    async fn setup(&self) -> Result<Setup, String> {
        if let Some(setup) = self.lock().setup.clone() {
            return Ok(setup);
        }
        self.set_up().await
    }

    fn command(&self, setup: &Setup, program: PathBuf, args: &[&str]) -> SdkCommand {
        SdkCommand {
            program,
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            env: setup.env.clone(),
        }
    }

    async fn adb(&self, setup: &Setup, args: &[&str], wait: Duration) -> Result<String, String> {
        let command = self.command(setup, setup.sdk.adb(), args);
        self.android.run(&command, None, wait).await
    }

    async fn build_now(&self) -> Result<Built, String> {
        let Ready::HostEmulator { sdk, image, node } = self.android.ready().await? else {
            return Err("The Android emulator wasn't checked.".into());
        };
        let setup = self.setup_for(sdk, node);
        self.stop_ours(&setup).await?;
        std::fs::create_dir_all(self.avd_home())
            .map_err(|err| format!("couldn't make the AVD folder: {err}"))?;
        let path = self.avd_dir().display().to_string();
        let create = self.command(
            &setup,
            setup.sdk.avdmanager(),
            &[
                "create",
                "avd",
                "--name",
                AVD,
                "--package",
                &image.package,
                "--path",
                &path,
                "--force",
            ],
        );
        // It asks whether to make a custom hardware profile.
        self.android
            .run(&create, Some("no\n"), AVDMANAGER_WAIT)
            .await?;
        setup.drive.install().await?;
        let serial = self.boot(&setup, &["-no-snapshot-load"], COLD_BOOT).await?;
        let saved = self
            .adb(
                &setup,
                &["-s", &serial, "emu", "avd", "snapshot", "save", SNAPSHOT],
                SAVE_WAIT,
            )
            .await
            .and_then(|said| {
                if said.lines().any(|line| line.starts_with("KO")) {
                    return Err(failed("The emulator's snapshot save", last_line(&said)));
                }
                Ok(())
            });
        let stopped = self.stop_ours(&setup).await;
        saved?;
        stopped?;
        Ok(Built {
            agent_device: Some(setup.drive.version().to_owned()),
            vm: None,
        })
    }

    /// Boots `mhq-test` headless on a free port with `extra` flags, waits
    /// until Android has started, and answers its serial.
    async fn boot(&self, setup: &Setup, extra: &[&str], limit: Duration) -> Result<String, String> {
        let port = PORTS
            .rev()
            .step_by(2)
            .find(|&port| self.android.port_free(port) && self.android.port_free(port + 1))
            .ok_or("No port is free for the emulator.")?;
        let serial = format!("emulator-{port}");
        let port = port.to_string();
        let mut args = vec![
            "-avd",
            AVD,
            "-port",
            &port,
            "-no-window",
            "-no-audio",
            "-no-boot-anim",
            "-no-snapshot-save",
        ];
        args.extend_from_slice(extra);
        let command = self.command(setup, setup.sdk.emulator(), &args);
        let log = self.data.join("test-hosts").join("emulator.log");
        let process = self.android.boot(&command, &log)?;
        if let Err(err) = std::fs::write(self.serial_file(), &serial) {
            warn!(%err, "couldn't write down the emulator's serial");
        }
        self.lock().running = Some(Running {
            serial: serial.clone(),
            process: process.clone(),
        });
        let deadline = self.clock.now() + limit;
        let booted = loop {
            if process.exited() {
                break Err(format!(
                    "The emulator stopped before Android started. Its log is {}.",
                    log.display()
                ));
            }
            let said = self
                .adb(
                    setup,
                    &["-s", &serial, "shell", "getprop", "sys.boot_completed"],
                    ADB_WAIT,
                )
                .await;
            if said.is_ok_and(|said| said.trim() == "1") {
                break Ok(serial);
            }
            if self.clock.now() >= deadline {
                break Err(format!(
                    "Android didn't finish starting in the emulator within {} minutes.",
                    limit.as_secs() / 60
                ));
            }
            self.clock.sleep_until(self.clock.now() + BOOT_POLL).await;
        };
        if booted.is_err() {
            let _ = self.stop_ours(setup).await;
        }
        booted
    }

    /// Stops the emulator `maxd` started, if one runs: the one it holds,
    /// or the one whose serial it wrote down, once that one says it runs
    /// `mhq-test`. Any other emulator is left alone.
    async fn stop_ours(&self, setup: &Setup) -> Result<(), String> {
        let running = self.lock().running.take();
        let file = self.serial_file();
        let (serial, process) = match running {
            Some(Running { serial, process }) => (serial, Some(process)),
            None => {
                let Ok(serial) = std::fs::read_to_string(&file) else {
                    return Ok(());
                };
                let serial = serial.trim().to_owned();
                let name = self
                    .adb(setup, &["-s", &serial, "emu", "avd", "name"], ADB_WAIT)
                    .await;
                if !name.is_ok_and(|name| name.lines().next().map(str::trim) == Some(AVD)) {
                    let _ = std::fs::remove_file(&file);
                    return Ok(());
                }
                (serial, None)
            }
        };
        let _ = self
            .adb(setup, &["-s", &serial, "emu", "kill"], ADB_WAIT)
            .await;
        let deadline = self.clock.now() + STOP_WAIT;
        let stopped = loop {
            let gone = match &process {
                Some(process) => process.exited(),
                None => !self.listed(setup, &serial).await,
            };
            if gone {
                break Ok(());
            }
            if self.clock.now() >= deadline {
                match &process {
                    Some(process) => {
                        process.kill();
                        break Ok(());
                    }
                    None => break Err(format!("The emulator {serial} didn't stop.")),
                }
            }
            self.clock.sleep_until(self.clock.now() + STOP_POLL).await;
        };
        if stopped.is_ok() {
            let _ = std::fs::remove_file(&file);
        }
        stopped
    }

    /// Whether `adb devices` lists `serial`.
    async fn listed(&self, setup: &Setup, serial: &str) -> bool {
        self.adb(setup, &["devices"], ADB_WAIT)
            .await
            .is_ok_and(|listed| {
                listed
                    .lines()
                    .any(|line| line.split_whitespace().next() == Some(serial))
            })
    }

    fn snapshot_missing(&self) -> Option<String> {
        let snapshot = self.avd_dir().join("snapshots").join(SNAPSHOT);
        (!snapshot.exists())
            .then(|| "The emulator's clean snapshot is gone, so build the host again.".to_owned())
    }

    /// The setup and the target for a lease.
    async fn target(&self, on: &LeaseTarget) -> Result<(Setup, Target, String), String> {
        let setup = self.setup().await?;
        let target = Target {
            platform: Platform::Android,
            id: on.target.clone(),
        };
        Ok((setup, target, agent_device::session(on.lease)))
    }
}

impl<A: Android> TestBackend for Emulator<A> {
    fn kind(&self) -> TestKind {
        TestKind::AndroidEmulator
    }

    fn backend(&self) -> TestHostBackend {
        TestHostBackend::HostEmulator
    }

    fn detect(&self) -> BoxFuture<'_, Option<String>> {
        async move { self.android.ready().await.err() }.boxed()
    }

    fn build(&self) -> BoxFuture<'_, Result<Built, String>> {
        async move {
            let _turn = self.turn.lock().await;
            self.build_now().await
        }
        .boxed()
    }

    fn start(&self, _lease: LeaseId) -> BoxFuture<'_, Result<String, String>> {
        async move {
            let _turn = self.turn.lock().await;
            let setup = self.setup().await?;
            if let Some(missing) = self.snapshot_missing() {
                return Err(missing);
            }
            self.stop_ours(&setup).await?;
            self.boot(&setup, &["-snapshot", SNAPSHOT], SNAPSHOT_BOOT)
                .await
        }
        .boxed()
    }

    fn run<'a>(
        &'a self,
        on: &'a LeaseTarget,
        command: &'a [String],
        timeout: Duration,
    ) -> BoxFuture<'a, Result<RunOutput, String>> {
        async move {
            let (setup, target, session) = self.target(on).await?;
            setup.drive.run(&target, &session, command, timeout).await
        }
        .boxed()
    }

    fn screenshot<'a>(&'a self, on: &'a LeaseTarget) -> BoxFuture<'a, Result<Vec<u8>, String>> {
        async move {
            let (setup, target, session) = self.target(on).await?;
            setup.drive.screenshot(&target, &session).await
        }
        .boxed()
    }

    fn install<'a>(
        &'a self,
        on: &'a LeaseTarget,
        source: PathBuf,
    ) -> BoxFuture<'a, Result<String, String>> {
        async move {
            let (setup, target, session) = self.target(on).await?;
            setup.drive.install_app(&target, &session, &source).await?;
            let name = source.file_name().unwrap_or(source.as_os_str());
            Ok(format!(
                "Installed {} on {}.",
                name.to_string_lossy(),
                on.target
            ))
        }
        .boxed()
    }

    fn close<'a>(&'a self, on: &'a LeaseTarget) -> BoxFuture<'a, Result<(), String>> {
        async move {
            let _turn = self.turn.lock().await;
            let (setup, target, session) = self.target(on).await?;
            // Stopping the emulator throws away all the lease did anyway.
            if let Err(err) = setup.drive.close(&target, &session).await {
                warn!(%err, serial = %on.target, "agent-device didn't close the session");
            }
            self.stop_ours(&setup).await
        }
        .boxed()
    }

    fn reset(&self) -> BoxFuture<'_, Result<(), String>> {
        async move {
            let _turn = self.turn.lock().await;
            let setup = self.setup().await?;
            let stopped = self.stop_ours(&setup).await;
            setup.drive.stop().await;
            stopped?;
            self.snapshot_missing().map_or(Ok(()), Err)
        }
        .boxed()
    }
}

/// A program's file name, for what a person reads.
fn program_name(program: &Path) -> &str {
    program
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("The command")
}

fn last_line(text: &str) -> Option<&str> {
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
}

fn failed(step: &str, line: Option<&str>) -> String {
    let line = line.map_or(String::new(), |line| format!(": {line}"));
    format!("{step} failed{line}")
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
#[path = "emulator/tests.rs"]
mod tests;
