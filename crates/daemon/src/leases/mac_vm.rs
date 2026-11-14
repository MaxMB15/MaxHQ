// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The `macos` and `ios_simulator` test hosts in a macOS VM on this Mac.
//! `maxd-vm`, from next to `maxd`, runs each VM; see [`maxhq_vm`].
//!
//! Both kinds share one base bundle, which a build makes once from a
//! restore image the person downloads: the install, the setup assistant
//! and one command typed in the guest's Terminal, then Xcode, the iOS
//! runtime, Node and agent-device over the guest link, then a saved state.
//! Each lease runs in an APFS clone of the base, restored from that state,
//! and the clone goes at the lease's end. Its guest runs `maxd guest --vm`
//! on vsock, which the helper relays to the socket the guest links use.
//!
//! The folder holds only what `maxd` made: the base, the base while it
//! builds with its step and shared folder, and each lease's clone.

use std::collections::HashMap;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures::future::BoxFuture;
use futures::FutureExt;
use maxhq_protocol::guest::GuestToHost;
use maxhq_protocol::ids::{LeaseId, WorkspaceId};
use maxhq_protocol::leases::{RunOutput, TestHostBackend, TestKind, VmSize};
use maxhq_transfer::beneath::Dir;
use maxhq_vm::{bundle, Event, Op, Request, Shape, Share, GUEST_SHARE};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;

use super::backend::{Built, LeaseTarget, TestBackend};
use super::detect::{self, Ready};
use super::mac_host::copy_app;
use super::via_guest::{self, Guest};
use crate::agent_device;
use crate::incus::InstanceName;
use crate::transfer::{self, MakeRequest};
use crate::workspace::GuestLinks;

/// The base bundle, once built.
const BASE: &str = "base";

/// The base while it builds.
const BUILDING: &str = "building";

/// The build's next step, kept so a build picks up where the last one
/// stopped.
const STEP_FILE: &str = "build.json";

/// The folder the build shares with the guest.
const BUILD_SHARE: &str = "build-share";

/// Where the build's large copies wait, beside its shared folder.
const BUILD_STAGE: &str = "build-stage";

/// Each lease's clone, in a folder for its kind.
const LEASES: &str = "leases";

/// The folder a lease's clone shares with its guest.
const SHARE: &str = "share";

/// The file in the shared folder that tells the guest what it is.
pub(crate) const LEASE_FILE: &str = "lease.json";

/// `maxd` in the shared folder, which the guest runs.
pub(crate) const MAXD_FILE: &str = "maxd";

/// The setup script in the build's shared folder.
const SETUP_FILE: &str = "setup";

/// Where uploads go in a lease's shared folder.
const UPLOADS: &str = "uploads";

/// Where uploads wait in a lease's clone, beside the shared folder, on the
/// same disk and out of the guest's sight.
const STAGE: &str = "stage";

/// How many names an upload tries before it gives up on a guest that keeps
/// taking them.
const UPLOAD_TRIES: usize = 64;

/// The size a new base gets. The restore image may ask for more.
const SHAPE: Shape = Shape {
    cpus: 4,
    memory_bytes: 8 << 30,
    disk_bytes: 96 << 30,
    mac: None,
};

/// How long an install from the restore image may take.
const INSTALL_WAIT: Duration = Duration::from_secs(60 * 60);

/// How long a VM may take to start or resume.
const START_WAIT: Duration = Duration::from_secs(5 * 60);

/// How long a saved state may take to write.
const SAVE_WAIT: Duration = Duration::from_secs(10 * 60);

/// How long a stop may take, then how long the helper has to exit.
const STOP_WAIT: Duration = Duration::from_secs(60);

/// How long a resumed or booted guest has to link.
const GUEST_WAIT: Duration = Duration::from_secs(3 * 60);

/// How long the person has for the setup assistant and the command.
const SETUP_WAIT: Duration = Duration::from_secs(2 * 60 * 60);

/// How long a copy into a shared folder may take, which Xcode's is.
const COPY_WAIT: Duration = Duration::from_secs(30 * 60);

/// How long one build step may run in the guest, the longest a run may.
const STEP_WAIT: Duration = Duration::from_secs(maxhq_core::leases::MAX_RUN_SECONDS as u64);

/// The command the person types in the guest's Terminal, which the setup
/// window shows below [`SETUP_NOTE`].
pub const SETUP_COMMAND: &str = r#"sh "/Volumes/My Shared Files/setup""#;

/// What the setup window shows above the guest's screen.
const SETUP_NOTE: &str = "Click through the setup assistant, make a user and leave \
FileVault off. Use a password you use nowhere else, since the VM keeps it to log in by itself \
and anything running in the VM can read it. Then open Terminal in the VM and type this, then \
your password once:";

/// What the guest knows about the run it's in, from [`LEASE_FILE`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LeaseFile {
    /// The kind it serves.
    pub(crate) kind: TestKind,
    /// The instance name the host links to it by.
    pub(crate) instance: String,
}

