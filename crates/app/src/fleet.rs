// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: target
// collection with its skip reasons, the providers-then-servers run order
// and the run statuses. Rewritten in Rust for maxd, its guest binary and the agent CLIs, with MaxHQ's own skip
// reasons.

//! Update everything: bring each device's `maxd`, its guest binary and the
//! agent CLIs it lets MaxHQ update to what this app carries.
//!
//! [`plan`] picks the devices that need something and says why each other
//! one is left out. [`run`] then updates every device at once, each in
//! turn: its CLIs, then `maxd`, which restarts, then the guest binary. Each
//! step reaches the window as a [`FleetProgress`].

#[cfg(feature = "desktop")]
use std::collections::BTreeMap;
use std::future::Future;
#[cfg(feature = "desktop")]
use std::io::Read;
#[cfg(feature = "desktop")]
use std::path::Path;
use std::path::PathBuf;
#[cfg(feature = "desktop")]
use std::sync::Mutex;
use std::time::Duration;
#[cfg(feature = "desktop")]
use std::time::Instant;

use maxhq_protocol::ids::{AttachmentId, DeviceId, IdempotencyKey};
use maxhq_protocol::link::LinkState;
#[cfg(feature = "desktop")]
use maxhq_protocol::message::UploadTarget;
use maxhq_protocol::message::{ActionOutcome, Capabilities, SignedRelease, ToolTarget, UpdateKind};
use maxhq_protocol::model::Tool;
use maxhq_protocol::state::{Entity, EntityKey, ToolInfo, ToolState, ToolUpdate};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::ClientMessage;
use serde::Serialize;
#[cfg(feature = "desktop")]
use sha2::{Digest, Sha256};
use specta::Type;

use crate::devices::Connector;
#[cfg(feature = "desktop")]
use crate::devices::{Devices, OnProgress};
use crate::merge::Merge;
use crate::request::CommandError;
use crate::App;

/// How long a device has to come back on the new build after it restarts.
pub const RESTART_WITHIN: Duration = Duration::from_secs(90);

/// How often the view is checked for the new build while a device restarts.
#[cfg(feature = "desktop")]
const RESTART_POLL: Duration = Duration::from_secs(1);

/// The environment variable that names a folder of Linux binaries, for a
/// build run from the source tree.
pub const BIN_DIR_ENV: &str = "MAXHQ_FLEET_BIN_DIR";

/// Where one device is in an update, as its row shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FleetStatus {
    /// Its turn hasn't come.
    Waiting,
    /// Its agent CLIs are updating.
    Tools,
    /// The new `maxd` is on its way there.
    Uploading,
    /// `maxd` is restarting on the new build.
    Restarting,
    /// The guest binary for its containers is on its way.
    Guests,
    /// It's up to date.
    Done,
    /// It was left out.
    Skipped {
        /// Why, for a person to read.
        reason: String,
    },
    /// A step failed. Later steps didn't run.
    Failed {
        /// Why, for a person to read.
        reason: String,
    },
}

/// One step of an update, for one device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct FleetProgress {
    /// The device.
    pub device: DeviceId,
    /// Where it is now.
    pub status: FleetStatus,
}

/// The name of the signed fleet manifest a release puts beside its
/// binaries. It lists the release's version and each `maxd-<target>`'s
/// sha256.
pub const RELEASE_MANIFEST: &str = "maxd-release.json";

/// The name of the manifest's minisign signature, beside it.
pub const RELEASE_SIGNATURE: &str = "maxd-release.json.sig";

/// A Linux binary this app can send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binary {
    /// Where it is on this machine.
    pub path: PathBuf,
    /// Its sha256, in hex.
    pub sha256: String,
    /// The signed manifest found beside it, which the device checks before
    /// it runs the binary. None for a build from source, which goes out
    /// only once you agree to send a build no release signed.
    pub release: Option<SignedRelease>,
}

impl Binary {
    /// The build a `maxd` running from it reports: the first 16 hex
    /// characters of its sha256.
    pub fn build(&self) -> &str {
        &self.sha256[..16.min(self.sha256.len())]
    }
}

