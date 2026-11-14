// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: iOS needs macOS
// and `xcrun`; the Android SDK comes from ANDROID_HOME or ANDROID_SDK_ROOT,
// else ~/Library/Android/sdk or ~/Android/Sdk, else the folder above an
// `adb` on PATH, and needs platform-tools/adb, emulator/emulator and
// cmdline-tools/latest/bin/avdmanager; and the sentences for each one
// missing. MaxHQ adds the iOS runtime, the system image, /dev/kvm, the Node
// version, Incus and the macOS VM.

//! What this device can host for each kind of test machine. Each backend
//! comes to either what a build would make it from, or one sentence that
//! says what's missing and how to fix it.

use std::cmp::Reverse;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use futures::future::BoxFuture;
use futures::FutureExt;
use maxhq_protocol::ids::{DeviceId, TestHostId};
use maxhq_protocol::leases::{self, TestHostBackend, TestKind};
use serde::Deserialize;
use tracing::info;

/// The oldest Node agent-device runs on.
pub const NODE_MAJOR: u32 = 22;

/// The oldest macOS that runs the test VM.
const MACOS_MAJOR: u32 = 14;

/// How long one command may take to answer.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// The line after anything MaxHQ will later install for you.
const NOT_YET_FOR_YOU: &str = "MaxHQ can't do this for you yet.";

/// The system images to prefer for the emulator, best first. Any other tag
/// comes after these.
const IMAGE_TAGS: [&str; 3] = ["google_apis", "default", "google_apis_playstore"];

/// What detection reads of the device. Tests give it fake trees and
/// answers, so no test runs a real `xcrun`, `node` or Incus.
pub trait System: Send + Sync {
    /// The OS, as Rust names it: `macos` or `linux`.
    fn os(&self) -> &str;
    /// The CPU, as Rust names it: `aarch64` or `x86_64`.
    fn arch(&self) -> &str;
    /// An environment variable, if set.
    fn env(&self, name: &str) -> Option<String>;
    /// The home folder.
    fn home(&self) -> Option<PathBuf>;
    /// Whether `path` exists.
    fn exists(&self, path: &Path) -> bool;
    /// `path` with its links followed, or `path` itself.
    fn real_path(&self, path: &Path) -> PathBuf;
    /// The names in the folder `dir`.
    fn list(&self, dir: &Path) -> Vec<String>;
    /// Whether maxd may open `path` to read and write.
    fn opens(&self, path: &Path) -> bool;
    /// The program called `name`, from PATH or a usual install folder.
    fn program(&self, name: &str) -> Option<PathBuf>;
    /// What `program` prints with `args`, when it succeeds.
    fn output<'a>(
        &'a self,
        program: &'a Path,
        args: &'a [&'a str],
    ) -> BoxFuture<'a, Option<String>>;
    /// Whether Incus answers on `socket`, or why not.
    fn incus<'a>(&'a self, socket: &'a Path) -> BoxFuture<'a, Result<(), String>>;
}

/// The device maxd runs on.
pub struct HostSystem;

impl System for HostSystem {
    fn os(&self) -> &str {
        std::env::consts::OS
    }

    fn arch(&self) -> &str {
        std::env::consts::ARCH
    }

    fn env(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn home(&self) -> Option<PathBuf> {
        std::env::home_dir()
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn real_path(&self, path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
    }

    fn list(&self, dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .collect()
    }

    fn opens(&self, path: &Path) -> bool {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .is_ok()
    }

    fn program(&self, name: &str) -> Option<PathBuf> {
        crate::service::find_program(name)
    }

    fn output<'a>(
        &'a self,
        program: &'a Path,
        args: &'a [&'a str],
    ) -> BoxFuture<'a, Option<String>> {
        async move {
            let run = tokio::process::Command::new(program)
                .args(args)
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .output();
            let output = tokio::time::timeout(COMMAND_TIMEOUT, run)
                .await
                .ok()?
                .ok()?;
            output
                .status
                .success()
                .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        }
        .boxed()
    }

    fn incus<'a>(&'a self, socket: &'a Path) -> BoxFuture<'a, Result<(), String>> {
        async move {
            crate::incus::Incus::new(socket)
                .ping()
                .await
                .map_err(|error| error.to_string())
        }
        .boxed()
    }
}

/// Where detection looks for what maxd keeps itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    /// The macOS test VM's base image.
    pub vm_base: PathBuf,
    /// The restore image the person saves for the base's build.
    pub vm_restore: PathBuf,
    /// The base build's step, while one is under way.
    pub vm_build: PathBuf,
    /// The Incus socket, as workspaces use it.
    pub incus_socket: PathBuf,
}