/// Runs the VMs. [`Helper`] does it through `maxd-vm`.
pub(super) trait Vms: Send + Sync + 'static {
    /// Makes a bundle at `bundle` and installs macOS into it from the
    /// restore image file `restore_image`.
    fn install<'a>(
        &'a self,
        restore_image: &'a Path,
        bundle: &'a Path,
    ) -> BoxFuture<'a, Result<(), String>>;
    /// Runs a bundle as `start`, an [`Op::Start`], and answers once it
    /// runs.
    fn start(&self, start: Op) -> BoxFuture<'_, Result<Box<dyn Vm>, String>>;
}

/// One running VM.
pub(super) trait Vm: Send + Sync {
    /// Pauses it and saves its state into its bundle.
    fn save(&self) -> BoxFuture<'_, Result<(), String>>;
    /// Stops it and waits for its helper to exit, so the bundle can run
    /// again.
    fn stop(&self) -> BoxFuture<'_, ()>;
}

/// The guest links, as a VM's guest uses them. [`Wired`] does it.
pub(super) trait Links: Send + Sync + 'static {
    /// Links to the guest in `instance` and waits up to `wait` for it.
    fn up<'a>(
        &'a self,
        workspace: WorkspaceId,
        instance: &'a InstanceName,
        wait: Duration,
    ) -> BoxFuture<'a, Result<(), String>>;
    /// Drops the link.
    fn down(&self, workspace: WorkspaceId);
    /// Sends the guest the request `make` builds, as [`Guest::ask`].
    fn ask(
        &self,
        workspace: WorkspaceId,
        wait: Duration,
        make: MakeRequest,
    ) -> BoxFuture<'_, Result<GuestToHost, String>>;
}

/// What the base build takes from the Mac. [`ThisMac`] finds it.
pub(super) trait Mac: Send + Sync + 'static {
    /// The Xcode that `xcode-select` picks, as its `.app` folder.
    fn xcode(&self) -> BoxFuture<'_, Result<PathBuf, String>>;
    /// The disk image of Xcode's newest iOS Simulator runtime.
    fn ios_runtime(&self) -> BoxFuture<'_, Result<PathBuf, String>>;
}

/// What a build makes the base from, as detection found it.
type Find = Arc<dyn Fn() -> BoxFuture<'static, Result<Ready, String>> + Send + Sync>;

/// What both kinds share.
struct Shared {
    /// `<data>/test-hosts/vm`.
    folder: PathBuf,
    /// The folder the guest links' sockets are under.
    runtime: PathBuf,
    /// The `maxd` the guest runs.
    maxd: PathBuf,
    vms: Arc<dyn Vms>,
    links: Arc<dyn Links>,
    mac: Arc<dyn Mac>,
    find: Find,
    /// Held while the base builds, so the two kinds build it once.
    building: tokio::sync::Mutex<()>,
}

/// One kind's test host in a macOS VM.
pub struct MacVm {
    kind: TestKind,
    shared: Arc<Shared>,
    /// The lease each running VM serves.
    running: Mutex<HashMap<LeaseId, Running>>,
    /// Numbers each upload's folder.
    uploads: AtomicU64,
}

/// A lease's VM.
struct Running {
    vm: Arc<dyn Vm>,
    workspace: WorkspaceId,
    instance: InstanceName,
}

