// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The Mac's own iOS Simulator as a test host. `maxd` creates one
//! Simulator of its own, keeps its UDID, and never touches a Simulator it
//! didn't create. A lease boots it, agent-device drives it inside a
//! Seatbelt profile, and the lease's end shuts it down and erases it.
//!
//! The app under test runs as you outside any profile `maxd` sets, since
//! `launchd_sim` starts it, so this host is off until you allow it.
//!
//! The `simctl` helpers at the top take any [`Xcrun`], so the macOS VM's
//! guest runs the same steps on its own Simulator.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use futures::FutureExt;
use maxhq_protocol::ids::LeaseId;
use maxhq_protocol::leases::{RunOutput, TestHostBackend, TestKind};
use serde::Deserialize;

use super::backend::{Built, LeaseTarget, TestBackend};
use super::detect::{self, Ready};
use crate::agent_device::allow::Platform;
use crate::agent_device::{self, process, AgentDevice, Config, Target, VERSION};

/// The name of the Simulator `maxd` creates.
pub const SIMULATOR_NAME: &str = "MaxHQ test";

/// What this host is called under `maxd`'s data folder.
const HOST: &str = "mac-host";

/// The file under the host's folder that keeps the Simulator's UDID.
const UDID_FILE: &str = "simulator";

/// The file that keeps the UDID of the Simulator `maxd` created, under
/// `maxd`'s data folder `data`.
pub fn udid_file(data: &Path) -> PathBuf {
    data.join("test-hosts").join(HOST).join(UDID_FILE)
}

/// The folder under agent-device's state folder that holds the apps a
/// lease installs, since the profile lets it read nothing else of yours.
const APPS_DIR: &str = "apps";

/// How long a boot may take. The first boot after an erase sets the
/// Simulator up again, which took about 34 s in the VM trial.
const BOOT_WAIT: Duration = Duration::from_secs(5 * 60);

/// How long a create, erase, delete or shutdown may take.
const CHANGE_WAIT: Duration = Duration::from_secs(2 * 60);

/// How long a list may take.
const LIST_WAIT: Duration = Duration::from_secs(30);

/// The most `simctl` output read back.
const MAX_SIMCTL_OUTPUT: usize = 4 * 1024 * 1024;

/// The Seatbelt profile agent-device runs in on the Mac. Everything else
/// stays as it is, but your home folder is closed except for what
/// agent-device, `simctl` and `xcodebuild` need: its own state folder, the
/// `~/.agent-device` folder it keeps device claims, logs and its iOS
/// runner's builds in whatever its state folder is, the developer folders
/// that hold Simulators and Xcode's build products, and read access to the
/// installed CLI and Node. Names and sizes under your home folder stay
/// visible, so paths resolve.
///
/// `sandbox-exec` fills the parameters in: `HOME`, `STATE`, `TOOLS` and
/// `NODE`, each a real path.
pub const SEATBELT_PROFILE: &str = r#"(version 1)
(allow default)
(deny file-read* file-write* (subpath (param "HOME")))
(allow file-read-metadata (subpath (param "HOME")))
(allow file-read* file-write*
  (subpath (param "STATE"))
  (subpath (string-append (param "HOME") "/.agent-device"))
  (subpath (string-append (param "HOME") "/Library/Developer"))
  (subpath (string-append (param "HOME") "/Library/Caches/com.apple.dt.xcodebuild"))
  (subpath (string-append (param "HOME") "/Library/Logs/CoreSimulator")))
(allow file-read*
  (subpath (param "TOOLS"))
  (subpath (param "NODE")))
"#;

/// Runs `xcrun simctl`. Tests give it fake answers, so no test boots a
/// Simulator.
pub trait Xcrun: Send + Sync {
    /// What `simctl <args>` prints, or why it failed, for a person to
    /// read.
    fn simctl<'a>(
        &'a self,
        args: &'a [&'a str],
        timeout: Duration,
    ) -> BoxFuture<'a, Result<String, String>>;
}

/// The Mac's own `xcrun`.
pub struct HostXcrun;

impl Xcrun for HostXcrun {
    fn simctl<'a>(
        &'a self,
        args: &'a [&'a str],
        timeout: Duration,
    ) -> BoxFuture<'a, Result<String, String>> {
        async move {
            let mut command = std::process::Command::new("/usr/bin/xcrun");
            command.arg("simctl").args(args);
            let verb = args.first().copied().unwrap_or("simctl");
            let ran = process::run(command, timeout, MAX_SIMCTL_OUTPUT)
                .await
                .map_err(|err| format!("simctl couldn't run: {err}"))?;
            if ran.timed_out {
                return Err(format!("simctl {verb} didn't finish in time."));
            }
            if ran.exit_code == Some(0) {
                return Ok(ran.stdout);
            }
            let line = process::last_line(&ran).map_or(String::new(), |line| format!(": {line}"));
            Err(format!("simctl {verb} failed{line}"))
        }
        .boxed()
    }
}

