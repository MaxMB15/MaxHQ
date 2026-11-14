// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The agent CLIs on the device and in its container workspaces: where
//! they are, what version they run, what the latest is, and updating them.
//! [`Tools`] publishes all of it as one [`DeviceTools`] entity.
//!
//! On the device the CLIs are yours, so MaxHQ updates one only when you've
//! ticked it in the device's settings, and only when it can without root.
//! In the workspaces MaxHQ made, it installed them, so Claude updates
//! itself there as the agent user. Codex there follows the recipe's pinned
//! release, which moves with maxd updates.
//!
//! Tools you ask MaxHQ to install go through [`install`]'s installer, in
//! its own folders, so they never touch yours.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use futures::FutureExt;
use maxhq_protocol::message::{ActionOutcome, ToolTarget};
use maxhq_protocol::model::{Tool, WorkspaceLocation};
use maxhq_protocol::state::{
    Change, DeviceTools, Entity, EntityKey, ToolInfo, ToolKind, ToolOwner, ToolPlace, ToolState,
    ToolUpdate,
};
use tokio::sync::broadcast::error::RecvError;
use tracing::{info, warn};

use crate::hub::Hub;
use crate::incus::{Incus, InstanceName};
use crate::recipe::{Exec, ExecError, ExecOutput, ExecUser};

pub mod android;
pub mod defs;
pub mod install;
pub mod resolve;
pub mod run;

use defs::Def;
use resolve::{Action, Command, Probe};
use run::Locks;

/// How often the latest versions are checked.
const LATEST_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// How long a registry lookup may take.
const LATEST_WAIT: Duration = Duration::from_secs(4);
/// The most of a file read to spot a mise wrapper.
const WRAPPER_MAX_BYTES: u64 = 16 * 1024;

/// Where the latest release of a package comes from. Tests use a fake.
pub trait Registry: Send + Sync {
    /// The latest version of `package`, if it could be read.
    fn latest<'a>(&'a self, package: &'a str) -> BoxFuture<'a, Option<String>>;
}

/// The npm registry.
pub struct Npm(reqwest::Client);

impl Npm {
    /// A registry client.
    pub fn new() -> Self {
        // reqwest is built without a TLS crypto provider of its own.
        let _ = rustls::crypto::ring::default_provider().install_default();
        Self(
            reqwest::Client::builder()
                .timeout(LATEST_WAIT)
                .build()
                .unwrap_or_default(),
        )
    }
}

impl Registry for Npm {
    fn latest<'a>(&'a self, package: &'a str) -> BoxFuture<'a, Option<String>> {
        async move {
            #[derive(serde::Deserialize)]
            struct Latest {
                version: String,
            }
            let url = format!("https://registry.npmjs.org/{package}/latest");
            let response = self.0.get(url).send().await.ok()?.error_for_status().ok()?;
            let body = response.text().await.ok()?;
            Some(serde_json::from_str::<Latest>(&body).ok()?.version)
        }
        .boxed()
    }
}

/// Runs commands on the device as the user maxd runs as. It refuses
/// [`ExecUser::Root`] and any program that switches user, so nothing it
/// runs uses sudo.
pub struct HostExec;

/// Programs that run something as another user.
const SWITCHES_USER: &[&str] = &["sudo", "doas", "su", "pkexec", "run0"];

impl HostExec {
    /// A command for `program`, or a refusal when it would switch user.
    pub fn command(&self, program: &str) -> Result<tokio::process::Command, ExecError> {
        let name = Path::new(program)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if SWITCHES_USER.contains(&name.as_str()) {
            return Err(ExecError::Conflict(format!(
                "MaxHQ doesn't run {name} on the device"
            )));
        }
        Ok(tokio::process::Command::new(program))
    }
}