impl MacVm {
    /// Both kinds on this Mac, with VMs under `data`, guest sockets under
    /// `runtime` and the guest links in `links` and `guests`.
    pub fn on_this_mac(
        data: &Path,
        runtime: PathBuf,
        links: Arc<dyn GuestLinks>,
        guests: Arc<dyn transfer::Guests>,
    ) -> [MacVm; 2] {
        let maxd = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("maxd"));
        let helper = maxd.with_file_name(maxhq_vm::BINARY);
        let find: Find =
            Arc::new(|| detect::ready(TestKind::Macos, TestHostBackend::MacVm).boxed());
        let shared = Arc::new(Shared {
            folder: detect::Paths::under(data).vm_folder(),
            runtime,
            maxd,
            vms: Arc::new(Helper { binary: helper }),
            links: Arc::new(Wired { links, guests }),
            mac: Arc::new(ThisMac),
            find,
            building: tokio::sync::Mutex::new(()),
        });
        [
            MacVm::new(TestKind::Macos, shared.clone()),
            MacVm::new(TestKind::IosSimulator, shared),
        ]
    }

    fn new(kind: TestKind, shared: Arc<Shared>) -> Self {
        Self {
            kind,
            shared,
            running: Mutex::new(HashMap::new()),
            uploads: AtomicU64::new(0),
        }
    }

    fn running(&self) -> std::sync::MutexGuard<'_, HashMap<LeaseId, Running>> {
        self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The folder this kind's clones are in.
    fn clones(&self) -> PathBuf {
        self.shared.folder.join(LEASES).join(self.kind.name())
    }

    /// The guest of `lease`'s VM.
    fn guest(&self, lease: LeaseId) -> Result<Linked<'_>, String> {
        let workspace = self
            .running()
            .get(&lease)
            .map(|running| running.workspace)
            .ok_or("That lease's VM isn't running.")?;
        Ok(Linked {
            links: self.shared.links.as_ref(),
            workspace,
        })
    }

    async fn begin(&self, lease: LeaseId) -> Result<String, String> {
        let base = self.shared.folder.join(BASE);
        if !base.join(bundle::STATE).exists() {
            return Err("The macOS test VM isn't built yet.".into());
        }
        let clone = self.clones().join(lease.to_string());
        remove(&clone)?;
        copy_app(&base, &clone, COPY_WAIT).await?;
        let instance = instance(&lease.to_string());
        let started = self.run_clone(&clone, &instance).await;
        match started {
            Ok((vm, workspace)) => {
                self.running().insert(
                    lease,
                    Running {
                        vm,
                        workspace,
                        instance: instance.clone(),
                    },
                );
                Ok(instance.to_string())
            }
            Err(error) => {
                let _ = remove(&clone);
                Err(error)
            }
        }
    }

    /// Resumes the clone at `clone` and links to its guest.
    async fn run_clone(
        &self,
        clone: &Path,
        instance: &InstanceName,
    ) -> Result<(Arc<dyn Vm>, WorkspaceId), String> {
        let share = clone.join(SHARE);
        prepare_share(&share, self.kind, instance, &self.shared.maxd).await?;
        let workspace = workspace(instance);
        let vm: Arc<dyn Vm> = self
            .shared
            .vms
            .start(Op::Start {
                bundle: clone.to_owned(),
                share: Share {
                    path: share,
                    read_only: false,
                },
                socket: self.shared.socket(instance)?,
                restore: true,
                window: false,
                note: None,
            })
            .await?
            .into();
        if let Err(error) = self.shared.links.up(workspace, instance, GUEST_WAIT).await {
            self.shared.links.down(workspace);
            vm.stop().await;
            return Err(format!("The VM's guest didn't link: {error}"));
        }
        Ok((vm, workspace))
    }

    /// Stops `lease`'s VM and deletes its clone.
    async fn end(&self, lease: LeaseId) -> Result<(), String> {
        let running = self.running().remove(&lease);
        if let Some(running) = running {
            self.shared.links.down(running.workspace);
            running.vm.stop().await;
            if let Ok(socket) = self.shared.socket(&running.instance) {
                let _ = std::fs::remove_file(socket);
            }
        }
        remove(&self.clones().join(lease.to_string()))
    }

    async fn put(&self, on: &LeaseTarget, source: &Path) -> Result<String, String> {
        let name = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("{} has no name to install it by.", source.display()))?
            .to_owned();
        let guest = self.guest(on.lease)?;
        let clone = self.clones().join(on.lease.to_string());
        let stage = clone.join(STAGE);
        let n = self.uploads.fetch_add(1, Ordering::Relaxed).to_string();
        let staged = stage.join(&n);
        remove(&staged)?;
        let placed = match copy_app(source, &staged.join(&name), COPY_WAIT).await {
            Ok(()) => place_upload(&clone.join(SHARE), &stage, &n, &self.uploads),
            Err(error) => Err(error),
        };
        remove(&staged)?;
        let path = Path::new(GUEST_SHARE)
            .join(UPLOADS)
            .join(placed?)
            .join(&name);
        via_guest::install(&guest, on.lease, path.display().to_string()).await?;
        Ok(match self.kind {
            TestKind::IosSimulator => format!("Installed {name} on the Simulator."),
            _ => format!("Copied {name} into the test user's home folder."),
        })
    }

    /// The base's size, and the agent-device it drives with.
    fn built(&self) -> Result<Built, String> {
        let base = self.shared.folder.join(BASE);
        let shape = read_shape(&base)?;
        let disk = std::fs::metadata(base.join(bundle::DISK))
            .map(|meta| std::os::unix::fs::MetadataExt::blocks(&meta) * 512)
            .unwrap_or(0);
        Ok(Built {
            agent_device: Some(agent_device::VERSION.to_owned()),
            vm: Some(VmSize {
                disk_bytes: disk,
                memory_bytes: shape.memory_bytes,
            }),
        })
    }
}

impl TestBackend for MacVm {
    fn kind(&self) -> TestKind {
        self.kind
    }

    fn backend(&self) -> TestHostBackend {
        TestHostBackend::MacVm
    }

    fn detect(&self) -> BoxFuture<'_, Option<String>> {
        async move { detect::ready(self.kind, TestHostBackend::MacVm).await.err() }.boxed()
    }

    fn build(&self) -> BoxFuture<'_, Result<Built, String>> {
        async move {
            let _held = self.shared.building.lock().await;
            if !self.shared.folder.join(BASE).join(bundle::STATE).exists() {
                build_base(&self.shared).await?;
            }
            self.built()
        }
        .boxed()
    }

    fn start(&self, lease: LeaseId) -> BoxFuture<'_, Result<String, String>> {
        self.begin(lease).boxed()
    }

    fn run<'a>(
        &'a self,
        on: &'a LeaseTarget,
        command: &'a [String],
        timeout: Duration,
    ) -> BoxFuture<'a, Result<RunOutput, String>> {
        async move {
            let guest = self.guest(on.lease)?;
            via_guest::run(&guest, on.lease, command, timeout).await
        }
        .boxed()
    }

    fn screenshot<'a>(&'a self, on: &'a LeaseTarget) -> BoxFuture<'a, Result<Vec<u8>, String>> {
        async move {
            let guest = self.guest(on.lease)?;
            via_guest::screenshot(&guest, on.lease).await
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
        async move {
            // The clone goes whatever the guest said.
            if let Ok(guest) = self.guest(on.lease)
                && let Err(error) = via_guest::close(&guest, on.lease).await
            {
                let lease = on.lease;
                tracing::warn!(%lease, %error, "the VM's guest didn't close the lease");
            }
            self.end(on.lease).await
        }
        .boxed()
    }

    fn reset(&self) -> BoxFuture<'_, Result<(), String>> {
        async move {
            let leases: Vec<LeaseId> = self.running().keys().copied().collect();
            for lease in leases {
                self.end(lease).await?;
            }
            remove(&self.clones())
        }
        .boxed()
    }
}