#[derive(Deserialize)]
struct Runtimes {
    runtimes: Vec<Runtime>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Runtime {
    identifier: String,
    #[serde(default)]
    supported_device_types: Vec<DeviceType>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceType {
    identifier: String,
    #[serde(default)]
    product_family: String,
}

#[derive(Deserialize)]
struct Devices {
    devices: std::collections::HashMap<String, Vec<Device>>,
}

#[derive(Deserialize)]
struct Device {
    udid: String,
}

/// The iPhone to create on `runtime`: the first `simctl` lists for it,
/// which is the newest.
pub async fn iphone_for(xcrun: &dyn Xcrun, runtime: &str) -> Result<String, String> {
    let listed = xcrun
        .simctl(&["list", "runtimes", "--json"], LIST_WAIT)
        .await?;
    let Runtimes { runtimes } = serde_json::from_str(&listed)
        .map_err(|err| format!("simctl listed runtimes it couldn't read: {err}"))?;
    runtimes
        .into_iter()
        .find(|found| found.identifier == runtime)
        .and_then(|found| {
            found
                .supported_device_types
                .into_iter()
                .find(|kind| kind.product_family == "iPhone")
        })
        .map(|kind| kind.identifier)
        .ok_or_else(|| format!("{runtime} offers no iPhone to create."))
}

/// Creates a Simulator called `name`, an iPhone on `runtime`, and answers
/// its UDID.
pub async fn create(xcrun: &dyn Xcrun, name: &str, runtime: &str) -> Result<String, String> {
    let iphone = iphone_for(xcrun, runtime).await?;
    let udid = xcrun
        .simctl(&["create", name, &iphone, runtime], CHANGE_WAIT)
        .await?;
    let udid = udid.trim();
    if udid.is_empty() {
        return Err("simctl create answered no UDID.".into());
    }
    Ok(udid.to_owned())
}

/// Whether the Simulator `udid` still exists.
pub async fn exists(xcrun: &dyn Xcrun, udid: &str) -> Result<bool, String> {
    let listed = xcrun
        .simctl(&["list", "devices", "--json"], LIST_WAIT)
        .await?;
    let Devices { devices } = serde_json::from_str(&listed)
        .map_err(|err| format!("simctl listed devices it couldn't read: {err}"))?;
    Ok(devices.values().flatten().any(|device| device.udid == udid))
}

/// Boots `udid` and waits until it's ready. A booted one stays as it is.
pub async fn boot(xcrun: &dyn Xcrun, udid: &str) -> Result<(), String> {
    xcrun
        .simctl(&["bootstatus", udid, "-b"], BOOT_WAIT)
        .await
        .map(drop)
}

/// Shuts `udid` down. One that's shut down already counts.
pub async fn shutdown(xcrun: &dyn Xcrun, udid: &str) -> Result<(), String> {
    match xcrun.simctl(&["shutdown", udid], CHANGE_WAIT).await {
        Err(error) if error.contains("current state: Shutdown") => Ok(()),
        done => done.map(drop),
    }
}

/// Erases `udid`'s contents and settings. It must be shut down.
pub async fn erase(xcrun: &dyn Xcrun, udid: &str) -> Result<(), String> {
    xcrun.simctl(&["erase", udid], CHANGE_WAIT).await.map(drop)
}

/// Shuts `udid` down and erases it, for the next lease.
pub async fn clean(xcrun: &dyn Xcrun, udid: &str) -> Result<(), String> {
    shutdown(xcrun, udid).await?;
    erase(xcrun, udid).await
}

/// Shuts `udid` down and deletes it.
pub async fn delete(xcrun: &dyn Xcrun, udid: &str) -> Result<(), String> {
    shutdown(xcrun, udid).await?;
    xcrun.simctl(&["delete", udid], CHANGE_WAIT).await.map(drop)
}

/// What a build makes the host from, as detection found it.
type Find = Arc<dyn Fn() -> BoxFuture<'static, Result<Ready, String>> + Send + Sync>;

/// The Mac's own Simulator.
pub struct MacHost {
    xcrun: Arc<dyn Xcrun>,
    find: Find,
    /// The host's own folder, which keeps the Simulator's UDID.
    folder: PathBuf,
    /// Where agent-device lives for this host, before detection names
    /// its Node.
    places: Config,
    /// Whether agent-device runs in [`SEATBELT_PROFILE`].
    sandbox: bool,
    /// agent-device, made with the Node detection found.
    agent: tokio::sync::Mutex<Option<Arc<AgentDevice>>>,
}

impl MacHost {
    /// The Mac's Simulator with `maxd`'s data in `data`, driven by
    /// agent-device in its Seatbelt profile, with what the installed
    /// detector finds.
    pub fn on_this_mac(data: &Path) -> Self {
        let find: Find =
            Arc::new(|| detect::ready(TestKind::IosSimulator, TestHostBackend::MacHost).boxed());
        Self::new(
            Arc::new(HostXcrun),
            find,
            data.join("test-hosts").join(HOST),
            Config::under(data, HOST),
            true,
        )
    }

    fn new(
        xcrun: Arc<dyn Xcrun>,
        find: Find,
        folder: PathBuf,
        places: Config,
        sandbox: bool,
    ) -> Self {
        Self {
            xcrun,
            find,
            folder,
            places,
            sandbox,
            agent: tokio::sync::Mutex::new(None),
        }
    }

    /// The Simulator `maxd` created, if it did.
    fn udid(&self) -> Option<String> {
        let udid = std::fs::read_to_string(self.folder.join(UDID_FILE)).ok()?;
        let udid = udid.trim();
        (!udid.is_empty()).then(|| udid.to_owned())
    }

    /// The Simulator `maxd` created, or why there's none.
    fn own(&self) -> Result<String, String> {
        self.udid()
            .ok_or_else(|| "This Mac's test Simulator isn't built yet.".into())
    }

    fn keep_udid(&self, udid: &str) -> Result<(), String> {
        std::fs::create_dir_all(&self.folder)
            .and_then(|()| std::fs::write(self.folder.join(UDID_FILE), udid))
            .map_err(|err| format!("couldn't keep the Simulator's UDID: {err}"))
    }

    /// The runtime and Node detection finds.
    async fn ready(&self) -> Result<(detect::IosRuntime, PathBuf), String> {
        match (self.find)().await? {
            Ready::MacHost { runtime, node } => Ok((runtime, node)),
            _ => Err("Detection answered for another host.".into()),
        }
    }

    /// agent-device as it runs here, with `node` and the `npm` beside it.
    fn config(&self, node: PathBuf) -> Result<Config, String> {
        let mut config = self.places.clone();
        config.npm = node.with_file_name("npm");
        if self.sandbox {
            config.wrap = sandboxed(&config, &node)?;
        }
        config.node = node;
        Ok(config)
    }

    /// agent-device, made fresh with what detection finds when `again`.
    async fn agent(&self, again: bool) -> Result<Arc<AgentDevice>, String> {
        let mut agent = self.agent.lock().await;
        if let Some(made) = agent.as_ref().filter(|_| !again) {
            return Ok(made.clone());
        }
        let (_, node) = self.ready().await?;
        let made = Arc::new(AgentDevice::new(self.config(node)?));
        *agent = Some(made.clone());
        Ok(made)
    }

    /// The agent-device daemon, stopped. It's best effort.
    async fn stop_agent(&self) {
        if let Ok(agent) = self.agent(false).await
            && agent.install().await.is_ok()
        {
            agent.stop().await;
        }
    }

    async fn make(&self) -> Result<Built, String> {
        let (runtime, _) = self.ready().await?;
        let agent = self.agent(true).await?;
        agent.install().await?;
        agent.stop().await;
        let xcrun = self.xcrun.as_ref();
        if let Some(old) = self.udid()
            && exists(xcrun, &old).await?
        {
            delete(xcrun, &old).await?;
        }
        let udid = create(xcrun, SIMULATOR_NAME, &runtime.identifier).await?;
        self.keep_udid(&udid)?;
        Ok(Built {
            agent_device: Some(VERSION.to_owned()),
            vm: None,
        })
    }

    async fn end(&self, on: &LeaseTarget) -> Result<(), String> {
        let closed = match self.agent(false).await {
            Ok(agent) => {
                agent
                    .close(&target(on), &agent_device::session(on.lease))
                    .await
            }
            Err(error) => Err(error),
        };
        let cleaned = clean(self.xcrun.as_ref(), &on.target).await;
        closed.and(cleaned)
    }

    async fn put(&self, on: &LeaseTarget, source: &Path) -> Result<String, String> {
        let name = source
            .file_name()
            .ok_or_else(|| format!("{} doesn't name an app.", source.display()))?;
        let agent = self.agent(false).await?;
        let session = agent_device::session(on.lease);
        let folder = self.places.state.join(APPS_DIR).join(&session);
        let _ = std::fs::remove_dir_all(&folder);
        let copy = folder.join(name);
        let installed = match copy_app(source, &copy, CHANGE_WAIT).await {
            Ok(()) => agent.install_app(&target(on), &session, &copy).await,
            Err(error) => Err(error),
        };
        let _ = std::fs::remove_dir_all(&folder);
        installed?;
        Ok(format!(
            "Installed {} on the Simulator.",
            name.to_string_lossy()
        ))
    }

    async fn clean_all(&self) -> Result<(), String> {
        self.stop_agent().await;
        let udid = self.own()?;
        let xcrun = self.xcrun.as_ref();
        if !exists(xcrun, &udid).await? {
            return Err("This Mac's test Simulator is gone. Build it again.".into());
        }
        clean(xcrun, &udid).await
    }
}

/// agent-device's target for a lease.
fn target(on: &LeaseTarget) -> Target {
    Target {
        platform: Platform::Ios,
        id: on.target.clone(),
    }
}

/// `sandbox-exec` with [`SEATBELT_PROFILE`] and its parameters, for
/// agent-device as `config` places it and `node` runs it.
fn sandboxed(config: &Config, node: &Path) -> Result<Vec<String>, String> {
    let home = std::env::home_dir().ok_or("maxd can't tell your home folder.")?;
    std::fs::create_dir_all(&config.state)
        .and_then(|()| std::fs::create_dir_all(&config.tools))
        .map_err(|err| format!("couldn't make agent-device's folders: {err}"))?;
    let node = std::fs::canonicalize(node).unwrap_or_else(|_| node.to_owned());
    // The install the binary sits in, such as `<prefix>/bin/node`.
    let node = node.ancestors().nth(2).unwrap_or(&node).to_owned();
    Ok(seatbelt(&[
        ("HOME", &home),
        ("STATE", &config.state),
        ("TOOLS", &config.tools),
        ("NODE", &node),
    ]))
}

/// The `sandbox-exec` words for [`SEATBELT_PROFILE`], with each parameter
/// as its real path, since Seatbelt matches those.
fn seatbelt(params: &[(&str, &Path)]) -> Vec<String> {
    let mut words = vec!["/usr/bin/sandbox-exec".to_owned()];
    for (name, path) in params {
        let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        words.push("-D".to_owned());
        words.push(format!("{name}={}", real.display()));
    }
    words.push("-p".to_owned());
    words.push(SEATBELT_PROFILE.to_owned());
    words
}

/// Copies the app or folder at `from` to `to`, as a clone where the disk
/// can, waiting up to `wait`.
pub(super) async fn copy_app(from: &Path, to: &Path, wait: Duration) -> Result<(), String> {
    let parent = to.parent().unwrap_or(to);
    std::fs::create_dir_all(parent)
        .map_err(|err| format!("couldn't make the lease's app folder: {err}"))?;
    // macOS's `cp -c` clones, and copies where it can't. NixOS has no
    // /bin/cp, so other systems take it from the path.
    let (program, flags) = if cfg!(target_os = "macos") {
        ("/bin/cp", "-Rc")
    } else {
        ("cp", "-R")
    };
    let mut command = std::process::Command::new(program);
    command.arg(flags).arg(from).arg(to);
    let ran = process::run(command, wait, MAX_SIMCTL_OUTPUT)
        .await
        .map_err(|err| format!("couldn't copy {}: {err}", from.display()))?;
    if ran.exit_code == Some(0) {
        return Ok(());
    }
    let line = process::last_line(&ran).map_or(String::new(), |line| format!(": {line}"));
    Err(format!("couldn't copy {}{line}", from.display()))
}

impl TestBackend for MacHost {
    fn kind(&self) -> TestKind {
        TestKind::IosSimulator
    }

    fn backend(&self) -> TestHostBackend {
        TestHostBackend::MacHost
    }

    fn detect(&self) -> BoxFuture<'_, Option<String>> {
        async move { self.ready().await.err() }.boxed()
    }

    fn build(&self) -> BoxFuture<'_, Result<Built, String>> {
        self.make().boxed()
    }