/// Where the binaries come from.
pub trait Binaries: Send + Sync {
    /// The binary of `kind` for a Rust target such as
    /// `x86_64-unknown-linux-musl`, or None when this app has none.
    fn find(&self, kind: UpdateKind, target: &str) -> Option<Binary>;
}

/// Binaries named `maxd-<target>` and `maxd-guest-<target>` in a few
/// folders, the first folder that has one winning. A musl `maxd` is static
/// and serves as the guest too, so it stands in for a missing guest file.
/// A folder with a `SHA256SUMS` must list the file with the same sum. A
/// folder with [`RELEASE_MANIFEST`] and [`RELEASE_SIGNATURE`] sends both
/// with its binaries, for the device to check.
#[cfg(feature = "desktop")]
pub struct BinaryDirs {
    dirs: Vec<PathBuf>,
    /// Sums by path, with the size and change time they were read at.
    sums: Mutex<BTreeMap<PathBuf, Sum>>,
}

/// A file's sha256, with the size and change time it was read at.
#[cfg(feature = "desktop")]
type Sum = (u64, Option<std::time::SystemTime>, String);

#[cfg(feature = "desktop")]
impl BinaryDirs {
    /// Binaries in these folders, in order.
    pub fn new(dirs: Vec<PathBuf>) -> Self {
        Self {
            dirs,
            sums: Mutex::new(BTreeMap::new()),
        }
    }

    /// The folders this app looks in: its bundle's `Resources`, and in a
    /// build from the source tree, the folder [`BIN_DIR_ENV`] names.
    pub fn for_app() -> Self {
        let mut dirs = Vec::new();
        if let Some(resources) = resources_dir() {
            dirs.push(resources);
        }
        if cfg!(debug_assertions)
            && let Some(dir) = std::env::var_os(BIN_DIR_ENV).filter(|dir| !dir.is_empty())
        {
            dirs.push(PathBuf::from(dir));
        }
        Self::new(dirs)
    }

    /// The file with `name` in the first folder that has it, checked.
    fn named(&self, name: &str) -> Option<Binary> {
        self.dirs.iter().find_map(|dir| {
            let path = dir.join(name);
            if !path.is_file() {
                return None;
            }
            let sha256 = self.sha256(&path)?;
            match listed_sum(dir, name) {
                Some(listed) if !listed.eq_ignore_ascii_case(&sha256) => {
                    tracing::warn!(path = %path.display(), "a binary doesn't match its SHA256SUMS");
                    None
                }
                _ => Some(Binary {
                    path,
                    sha256,
                    release: release_in(dir),
                }),
            }
        })
    }

    /// The file's sha256, read again only when it changed.
    fn sha256(&self, path: &Path) -> Option<String> {
        let meta = std::fs::metadata(path).ok()?;
        let stamp = (meta.len(), meta.modified().ok());
        let mut sums = self
            .sums
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let Some((len, modified, sum)) = sums.get(path)
            && (*len, *modified) == stamp
        {
            return Some(sum.clone());
        }
        let sum = sha256_file(path).ok()?;
        sums.insert(path.to_owned(), (stamp.0, stamp.1, sum.clone()));
        Some(sum)
    }
}

#[cfg(feature = "desktop")]
impl BinaryDirs {
    /// The `maxd` builds the folders hold, one for each target, as the
    /// first folder that has it gives it.
    pub fn bundled(&self) -> Vec<BundledBuild> {
        let mut targets: Vec<String> = self
            .dirs
            .iter()
            .filter_map(|dir| std::fs::read_dir(dir).ok())
            .flatten()
            .filter_map(|entry| {
                let name = entry.ok()?.file_name().into_string().ok()?;
                let target = name.strip_prefix("maxd-")?;
                (!target.starts_with("guest-")).then(|| target.to_owned())
            })
            .collect();
        targets.sort();
        targets.dedup();
        targets
            .into_iter()
            .filter_map(|target| {
                let found = self.find(UpdateKind::Maxd, &target)?;
                Some(BundledBuild {
                    build: found.build().to_owned(),
                    signed: found.release.is_some(),
                    target,
                })
            })
            .collect()
    }
}