impl Shared {
    /// The socket the guest link for `instance` connects to.
    fn socket(&self, instance: &InstanceName) -> Result<PathBuf, String> {
        let socket = crate::guestlink::socket_path(&self.runtime, instance);
        if let Some(parent) = socket.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("couldn't make the guest links' folder: {err}"))?;
        }
        Ok(socket)
    }
}

/// The guest of one running VM.
struct Linked<'a> {
    links: &'a dyn Links,
    workspace: WorkspaceId,
}

impl Guest for Linked<'_> {
    fn ask(&self, wait: Duration, make: MakeRequest) -> BoxFuture<'_, Result<GuestToHost, String>> {
        self.links.ask(self.workspace, wait, make)
    }
}

/// The instance name a VM's guest links by, from `seed`, as a lease ID.
fn instance(seed: &str) -> InstanceName {
    let short: String = seed
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .take(8)
        .collect();
    InstanceName::new(&format!("mhq-vm-{short}")).expect("a VM's instance name is valid")
}

/// The workspace a VM's guest link is kept under.
fn workspace(instance: &InstanceName) -> WorkspaceId {
    WorkspaceId::derive(&format!("mac-vm/{instance}"))
}

/// Writes what the guest reads from its shared folder: what it serves, and
/// the `maxd` it should run.
async fn prepare_share(
    share: &Path,
    kind: TestKind,
    instance: &InstanceName,
    maxd: &Path,
) -> Result<(), String> {
    std::fs::create_dir_all(share)
        .map_err(|err| format!("couldn't make the VM's shared folder: {err}"))?;
    write_lease_file(share, kind, instance)?;
    let copy = share.join(MAXD_FILE);
    remove(&copy)?;
    copy_app(maxd, &copy, COPY_WAIT).await
}

fn write_lease_file(share: &Path, kind: TestKind, instance: &InstanceName) -> Result<(), String> {
    let file = LeaseFile {
        kind,
        instance: instance.to_string(),
    };
    let json = serde_json::to_vec(&file).map_err(|err| err.to_string())?;
    write_to_share(share, LEASE_FILE, &json)
        .map_err(|err| format!("couldn't write the VM's lease file: {err}"))
}

/// Writes the file `name` in `share`. The guest can write there too, so the
/// bytes go to a new file, which can't be a link, and a rename replaces
/// whatever has the name without following it.
fn write_to_share(share: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    let temp = share.join(format!("{name}.new"));
    remove(&temp)?;
    std::fs::File::create_new(&temp)
        .and_then(|mut file| file.write_all(bytes))
        .and_then(|()| std::fs::rename(&temp, share.join(name)))
        .map_err(|err| err.to_string())
}

/// Copies `source` into `share` as `name`. `cp` follows links and the guest
/// can plant one there once it runs, so the copy goes to `stage` and a rename
/// moves it in. The rename never follows a link.
async fn copy_to_share(
    source: &Path,
    stage: &Path,
    share: &Path,
    name: &str,
) -> Result<(), String> {
    let staged = stage.join(name);
    remove(&staged)?;
    copy_app(source, &staged, COPY_WAIT).await?;
    let copy = share.join(name);
    remove(&copy)?;
    std::fs::rename(&staged, &copy)
        .map_err(|err| format!("couldn't move {name} into the VM's shared folder: {err}"))
}

/// Moves the folder `from` in `stage` to the uploads in `share`, and says the
/// name it got there. The guest can change anything in `share`, so this goes
/// through open folders that can't be links, and takes the next of `numbers`
/// when the guest already holds the name.
fn place_upload(
    share: &Path,
    stage: &Path,
    from: &str,
    numbers: &AtomicU64,
) -> Result<String, String> {
    let failed = |err: std::io::Error| format!("couldn't put the upload in the VM's folder: {err}");
    let root = Dir::open(share).map_err(failed)?;
    root.make_dir(UPLOADS, 0o755).map_err(failed)?;
    let uploads = root.dir(&[UPLOADS.to_owned()]).map_err(failed)?;
    let stage = Dir::open(stage).map_err(failed)?;
    let mut name = from.to_owned();
    for _ in 0..UPLOAD_TRIES {
        match stage.rename_into(from, &uploads, &name) {
            Ok(()) => return Ok(name),
            Err(err)
                if matches!(
                    err.kind(),
                    ErrorKind::NotADirectory
                        | ErrorKind::DirectoryNotEmpty
                        | ErrorKind::AlreadyExists
                ) =>
            {
                name = numbers.fetch_add(1, Ordering::Relaxed).to_string();
            }
            Err(err) => return Err(failed(err)),
        }
    }
    Err("The VM's guest keeps taking the upload's name.".to_owned())
}