/// Starts a process with `start`, trying again for a moment while Linux
/// says its program is busy. A child that another thread forks while the
/// program file is still open for writing holds it open until that child
/// execs, and Linux won't run the file until then.
pub async fn retry_busy<T, Fut>(mut start: impl FnMut() -> Fut) -> std::io::Result<T>
where
    Fut: Future<Output = std::io::Result<T>>,
{
    let mut busy = 0;
    loop {
        match start().await {
            Err(err) if err.kind() == std::io::ErrorKind::ExecutableFileBusy && busy < 10 => {
                busy += 1;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            started => return started,
        }
    }
}

impl Exec for HostExec {
    fn run<'a>(
        &'a self,
        cmd: &'a [&'a str],
        user: ExecUser,
        stdin: Option<&'a [u8]>,
    ) -> BoxFuture<'a, Result<ExecOutput, ExecError>> {
        async move {
            if user == ExecUser::Root {
                return Err(ExecError::Conflict(
                    "MaxHQ doesn't run commands as root on the device".to_owned(),
                ));
            }
            let (program, args) = cmd
                .split_first()
                .ok_or_else(|| ExecError::Transport("no command".to_owned()))?;
            let mut command = self.command(program)?;
            command
                .args(args)
                .stdin(if stdin.is_some() {
                    std::process::Stdio::piped()
                } else {
                    std::process::Stdio::null()
                })
                .kill_on_drop(true);
            let mut child = command
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|err| ExecError::Transport(err.to_string()))?;
            if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
                use tokio::io::AsyncWriteExt;
                pipe.write_all(bytes)
                    .await
                    .map_err(|err| ExecError::Transport(err.to_string()))?;
            }
            let output = child
                .wait_with_output()
                .await
                .map_err(|err| ExecError::Transport(err.to_string()))?;
            Ok(ExecOutput {
                status: output.status.code().unwrap_or(-1),
                stdout: output.stdout,
                stderr: output.stderr,
            })
        }
        .boxed()
    }

    fn read<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<Option<Vec<u8>>, ExecError>> {
        async move {
            match tokio::fs::read(path).await {
                Ok(bytes) => Ok(Some(bytes)),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(err) => Err(ExecError::Transport(err.to_string())),
            }
        }
        .boxed()
    }

    fn write<'a>(
        &'a self,
        path: &'a str,
        bytes: &'a [u8],
        mode: u32,
        owner: ExecUser,
    ) -> BoxFuture<'a, Result<(), ExecError>> {
        async move {
            if owner == ExecUser::Root {
                return Err(ExecError::Conflict(
                    "MaxHQ doesn't write files as root on the device".to_owned(),
                ));
            }
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::write(path, bytes)
                .await
                .and_then(|()| {
                    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
                })
                .map_err(|err| ExecError::Transport(err.to_string()))
        }
        .boxed()
    }
}

/// The real file system.
struct SystemProbe;

impl Probe for SystemProbe {
    fn exists(&self, path: &str) -> bool {
        Path::new(path).exists()
    }

    fn writable(&self, path: &str) -> bool {
        let Ok(c) = std::ffi::CString::new(path) else {
            return false;
        };
        #[allow(unsafe_code)]
        // SAFETY: `c` is a valid C string that outlives the call.
        let writable = unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0;
        // A folder root owns isn't one to update in, even for root.
        writable && std::fs::metadata(path).is_ok_and(|meta| meta.uid() != 0 || current_uid() == 0)
    }

    fn small_file(&self, path: &str) -> Option<String> {
        let file = std::fs::File::open(path).ok()?;
        if file.metadata().ok()?.len() > WRAPPER_MAX_BYTES {
            return None;
        }
        let mut text = String::new();
        file.take(WRAPPER_MAX_BYTES)
            .read_to_string(&mut text)
            .ok()?;
        Some(text)
    }

    fn output(&self, program: &str, args: &[&str]) -> Option<String> {
        let output = std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }
}

#[allow(unsafe_code)]
fn current_uid() -> u32 {
    // SAFETY: getuid can't fail.
    unsafe { libc::getuid() }
}

/// One CLI on the device, as last found.
#[derive(Clone)]
struct HostTool {
    path: PathBuf,
    action: Action,
}

#[derive(Default)]
struct Known {
    /// The latest version of each package.
    latest: HashMap<Tool, String>,
    /// The device's CLIs.
    host: HashMap<Tool, HostTool>,
    /// What each CLI in each place is doing.
    states: HashMap<ToolTarget, ToolState>,
    /// Every entry, as last published.
    tools: Vec<ToolInfo>,
    /// The CLIs you'd ticked when they were last published.
    ticked: Vec<Tool>,
    /// What each install or removal you asked for is doing.
    installs: HashMap<(ToolKind, ToolPlace), ToolState>,
}

/// The device's agent CLIs.
pub struct Tools {
    hub: Hub,
    incus: Option<Incus>,
    registry: Arc<dyn Registry>,
    /// Where to look for CLIs, or PATH and the usual folders when none.
    search: Option<OsString>,
    locks: Locks,
    known: Mutex<Known>,
    /// Told when this device's Codex is updated, so its app-servers move
    /// onto the new binary.
    codex_updated: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>>,
    /// Installs and removes tools, once [`Tools::start`] set it.
    installer: std::sync::OnceLock<Arc<dyn install::Installer>>,
    /// The home folder tools install into on this device, if it has one.
    home: Option<String>,
    /// How the Android SDK installs here.
    android: android::Sdk,
}

impl Tools {
    /// Calls `then` after each update of this device's own Codex.
    pub fn on_codex_update(&self, then: impl Fn() + Send + Sync + 'static) {
        let _ = self.codex_updated.set(Box::new(then));
    }