/// A `maxd` build the app carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct BundledBuild {
    /// The Rust target it runs on.
    pub target: String,
    /// Its build: the first 16 hex characters of its sha256.
    pub build: String,
    /// Whether a signed release manifest came with it. A build from source
    /// has none, and goes out only once you agree to send it unsigned.
    pub signed: bool,
}

#[cfg(feature = "desktop")]
impl Binaries for BinaryDirs {
    fn find(&self, kind: UpdateKind, target: &str) -> Option<Binary> {
        // A device names its target, and that mustn't lead out of the folder.
        let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_');
        if !target.chars().all(plain) {
            return None;
        }
        match kind {
            UpdateKind::Maxd => self.named(&format!("maxd-{target}")),
            UpdateKind::Guest => self.named(&format!("maxd-guest-{target}")).or_else(|| {
                target
                    .ends_with("-musl")
                    .then(|| self.named(&format!("maxd-{target}")))?
            }),
        }
    }
}

/// The folder the app's resources were installed in. None outside a
/// bundle or package.
#[cfg(feature = "desktop")]
fn resources_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    resources_of(&exe, cfg!(target_os = "linux"))
}

/// The resources folder of the app whose program is at `exe`. On macOS
/// it's the bundle's `Resources`, beside `Contents/MacOS`. On Linux it's
/// `lib/MaxHQ` beside the program's `bin`, which a deb or rpm puts at
/// `/usr/lib/MaxHQ` and an AppImage at `$APPDIR/usr/lib/MaxHQ`, as the
/// shell's own resource folder finds it. None when it isn't there.
#[cfg(feature = "desktop")]
fn resources_of(exe: &Path, linux: bool) -> Option<PathBuf> {
    let above = exe.parent()?.parent()?;
    let resources = if linux {
        above.join("lib/MaxHQ")
    } else {
        above.join("Resources")
    };
    resources.is_dir().then_some(resources)
}

/// The bundle `pnpm dev` runs the app from, so Notification Center works.
/// It's made in the target folder, and still counts as the source tree.
pub const DEV_BUNDLE: &str = "MaxHQ Dev.app";

/// Whether this app runs from the source tree rather than a bundle, or on
/// Linux an AppImage or package.
#[cfg(feature = "desktop")]
pub fn from_dev_tree() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return true;
    };
    if cfg!(target_os = "linux") {
        !crate::app_update::LinuxApp::here(&exe).installed()
    } else {
        !in_bundle(&exe)
    }
}

/// Whether `exe` runs from inside a bundle other than the one `pnpm dev`
/// makes.
#[cfg(feature = "desktop")]
fn in_bundle(exe: &Path) -> bool {
    exe.components().any(|part| {
        let part = part.as_os_str().to_string_lossy();
        part.ends_with(".app") && part != DEV_BUNDLE
    })
}

/// The signed manifest in `dir`, when both it and its signature are there.
/// The device checks them, so the app only carries them.
#[cfg(feature = "desktop")]
fn release_in(dir: &Path) -> Option<SignedRelease> {
    let manifest = std::fs::read_to_string(dir.join(RELEASE_MANIFEST)).ok()?;
    let signature = std::fs::read_to_string(dir.join(RELEASE_SIGNATURE)).ok()?;
    Some(SignedRelease {
        manifest,
        signature,
    })
}

/// The sum `SHA256SUMS` in `dir` gives for `name`, in the `sha256sum`
/// format, when the file is there and lists it.
#[cfg(feature = "desktop")]
fn listed_sum(dir: &Path, name: &str) -> Option<String> {
    let sums = std::fs::read_to_string(dir.join("SHA256SUMS")).ok()?;
    parse_sums(&sums, name)
}

/// The sum a `sha256sum` listing gives for `name`.
#[cfg(feature = "desktop")]
fn parse_sums(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (sum, file) = line.trim().split_once(char::is_whitespace)?;
        let file = file.trim_start().trim_start_matches('*');
        let file = file.rsplit('/').next().unwrap_or(file);
        (file == name).then(|| sum.to_ascii_lowercase())
    })
}