fn read_shape(bundle: &Path) -> Result<Shape, String> {
    std::fs::read(bundle.join(bundle::SHAPE))
        .map_err(|err| err.to_string())
        .and_then(|json| serde_json::from_slice(&json).map_err(|err| err.to_string()))
        .map_err(|err| format!("the VM's shape doesn't read: {err}"))
}

/// Deletes `path`, which `maxd` made, if it's there.
fn remove(path: &Path) -> Result<(), String> {
    let gone = match std::fs::symlink_metadata(path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => Err(err),
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
    };
    gone.map_err(|err| format!("couldn't delete {}: {err}", path.display()))
}

/// One step of the base build, kept in [`STEP_FILE`] as the next to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Step {
    /// Install macOS from the restore image.
    Install,
    /// Wait for the person's setup assistant and command.
    Setup,
    /// Copy in the Mac's Xcode.
    Xcode,
    /// Accept Xcode's license and run its first launch.
    FirstLaunch,
    /// Add the Mac's iOS runtime.
    Runtime,
    /// Install Node.
    Node,
    /// Make the Simulator and start agent-device on it.
    AgentDevice,
    /// Save the state as the base.
    Save,
}

#[derive(Serialize, Deserialize)]
struct StepFile {
    step: Step,
}

fn read_step(folder: &Path) -> Step {
    std::fs::read(folder.join(STEP_FILE))
        .ok()
        .and_then(|json| serde_json::from_slice::<StepFile>(&json).ok())
        .map_or(Step::Install, |file| file.step)
}

fn keep_step(folder: &Path, step: Step) -> Result<(), String> {
    let json = serde_json::to_vec(&StepFile { step }).map_err(|err| err.to_string())?;
    std::fs::write(folder.join(STEP_FILE), json)
        .map_err(|err| format!("couldn't keep the build's step: {err}"))
}

/// Makes the base, from the step the last build stopped at.
async fn build_base(shared: &Shared) -> Result<(), String> {
    let folder = &shared.folder;
    std::fs::create_dir_all(folder).map_err(|err| format!("couldn't make the VM folder: {err}"))?;
    let building = folder.join(BUILDING);
    let mut step = read_step(folder);
    if step == Step::Install {
        let restore = match (shared.find)().await? {
            Ready::MacVm {
                restore: Some(restore),
                ..
            } => restore,
            Ready::MacVm { restore: None, .. } => {
                return Err("The macOS restore image is gone.".into());
            }
            _ => return Err("Detection answered for another host.".into()),
        };
        remove(&building)?;
        shared.vms.install(&restore, &building).await?;
        step = Step::Setup;
        keep_step(folder, step)?;
    }

    let share = folder.join(BUILD_SHARE);
    let instance = instance("build");
    prepare_share(&share, TestKind::Macos, &instance, &shared.maxd).await?;
    write_to_share(&share, SETUP_FILE, setup_script().as_bytes())
        .map_err(|err| format!("couldn't write the setup script: {err}"))?;
    let setup = step == Step::Setup;
    let vm = shared
        .vms
        .start(Op::Start {
            bundle: building.clone(),
            share: Share {
                path: share.clone(),
                read_only: false,
            },
            socket: shared.socket(&instance)?,
            restore: false,
            window: setup,
            note: setup.then(|| format!("{SETUP_NOTE}\n{SETUP_COMMAND}")),
        })
        .await?;
    let workspace = workspace(&instance);
    let wait = if setup { SETUP_WAIT } else { GUEST_WAIT };
    let made = async {
        shared
            .links
            .up(workspace, &instance, wait)
            .await
            .map_err(|error| format!("The VM's guest didn't link: {error}"))?;
        let guest = Linked {
            links: shared.links.as_ref(),
            workspace,
        };
        while step < Step::Save {
            if step > Step::Setup {
                do_step(shared, &guest, &share, &instance, step).await?;
            }
            step = next(step);
            keep_step(folder, step)?;
        }
        vm.save().await
    }
    .await;
    shared.links.down(workspace);
    vm.stop().await;
    made?;
    std::fs::rename(&building, folder.join(BASE))
        .map_err(|err| format!("couldn't keep the base: {err}"))?;
    remove(&share)?;
    remove(&folder.join(BUILD_STAGE))?;
    remove(&folder.join(STEP_FILE))
}

fn next(step: Step) -> Step {
    match step {
        Step::Install => Step::Setup,
        Step::Setup => Step::Xcode,
        Step::Xcode => Step::FirstLaunch,
        Step::FirstLaunch => Step::Runtime,
        Step::Runtime => Step::Node,
        Step::Node => Step::AgentDevice,
        Step::AgentDevice | Step::Save => Step::Save,
    }
}