impl Paths {
    /// The folder the macOS test VMs are in.
    pub fn vm_folder(&self) -> PathBuf {
        self.vm_base
            .parent()
            .map(Path::to_owned)
            .unwrap_or_default()
    }

    /// The paths under maxd's data folder `data`.
    pub fn under(data: &Path) -> Self {
        Self {
            vm_base: data.join("test-hosts").join("vm").join("base"),
            vm_restore: data.join("test-hosts").join("vm").join("restore.ipsw"),
            vm_build: data.join("test-hosts").join("vm").join("build.json"),
            incus_socket: crate::incus::SOCKET.into(),
        }
    }
}

/// What one backend for one kind came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// The kind.
    pub kind: TestKind,
    /// The backend.
    pub backend: TestHostBackend,
    /// What a build makes it from, or the setup sentence.
    pub found: Result<Ready, String>,
}

impl Finding {
    /// The host it's about on `device`.
    pub fn host(&self, device: DeviceId) -> TestHostId {
        leases::host_id(device, self.kind, self.backend)
    }

    /// What's missing, for [`TestHost::setup`](leases::TestHost::setup).
    pub fn setup(&self) -> Option<String> {
        self.found.as_ref().err().cloned()
    }
}

/// What a backend has, which its build makes the test machine from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ready {
    /// The macOS VM, cloned from this base image.
    MacVm {
        /// The base image, built or not.
        base: PathBuf,
        /// The restore image a build installs from, when there's no base
        /// yet.
        restore: Option<PathBuf>,
    },
    /// The Mac's own Simulator, created on this runtime.
    MacHost {
        /// The newest iOS runtime Xcode has.
        runtime: IosRuntime,
        /// The Node that runs agent-device.
        node: PathBuf,
    },
    /// An Android virtual device, created from this image.
    HostEmulator {
        /// The SDK.
        sdk: AndroidSdk,
        /// The newest system image for this device's CPU.
        image: SystemImage,
        /// The Node that runs agent-device.
        node: PathBuf,
    },
    /// The `mhq-test-linux` container, made from its recipe.
    IncusTest,
}

/// An iOS Simulator runtime, from `simctl list runtimes`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IosRuntime {
    /// What `simctl create` takes, such as
    /// `com.apple.CoreSimulator.SimRuntime.iOS-18-2`.
    pub identifier: String,
    /// Its name, such as `iOS 18.2`.
    pub name: String,
}

/// An Android SDK with what the emulator needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AndroidSdk {
    /// Its folder, for ANDROID_HOME.
    pub root: PathBuf,
}

impl AndroidSdk {
    /// `adb`.
    pub fn adb(&self) -> PathBuf {
        self.root.join("platform-tools").join("adb")
    }

    /// `emulator`.
    pub fn emulator(&self) -> PathBuf {
        self.root.join("emulator").join("emulator")
    }

    /// `avdmanager`.
    pub fn avdmanager(&self) -> PathBuf {
        avdmanager_in(&self.root, LATEST_TOOLS)
    }
}

/// An Android system image in the SDK.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemImage {
    /// What `avdmanager create avd --package` takes, such as
    /// `system-images;android-35;google_apis;arm64-v8a`.
    pub package: String,
    /// Its API level.
    pub api: u32,
}

/// Looks at this device.
pub struct Detector<S = HostSystem> {
    system: S,
    paths: Paths,
}

impl<S: System> Detector<S> {
    /// A detector that reads `system`.
    pub fn new(system: S, paths: Paths) -> Self {
        Self { system, paths }
    }

    /// Looks at every backend for each of `kinds`, in their order.
    pub async fn detect(&self, kinds: &[TestKind]) -> Vec<Finding> {
        let mut look = Look {
            system: &self.system,
            paths: &self.paths,
            node: None,
            mac_vm: None,
        };
        let mut found = Vec::new();
        for &kind in kinds {
            for &backend in kind.backends() {
                let ready = match backend {
                    TestHostBackend::MacVm => look.mac_vm().await,
                    TestHostBackend::MacHost => look.mac_host().await,
                    TestHostBackend::HostEmulator => look.emulator().await,
                    TestHostBackend::IncusTest => look.incus().await,
                };
                found.push(Finding {
                    kind,
                    backend,
                    found: ready,
                });
            }
        }
        found
    }

    /// What `backend` of `kind` comes to, looked at alone.
    pub async fn ready(&self, kind: TestKind, backend: TestHostBackend) -> Result<Ready, String> {
        self.detect(&[kind])
            .await
            .into_iter()
            .find(|finding| finding.backend == backend)
            .map_or_else(
                || Err("MaxHQ doesn't look for that test host.".into()),
                |finding| finding.found,
            )
    }