/// A file's sha256, in hex.
#[cfg(feature = "desktop")]
pub(crate) fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// What the plan needs to know about one device.
#[derive(Debug, Clone, PartialEq)]
pub struct Machine {
    /// The device.
    pub device: DeviceId,
    /// What the view calls it.
    pub name: String,
    /// Whether a link to it is connected.
    pub connected: bool,
    /// Whether it's winding down for a restart.
    pub winding_down: bool,
    /// What its `maxd` does, from its last hello.
    pub capabilities: Option<Capabilities>,
    /// The build its `maxd` runs.
    pub build: Option<String>,
    /// The version its `maxd` runs, from its last hello.
    pub version: Option<String>,
    /// The Rust target its `maxd` was built for.
    pub target: Option<String>,
    /// Whether it's this Mac.
    pub local: bool,
    /// Whether it runs container workspaces.
    pub workspaces: bool,
    /// The agent CLIs it lets MaxHQ update that are behind.
    pub tools: Vec<ToolTarget>,
}

/// One device the run updates.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    /// The device.
    pub device: DeviceId,
    /// What the view calls it.
    pub name: String,
    /// The CLIs to update first.
    pub tools: Vec<ToolTarget>,
    /// The `maxd` to send, when it's behind.
    pub maxd: Option<Binary>,
    /// The guest binary to send after it.
    pub guest: Option<Binary>,
    /// Whether you agreed to send binaries no release signed, which marks
    /// any such upload unsigned so the device takes it.
    pub unsigned: bool,
}

/// A device the run leaves out, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// The device.
    pub device: DeviceId,
    /// What the view calls it.
    pub name: String,
    /// Why, for a person to read.
    pub reason: String,
}

/// Why a device that would get a `maxd` no release signed is left out
/// until you agree to send it.
pub const UNSIGNED_SKIP: &str =
    "this app's maxd wasn't signed by a release, so it goes only once you choose Install anyway";

/// What a run does.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Plan {
    /// The devices it updates, in the order given.
    pub targets: Vec<Target>,
    /// The ones it leaves out.
    pub skipped: Vec<Skipped>,
}

/// Whether a `maxd` runs a newer version than this app, so the app's
/// build would take it back. One whose version doesn't read can't say, so
/// its build decides as before.
pub fn newer_than_app(version: Option<&str>) -> bool {
    let app = semver::Version::parse(env!("CARGO_PKG_VERSION")).ok();
    let running = version.and_then(|version| semver::Version::parse(version.trim()).ok());
    matches!((running, app), (Some(running), Some(app)) if running > app)
}