/// The build's lease, which its guest runs every step for.
fn build_lease() -> LeaseId {
    LeaseId::derive("mac-vm-build")
}

async fn do_step(
    shared: &Shared,
    guest: &Linked<'_>,
    share: &Path,
    instance: &InstanceName,
    step: Step,
) -> Result<(), String> {
    match step {
        Step::Xcode => {
            let xcode = shared.mac.xcode().await?;
            let stage = shared.folder.join(BUILD_STAGE);
            copy_to_share(&xcode, &stage, share, "Xcode.app").await?;
            let ran = in_guest(guest, step, XCODE).await;
            remove(&share.join("Xcode.app"))?;
            ran
        }
        Step::FirstLaunch => in_guest(guest, step, FIRST_LAUNCH).await,
        Step::Runtime => {
            let image = shared.mac.ios_runtime().await?;
            let stage = shared.folder.join(BUILD_STAGE);
            copy_to_share(&image, &stage, share, RUNTIME_FILE).await?;
            let ran = in_guest(guest, step, RUNTIME).await;
            remove(&share.join(RUNTIME_FILE))?;
            ran
        }
        Step::Node => in_guest(guest, step, NODE).await,
        Step::AgentDevice => {
            // The guest makes and boots the Simulator and starts
            // agent-device for its first screenshot, so the saved state
            // has them running.
            write_lease_file(share, TestKind::IosSimulator, instance)?;
            let shot = via_guest::screenshot(guest, build_lease()).await.map(drop);
            write_lease_file(share, TestKind::Macos, instance)?;
            shot.map_err(|error| format!("The VM's Simulator didn't start: {error}"))
        }
        Step::Install | Step::Setup | Step::Save => Ok(()),
    }
}

/// Runs `script` in the guest for `step`, and fails with its last line.
async fn in_guest(guest: &Linked<'_>, step: Step, script: &str) -> Result<(), String> {
    let command = ["/bin/sh".to_owned(), "-c".to_owned(), script.to_owned()];
    let ran = via_guest::run(guest, build_lease(), &command, STEP_WAIT).await?;
    if ran.exit_code == Some(0) {
        return Ok(());
    }
    let line = ran
        .stderr
        .lines()
        .chain(ran.stdout.lines())
        .rfind(|line| !line.trim().is_empty())
        .unwrap_or("");
    let why = if ran.timed_out { "timed out" } else { "failed" };
    Err(format!("The VM's {} step {why}: {line}", step_name(step)))
}

fn step_name(step: Step) -> &'static str {
    match step {
        Step::Install => "install",
        Step::Setup => "setup",
        Step::Xcode => "Xcode",
        Step::FirstLaunch => "Xcode first launch",
        Step::Runtime => "iOS runtime",
        Step::Node => "Node",
        Step::AgentDevice => "agent-device",
        Step::Save => "save",
    }
}

/// The iOS runtime's copy in the build's shared folder.
const RUNTIME_FILE: &str = "ios-runtime.dmg";

/// Moves the Xcode in the shared folder into `/Applications`. Xcode can't
/// run from the shared folder, so it comes in whole.
const XCODE: &str = r#"set -e
share="/Volumes/My Shared Files"
sudo rm -rf /Applications/Xcode.app
/usr/bin/tar -cf - -C "$share" Xcode.app | sudo /usr/bin/tar -xpf - -C /Applications
sudo xcode-select -s /Applications/Xcode.app/Contents/Developer"#;

/// Accepts Xcode's license and installs its first-launch packages.
const FIRST_LAUNCH: &str = r#"set -e
sudo xcodebuild -license accept
sudo xcodebuild -runFirstLaunch"#;

/// Adds the iOS runtime from the shared folder.
const RUNTIME: &str = r#"set -e
xcrun simctl runtime add "/Volumes/My Shared Files/ios-runtime.dmg""#;

/// Installs the newest Node 22 from nodejs.org into `~/.maxhq/node`, once
/// its checksum matches.
const NODE: &str = r#"set -e
base=https://nodejs.org/dist/latest-v22.x
dir="$(mktemp -d)"
cd "$dir"
curl -fsSLo SHASUMS256.txt "$base/SHASUMS256.txt"
name="$(awk '/ node-v22\.[0-9.]*-darwin-arm64\.tar\.gz$/ {print $2}' SHASUMS256.txt)"
test -n "$name"
curl -fsSLo "$name" "$base/$name"
grep " $name\$" SHASUMS256.txt | shasum -a 256 -c -
rm -rf "$HOME/.maxhq/node"
mkdir -p "$HOME/.maxhq/node"
tar -xzf "$name" -C "$HOME/.maxhq/node" --strip-components 1
cd /
rm -rf "$dir"
"$HOME/.maxhq/node/bin/node" --version"#;

/// The LaunchAgent's label.
pub(crate) const AGENT_LABEL: &str = "com.maxhq.guest";