    /// Finds the CLIs now and keeps the latest versions fresh.
    pub fn start(hub: Hub, incus: Option<Incus>, registry: Arc<dyn Registry>) -> Arc<Self> {
        let tools = Arc::new(Self::new(hub, incus, registry, None));
        tools.set_installer(Arc::new(install::Setup::new(
            Arc::new(DevicePlaces(Arc::downgrade(&tools))),
            Arc::new(install::Publishers::default()),
            tools.listener(),
        )));
        tools.follow_ticks();
        let task = Arc::clone(&tools);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(LATEST_EVERY);
            loop {
                tick.tick().await;
                task.check_latest().await;
                task.scan().await;
            }
        });
        tools
    }

    fn new(
        hub: Hub,
        incus: Option<Incus>,
        registry: Arc<dyn Registry>,
        search: Option<OsString>,
    ) -> Self {
        let home = std::env::var("HOME").ok().filter(|home| !home.is_empty());
        Self {
            hub,
            incus,
            registry,
            search,
            locks: Locks::default(),
            known: Mutex::new(Known::default()),
            codex_updated: std::sync::OnceLock::new(),
            installer: std::sync::OnceLock::new(),
            android: android::Sdk::for_host(home.as_deref()),
            home,
        }
    }

    /// This device's install layout, for the user maxd runs as.
    fn host_layout(&self) -> Option<install::Layout> {
        self.home.as_deref().map(install::Layout::host)
    }

    /// Makes `installer` the one installs go through.
    fn set_installer(&self, installer: Arc<dyn install::Installer>) {
        let _ = self.installer.set(installer);
    }

    /// Hears each install's progress and publishes it.
    fn listener(self: &Arc<Self>) -> install::Listener {
        let tools = Arc::downgrade(self);
        Arc::new(
            move |kind: &ToolKind, place: &ToolPlace, state: ToolState| {
                if let Some(tools) = tools.upgrade() {
                    lock(&tools.known)
                        .installs
                        .insert((kind.clone(), *place), state);
                    tools.publish();
                }
            },
        )
    }

    /// Reads the latest version of each CLI.
    async fn check_latest(&self) {
        for def in defs::ALL {
            let Some(package) = def.package else {
                continue;
            };
            if let Some(version) = self.registry.latest(package).await {
                lock(&self.known).latest.insert(def.tool, version);
            }
        }
    }

    /// Finds every CLI and publishes what it found.
    pub async fn scan(&self) {
        let mut tools = Vec::new();
        for def in defs::ALL {
            if let Some(info) = self.scan_host(def).await {
                tools.push(info);
            }
        }
        if let Some(layout) = self.host_layout() {
            tools.extend(host_helpers(&layout));
        }
        if let Some(incus) = &self.incus {
            for (workspace, instance) in self.container_workspaces() {
                for def in defs::ALL {
                    let place = ToolPlace::Workspace { workspace };
                    let version = container_version(incus, &instance, def).await;
                    tools.push(ToolInfo {
                        kind: ToolKind::Agent { tool: def.tool },
                        place,
                        path: Some(def.container_path.to_owned()),
                        version,
                        latest: None,
                        owner: ToolOwner::MaxHq,
                        method: def.container_method.to_owned(),
                        update: if def.updates_in_container() {
                            ToolUpdate::Auto
                        } else {
                            ToolUpdate::Never
                        },
                        state: ToolState::Idle,
                    });
                }
            }
        }
        lock(&self.known).tools = tools;
        self.publish();
    }

    async fn scan_host(&self, def: &'static Def) -> Option<ToolInfo> {
        let path = self.find(def.binary)?;
        let real = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        let resolved = {
            let path = path.to_string_lossy().into_owned();
            let real = real.to_string_lossy().into_owned();
            tokio::task::spawn_blocking(move || resolve::resolve(def, &path, &real, &SystemProbe))
                .await
                .ok()?
        };
        let version = run::read_version(def, &path).await;
        let update = match &resolved.action {
            // Publishing makes it allowed once you've ticked it.
            Action::Run(_) => ToolUpdate::NeedsTick,
            Action::Manual { command } => ToolUpdate::Manual {
                command: command.clone(),
            },
            Action::Never => ToolUpdate::Never,
        };
        lock(&self.known).host.insert(
            def.tool,
            HostTool {
                path: path.clone(),
                action: resolved.action,
            },
        );
        let kind = ToolKind::Agent { tool: def.tool };
        let owner = if self
            .host_layout()
            .is_some_and(|layout| ours(&layout, &kind, &real))
        {
            ToolOwner::MaxHq
        } else {
            ToolOwner::User
        };
        Some(ToolInfo {
            kind,
            place: ToolPlace::Host,
            path: Some(path.to_string_lossy().into_owned()),
            version,
            latest: None,
            owner,
            method: resolved.method.to_owned(),
            update,
            state: ToolState::Idle,
        })
    }

    fn find(&self, name: &str) -> Option<PathBuf> {
        match &self.search {
            Some(search) => std::env::split_paths(search)
                .map(|dir| dir.join(name))
                .find(|file| file.is_file()),
            None => crate::service::find_program(name),
        }
    }

    /// Whether you've ticked `tool` for MaxHQ to update on this device.
    fn ticked(&self, tool: Tool) -> bool {
        self.all_ticked().contains(&tool)
    }

    /// The CLIs you've ticked for MaxHQ to update on this device.
    fn all_ticked(&self) -> Vec<Tool> {
        match self.hub.get(&self.settings_key()) {
            Some(Entity::DeviceSettings(settings)) => settings.tool_updates,
            _ => Vec::new(),
        }
    }

    fn settings_key(&self) -> EntityKey {
        EntityKey::DeviceSettings(self.hub.device().id)
    }

    /// Publishes again whenever you tick or untick a CLI in this device's
    /// settings, so what may update follows without a scan.
    fn follow_ticks(self: &Arc<Self>) {
        let mut deltas = self.hub.subscribe(None).deltas;
        let tools = Arc::clone(self);
        tokio::spawn(async move {
            let key = tools.settings_key();
            loop {
                let settings = match deltas.recv().await {
                    Ok(delta) => delta.changes.iter().any(|change| match change {
                        Change::Upsert { value } => value.key() == key,
                        _ => false,
                    }),
                    Err(RecvError::Lagged(_)) => true,
                    Err(RecvError::Closed) => return,
                };
                if settings && tools.all_ticked() != lock(&tools.known).ticked {
                    tools.publish();
                }
            }
        });
    }

    /// Each container workspace MaxHQ made on this device, with its instance.
    fn container_workspaces(&self) -> Vec<(maxhq_protocol::ids::WorkspaceId, String)> {
        self.hub
            .snapshot()
            .entities
            .into_iter()
            .filter_map(|entity| match entity {
                Entity::Workspace(workspace) if workspace.managed.is_some() => {
                    match workspace.location {
                        WorkspaceLocation::Container { instance, .. } => {
                            Some((workspace.id, instance))
                        }
                        WorkspaceLocation::Folder { .. } => None,
                    }
                }
                _ => None,
            })
            .collect()
    }

    /// Publishes every entry with its latest version and state.
    fn publish(&self) {
        let ticked = self.all_ticked();
        let entity = {
            let mut known = lock(&self.known);
            let mut tools: Vec<ToolInfo> = known
                .tools
                .iter()
                .cloned()
                .map(|mut info| {
                    if let Some(state) = known.installs.get(&(info.kind.clone(), info.place)) {
                        info.state = state.clone();
                    }
                    let Some(tool) = info.tool() else {
                        return info;
                    };
                    info.latest = known.latest.get(&tool).cloned();
                    let target = ToolTarget {
                        tool,
                        place: info.place,
                    };
                    if let Some(state) = known.states.get(&target) {
                        info.state = state.clone();
                    }
                    if info.place == ToolPlace::Host
                        && matches!(info.update, ToolUpdate::Allowed | ToolUpdate::NeedsTick)
                    {
                        info.update = if ticked.contains(&tool) {
                            ToolUpdate::Allowed
                        } else {
                            ToolUpdate::NeedsTick
                        };
                    }
                    info
                })
                .collect();
            // A tool being installed shows before a scan finds it.
            for ((kind, place), state) in &known.installs {
                if !tools
                    .iter()
                    .any(|info| info.kind == *kind && info.place == *place)
                {
                    tools.push(ToolInfo {
                        kind: kind.clone(),
                        place: *place,
                        path: None,
                        version: None,
                        latest: None,
                        owner: ToolOwner::MaxHq,
                        method: MAXHQ_METHOD.to_owned(),
                        update: ToolUpdate::Never,
                        state: state.clone(),
                    });
                }
            }
            known.ticked = ticked;
            Entity::DeviceTools(DeviceTools {
                device: self.hub.device().id,
                tools,
            })
        };
        self.hub.apply([Change::Upsert { value: entity }]);
    }

    fn set_state(&self, target: ToolTarget, state: ToolState) {
        lock(&self.known).states.insert(target, state);
        self.publish();
    }

    /// Starts updating `targets`. Refuses them all when one can't be.
    pub async fn update(self: &Arc<Self>, targets: Vec<ToolTarget>) -> ActionOutcome {
        if targets.is_empty() {
            return ActionOutcome::Done;
        }
        let mut jobs = Vec::new();
        for target in targets {
            let def = defs::def(target.tool);
            match target.place {
                ToolPlace::Host => {
                    let host = lock(&self.known).host.get(&target.tool).cloned();
                    let Some(host) = host else {
                        return refused(format!("{} isn't installed on this device.", def.binary));
                    };
                    let command = match host.action {
                        Action::Run(command) => command,
                        Action::Manual { command } => {
                            return refused(format!(
                                "MaxHQ can't update {} here. Run `{command}` yourself.",
                                def.binary
                            ));
                        }
                        Action::Never => {
                            return refused(format!("MaxHQ doesn't update {}.", def.binary));
                        }
                    };
                    if !self.ticked(target.tool) {
                        return refused(format!(
                            "Tick {} in this device's settings first.",
                            def.binary
                        ));
                    }
                    jobs.push(Job::Host {
                        target,
                        command,
                        path: host.path,
                    });
                }
                ToolPlace::Workspace { workspace } => {
                    if !def.updates_in_container() {
                        return refused(format!(
                            "{} in a workspace moves with maxd, not on its own.",
                            def.binary
                        ));
                    }
                    let Some((_, instance)) = self
                        .container_workspaces()
                        .into_iter()
                        .find(|(id, _)| *id == workspace)
                    else {
                        return refused("That workspace isn't one MaxHQ made here.".to_owned());
                    };
                    if self.incus.is_none() {
                        return refused("This device runs no container workspaces.".to_owned());
                    }
                    jobs.push(Job::Workspace { target, instance });
                }
            }
        }
        for job in jobs {
            let tools = Arc::clone(self);
            tokio::spawn(async move { tools.run_job(job).await });
        }
        ActionOutcome::Done
    }

    /// Starts installing `kind` in `place`, at `version` or the latest. A
    /// workspace's CLIs follow the recipe's pins unless you name one.
    pub fn install(
        self: &Arc<Self>,
        kind: ToolKind,
        place: ToolPlace,
        version: Option<String>,
    ) -> ActionOutcome {
        // A version becomes a folder name, a package spec and wrapper text.
        if version
            .as_deref()
            .is_some_and(|named| !crate::acp_registry::bounded_version(named))
        {
            return refused("That isn't a version MaxHQ can install.".to_owned());
        }
        let workspace = matches!(place, ToolPlace::Workspace { .. });
        if let Some(reason) = android::refusal(&kind, workspace, self.android.on_mac) {
            return refused(reason);
        }
        if kind == android::SDK {
            return self.install_android();
        }
        let Some(def) = defs::plan(&kind, workspace) else {
            return refused("MaxHQ can't install that yet.".to_owned());
        };
        let version = version.or_else(|| defs::pin(&kind, workspace).map(str::to_owned));
        let name = install::name(&kind);
        self.start_install(kind, place, format!("Getting ready to install {name}"), {
            move |installer: Arc<dyn install::Installer>, place: ToolPlace| {
                Box::pin(async move {
                    installer
                        .ensure(&def, version.as_deref(), &place)
                        .await
                        .map(|installed| {
                            info!(tool = %name, version = %installed.version, "installed");
                            None
                        })
                })
            }
        })
    }

    /// Starts installing the Android SDK on this device, which waits when
    /// it meets a licence you haven't accepted.
    fn install_android(self: &Arc<Self>) -> ActionOutcome {
        let Some(home) = self.home.clone() else {
            return refused("maxd has no home folder to install into.".to_owned());
        };
        let tools = Arc::downgrade(self);
        let sdk = self.android.clone();
        let message = "Getting ready to install the Android SDK".to_owned();
        self.start_install(android::SDK, ToolPlace::Host, message, {
            move |installer: Arc<dyn install::Installer>, _place: ToolPlace| {
                Box::pin(async move {
                    let layout = install::Layout::host(&home);
                    let say = |message: String| {
                        if let Some(tools) = tools.upgrade() {
                            tools.show(android::SDK, ToolState::running(message));
                        }
                    };
                    let result = android::install(&sdk, &*installer, &layout, &say).await;
                    // The JDK's own progress ends with the SDK's.
                    if let Some(tools) = tools.upgrade() {
                        let mut known = lock(&tools.known);
                        let key = (android::JDK, ToolPlace::Host);
                        if matches!(known.installs.get(&key), Some(ToolState::Running { .. })) {
                            known.installs.remove(&key);
                        }
                    }
                    let pending = result?;
                    if pending.is_none() {
                        info!("installed the Android SDK");
                    }
                    Ok(pending.map(|pending| ToolState::Licences { pending }))
                })
            }
        })
    }

    /// Shows `state` for the host's `kind` while it installs.
    fn show(&self, kind: ToolKind, state: ToolState) {
        lock(&self.known)
            .installs
            .insert((kind, ToolPlace::Host), state);
        self.publish();
    }

    /// Accepts the licence `licence` with the text whose SHA-256 is
    /// `text_sha256`, which an install here waits on. The install goes on
    /// once you've accepted every licence it waits on.
    pub fn accept_licence(self: &Arc<Self>, licence: &str, text_sha256: &str) -> ActionOutcome {
        let record = match self.android.record() {
            Ok(record) => record,
            Err(reason) => return refused(reason),
        };
        let resume = {
            let mut known = lock(&self.known);
            let Some((key, pending)) =
                known
                    .installs
                    .iter_mut()
                    .find_map(|(key, state)| match state {
                        ToolState::Licences { pending }
                            if pending.iter().any(|shown| shown.id == licence) =>
                        {
                            Some((key.clone(), pending))
                        }
                        _ => None,
                    })
            else {
                return refused("No install is waiting on that licence.".to_owned());
            };
            if !pending
                .iter()
                .any(|shown| shown.id == licence && shown.text_sha256 == text_sha256)
            {
                return refused(
                    "That licence's text changed since you read it. Read it again.".to_owned(),
                );
            }
            if let Err(err) = record.accept(licence, text_sha256) {
                warn!(%err, "couldn't record an accepted licence");
                return refused("MaxHQ couldn't record that you accepted it.".to_owned());
            }
            info!(%licence, "a licence was accepted");
            pending.retain(|shown| shown.id != licence);
            pending.is_empty().then_some(key)
        };
        self.publish();
        match resume {
            Some((kind, place)) => self.install(kind, place, None),
            None => ActionOutcome::Done,
        }
    }

    /// Starts removing a tool MaxHQ installed in `place`.
    pub fn remove(self: &Arc<Self>, kind: ToolKind, place: ToolPlace) -> ActionOutcome {
        let name = install::name(&kind);
        let job_kind = kind.clone();
        // The SDK's packages go with its command-line tools.
        let sdk_root = (kind == android::SDK && place == ToolPlace::Host)
            .then(|| self.host_layout().map(|layout| android::sdk_root(&layout)))
            .flatten();
        self.start_install(kind, place, format!("Removing {name}"), {
            move |installer: Arc<dyn install::Installer>, place: ToolPlace| {
                Box::pin(async move {
                    if let Some(root) = sdk_root {
                        match tokio::fs::remove_dir_all(&root).await {
                            Ok(()) => {}
                            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                            Err(err) => {
                                return Err(install::InstallError::Failed(format!(
                                    "MaxHQ couldn't remove the Android SDK's folder: {err}"
                                )));
                            }
                        }
                    }
                    installer.remove(&job_kind, &place).await.map(|()| None)
                })
            }
        })
    }

    /// Checks the place, marks the tool running there and runs `job` on
    /// its own task, then scans again or keeps why it failed. A job that
    /// gives a state, such as licences to accept, leaves the tool in it.
    fn start_install(
        self: &Arc<Self>,
        kind: ToolKind,
        place: ToolPlace,
        message: String,
        job: impl FnOnce(
                Arc<dyn install::Installer>,
                ToolPlace,
            )
                -> BoxFuture<'static, Result<Option<ToolState>, install::InstallError>>
            + Send
            + 'static,
    ) -> ActionOutcome {
        let installer = match self.begin(&kind, place, message) {
            Ok(installer) => installer,
            Err(reason) => return refused(reason),
        };
        let tools = Arc::clone(self);
        tokio::spawn(async move {
            let result = job(installer, place).await;
            tools.finish(kind, place, result.as_ref().cloned()).await;
        });
        ActionOutcome::Done
    }

    /// An installer whose installs and removals show in this device's
    /// tools as they go, for the ones that start elsewhere.
    pub fn installer(self: &Arc<Self>) -> Arc<dyn install::Installer> {
        Arc::new(Tracked(Arc::downgrade(self)))
    }

    /// Checks the place and marks `kind` running there, with the installer
    /// to run it through, or says why it can't start.
    fn begin(
        self: &Arc<Self>,
        kind: &ToolKind,
        place: ToolPlace,
        message: String,
    ) -> Result<Arc<dyn install::Installer>, String> {
        let Some(installer) = self.installer.get().cloned() else {
            return Err("This maxd can't install tools.".to_owned());
        };
        if let ToolPlace::Workspace { workspace } = place {
            if self.incus.is_none() {
                return Err("This device runs no container workspaces.".to_owned());
            }
            if !self
                .container_workspaces()
                .iter()
                .any(|(id, _)| *id == workspace)
            {
                return Err("That workspace isn't one MaxHQ made here.".to_owned());
            }
        }
        {
            let mut known = lock(&self.known);
            let key = (kind.clone(), place);
            if matches!(known.installs.get(&key), Some(ToolState::Running { .. })) {
                return Err("That tool is already installing there.".to_owned());
            }
            known.installs.insert(key, ToolState::running(message));
        }
        self.publish();
        Ok(installer)
    }

    /// Scans again after `kind` installed in `place`, leaves it in the
    /// state its install gave, or keeps why it failed.
    async fn finish(
        &self,
        kind: ToolKind,
        place: ToolPlace,
        result: Result<Option<ToolState>, &install::InstallError>,
    ) {
        let key = (kind, place);
        match result {
            Ok(None) => {
                lock(&self.known).installs.remove(&key);
                self.scan().await;
            }
            Ok(Some(state)) => {
                lock(&self.known).installs.insert(key, state);
                self.publish();
            }
            Err(err) => {
                let reason = crate::fleet::sentence(&err.to_string());
                warn!(tool = %install::name(&key.0), %reason, "an install failed");
                lock(&self.known)
                    .installs
                    .insert(key, ToolState::Failed { reason });
                self.publish();
            }
        }
    }

    async fn run_job(self: Arc<Self>, job: Job) {
        let target = job.target();
        let def = defs::def(target.tool);
        self.set_state(
            target,
            ToolState::running(format!("Updating {}", def.binary)),
        );
        let result = match job {
            Job::Host { command, path, .. } => {
                let latest = lock(&self.known).latest.get(&target.tool).cloned();
                let progress = {
                    let tools = Arc::clone(&self);
                    move |message: String| tools.set_state(target, ToolState::running(message))
                };
                run::update(
                    def,
                    &command,
                    &path,
                    latest.as_deref(),
                    &self.locks,
                    run::UPDATE_TIMEOUT,
                    &progress,
                )
                .await
                .map(drop)
            }
            Job::Workspace { instance, .. } => self.update_in_container(def, &instance).await,
        };
        match result {
            Ok(()) => {
                info!(tool = def.binary, "updated");
                lock(&self.known).states.remove(&target);
                if target.tool == Tool::Codex
                    && target.place == ToolPlace::Host
                    && let Some(then) = self.codex_updated.get()
                {
                    then();
                }
                self.scan().await;
            }
            Err(reason) => {
                warn!(tool = def.binary, %reason, "an update failed");
                self.set_state(target, ToolState::Failed { reason });
            }
        }
    }

    /// Runs the CLI's own updater in a container workspace.
    async fn update_in_container(&self, def: &Def, instance: &str) -> Result<(), String> {
        let incus = self.incus.clone().ok_or("No Incus here.")?;
        let name = InstanceName::new(instance).map_err(|err| err.to_string())?;
        let Some(args) = def.self_update else {
            return Err(format!("{} has no updater of its own.", def.binary));
        };
        let exec = crate::workspace::IncusExec::new(incus, name);
        let mut cmd = vec![
            "env",
            "HOME=/home/agent",
            "PATH=/home/agent/.local/bin:/usr/local/bin:/usr/bin:/bin",
            def.container_path,
        ];
        cmd.extend(args);
        let lock_key = format!("{}-{instance}", def.binary);
        let Some(_held) = self.locks.take(&lock_key) else {
            return Err("An update is already running.".to_owned());
        };
        let run = exec.run(&cmd, def.container_user, None);
        let output = tokio::time::timeout(run::UPDATE_TIMEOUT, run)
            .await
            .map_err(|_| "Update timed out.".to_owned())?
            .map_err(|err| err.to_string())?;
        if output.success() {
            Ok(())
        } else {
            Err(run::failure_message(&run::Ran {
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                code: Some(output.status),
                timed_out: false,
            }))
        }
    }
}