/// Picks the devices that need an update, leaving out the ones that are
/// current, or run a newer `maxd` than this app, without a word and naming
/// why each other one can't. `tools`
/// says whether to update agent CLIs too. This Mac's own `maxd` moves
/// with the app, and `dev_tree` says the app runs from the source tree,
/// where it restarts with Start maxd instead. `unsigned` says you agreed
/// to send a `maxd` no release signed, built on this Mac. Without it a
/// device that would get one is left out.
pub fn plan(
    machines: &[Machine],
    binaries: &dyn Binaries,
    tools: bool,
    dev_tree: bool,
    unsigned: bool,
) -> Plan {
    let mut plan = Plan::default();
    for machine in machines {
        let skip = |plan: &mut Plan, reason: &str| {
            plan.skipped.push(Skipped {
                device: machine.device,
                name: machine.name.clone(),
                reason: reason.to_owned(),
            });
        };
        let wanted_tools = if tools {
            machine.tools.clone()
        } else {
            Vec::new()
        };
        // A maxd from before builds can't say which it runs, so it's behind.
        let maxd = machine
            .target
            .as_deref()
            .and_then(|target| binaries.find(UpdateKind::Maxd, target));
        let behind = match (&machine.build, &maxd) {
            (Some(build), Some(binary)) => build != binary.build(),
            // Without a binary for its target the app can't tell, so it
            // says so rather than call the device current.
            (None, _) | (Some(_), None) => true,
        } && !newer_than_app(machine.version.as_deref());
        if !behind && wanted_tools.is_empty() {
            continue;
        }
        // This Mac's maxd ships inside the app and moves with it, so a
        // bundled app leaves it be.
        // Its CLIs still update.
        if machine.local && wanted_tools.is_empty() {
            if dev_tree {
                skip(
                    &mut plan,
                    "runs from your source tree, so restart it with Start maxd",
                );
            }
            continue;
        }
        let behind = behind && !machine.local;
        if !machine.connected {
            skip(&mut plan, "not connected");
            continue;
        }
        if machine.winding_down {
            skip(&mut plan, "winding down");
            continue;
        }
        let updates = machine.capabilities.is_some_and(|does| does.updates);
        if behind && (!updates || machine.target.is_none()) {
            skip(
                &mut plan,
                "its maxd is too old to update itself, so set it up again",
            );
            continue;
        }
        if behind && maxd.is_none() && wanted_tools.is_empty() {
            let target = machine.target.as_deref().unwrap_or("its system");
            skip(&mut plan, &format!("this app has no maxd for {target}"));
            continue;
        }
        let maxd = maxd.filter(|_| behind);
        let guest = match (&maxd, &machine.target) {
            (Some(_), Some(target)) if machine.workspaces => {
                binaries.find(UpdateKind::Guest, target)
            }
            _ => None,
        };
        // A build no release signed only goes out once you've agreed to it.
        let unsigned_binary = [&maxd, &guest]
            .into_iter()
            .flatten()
            .any(|binary| binary.release.is_none());
        if unsigned_binary && !unsigned {
            skip(&mut plan, UNSIGNED_SKIP);
            continue;
        }
        plan.targets.push(Target {
            device: machine.device,
            name: machine.name.clone(),
            tools: wanted_tools,
            maxd,
            guest,
            unsigned,
        });
    }
    plan
}

/// The agent CLIs in `tools` that MaxHQ may update now and that are behind
/// the latest release. One on the host waiting for a tick counts once it's
/// in `ticked`, as the device's settings say. One already updating is left
/// alone.
pub fn tools_to_update(tools: &[ToolInfo], ticked: &[Tool]) -> Vec<ToolTarget> {
    tools
        .iter()
        .filter_map(|info| info.tool().map(|tool| (tool, info)))
        .filter(|(tool, info)| match info.update {
            ToolUpdate::Auto | ToolUpdate::Allowed => true,
            ToolUpdate::NeedsTick => ticked.contains(tool),
            ToolUpdate::Manual { .. } | ToolUpdate::Never => false,
        })
        .filter(|(_, info)| matches!(info.state, ToolState::Idle | ToolState::Failed { .. }))
        .filter(|(_, info)| match (&info.version, &info.latest) {
            (Some(version), Some(latest)) => version != latest,
            _ => false,
        })
        .map(|(tool, info)| ToolTarget {
            tool,
            place: info.place,
        })
        .collect()
}

/// What a run asks of the devices.
pub trait FleetOps: Sync {
    /// Has the device update these CLIs.
    fn update_tools(
        &self,
        device: DeviceId,
        tools: Vec<ToolTarget>,
    ) -> impl Future<Output = Result<ActionOutcome, CommandError>> + Send;

    /// Uploads a file to the device's staging folder, and gives the
    /// attachment it became.
    fn upload(
        &self,
        device: DeviceId,
        path: PathBuf,
    ) -> impl Future<Output = Result<AttachmentId, CommandError>> + Send;

    /// Has the device install an uploaded binary, with the signed release
    /// that lists it, or else marked `unsigned` when you agreed to send it.
    fn install(
        &self,
        device: DeviceId,
        staged: AttachmentId,
        kind: UpdateKind,
        sha256: String,
        release: Option<SignedRelease>,
        unsigned: bool,
    ) -> impl Future<Output = Result<ActionOutcome, CommandError>> + Send;

    /// Waits until the device's `maxd` says hello on `build`, for at most
    /// `within`, and says whether it did.
    fn wait_for_build(
        &self,
        device: DeviceId,
        build: String,
        within: Duration,
    ) -> impl Future<Output = bool> + Send;
}