/// The script the person runs once in the guest's Terminal, from the
/// build's shared folder. It reads their password once, then gives the
/// user passwordless sudo, logs them in at boot, keeps the VM awake and
/// starts `maxd guest --vm` as a LaunchAgent, since the Simulator needs a
/// logged-in session. The VM is the boundary, as in Tart's images.
fn setup_script() -> String {
    format!(
        r#"#!/bin/sh
# MaxHQ's test VM setup, run once from the shared folder.
set -eu
share="{GUEST_SHARE}"
user="$(id -un)"
printf 'Your password in this VM: '
stty -echo
read -r password
stty echo
echo
printf '%s\n' "$password" | sudo -S -v
printf '%s ALL=(ALL) NOPASSWD: ALL\n' "$user" | sudo tee /etc/sudoers.d/maxhq >/dev/null
sudo chmod 440 /etc/sudoers.d/maxhq
sudo visudo -cf /etc/sudoers.d/maxhq >/dev/null
sudo sysadminctl -autologin set -userName "$user" -password "$password"
unset password
sudo pmset -a sleep 0 displaysleep 0 disksleep 0
defaults -currentHost write com.apple.screensaver idleTime -int 0
mkdir -p "$HOME/.maxhq/bin" "$HOME/Library/LaunchAgents"
cp "$share/{MAXD_FILE}" "$HOME/.maxhq/bin/maxd"
plist="$HOME/Library/LaunchAgents/{AGENT_LABEL}.plist"
cat > "$plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{AGENT_LABEL}</string>
  <key>ProgramArguments</key>
  <array><string>$HOME/.maxhq/bin/maxd</string><string>guest</string><string>--vm</string></array>
  <key>EnvironmentVariables</key>
  <dict><key>PATH</key><string>$HOME/.maxhq/node/bin:/usr/bin:/bin:/usr/sbin:/sbin</string></dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardErrorPath</key><string>$HOME/.maxhq/guest.log</string>
</dict>
</plist>
PLIST
launchctl bootout "gui/$(id -u)/{AGENT_LABEL}" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$plist"
echo "Done. MaxHQ takes it from here, so you can close this window."
"#
    )
}

/// `maxd-vm`, one process for each VM.
struct Helper {
    binary: PathBuf,
}

impl Vms for Helper {
    fn install<'a>(
        &'a self,
        restore_image: &'a Path,
        bundle: &'a Path,
    ) -> BoxFuture<'a, Result<(), String>> {
        async move {
            let process = Process::spawn(&self.binary).await?;
            let op = Op::Install {
                restore_image: restore_image.to_owned(),
                bundle: bundle.to_owned(),
                shape: SHAPE,
            };
            let installed = process.ask(op, INSTALL_WAIT).await;
            process.exited().await;
            installed.map(drop)
        }
        .boxed()
    }

    fn start(&self, start: Op) -> BoxFuture<'_, Result<Box<dyn Vm>, String>> {
        async move {
            let process = Process::spawn(&self.binary).await?;
            if let Err(error) = process.ask(start, START_WAIT).await {
                process.stop().await;
                return Err(error);
            }
            Ok(Box::new(process) as Box<dyn Vm>)
        }
        .boxed()
    }
}

type Answers = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Option<String>, String>>>>>;

/// One `maxd-vm` process and the answers it owes.
struct Process {
    stdin: tokio::sync::Mutex<Option<tokio::process::ChildStdin>>,
    child: tokio::sync::Mutex<tokio::process::Child>,
    next: AtomicU64,
    answers: Answers,
}

impl Process {
    async fn spawn(binary: &Path) -> Result<Self, String> {
        let mut command = tokio::process::Command::new(binary);
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = crate::tools::retry_busy(|| std::future::ready(command.spawn()))
            .await
            .map_err(|err| format!("couldn't start {}: {err}", binary.display()))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().ok_or("maxd-vm has no stdout")?;
        let answers = Answers::default();
        tokio::spawn(read_events(stdout, answers.clone()));
        Ok(Self {
            stdin: tokio::sync::Mutex::new(stdin),
            child: tokio::sync::Mutex::new(child),
            next: AtomicU64::new(1),
            answers,
        })
    }

    /// Sends `op` and waits up to `wait` for its answer.
    async fn ask(&self, op: Op, wait: Duration) -> Result<Option<String>, String> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        lock(&self.answers).insert(id, tx);
        let line = Request { id, op }.line();
        let sent = match self.stdin.lock().await.as_mut() {
            Some(stdin) => stdin.write_all(line.as_bytes()).await,
            None => Err(std::io::ErrorKind::BrokenPipe.into()),
        };
        if let Err(err) = sent {
            lock(&self.answers).remove(&id);
            return Err(format!("maxd-vm didn't take the request: {err}"));
        }
        match tokio::time::timeout(wait, rx).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(_)) => Err("maxd-vm exited".into()),
            Err(_) => {
                lock(&self.answers).remove(&id);
                Err(format!("maxd-vm didn't answer within {} s", wait.as_secs()))
            }
        }
    }

    /// Closes stdin and waits for the process to exit, killing it after
    /// [`STOP_WAIT`].
    async fn exited(&self) {
        self.stdin.lock().await.take();
        let mut child = self.child.lock().await;
        if tokio::time::timeout(STOP_WAIT, child.wait()).await.is_err() {
            let _ = child.kill().await;
        }
    }
}