    /// Looks again at each of `kinds`, and logs what it found.
    pub async fn check(&self, kinds: &[TestKind]) -> Vec<Finding> {
        let found = self.detect(kinds).await;
        for finding in &found {
            let setup = finding.setup();
            info!(
                kind = finding.kind.name(),
                backend = finding.backend.name(),
                setup = setup.as_deref().unwrap_or("none"),
                "looked at a test host"
            );
        }
        found
    }
}

static DETECTOR: OnceLock<Detector> = OnceLock::new();

/// Makes `detector` the one the test host store checks with. Only the
/// first call counts.
pub fn install(detector: Detector) {
    let _ = DETECTOR.set(detector);
}

/// This device's detector, once maxd installed it.
pub fn installed() -> Option<&'static Detector> {
    DETECTOR.get()
}

/// What the installed detector finds for `kind` on `backend` alone, for a
/// backend's own [`TestBackend::detect`](super::backend::TestBackend::detect)
/// and build.
pub async fn ready(kind: TestKind, backend: TestHostBackend) -> Result<Ready, String> {
    match installed() {
        Some(detector) => detector.ready(kind, backend).await,
        None => Err("MaxHQ hasn't looked at this device yet.".into()),
    }
}

/// One round of detection, which asks each shared question once.
struct Look<'a, S> {
    system: &'a S,
    paths: &'a Paths,
    node: Option<Result<PathBuf, String>>,
    mac_vm: Option<Result<Ready, String>>,
}