/// Runs a plan: each skipped device is told so, every target waits, then
/// all of them update at once, each in turn through its CLIs, `maxd` and
/// the guest binary. A failed CLI update doesn't stop `maxd`'s, but the
/// device ends failed. Any other failure ends that device's run.
pub async fn run(plan: Plan, ops: &impl FleetOps, report: &(dyn Fn(FleetProgress) + Sync)) {
    for skipped in &plan.skipped {
        report(FleetProgress {
            device: skipped.device,
            status: FleetStatus::Skipped {
                reason: skipped.reason.clone(),
            },
        });
    }
    for target in &plan.targets {
        report(FleetProgress {
            device: target.device,
            status: FleetStatus::Waiting,
        });
    }
    let runs = plan.targets.into_iter().map(|target| async move {
        let device = target.device;
        let status = match update_one(target, ops, report).await {
            Ok(()) => FleetStatus::Done,
            Err(reason) => FleetStatus::Failed { reason },
        };
        report(FleetProgress { device, status });
    });
    futures::future::join_all(runs).await;
}

/// Updates one device, reporting each step, and gives why it failed.
async fn update_one(
    target: Target,
    ops: &impl FleetOps,
    report: &(dyn Fn(FleetProgress) + Sync),
) -> Result<(), String> {
    let device = target.device;
    let consent = target.unsigned;
    // The mark goes only on an upload no release signed, and only when you
    // agreed to send one.
    let mark = |binary: &Binary| consent && binary.release.is_none();
    let step = |status| report(FleetProgress { device, status });
    let mut failure = None;
    if !target.tools.is_empty() {
        step(FleetStatus::Tools);
        if let Err(reason) = accepted(ops.update_tools(device, target.tools).await) {
            failure = Some(format!("The agent CLIs didn't update: {reason}"));
        }
    }
    if let Some(maxd) = target.maxd {
        step(FleetStatus::Uploading);
        let staged = ops.upload(device, maxd.path.clone()).await.map_err(words)?;
        let installed = ops
            .install(
                device,
                staged,
                UpdateKind::Maxd,
                maxd.sha256.clone(),
                maxd.release.clone(),
                mark(&maxd),
            )
            .await;
        accepted(installed)?;
        step(FleetStatus::Restarting);
        if !ops
            .wait_for_build(device, maxd.build().to_owned(), RESTART_WITHIN)
            .await
        {
            return Err(format!(
                "maxd didn't come back on the new build within {} seconds",
                RESTART_WITHIN.as_secs()
            ));
        }
    }
    if let Some(guest) = target.guest {
        step(FleetStatus::Guests);
        let staged = ops
            .upload(device, guest.path.clone())
            .await
            .map_err(words)?;
        accepted(
            ops.install(
                device,
                staged,
                UpdateKind::Guest,
                guest.sha256.clone(),
                guest.release.clone(),
                mark(&guest),
            )
            .await,
        )?;
    }
    failure.map_or(Ok(()), Err)
}

/// An action's result as a failure in words, when the device refused.
fn accepted(outcome: Result<ActionOutcome, CommandError>) -> Result<(), String> {
    match outcome.map_err(words)? {
        ActionOutcome::Refused { reason } => Err(reason),
        _ => Ok(()),
    }
}

/// A request error in words.
fn words(error: CommandError) -> String {
    error.to_string()
}