impl Vm for Process {
    fn save(&self) -> BoxFuture<'_, Result<(), String>> {
        async move { self.ask(Op::Save, SAVE_WAIT).await.map(drop) }.boxed()
    }

    fn stop(&self) -> BoxFuture<'_, ()> {
        async move {
            if let Err(error) = self.ask(Op::Stop, STOP_WAIT).await {
                tracing::warn!(%error, "maxd-vm didn't stop its VM");
            }
            self.exited().await;
        }
        .boxed()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Hands each answer on `stdout` to its asker, until the helper exits.
async fn read_events(stdout: tokio::process::ChildStdout, answers: Answers) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let answered = match serde_json::from_str::<Event>(&line) {
            Ok(Event::Done { id, version }) => Some((id, Ok(version))),
            Ok(Event::Failed { id, error }) => Some((id, Err(error))),
            Ok(Event::Progress { fraction }) => {
                tracing::info!("the macOS install is {:.0}% done", fraction * 100.0);
                None
            }
            Ok(Event::Stopped { error }) => {
                tracing::warn!(?error, "a test VM stopped by itself");
                None
            }
            Err(err) => {
                tracing::warn!(%err, "maxd-vm said something that didn't parse");
                None
            }
        };
        if let Some((id, answer)) = answered
            && let Some(tx) = lock(&answers).remove(&id)
        {
            let _ = tx.send(answer);
        }
    }
    // Dropping the senders fails every asker still waiting.
    lock(&answers).clear();
}

/// The guest links `maxd` runs.
struct Wired {
    links: Arc<dyn GuestLinks>,
    guests: Arc<dyn transfer::Guests>,
}

impl Links for Wired {
    fn up<'a>(
        &'a self,
        workspace: WorkspaceId,
        instance: &'a InstanceName,
        wait: Duration,
    ) -> BoxFuture<'a, Result<(), String>> {
        async move {
            self.links.open(workspace, instance, None);
            match self.links.wait_up(workspace, wait).await? {
                Some(_) => Ok(()),
                None => Err("This maxd doesn't link to guests.".into()),
            }
        }
        .boxed()
    }

    fn down(&self, workspace: WorkspaceId) {
        self.links.close(workspace);
    }

    fn ask(
        &self,
        workspace: WorkspaceId,
        wait: Duration,
        make: MakeRequest,
    ) -> BoxFuture<'_, Result<GuestToHost, String>> {
        async move {
            let out = self
                .guests
                .link(workspace)
                .ok_or_else(|| "the VM's guest isn't linked".to_owned())?;
            out.request(wait, make).await
        }
        .boxed()
    }
}

/// This Mac's Xcode and iOS runtime.
struct ThisMac;

impl Mac for ThisMac {
    fn xcode(&self) -> BoxFuture<'_, Result<PathBuf, String>> {
        async move {
            let developer = output("/usr/bin/xcode-select", &["-p"]).await?;
            xcode_app(Path::new(developer.trim()))
                .ok_or_else(|| format!("{} isn't inside an Xcode app.", developer.trim()))
        }
        .boxed()
    }

    fn ios_runtime(&self) -> BoxFuture<'_, Result<PathBuf, String>> {
        async move {
            let listed = output("/usr/bin/xcrun", &["simctl", "runtime", "list", "-j"]).await?;
            newest_ios_image(&listed)
                .ok_or_else(|| "Xcode has no iOS Simulator runtime image to copy.".to_owned())
        }
        .boxed()
    }
}

async fn output(program: &str, args: &[&str]) -> Result<String, String> {
    let mut command = std::process::Command::new(program);
    command.args(args);
    let ran = agent_device::process::run(command, Duration::from_secs(60), 4 << 20)
        .await
        .map_err(|err| format!("couldn't run {program}: {err}"))?;
    match ran.exit_code {
        Some(0) => Ok(ran.stdout),
        _ => Err(format!("{program} failed: {}", ran.stderr.trim())),
    }
}

/// The `.app` folder `developer`, Xcode's developer folder, is in.
fn xcode_app(developer: &Path) -> Option<PathBuf> {
    developer
        .ancestors()
        .find(|path| path.extension().is_some_and(|ext| ext == "app"))
        .map(Path::to_owned)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeImage {
    path: Option<PathBuf>,
    platform_identifier: Option<String>,
    version: Option<String>,
    state: Option<String>,
}

/// The newest ready iOS runtime's disk image in `simctl runtime list -j`.
fn newest_ios_image(listed: &str) -> Option<PathBuf> {
    let images: HashMap<String, RuntimeImage> = serde_json::from_str(listed).ok()?;
    images
        .into_values()
        .filter(|image| {
            image.platform_identifier.as_deref() == Some("com.apple.platform.iphonesimulator")
                && image.state.as_deref().is_none_or(|state| state == "Ready")
        })
        .filter_map(|image| {
            let version: Vec<u32> = image
                .version?
                .split('.')
                .map(|part| part.parse().unwrap_or(0))
                .collect();
            Some((version, image.path?))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, path)| path)
}

#[cfg(test)]
#[path = "mac_vm/tests.rs"]
mod tests;