enum Job {
    Host {
        target: ToolTarget,
        command: Command,
        path: PathBuf,
    },
    Workspace {
        target: ToolTarget,
        instance: String,
    },
}

impl Job {
    fn target(&self) -> ToolTarget {
        match self {
            Job::Host { target, .. } | Job::Workspace { target, .. } => *target,
        }
    }
}

/// The version a container workspace's CLI reports.
async fn container_version(incus: &Incus, instance: &str, def: &Def) -> Option<String> {
    let name = InstanceName::new(instance).ok()?;
    let exec = crate::workspace::IncusExec::new(incus.clone(), name);
    let mut cmd = vec!["env", def.container_path];
    cmd.extend(def.version_args);
    let output = exec.run(&cmd, def.container_user, None).await.ok()?;
    output
        .success()
        .then(|| defs::version_text(&String::from_utf8_lossy(&output.stdout)))
        .flatten()
}

/// How a tool MaxHQ installed reads as installed.
const MAXHQ_METHOD: &str = "maxhq";

/// True when `real` is the tool MaxHQ installed in `layout`: inside its
/// folder, or made by the tool's own script MaxHQ ran.
fn ours(layout: &install::Layout, kind: &ToolKind, real: &Path) -> bool {
    let dir = layout.dir(kind);
    real.starts_with(&dir) || Path::new(&dir).join("script").is_file()
}