/// What the plan needs to know about every device the view shows, or only
/// those named in `only`.
pub fn machines(merge: &Merge, only: Option<&[DeviceId]>) -> Vec<Machine> {
    let store = merge.store();
    let now = Timestamp::now();
    let mut machines = Vec::new();
    for entity in store.entities() {
        let Entity::Device(device) = entity else {
            continue;
        };
        if only.is_some_and(|only| !only.contains(&device.id)) {
            continue;
        }
        let links: Vec<_> = store
            .entities()
            .filter_map(|entity| match entity {
                Entity::DeviceLink(link) if link.device == Some(device.id) => Some(link),
                _ => None,
            })
            .collect();
        let connected = links
            .iter()
            .find(|link| matches!(link.state, LinkState::Connected { .. }));
        let ticked = match store.get(&EntityKey::DeviceSettings(device.id)) {
            Some(Entity::DeviceSettings(settings)) => settings.tool_updates.as_slice(),
            _ => &[],
        };
        let tools = match store.get(&EntityKey::DeviceTools(device.id)) {
            Some(Entity::DeviceTools(found)) => tools_to_update(&found.tools, ticked),
            _ => Vec::new(),
        };
        machines.push(Machine {
            device: device.id,
            name: device.name.clone(),
            connected: connected.is_some(),
            winding_down: device
                .winding_down_until
                .is_some_and(|until| until.0 > now.0),
            capabilities: connected
                .and_then(|link| link.capabilities)
                .or_else(|| links.iter().find_map(|link| link.capabilities)),
            build: device.build.clone(),
            version: Some(device.maxd_version.clone()),
            target: device.target.clone(),
            local: links
                .iter()
                .any(|link| matches!(link.route, maxhq_protocol::link::Route::Local)),
            workspaces: device.workspace_host.is_some(),
            tools,
        });
    }
    machines
}

/// The build the view shows a device's `maxd` on.
#[cfg(feature = "desktop")]
fn shown_build(merge: &Merge, device: DeviceId) -> Option<String> {
    match merge.store().get(&EntityKey::Device(device)) {
        Some(Entity::Device(found)) => found.build.clone(),
        _ => None,
    }
}

/// A run's way to the devices, through the app.
#[cfg(feature = "desktop")]
struct Through<'a, C: Connector> {
    devices: &'a Devices<C>,
    view: &'a crate::view::View,
}

#[cfg(feature = "desktop")]
impl<C: Connector> FleetOps for Through<'_, C> {
    async fn update_tools(
        &self,
        device: DeviceId,
        tools: Vec<ToolTarget>,
    ) -> Result<ActionOutcome, CommandError> {
        let key = IdempotencyKey::generate();
        self.devices
            .act_on(device, ClientMessage::UpdateTools { key, tools })
            .await
    }

    async fn upload(&self, device: DeviceId, path: PathBuf) -> Result<AttachmentId, CommandError> {
        let quiet: OnProgress = std::sync::Arc::new(|_| {});
        match self
            .devices
            .upload(device, UploadTarget::Device, path, quiet)
            .await?
        {
            ActionOutcome::Staged { attachment } => Ok(attachment.id),
            ActionOutcome::Refused { reason } => Err(CommandError::Failed { message: reason }),
            _ => Err(CommandError::Failed {
                message: "The device didn't keep the upload".to_owned(),
            }),
        }
    }

    async fn install(
        &self,
        device: DeviceId,
        staged: AttachmentId,
        kind: UpdateKind,
        sha256: String,
        release: Option<SignedRelease>,
        unsigned: bool,
    ) -> Result<ActionOutcome, CommandError> {
        let key = IdempotencyKey::generate();
        let action = ClientMessage::InstallUpdate {
            key,
            staged,
            kind,
            sha256,
            release,
            unsigned,
        };
        self.devices.act_on(device, action).await
    }

    async fn wait_for_build(&self, device: DeviceId, build: String, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        loop {
            if self
                .view
                .read(|merge| shown_build(merge, device))
                .as_deref()
                == Some(&build)
            {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(RESTART_POLL).await;
        }
    }
}

/// What a device needs for one of the fleet requests.
#[derive(Clone, Copy)]
pub(crate) enum Needs {
    /// `ReadLoad` and `ReadDiagnostics`.
    Load,
    /// `UpdateTools`.
    Tools,
    /// Permissions and access keys.
    AgentTools,
    /// Scheduled tasks and their webhooks.
    Tasks,
    /// Links to other devices.
    Peers,
    /// Pull requests: the list, the viewer's reads, and links.
    Pulls,
    /// Your GitHub sign-in, and merging and approving as you.
    GithubUser,
    /// Test hosts and their leases.
    TestHosts,
    /// Workflow runs and their nodes.
    Workflows,
    /// The skill library, project skills and proposals.
    Skills,
    /// Moving the skill library, and pushing its sources.
    LibraryMoves,
    /// Checking and applying the host spec.
    HostSpec,
    /// A workspace's snapshots.
    Snapshots,
    /// Moving a workspace onto MaxHQ.
    Moves,
    /// Installing and removing tools, and their licences.
    ToolInstalls,
    /// Phones, and the APNs key that pushes to them.
    Phones,
    /// Local models: model servers, downloads and loads.
    LocalModels,
}