impl<S: System> Look<'_, S> {
    /// The macOS VM: Apple silicon, macOS 14 or later, Xcode to copy in,
    /// and the base image or the restore image to build it from.
    async fn mac_vm(&mut self) -> Result<Ready, String> {
        if let Some(found) = &self.mac_vm {
            return found.clone();
        }
        let found = self.look_at_mac_vm().await;
        self.mac_vm = Some(found.clone());
        found
    }

    async fn look_at_mac_vm(&self) -> Result<Ready, String> {
        if self.system.os() != "macos" || self.system.arch() != "aarch64" {
            return Err("macOS VMs need a Mac with Apple silicon.".into());
        }
        let version = match self.system.program("sw_vers") {
            Some(sw_vers) => self.system.output(&sw_vers, &["-productVersion"]).await,
            None => None,
        };
        if let Some(version) = version
            && major(&version).is_some_and(|major| major < MACOS_MAJOR)
        {
            return Err(format!(
                "macOS VMs need macOS {MACOS_MAJOR} or later, and this Mac runs {version}."
            ));
        }
        self.xcrun()?;
        let restore = if self.system.exists(&self.paths.vm_base)
            || self.system.exists(&self.paths.vm_build)
        {
            None
        } else if self.system.exists(&self.paths.vm_restore) {
            Some(self.paths.vm_restore.clone())
        } else {
            return Err(format!(
                "The macOS test VM is built from a macOS restore image. Download the \
                 one for this Mac's macOS and save it as {}.",
                self.paths.vm_restore.display()
            ));
        };
        Ok(Ready::MacVm {
            base: self.paths.vm_base.clone(),
            restore,
        })
    }

    /// The Mac's own Simulator: macOS, Xcode, an iOS runtime and Node.
    async fn mac_host(&mut self) -> Result<Ready, String> {
        if self.system.os() != "macos" {
            return Err("iOS Simulators need macOS with Xcode.".into());
        }
        let xcrun = self.xcrun()?;
        let Some(listed) = self
            .system
            .output(&xcrun, &["simctl", "list", "runtimes", "--json"])
            .await
        else {
            return Err(
                "Xcode's Simulator tools didn't answer. Install Xcode, open it once, \
                        and pick it under Locations in Xcode's Settings."
                    .into(),
            );
        };
        let Some(runtime) = newest_ios_runtime(&listed) else {
            return Err(format!(
                "Xcode has no iOS Simulator runtime. Add one under Components in Xcode's \
                 Settings. {NOT_YET_FOR_YOU}"
            ));
        };
        let node = self.node().await?;
        Ok(Ready::MacHost { runtime, node })
    }

    /// The Android emulator on this device: the SDK, a system image for
    /// its CPU, /dev/kvm on Linux, and Node.
    async fn emulator(&mut self) -> Result<Ready, String> {
        let root = self.android_sdk()?;
        let abi = match self.system.arch() {
            "aarch64" => "arm64-v8a",
            "x86_64" => "x86_64",
            _ => return Err("The Android emulator needs an arm64 or x86_64 processor.".into()),
        };
        let Some(image) = newest_image(self.system, &root, abi) else {
            return Err(format!(
                "No Android system image for {abi} is in {}. Install one in Android Studio's \
                 SDK Manager, under SDK Platforms with Show Package Details on. \
                 {NOT_YET_FOR_YOU}",
                root.display()
            ));
        };
        if self.system.os() == "linux" {
            let kvm = Path::new("/dev/kvm");
            if !self.system.exists(kvm) {
                return Err(
                    "The Android emulator needs /dev/kvm, which this device lacks. \
                            Turn on virtualization in its firmware and load the kvm module."
                        .into(),
                );
            }
            if !self.system.opens(kvm) {
                return Err("maxd can't open /dev/kvm. Add its user to the kvm group, \
                            then restart maxd."
                    .into());
            }
        }
        let node = self.node().await?;
        Ok(Ready::HostEmulator {
            sdk: AndroidSdk { root },
            image,
            node,
        })
    }

    /// The Linux container: Linux, and Incus answering as workspaces use
    /// it.
    async fn incus(&self) -> Result<Ready, String> {
        if self.system.os() != "linux" {
            return Err("Linux test containers need a Linux device with Incus.".into());
        }
        let socket = &self.paths.incus_socket;
        if !self.system.exists(socket) {
            return Err(
                "Linux test containers need Incus, which isn't on this device. Install it, \
                 then check again."
                    .into(),
            );
        }
        self.system.incus(socket).await.map_err(|error| {
            format!(
                "Incus doesn't answer on this device: {error}. Make sure it runs and that \
                 maxd's user is in the incus-admin group."
            )
        })?;
        Ok(Ready::IncusTest)
    }

    fn xcrun(&self) -> Result<PathBuf, String> {
        self.system
            .program("xcrun")
            .ok_or_else(|| "Xcode command line tools were not found.".into())
    }

    /// Node 22 or later, asked once a round.
    async fn node(&mut self) -> Result<PathBuf, String> {
        if let Some(found) = &self.node {
            return found.clone();
        }
        let found = self.look_at_node().await;
        self.node = Some(found.clone());
        found
    }

    async fn look_at_node(&self) -> Result<PathBuf, String> {
        let need = format!("Test hosts need Node.js {NODE_MAJOR} or later for agent-device");
        let Some(node) = self.system.program("node") else {
            return Err(format!(
                "{need}. Install it and make sure node is on PATH, then check again. \
                 {NOT_YET_FOR_YOU}"
            ));
        };
        let version = self.system.output(&node, &["--version"]).await;
        let version = version.as_deref().map(|v| v.trim_start_matches('v'));
        match version.and_then(major) {
            Some(major) if major >= NODE_MAJOR => Ok(node),
            _ => Err(format!(
                "{need}, and this device has {}. Update it, then check again. \
                 {NOT_YET_FOR_YOU}",
                version.unwrap_or("one that doesn't say its version")
            )),
        }
    }

    /// The SDK's folder, or what's missing from it.
    fn android_sdk(&self) -> Result<PathBuf, String> {
        let Some(sdk) = find_android_sdk(self.system) else {
            return Err(format!(
                "Android SDK was not found. Install it with Android Studio or set ANDROID_HOME \
                 to your SDK directory. {NOT_YET_FOR_YOU}"
            ));
        };
        let root = sdk.root.display();
        let missing = if !sdk.adb {
            format!(
                "Android SDK Platform-Tools are missing from {root}. Install them in Android \
                 Studio's SDK Manager."
            )
        } else if !sdk.emulator {
            format!(
                "Android Emulator is missing from {root}. Install it in Android Studio's SDK \
                 Manager."
            )
        } else if sdk.legacy_avdmanager {
            format!(
                "The Android SDK command-line tools in {root} appear to be an older, \
                 unsupported version. Install Android SDK Command-line Tools (latest) in \
                 Android Studio's SDK Manager under SDK Tools."
            )
        } else if !sdk.avdmanager {
            format!(
                "Android SDK Command-line Tools (latest) are missing from {root}. Install them \
                 in Android Studio's SDK Manager."
            )
        } else {
            return Ok(sdk.root);
        };
        Err(format!("{missing} {NOT_YET_FOR_YOU}"))
    }
}

/// The command-line tools folder, and the one before it.
const LATEST_TOOLS: &str = "cmdline-tools/latest";
const LEGACY_TOOLS: &str = "tools";

fn avdmanager_in(root: &Path, tools: &str) -> PathBuf {
    root.join(tools).join("bin").join("avdmanager")
}