/// The helpers MaxHQ installed in `layout`, as their records say.
fn host_helpers(layout: &install::Layout) -> Vec<ToolInfo> {
    use maxhq_protocol::state::HelperTool;
    [
        HelperTool::Node,
        HelperTool::Uv,
        HelperTool::Gh,
        HelperTool::Jdk,
        HelperTool::AndroidSdk,
    ]
    .into_iter()
    .filter_map(|tool| {
        let kind = ToolKind::Helper { tool };
        let dir = layout.dir(&kind);
        let record = std::fs::read_to_string(format!("{dir}/current/.install-complete")).ok()?;
        let line = |key: &str| {
            record.lines().find_map(|line| {
                line.strip_prefix(key)
                    .and_then(|rest| rest.strip_prefix(' '))
                    .map(str::to_owned)
            })
        };
        let path = match tool {
            HelperTool::Jdk => Some(format!("{}/bin/java", android::jdk_home(layout))),
            HelperTool::AndroidSdk => Some(android::sdk_root(layout)),
            _ => line("link")
                .and_then(|link| link.split(' ').next().map(str::to_owned))
                .map(|link| format!("{}/{link}", layout.bin()))
                .or_else(|| line("entry").map(|entry| format!("{dir}/current/{entry}"))),
        };
        Some(ToolInfo {
            kind,
            place: ToolPlace::Host,
            path,
            version: line("version"),
            latest: None,
            owner: ToolOwner::MaxHq,
            method: MAXHQ_METHOD.to_owned(),
            update: ToolUpdate::Never,
            state: ToolState::Idle,
        })
    })
    .collect()
}