/// Refuses in words a request a device's `maxd` is too old for, since it
/// wouldn't answer at all.
pub(crate) fn check_can(merge: &Merge, device: DeviceId, needs: Needs) -> Result<(), CommandError> {
    let Some(does) = crate::devices::capabilities(merge, device) else {
        return Ok(());
    };
    let (can, what) = match needs {
        Needs::Load => (does.load, "report its load"),
        Needs::Tools => (does.tools, "update its agent CLIs"),
        Needs::AgentTools => (does.agent_tools, "let agents run agents"),
        Needs::Tasks => (does.scheduled_tasks, "run scheduled tasks"),
        Needs::Peers => (does.peers, "link to other devices"),
        Needs::Pulls => (does.pulls, "read pull requests"),
        Needs::GithubUser => (does.github_user, "hold your GitHub sign-in"),
        Needs::TestHosts => (does.test_hosts, "offer test hosts"),
        Needs::Workflows => (does.workflows, "report its workflows"),
        Needs::Skills => (does.skills, "keep skills"),
        Needs::LibraryMoves => (does.library_moves, "move the skill library"),
        Needs::HostSpec => (does.host_spec, "check its host"),
        Needs::Snapshots => (does.snapshots, "keep snapshots"),
        Needs::Moves => (does.moves, "move workspaces"),
        Needs::ToolInstalls => (does.tool_installs, "install tools"),
        Needs::Phones => (does.phones, "push to phones"),
        Needs::LocalModels => (does.local_models, "run local models"),
    };
    if can {
        Ok(())
    } else {
        Err(CommandError::Failed {
            message: format!(
                "{}'s maxd is too old to {what}. Update it first",
                device_name(merge, device)
            ),
        })
    }
}

/// What the view calls a device, from the device or else a link to it.
pub(crate) fn device_name(merge: &Merge, device: DeviceId) -> String {
    let store = merge.store();
    match store.get(&EntityKey::Device(device)) {
        Some(Entity::Device(found)) => found.name.clone(),
        _ => store
            .entities()
            .find_map(|entity| match entity {
                Entity::DeviceLink(link) if link.device == Some(device) => Some(link.name.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "The device".to_owned()),
    }
}

impl<C: Connector> App<C> {
    /// Sends an action to `device` under a fresh key, once its `maxd` does
    /// what `needs` names.
    pub(crate) async fn act_needing(
        &self,
        device: DeviceId,
        needs: Needs,
        action: impl FnOnce(IdempotencyKey) -> ClientMessage,
    ) -> Result<ActionOutcome, CommandError> {
        self.view.read(|merge| check_can(merge, device, needs))?;
        self.devices
            .act_on(device, action(IdempotencyKey::generate()))
            .await
    }

    /// Updates `devices`, or every device when it's empty, to the `maxd`
    /// and guest binary this app carries, and their agent CLIs too when
    /// `tools` is set. `unsigned` says you agreed to send a `maxd` no
    /// release signed, built on this Mac; without it a device that would
    /// get one is skipped. `report` hears each device's every step. It ends
    /// once every device is done, failed or skipped.
    #[cfg(feature = "desktop")]
    pub async fn update_fleet(
        &self,
        devices: Vec<DeviceId>,
        tools: bool,
        unsigned: bool,
        binaries: &dyn Binaries,
        report: &(dyn Fn(FleetProgress) + Sync),
    ) {
        let only = (!devices.is_empty()).then_some(devices.as_slice());
        let machines = self.view.read(|merge| machines(merge, only));
        let plan = plan(&machines, binaries, tools, from_dev_tree(), unsigned);
        let through = Through {
            devices: &self.devices,
            view: &self.view,
        };
        run(plan, &through, report).await;
    }
}

#[cfg(test)]
mod tests;