    fn start(&self, _lease: LeaseId) -> BoxFuture<'_, Result<String, String>> {
        async move {
            let udid = self.own()?;
            boot(self.xcrun.as_ref(), &udid).await?;
            Ok(udid)
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
            let agent = self.agent(false).await?;
            let session = agent_device::session(on.lease);
            agent.run(&target(on), &session, command, timeout).await
        }
        .boxed()
    }

    fn screenshot<'a>(&'a self, on: &'a LeaseTarget) -> BoxFuture<'a, Result<Vec<u8>, String>> {
        async move {
            let agent = self.agent(false).await?;
            let session = agent_device::session(on.lease);
            agent.screenshot(&target(on), &session).await
        }
        .boxed()
    }

    fn install<'a>(
        &'a self,
        on: &'a LeaseTarget,
        source: PathBuf,
    ) -> BoxFuture<'a, Result<String, String>> {
        async move { self.put(on, &source).await }.boxed()
    }

    fn close<'a>(&'a self, on: &'a LeaseTarget) -> BoxFuture<'a, Result<(), String>> {
        self.end(on).boxed()
    }

    fn reset(&self) -> BoxFuture<'_, Result<(), String>> {
        self.clean_all().boxed()
    }
}

#[cfg(test)]
#[path = "mac_host/tests.rs"]
mod tests;