/// The places [`Tools`] installs into: this device, and the container
/// workspaces MaxHQ made on it.
/// The installer [`Tools::installer`] hands out: each call shows in the
/// device's tools while it runs.
struct Tracked(std::sync::Weak<Tools>);

impl install::Installer for Tracked {
    fn ensure<'a>(
        &'a self,
        def: &'a install::InstallDef,
        version: Option<&'a str>,
        place: &'a ToolPlace,
    ) -> BoxFuture<'a, Result<install::Installed, install::InstallError>> {
        Box::pin(async move {
            let tools = self.0.upgrade().ok_or(install::InstallError::NotBuilt)?;
            let message = format!("Getting ready to install {}", install::name(&def.kind));
            let installer = tools
                .begin(&def.kind, *place, message)
                .map_err(install::InstallError::Failed)?;
            let result = installer.ensure(def, version, place).await;
            tools
                .finish(def.kind.clone(), *place, result.as_ref().map(|_| None))
                .await;
            result
        })
    }

    fn remove<'a>(
        &'a self,
        kind: &'a ToolKind,
        place: &'a ToolPlace,
    ) -> BoxFuture<'a, Result<(), install::InstallError>> {
        Box::pin(async move {
            let tools = self.0.upgrade().ok_or(install::InstallError::NotBuilt)?;
            let message = format!("Removing {}", install::name(kind));
            let installer = tools
                .begin(kind, *place, message)
                .map_err(install::InstallError::Failed)?;
            let result = installer.remove(kind, place).await;
            tools
                .finish(kind.clone(), *place, result.as_ref().map(|()| None))
                .await;
            result
        })
    }
}