/// What an SDK folder holds.
struct SdkFolder {
    root: PathBuf,
    adb: bool,
    emulator: bool,
    avdmanager: bool,
    legacy_avdmanager: bool,
}

/// The Android SDK: the one ANDROID_HOME or ANDROID_SDK_ROOT names, else
/// the first usual folder with `adb` or `emulator` in it, else the folder
/// above an `adb` on PATH. PATH comes last, since its folders can be slow
/// to look at.
fn find_android_sdk(system: &impl System) -> Option<SdkFolder> {
    let explicit = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .into_iter()
        .filter_map(|name| system.env(name))
        .map(|value| value.trim().to_owned())
        .find(|value| !value.is_empty());
    let inspect = |root: PathBuf| {
        let adb = system.exists(&root.join("platform-tools").join("adb"));
        let emulator = system.exists(&root.join("emulator").join("emulator"));
        if explicit.is_none() && !adb && !emulator {
            return None;
        }
        let avdmanager = system.exists(&avdmanager_in(&root, LATEST_TOOLS));
        let legacy_avdmanager = !avdmanager && system.exists(&avdmanager_in(&root, LEGACY_TOOLS));
        Some(SdkFolder {
            root,
            adb,
            emulator,
            avdmanager,
            legacy_avdmanager,
        })
    };
    if let Some(explicit) = &explicit {
        return inspect(PathBuf::from(explicit));
    }
    let home = system.home().unwrap_or_default();
    let usual = [
        home.join("Library").join("Android").join("sdk"),
        home.join("Android").join("Sdk"),
        // The one MaxHQ installs on a Mac.
        crate::tools::android::sdk_root_in(&home),
    ];
    if let Some(sdk) = usual.into_iter().find_map(inspect) {
        return Some(sdk);
    }
    let path = system.env("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("adb"))
        .filter(|adb| system.exists(adb))
        .find_map(|adb| {
            let real = system.real_path(&adb);
            inspect(real.parent()?.parent()?.to_owned())
        })
}

/// The newest system image for `abi` in the SDK at `root`, preferring
/// [`IMAGE_TAGS`] in order at the same API level.
fn newest_image(system: &impl System, root: &Path, abi: &str) -> Option<SystemImage> {
    let images = root.join("system-images");
    let tag_rank = |tag: &str| {
        IMAGE_TAGS
            .iter()
            .position(|known| *known == tag)
            .unwrap_or(IMAGE_TAGS.len())
    };
    let mut found = Vec::new();
    for platform in system.list(&images) {
        let Some(api) = api_level(&platform) else {
            continue;
        };
        for tag in system.list(&images.join(&platform)) {
            let folder = images.join(&platform).join(&tag).join(abi);
            if system.exists(&folder.join("source.properties")) {
                let package = format!("system-images;{platform};{tag};{abi}");
                found.push((api, Reverse(tag_rank(&tag)), Reverse(package)));
            }
        }
    }
    found
        .into_iter()
        .max()
        .map(|(api, _, Reverse(package))| SystemImage { package, api })
}

/// The API level in a platform folder's name, such as 35 in `android-35`
/// or `android-34-ext12`. Previews named by letter have none.
fn api_level(platform: &str) -> Option<u32> {
    let rest = platform.strip_prefix("android-")?;
    let digits = rest.split(|c: char| !c.is_ascii_digit()).next()?;
    digits.parse().ok()
}

/// The major number of a version such as `22.11.0` or `15.1`.
fn major(version: &str) -> Option<u32> {
    version.trim().split('.').next()?.parse().ok()
}

/// A version's numbers, to compare.
fn numbers(version: &str) -> Vec<u32> {
    version
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

#[derive(Deserialize)]
struct Runtimes {
    runtimes: Vec<Runtime>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Runtime {
    identifier: String,
    name: String,
    version: String,
    #[serde(default)]
    platform: Option<String>,
    #[serde(default)]
    is_available: bool,
}

/// The newest available iOS runtime in `simctl list runtimes --json`.
pub(crate) fn newest_ios_runtime(listed: &str) -> Option<IosRuntime> {
    let Runtimes { runtimes } = serde_json::from_str(listed).ok()?;
    runtimes
        .into_iter()
        .filter(|runtime| runtime.is_available)
        .filter(|runtime| match &runtime.platform {
            Some(platform) => platform == "iOS",
            None => runtime.name.starts_with("iOS "),
        })
        .max_by_key(|runtime| numbers(&runtime.version))
        .map(|runtime| IosRuntime {
            identifier: runtime.identifier,
            name: runtime.name,
        })
}

#[cfg(test)]
#[path = "detect/tests.rs"]
mod tests;