struct DevicePlaces(std::sync::Weak<Tools>);

impl install::Places for DevicePlaces {
    fn open<'a>(
        &'a self,
        place: &'a ToolPlace,
        kind: &'a ToolKind,
    ) -> BoxFuture<'a, Result<install::Opened, install::InstallError>> {
        use install::InstallError::Failed;
        Box::pin(async move {
            let tools = self
                .0
                .upgrade()
                .ok_or_else(|| Failed("maxd is stopping.".to_owned()))?;
            match place {
                ToolPlace::Host => {
                    let layout = tools.host_layout().ok_or_else(|| {
                        Failed("maxd has no home folder to install into.".to_owned())
                    })?;
                    Ok((Arc::new(HostExec) as Arc<dyn Exec>, layout))
                }
                ToolPlace::Workspace { workspace } => {
                    let incus = tools.incus.clone().ok_or_else(|| {
                        Failed("This device runs no container workspaces.".to_owned())
                    })?;
                    let (_, instance) = tools
                        .container_workspaces()
                        .into_iter()
                        .find(|(id, _)| id == workspace)
                        .ok_or_else(|| {
                            Failed("That workspace isn't one MaxHQ made here.".to_owned())
                        })?;
                    let name =
                        InstanceName::new(&instance).map_err(|err| Failed(err.to_string()))?;
                    let exec = crate::workspace::IncusExec::new(incus, name);
                    let output = exec
                        .run(&["uname", "-m"], ExecUser::Root, None)
                        .await
                        .map_err(|err| Failed(err.to_string()))?;
                    let machine = String::from_utf8_lossy(&output.stdout).into_owned();
                    // Codex lives in /usr/local as root, as the recipe puts it.
                    let user = match kind {
                        ToolKind::Agent { tool: Tool::Codex } => ExecUser::Root,
                        _ => ExecUser::Agent,
                    };
                    let layout =
                        install::Layout::container(&format!("incus {instance}"), user, &machine);
                    Ok((Arc::new(exec) as Arc<dyn Exec>, layout))
                }
            }
        })
    }
}

fn refused(reason: String) -> ActionOutcome {
    ActionOutcome::Refused { reason }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests;
