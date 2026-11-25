// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The manager against a fake Incus.

use std::collections::{BTreeSet, HashMap};
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use futures::future::BoxFuture;
use futures::FutureExt;
use maxhq_core::names;
use maxhq_protocol::ids::{AccountId, AgentId, BootId, DeviceId, ProjectId, WorkspaceId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{
    Activity, Agent, AgentLink, AllowedWorkflowEnv, AllowedWorkflowVar, Control, Device,
    EgressCheck, GitAuthor, GuestState, ManagedWorkspace, Project, ProjectSettings, Vendor,
    Workspace, WorkspaceLocation, WorkspaceSpec, WorkspaceState,
};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::Secret;
use serde_json::{json, Value};

use super::create::slug;
use super::egress::{HostFacts, Listener};
use super::exec::{UNPUSHED_SCRIPT, WRITE_FIRST};
use super::keys::{key, Keys, Record};
use super::manager::{Config, Manager, Timing};
use super::manifest;
use super::probe::{Outcome, Report, Targets, Tried};
use super::test_host::TEST_HOST_KEY;
use super::unsaved::WORKTREE_SCRIPT;
use super::{Github, GuestLinks};
use crate::db::Db;
use crate::hub::Hub;
use crate::incus::fake::{self, ExecCall, ExecReply, FakeIncus, State};
use crate::incus::{Incus, InstanceName, MANAGED_KEY};
use crate::recipe::{Exec, ExecError, ExecUser, Step, MAXD};
use crate::session::holders::{AgentSummary, Checkout, Unsaved, Workspaces};
use crate::testing::TempDir;

/// This host's boot.
const BOOT: &str = "boot-2";
/// What the fake GitHub App mints.
const TOKEN: &str = "test-not-a-real-token";
/// The project's remote.
const REMOTE: &str = "git@github.com:Example-User/MaxHQ.git";
/// Where the repo lands in a container.
const REPO_DIR: &str = "/home/agent/work/MaxHQ";
/// The file the probe's targets go to before they move into place.
const PROBE_UPLOAD: &str = "/run/maxhq-probe.json.maxhq-new";
/// What the first recipe step makes.
const MARKER: &str = "/etc/maxhq-marker";
/// What the second recipe step makes.
const UPDATED: &str = "/etc/maxhq-updated";

fn device_id() -> DeviceId {
    DeviceId::derive("devbox")
}

fn project_id() -> ProjectId {
    ProjectId::derive("maxhq")
}

fn device() -> Device {
    Device {
        id: device_id(),
        name: "devbox".into(),
        os: "linux".into(),
        boot_id: BootId(BOOT.into()),
        maxd_version: "0.1.0".into(),
        winding_down_until: None,
        workspace_host: None,
        build: None,
        target: None,
        wsl: None,
    }
}

/// The workspace ID a container of the project gets.
fn workspace_of(instance: &str) -> WorkspaceId {
    names::workspace(
        device_id(),
        &WorkspaceLocation::Container {
            instance: instance.to_owned(),
            path: REPO_DIR.to_owned(),
        },
    )
}

/// Keys like the ones a finished create writes.
fn keys_for(instance: &str, recipe: u32, boot: bool) -> Keys {
    Keys {
        project: project_id(),
        device: device_id(),
        workspace: workspace_of(instance),
        title: "MaxHQ".to_owned(),
        repo: "https://github.com/Example-User/MaxHQ.git".to_owned(),
        boot,
        recipe,
        agents: Vec::new(),
        created: Timestamp(1_700_000_000_000),
        path: None,
    }
}

/// An instance with `keys`, a root disk on `agents` and a NIC on
/// `agentbr0`, as a create makes it.
/// A container as maxd makes one on a host whose spec names no parent,
/// with M3's guard.
fn container(name: &str, status: &str, keys: &Keys) -> Value {
    let guard = super::Caps::default();
    let mut config = keys.encode();
    config.extend(guard.create_keys(name, guard.memory(None)));
    let pairs: Vec<(&str, &str)> = config
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let mut value = fake::instance(name, status, &pairs);
    value["devices"] = json!({
        "root": { "type": "disk", "path": "/", "pool": "agents" },
        "eth0": { "type": "nic", "name": "eth0", "network": "agentbr0" },
    });
    value
}

fn summary(agent: &str) -> AgentSummary {
    AgentSummary {
        agent: AgentId::derive(agent),
        vendor: Vendor::Claude,
        account: AccountId::derive("acc"),
        session: "5d0c".to_owned(),
        branch: "fix-x".to_owned(),
        worktree: "/home/agent/work/.worktrees/fix-x".to_owned(),
    }
}

fn agent(id: &str, workspace: WorkspaceId, activity: Activity) -> Entity {
    Entity::Agent(Agent {
        id: AgentId::derive(id),
        workspace,
        parent: None,
        vendor: Vendor::Claude,
        account: None,
        model: None,
        permission: None,
        session: None,
        vendor_session_id: Some("5d0c".into()),
        control: Control::Observed,
        pid: Some(42),
        link: AgentLink::Live,
        title: "maxhq".into(),
        activity,
        context: None,
        started_at: Timestamp(1),
        last_activity_at: Timestamp(1),
        branch: None,
        inbox: Default::default(),
        origin: None,
        pulls: Vec::new(),
        taking_over: false,
        asleep: false,
        effort: None,
        running: None,
        moved: None,
        local: None,
    })
}

/// The host's network as the probe's targets see it.
fn host_facts() -> HostFacts {
    HostFacts {
        v4: vec![Ipv4Addr::new(192, 168, 1, 20), Ipv4Addr::new(100, 64, 0, 5)],
        v6: Vec::new(),
        gateways: vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))],
        listeners: vec![Listener {
            address: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: 8080,
        }],
    }
}

/// Two recipe steps: the first makes [`MARKER`] and the second, from
/// version 2, makes [`UPDATED`].
fn test_steps(done: u32) -> Vec<Step> {
    [
        Step {
            id: "marker",
            version: 1,
            check: |exec| exists(exec, MARKER),
            apply: |exec| touch(exec, MARKER),
        },
        Step {
            id: "updated",
            version: 2,
            check: |exec| exists(exec, UPDATED),
            apply: |exec| touch(exec, UPDATED),
        },
    ]
    .into_iter()
    .filter(|step| step.version > done)
    .collect()
}

fn exists<'a>(exec: &'a dyn Exec, path: &'static str) -> BoxFuture<'a, Result<bool, ExecError>> {
    async move {
        let output = exec
            .run(&["test", "-e", path], ExecUser::Root, None)
            .await?;
        Ok(output.success())
    }
    .boxed()
}

fn touch<'a>(exec: &'a dyn Exec, path: &'static str) -> BoxFuture<'a, Result<(), ExecError>> {
    async move {
        let cmd = ["touch", path];
        let output = exec.run(&cmd, ExecUser::Root, None).await?;
        if output.success() {
            Ok(())
        } else {
            Err(ExecError::failed(&cmd, &output))
        }
    }
    .boxed()
}

/// What the containers hold, as the fake's exec handler sees it.
#[derive(Default)]
struct Guest {
    /// Paths that exist, by instance.
    paths: BTreeSet<(String, String)>,
    /// What the last probe was asked to try.
    targets: Option<Targets>,
    /// Blocked targets that answer anyway, by label.
    leaks: BTreeSet<String>,
    /// What the check for unpushed work prints.
    unpushed: ExecReply,
    /// What auto-settle's check of one worktree prints.
    worktree: ExecReply,
    /// What git says when a push preflight fails.
    push_fails: Option<String>,
}

/// Answers one exec call the way a container would.
fn answer(guest: &Mutex<Guest>, call: &ExecCall) -> ExecReply {
    let mut guest = guest.lock().unwrap();
    let cmd: Vec<&str> = call.command.iter().map(String::as_str).collect();
    let instance = call.instance.clone();
    match cmd.as_slice() {
        ["systemctl", "is-system-running", "--wait"] => ExecReply::ok("running\n"),
        ["test", _, path] => {
            if guest.paths.contains(&(instance, (*path).to_owned())) {
                ExecReply::ok("")
            } else {
                ExecReply::fail(1, "")
            }
        }
        ["touch", path] => {
            guest.paths.insert((instance, (*path).to_owned()));
            ExecReply::ok("")
        }
        ["sh", "-c", script, "sh", temp, encoded] if *script == WRITE_FIRST => {
            if *temp == PROBE_UPLOAD {
                let bytes = STANDARD.decode(encoded).unwrap();
                guest.targets = Some(serde_json::from_slice(&bytes).unwrap());
            }
            ExecReply::ok("")
        }
        [maxd, "guest", "probe", _] if *maxd == MAXD => {
            let targets = guest.targets.clone().expect("the targets went in first");
            let results = targets
                .targets
                .iter()
                .map(|target| {
                    let label = target.label();
                    let outcome = if target.required || guest.leaks.contains(&label) {
                        Outcome::Connected
                    } else {
                        Outcome::TimedOut
                    };
                    Tried {
                        target: label,
                        outcome,
                        detail: None,
                    }
                })
                .collect();
            let report = serde_json::to_string(&Report { results }).unwrap();
            ExecReply::ok(&report)
        }
        ["sh", "-c", script, "sh", _] if *script == UNPUSHED_SCRIPT => guest.unpushed.clone(),
        ["sh", "-c", script, "sh", _] if *script == WORKTREE_SCRIPT => guest.worktree.clone(),
        ["env", .., "clone", "--progress", "--", _, dir] => {
            guest.paths.insert((instance, format!("{dir}/.git")));
            let mut reply = ExecReply::ok("");
            reply.stderr = b"Cloning into 'x'...\nReceiving objects:  42% (42/100)\rReceiving objects: 100% (100/100), done.\n".to_vec();
            reply
        }
        ["env", .., "push", "--dry-run", "origin", "HEAD"] => match &guest.push_fails {
            Some(stderr) => ExecReply::fail(1, stderr),
            None => ExecReply::ok(""),
        },
        _ => ExecReply::default(),
    }
}

/// Mints [`TOKEN`], or refuses with a set reason.
#[derive(Default)]
struct FakeGithub {
    refuse: Mutex<Option<String>>,
    asked: Mutex<Vec<String>>,
}

impl Github for FakeGithub {
    fn mint<'a>(&'a self, repo: &'a str) -> BoxFuture<'a, Result<Secret, String>> {
        async move {
            self.asked.lock().unwrap().push(repo.to_owned());
            match self.refuse.lock().unwrap().clone() {
                Some(reason) => Err(reason),
                None => Ok(Secret::new(TOKEN.to_owned())),
            }
        }
        .boxed()
    }
}

/// Guest links that come up after a pause, counting how many wait at once.
#[derive(Default)]
struct FakeLinks {
    delay: Duration,
    opened: Mutex<Vec<WorkspaceId>>,
    closed: Mutex<Vec<WorkspaceId>>,
    /// The workspaces forgotten, whose agents the session manager forgets.
    forgot: Mutex<Vec<WorkspaceId>>,
    waiting: AtomicUsize,
    most: AtomicUsize,
}

impl GuestLinks for FakeLinks {
    fn open(&self, workspace: WorkspaceId, _instance: &InstanceName, _repo: Option<&str>) {
        self.opened.lock().unwrap().push(workspace);
    }

    fn close(&self, workspace: WorkspaceId) {
        self.closed.lock().unwrap().push(workspace);
    }

    fn wait_up(
        &self,
        _workspace: WorkspaceId,
        _wait: Duration,
    ) -> BoxFuture<'_, Result<Option<String>, String>> {
        async move {
            let now = self.waiting.fetch_add(1, Ordering::SeqCst) + 1;
            self.most.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            self.waiting.fetch_sub(1, Ordering::SeqCst);
            Ok(Some("0.1.0".to_owned()))
        }
        .boxed()
    }

    fn forget(&self, workspace: WorkspaceId) {
        self.forgot.lock().unwrap().push(workspace);
    }
}

/// A running workspace whose container takes its time to stop, under the
/// manager the request functions answer through, for the server's tests.
/// Only the first install counts, so one test in the binary uses this.
pub(crate) struct Installed {
    /// The device's hub, for the server to share.
    pub(crate) hub: Hub,
    pub(crate) workspace: WorkspaceId,
    /// Keeps the fake Incus and the folders.
    _rig: Rig,
}

/// Starts a workspace whose stops take `delay`, and installs its manager.
pub(crate) async fn install_slow_stop(delay: Duration) -> Installed {
    let rig = Rig::new();
    rig.add("mhq-maxhq", "Stopped", &keys_for("mhq-maxhq", 2, true));
    let manager = rig.manager();
    manager.rebuild().await;
    let workspace = workspace_of("mhq-maxhq");
    assert_eq!(manager.start(workspace).await, ActionOutcome::Done);
    settles(&manager, workspace, WorkspaceState::Running).await;
    rig.fake.state().state_delay = delay;
    super::install(manager);
    Installed {
        hub: rig.hub.clone(),
        workspace,
        _rig: rig,
    }
}

/// A device with a fake Incus, a hub, a database and the fakes around them.
struct Rig {
    fake: FakeIncus,
    hub: Hub,
    db: Db,
    dir: TempDir,
    guest: Arc<Mutex<Guest>>,
    github: Arc<FakeGithub>,
    links: Arc<FakeLinks>,
}

impl Rig {
    fn new() -> Self {
        Self::with_links(FakeLinks::default())
    }

    fn with_links(links: FakeLinks) -> Self {
        let dir = TempDir::new();
        let guest = Arc::new(Mutex::new(Guest::default()));
        let handler = guest.clone();
        let mut state = State::default();
        state.exec = Some(Arc::new(move |call: &ExecCall| answer(&handler, call)));
        state.pool_source = Some(dir.path().display().to_string());
        std::fs::write(dir.path().join("maxd-guest"), b"guest maxd").unwrap();
        Self {
            fake: FakeIncus::start(state),
            hub: Hub::new(device(), HashMap::new()),
            db: Db::open_in_memory().unwrap(),
            dir,
            guest,
            github: Arc::new(FakeGithub::default()),
            links: Arc::new(links),
        }
    }

    fn runtime(&self) -> PathBuf {
        self.dir.path().join("run")
    }

    fn data(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    fn manager(&self) -> Manager {
        self.manager_timed(|_| {})
    }

    /// A manager whose timing `edit` changes.
    fn manager_timed(&self, edit: impl FnOnce(&mut Timing)) -> Manager {
        self.manager_with(|config| edit(&mut config.timing))
    }

    /// A manager under the host spec's `caps`.
    fn manager_capped(&self, caps: super::Caps) -> Manager {
        self.manager_with(|config| config.caps = caps)
    }

    /// A manager whose config `edit` changes.
    fn manager_with(&self, edit: impl FnOnce(&mut Config)) -> Manager {
        let mut config = Config::new(
            Incus::new(&self.fake.socket),
            self.hub.clone(),
            self.db.clone(),
            self.runtime(),
            self.data(),
        );
        config.github = self.github.clone();
        config.links = self.links.clone();
        config.guest_binary = Some(self.dir.path().join("maxd-guest"));
        config.recipe = 2;
        config.steps = test_steps;
        config.timing = Timing {
            retry: Duration::from_millis(20),
            resync: Duration::from_secs(3600),
            ready_tries: 3,
            ready_pause: Duration::from_millis(1),
            guest_wait: Duration::from_secs(1),
            state_timeout: Duration::from_secs(5),
            stop_answer: Duration::from_secs(5),
        };
        config.host_facts = host_facts;
        edit(&mut config);
        Manager::new(config)
    }

    /// Names `agentbr0` and `agents` in the device's settings.
    fn place(&self) {
        let mut settings = crate::hub::default_settings(self.hub.device());
        settings.workspace_network = Some("agentbr0".to_owned());
        settings.workspace_pool = Some("agents".to_owned());
        self.hub.apply([Change::Upsert {
            value: Entity::DeviceSettings(settings),
        }]);
    }

    fn project(&self, repo: Option<&str>) {
        self.hub.apply([Change::Upsert {
            value: Entity::Project(Project {
                id: project_id(),
                name: "MaxHQ".to_owned(),
                repo: repo.map(str::to_owned),
                default_vendor: None,
                default_account: None,
                default_workspace: None,
            }),
        }]);
    }

    fn add(&self, name: &str, status: &str, keys: &Keys) {
        self.fake
            .state()
            .instances
            .insert(name.to_owned(), container(name, status, keys));
    }

    fn config_key(&self, instance: &str, key: &str) -> Option<String> {
        self.fake.state().instances[instance]["config"][key]
            .as_str()
            .map(str::to_owned)
    }

    /// The writes the fake saw that change instances, as "METHOD target".
    /// Exec and its output logs are left out.
    fn changes(&self) -> Vec<String> {
        self.fake
            .state()
            .writes()
            .iter()
            .filter(|logged| !logged.target.contains("/exec") && !logged.target.contains("/logs/"))
            .map(|logged| format!("{} {}", logged.method, logged.target))
            .collect()
    }

    /// The state changes the fake saw for an instance, such as `start`.
    fn state_changes(&self, instance: &str) -> Vec<String> {
        let target = format!("/1.0/instances/{instance}/state");
        self.fake
            .state()
            .writes()
            .iter()
            .filter(|logged| logged.method == "PUT" && logged.target == target)
            .map(|logged| {
                logged.body["action"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect()
    }

    fn spec(&self, name: Option<&str>) -> WorkspaceSpec {
        WorkspaceSpec {
            project: project_id(),
            device: device_id(),
            name: name.map(str::to_owned),
            start_at_boot: true,
        }
    }
}

/// Waits up to five seconds for `check`.
async fn until(what: &str, mut check: impl FnMut() -> bool) {
    for _ in 0..1000 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for {what}");
}

/// Waits until nothing is changing the workspace and it stands at `want`.
async fn settles(manager: &Manager, workspace: WorkspaceId, want: WorkspaceState) {
    let what = format!("{want:?}");
    until(&what, || {
        let state = manager.record(workspace).map(|r| r.state);
        if let Some(WorkspaceState::Failed { reason }) = &state
            && !matches!(want, WorkspaceState::Failed { .. })
        {
            panic!("it failed: {reason}");
        }
        !manager.changing(workspace) && state.as_ref() == Some(&want)
    })
    .await;
}

/// Waits until the workspace is gone.
async fn forgotten(manager: &Manager, workspace: WorkspaceId) {
    until("the workspace to go", || {
        !manager.changing(workspace) && manager.record(workspace).is_none()
    })
    .await;
}

fn refusal(outcome: ActionOutcome) -> String {
    match outcome {
        ActionOutcome::Refused { reason } => reason,
        other => panic!("not refused: {other:?}"),
    }
}

#[tokio::test]
async fn creates_a_workspace_step_by_step() {
    let rig = Rig::new();
    rig.place();
    rig.project(Some(REMOTE));
    let manager = rig.manager();

    let mut deltas = rig.hub.subscribe(None).deltas;
    let outcome = manager.create(rig.spec(None)).await;
    let workspace = workspace_of("mhq-maxhq");
    assert_eq!(outcome, ActionOutcome::Created { workspace });
    settles(&manager, workspace, WorkspaceState::Running).await;

    // The clone step showed how much git had received.
    let mut steps = Vec::new();
    while let Ok(delta) = deltas.try_recv() {
        for change in delta.changes {
            if let Change::Upsert {
                value: Entity::Workspace(shown),
            } = change
                && let Some(ManagedWorkspace {
                    state: WorkspaceState::Creating { step, .. },
                    ..
                }) = shown.managed
                && step.starts_with("Cloning")
                && steps.last() != Some(&step)
            {
                steps.push(step);
            }
        }
    }
    assert_eq!(
        steps,
        [
            "Cloning Example-User/MaxHQ",
            "Cloning Example-User/MaxHQ, 42%",
            "Cloning Example-User/MaxHQ, 100%",
        ]
    );

    // The instance: the keys, M3's guard while the host spec names no
    // parent, no profiles, and the pool, network and guest proxy.
    let instance = rig.fake.state().instances["mhq-maxhq"].clone();
    let config = &instance["config"];
    assert_eq!(config[MANAGED_KEY], "true");
    assert_eq!(config[key::RECIPE], "2");
    assert_eq!(
        config[key::REPO],
        "https://github.com/Example-User/MaxHQ.git"
    );
    assert_eq!(config[key::TITLE], "MaxHQ");
    assert_eq!(config[key::BOOT], "true");
    assert_eq!(config["security.idmap.isolated"], "true");
    assert_eq!(config["limits.cpu"], "4");
    assert_eq!(config["limits.memory"], "8GiB");
    assert!(config.get("raw.lxc").is_none());
    assert_eq!(config["limits.memory.enforce"], "soft");
    assert_eq!(config["boot.autostart"], "false");
    assert!(config.get("security.privileged").is_none());
    assert!(config.get("security.nesting").is_none());
    assert_eq!(instance["profiles"], json!([]));
    assert_eq!(instance["devices"]["root"]["pool"], "agents");
    assert_eq!(instance["devices"]["eth0"]["network"], "agentbr0");
    assert_eq!(instance["devices"]["maxhq-guest"]["type"], "proxy");
    assert_eq!(instance["status"], "Running");

    // The guest binary went in, executable.
    let pushed = rig.fake.state().files[&(
        "mhq-maxhq".to_owned(),
        "/usr/local/lib/maxhq/maxd.new".to_owned(),
    )]
        .clone();
    assert_eq!(pushed.bytes, b"guest maxd");
    assert_eq!(pushed.mode, "0755");

    // The steps ran in order, and the token only ever went over stdin.
    let execs = rig.fake.state().execs.clone();
    let at = |what: &dyn Fn(&ExecCall) -> bool| execs.iter().position(what).unwrap();
    let ready = at(&|c| c.command[0] == "systemctl");
    let placed = at(&|c| c.command.iter().any(|w| w.contains("ln -sfn")));
    let probe = at(&|c| c.command[..2] == [MAXD, "guest"]);
    let marker = at(&|c| c.command == ["touch", MARKER]);
    let clone = at(&|c| c.command.iter().any(|w| w == "clone"));
    let push = at(&|c| c.command.iter().any(|w| w == "--dry-run"));
    assert!(ready < placed && placed < probe && probe < marker);
    assert!(marker < clone && clone < push);
    for call in [&execs[clone], &execs[push]] {
        assert!(call.streamed);
        assert_eq!(call.user, 1000);
        assert_eq!(call.stdin, format!("{TOKEN}\n").into_bytes());
    }
    assert!(execs[clone].command.ends_with(&[
        "clone".to_owned(),
        "--progress".to_owned(),
        "--".to_owned(),
        "https://github.com/Example-User/MaxHQ.git".to_owned(),
        REPO_DIR.to_owned(),
    ]));
    for call in &execs {
        assert!(
            call.command.iter().all(|w| !w.contains(TOKEN)),
            "{:?}",
            call.command
        );
        assert!(call.environment.values().all(|v| !v.contains(TOKEN)));
    }
    assert!(rig
        .github
        .asked
        .lock()
        .unwrap()
        .iter()
        .all(|repo| repo == "Example-User/MaxHQ"));

    // The probe tried the host and the ranges, and needed GitHub.
    let targets = rig.guest.lock().unwrap().targets.clone().unwrap();
    let labels: Vec<String> = targets.targets.iter().map(|t| t.label()).collect();
    assert!(labels.contains(&"192.168.1.20:8080".to_owned()));
    assert!(labels.contains(&"10.100.2.1:22".to_owned()));
    assert!(targets
        .targets
        .iter()
        .any(|t| t.required && t.label() == "github.com:443"));

    // What it keeps, shows and saves.
    let record = manager.record(workspace).unwrap();
    assert_eq!(record.keys.recipe, 2);
    assert!(matches!(record.egress, EgressCheck::Passed { .. }));
    assert_eq!(
        record.guest,
        GuestState::Connected {
            version: "0.1.0".to_owned()
        }
    );
    assert_eq!(record.host_boot.as_deref(), Some(BOOT));
    assert_eq!(record.blocked(), None);
    assert_eq!(rig.links.opened.lock().unwrap().as_slice(), [workspace]);
    match rig.hub.get(&EntityKey::Workspace(workspace)) {
        Some(Entity::Workspace(shown)) => {
            assert_eq!(shown.managed.unwrap().state, WorkspaceState::Running);
            assert_eq!(
                shown.location,
                WorkspaceLocation::Container {
                    instance: "mhq-maxhq".to_owned(),
                    path: REPO_DIR.to_owned(),
                }
            );
        }
        other => panic!("not shown: {other:?}"),
    }
    let saved: Vec<Record> = rig.db.workspaces().unwrap();
    assert_eq!(saved, [record]);

    // Asking again while it exists says so.
    let again = refusal(manager.create(rig.spec(None)).await);
    assert_eq!(again, "mhq-maxhq already exists on this device.");
}

#[tokio::test]
async fn a_running_workspace_takes_ssh_and_shows_its_host_key() {
    let rig = Rig::new();
    rig.place();
    rig.project(Some(REMOTE));
    let manager = rig.manager();
    let workspace = workspace_of("mhq-maxhq");
    assert!(manager.ssh_blocked(workspace).is_some());

    manager.create(rig.spec(None)).await;
    settles(&manager, workspace, WorkspaceState::Running).await;
    assert_eq!(manager.ssh_blocked(workspace), None);

    let key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcH";
    manager.set_ssh_host_key(workspace, key.to_owned());
    match rig.hub.get(&EntityKey::Workspace(workspace)) {
        Some(Entity::Workspace(shown)) => {
            assert_eq!(shown.managed.unwrap().ssh_host_key.as_deref(), Some(key));
        }
        other => panic!("not shown: {other:?}"),
    }
    let saved: Vec<Record> = rig.db.workspaces().unwrap();
    assert_eq!(saved[0].ssh_host_key.as_deref(), Some(key));

    manager.set_state(workspace, WorkspaceState::Stopped);
    assert_eq!(
        manager.ssh_blocked(workspace).as_deref(),
        Some("mhq-maxhq isn't running")
    );
}

#[tokio::test]
async fn create_refuses_what_it_cant_do_before_it_writes() {
    let rig = Rig::new();
    rig.project(Some(REMOTE));
    let manager = rig.manager();

    let reason = refusal(manager.create(rig.spec(None)).await);
    assert!(
        reason.starts_with("Pick a workspace network for devbox in its settings first."),
        "{reason}"
    );
    assert!(reason.contains("agentbr0"), "{reason}");

    rig.place();
    let mut other = rig.spec(None);
    other.device = DeviceId::derive("mac");
    assert_eq!(
        refusal(manager.create(other).await),
        "That workspace is for another device."
    );

    rig.fake.state().locked_pools.insert("agents".to_owned());
    assert_eq!(
        refusal(manager.create(rig.spec(None)).await),
        "The agents pool isn't available. Its storage may be locked."
    );
    rig.fake.state().locked_pools.clear();

    for name in ["Bad_Name", "trailing-", &"a".repeat(60)] {
        let reason = refusal(manager.create(rig.spec(Some(name))).await);
        assert!(
            reason.starts_with("Use lower-case letters"),
            "{name}: {reason}"
        );
    }

    let why = "The GitHub App can't reach Example-User/MaxHQ. Add the repo to it on GitHub.";
    *rig.github.refuse.lock().unwrap() = Some(why.to_owned());
    assert_eq!(refusal(manager.create(rig.spec(None)).await), why);
    *rig.github.refuse.lock().unwrap() = None;

    rig.fake.state().add("mhq-maxhq", "Stopped", &[]);
    assert_eq!(
        refusal(manager.create(rig.spec(None)).await),
        "mhq-maxhq already exists on this device."
    );

    rig.project(None);
    assert_eq!(
        refusal(manager.create(rig.spec(Some("other"))).await),
        "MaxHQ has no GitHub remote, so it can't have a container."
    );

    assert_eq!(rig.changes(), Vec::<String>::new());
    assert!(manager.shared().records.is_empty());
    assert!(manager.shared().ops.is_empty());
}

#[tokio::test]
async fn a_create_that_reaches_the_host_fails_and_names_what_answered() {
    let rig = Rig::new();
    rig.place();
    rig.project(Some(REMOTE));
    rig.guest
        .lock()
        .unwrap()
        .leaks
        .insert("192.168.1.20:8080".to_owned());
    let manager = rig.manager();

    let workspace = workspace_of("mhq-maxhq");
    assert_eq!(
        manager.create(rig.spec(None)).await,
        ActionOutcome::Created { workspace }
    );
    let reason = "Checking egress failed: the container reached 192.168.1.20:8080, which it \
                  shouldn't"
        .to_owned();
    settles(&manager, workspace, WorkspaceState::Failed { reason }).await;

    let record = manager.record(workspace).unwrap();
    assert!(matches!(
        &record.egress,
        EgressCheck::Failed { reached, .. } if reached == &["192.168.1.20:8080".to_owned()]
    ));
    assert!(record.blocked().unwrap().contains("192.168.1.20:8080"));
    // Nothing after the probe ran, and the container stays for a look.
    assert!(rig
        .fake
        .state()
        .commands()
        .iter()
        .all(|c| !c.contains("clone")));
    assert_eq!(
        rig.config_key("mhq-maxhq", key::RECIPE).as_deref(),
        Some("0")
    );
    assert_eq!(rig.links.closed.lock().unwrap().as_slice(), [workspace]);
}

#[tokio::test]
async fn a_push_preflight_that_fails_fails_the_create() {
    let rig = Rig::new();
    rig.place();
    rig.project(Some(REMOTE));
    rig.guest.lock().unwrap().push_fails = Some("remote: Permission denied".to_owned());
    let manager = rig.manager();

    let workspace = workspace_of("mhq-maxhq");
    manager.create(rig.spec(None)).await;
    until("the create to fail", || {
        matches!(
            manager.record(workspace).map(|r| r.state),
            Some(WorkspaceState::Failed { .. })
        )
    })
    .await;
    let WorkspaceState::Failed { reason } = manager.record(workspace).unwrap().state else {
        unreachable!()
    };
    assert!(
        reason.starts_with("Checking that pushes work failed: git push --dry-run didn't work"),
        "{reason}"
    );
    assert!(reason.contains("Example-User/MaxHQ"), "{reason}");
    assert!(reason.contains("remote: Permission denied"), "{reason}");
    assert!(!reason.contains(TOKEN));
    assert_eq!(
        rig.config_key("mhq-maxhq", key::RECIPE).as_deref(),
        Some("0")
    );

    // A start won't run a container whose create didn't finish.
    let start = refusal(manager.start(workspace).await);
    assert_eq!(
        start,
        "mhq-maxhq didn't finish being made. Delete it and make it again."
    );
}

#[tokio::test]
async fn the_cache_is_rebuilt_from_the_instances() {
    let rig = Rig::new();
    let other = Keys {
        device: DeviceId::derive("mac"),
        ..keys_for("mhq-mac", 2, true)
    };
    let mut unmarked = container(
        "mhq-unmarked",
        "Running",
        &keys_for("mhq-unmarked", 2, true),
    );
    unmarked["config"]
        .as_object_mut()
        .unwrap()
        .remove(MANAGED_KEY);
    rig.add("mhq-maxhq", "Running", &keys_for("mhq-maxhq", 2, true));
    rig.add("mhq-resting", "Stopped", &keys_for("mhq-resting", 2, false));
    rig.add("mhq-half", "Stopped", &keys_for("mhq-half", 0, true));
    rig.add("ws-maxhq", "Running", &keys_for("ws-maxhq", 2, true));
    rig.add("mhq-mac", "Running", &other);
    rig.fake
        .state()
        .instances
        .insert("mhq-unmarked".to_owned(), unmarked);
    rig.fake.state().add("mhq-plain", "Running", &[]);

    // A record whose instance is gone.
    let gone = keys_for("mhq-gone", 2, true);
    let stale = Record {
        instance: "mhq-gone".to_owned(),
        keys: gone.clone(),
        state: WorkspaceState::Stopped,
        guest: GuestState::Installing,
        egress: EgressCheck::NotRun,
        egress_note: None,
        pool: Some("agents".to_owned()),
        network: Some("agentbr0".to_owned()),
        host_boot: None,
        ssh_host_key: None,
        memory_soft: None,
        unsaved: None,
    };
    rig.db
        .put_workspace(gone.workspace, "mhq-gone", &stale)
        .unwrap();

    let manager = rig.manager();
    assert!(rig.hub.get(&EntityKey::Workspace(gone.workspace)).is_some());

    let running = manager.rebuild().await;
    assert_eq!(running, [workspace_of("mhq-maxhq")]);

    let records = manager.shared().records.clone();
    let instances: Vec<&str> = records.values().map(|r| r.instance.as_str()).collect();
    let mut sorted = instances.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, ["mhq-half", "mhq-maxhq", "mhq-resting"]);
    let state = |instance: &str| records[&workspace_of(instance)].state.clone();
    assert_eq!(state("mhq-maxhq"), WorkspaceState::Starting);
    assert_eq!(state("mhq-resting"), WorkspaceState::Stopped);
    assert!(matches!(state("mhq-half"), WorkspaceState::Failed { .. }));
    assert_eq!(
        records[&workspace_of("mhq-maxhq")].keys,
        keys_for("mhq-maxhq", 2, true)
    );
    assert_eq!(
        records[&workspace_of("mhq-resting")].pool.as_deref(),
        Some("agents")
    );

    // The gone one is forgotten everywhere, and only reads went to Incus.
    assert!(rig.hub.get(&EntityKey::Workspace(gone.workspace)).is_none());
    assert_eq!(rig.db.workspaces::<Record>().unwrap().len(), 3);
    assert_eq!(rig.changes(), Vec::<String>::new());

    // The project shows even with no folder of it on this device.
    match rig.hub.get(&EntityKey::Project(project_id())) {
        Some(Entity::Project(project)) => {
            assert_eq!(project.name, "MaxHQ");
            assert_eq!(
                project.repo.as_deref(),
                Some("https://github.com/Example-User/MaxHQ.git")
            );
        }
        other => panic!("no project: {other:?}"),
    }

    // A new maxd shows the saved records before it asks Incus.
    let hub = Hub::new(device(), HashMap::new());
    let mut config = Config::new(
        Incus::new(&rig.fake.socket),
        hub.clone(),
        rig.db.clone(),
        rig.runtime(),
        rig.data(),
    );
    config.links = rig.links.clone();
    let again = Manager::new(config);
    assert_eq!(again.shared().records.len(), 3);
    assert!(hub
        .get(&EntityKey::Workspace(workspace_of("mhq-resting")))
        .is_some());
}

#[tokio::test]
async fn delete_refuses_unpushed_work_until_told_to_discard_it() {
    let rig = Rig::new();
    rig.add("mhq-maxhq", "Running", &keys_for("mhq-maxhq", 2, true));
    rig.guest.lock().unwrap().unpushed =
        ExecReply::ok("commits\t3\tfix-x\nchanges\t/home/agent/work/.worktrees/fix-y\n");
    let manager = rig.manager();
    manager.rebuild().await;
    let workspace = workspace_of("mhq-maxhq");

    let reason = refusal(manager.delete(workspace, false).await);
    assert_eq!(
        reason,
        "mhq-maxhq has 3 commits that aren't pushed, on fix-x, and changes that aren't \
         committed in .worktrees/fix-y. Push them first, or delete anyway to lose them."
    );
    let check = rig
        .fake
        .state()
        .execs
        .iter()
        .find(|c| c.command.get(2).map(String::as_str) == Some(UNPUSHED_SCRIPT))
        .cloned()
        .unwrap();
    assert_eq!(check.user, 1000);
    assert_eq!(check.command.last().map(String::as_str), Some(REPO_DIR));
    assert_eq!(rig.changes(), Vec::<String>::new());
    assert!(!manager.changing(workspace));

    // The host socket Incus leaves behind goes with the instance.
    let socket = rig.runtime().join("guests/mhq-maxhq.sock");
    std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
    std::fs::write(&socket, b"").unwrap();

    assert_eq!(manager.delete(workspace, true).await, ActionOutcome::Done);
    forgotten(&manager, workspace).await;
    assert!(!rig.fake.state().instances.contains_key("mhq-maxhq"));
    assert_eq!(rig.state_changes("mhq-maxhq"), ["stop"]);
    assert!(!socket.exists());
    assert!(rig.db.workspaces::<Record>().unwrap().is_empty());
    assert!(rig.hub.get(&EntityKey::Workspace(workspace)).is_none());
}

#[tokio::test]
async fn auto_settle_checks_a_worktree_only_in_a_container_that_runs() {
    let rig = Rig::new();
    rig.add("mhq-maxhq", "Running", &keys_for("mhq-maxhq", 2, true));
    rig.add("mhq-resting", "Stopped", &keys_for("mhq-resting", 2, true));
    let manager = rig.manager();
    manager.rebuild().await;
    let tree = "/home/agent/work/.worktrees/fix-x";
    let check = |instance: &str| Checkout::Container {
        workspace: workspace_of(instance),
        dir: tree.to_owned(),
    };

    rig.guest.lock().unwrap().worktree = ExecReply::ok("");
    assert_eq!(manager.unsaved(check("mhq-maxhq")).await, Some(false));
    let call = rig
        .fake
        .state()
        .execs
        .iter()
        .find(|c| c.command.get(2).map(String::as_str) == Some(WORKTREE_SCRIPT))
        .cloned()
        .unwrap();
    assert_eq!(call.instance, "mhq-maxhq");
    assert_eq!(call.user, 1000, "the check runs as the agent");
    assert_eq!(call.command.last().map(String::as_str), Some(tree));

    rig.guest.lock().unwrap().worktree = ExecReply::ok("commits\t1\tfix-x\n");
    assert_eq!(manager.unsaved(check("mhq-maxhq")).await, Some(true));
    rig.guest.lock().unwrap().worktree = ExecReply::ok("commits\t1\tfix-x\nnoupstream\n");
    assert_eq!(manager.unsaved(check("mhq-maxhq")).await, None);
    rig.guest.lock().unwrap().worktree = ExecReply::fail(4, "fatal: bad object HEAD");
    assert_eq!(manager.unsaved(check("mhq-maxhq")).await, None);

    rig.guest.lock().unwrap().worktree = ExecReply::ok("");
    let before = rig.fake.state().execs.len();
    assert_eq!(
        manager.unsaved(check("mhq-resting")).await,
        None,
        "a stopped container can't be checked"
    );
    assert_eq!(rig.fake.state().execs.len(), before, "nothing ran in it");
    assert_eq!(rig.state_changes("mhq-resting"), Vec::<String>::new());
    let unknown = Checkout::Container {
        workspace: WorkspaceId::derive("elsewhere"),
        dir: tree.to_owned(),
    };
    assert_eq!(manager.unsaved(unknown).await, None);
}

#[tokio::test]
async fn delete_asks_first_when_it_cant_check_or_agents_are_at_work() {
    let rig = Rig::new();
    let mut keys = keys_for("mhq-maxhq", 2, true);
    keys.agents = vec![summary("a")];
    rig.add("mhq-maxhq", "Running", &keys);
    rig.add("mhq-resting", "Stopped", &keys_for("mhq-resting", 2, true));
    let manager = rig.manager();
    manager.rebuild().await;
    let busy = workspace_of("mhq-maxhq");
    let resting = workspace_of("mhq-resting");

    rig.hub.apply([Change::Upsert {
        value: agent("a", busy, Activity::Busy),
    }]);
    let reason = refusal(manager.delete(busy, false).await);
    assert_eq!(
        reason,
        "mhq-maxhq has agents at work. Stop them first, or delete anyway to stop them and lose \
         their work."
    );

    let reason = refusal(manager.delete(resting, false).await);
    assert_eq!(
        reason,
        "mhq-resting is stopped, so maxd can't check it for work that isn't pushed. Start it to \
         check, or delete anyway to lose any work that isn't pushed."
    );
    assert_eq!(rig.changes(), Vec::<String>::new());

    assert_eq!(manager.delete(resting, true).await, ActionOutcome::Done);
    forgotten(&manager, resting).await;
    assert!(rig.state_changes("mhq-resting").is_empty());
    assert!(!rig.fake.state().instances.contains_key("mhq-resting"));

    // With nothing at work and a clean repo, it goes without asking.
    rig.hub.apply([Change::Upsert {
        value: agent("a", busy, Activity::Idle),
    }]);
    assert_eq!(manager.delete(busy, false).await, ActionOutcome::Done);
    forgotten(&manager, busy).await;

    let unknown = refusal(manager.delete(busy, false).await);
    assert_eq!(unknown, "There's no workspace like that on this device.");
}

#[tokio::test]
async fn only_a_delete_that_goes_through_forgets_the_workspace() {
    let rig = Rig::new();
    rig.add("mhq-maxhq", "Running", &keys_for("mhq-maxhq", 2, true));
    rig.guest.lock().unwrap().unpushed = ExecReply::ok("commits\t1\tfix-x\n");
    let manager = rig.manager();
    manager.rebuild().await;
    let workspace = workspace_of("mhq-maxhq");

    // A refused delete keeps the workspace, and the links its agents.
    refusal(manager.delete(workspace, false).await);
    assert!(rig.links.forgot.lock().unwrap().is_empty());

    // So does one that fails, here as Incus stops answering once it began.
    assert_eq!(manager.delete(workspace, true).await, ActionOutcome::Done);
    rig.fake.state().down = true;
    until("the delete to fail", || {
        !manager.changing(workspace)
            && matches!(
                manager.record(workspace).map(|r| r.state),
                Some(WorkspaceState::Failed { .. })
            )
    })
    .await;
    assert!(rig.links.forgot.lock().unwrap().is_empty());
    assert!(rig.hub.get(&EntityKey::Workspace(workspace)).is_some());

    // One that goes through forgets it, once.
    rig.fake.state().down = false;
    assert_eq!(manager.delete(workspace, true).await, ActionOutcome::Done);
    forgotten(&manager, workspace).await;
    assert_eq!(rig.links.forgot.lock().unwrap().as_slice(), [workspace]);
}

#[tokio::test]
async fn locked_storage_shows_and_a_start_waits_for_it() {
    let rig = Rig::new();
    rig.add("mhq-maxhq", "Stopped", &keys_for("mhq-maxhq", 2, true));
    rig.fake.state().locked_pools.insert("agents".to_owned());
    let manager = rig.manager();
    manager.rebuild().await;
    let workspace = workspace_of("mhq-maxhq");
    assert_eq!(
        manager.record(workspace).unwrap().state,
        WorkspaceState::StorageLocked
    );

    assert_eq!(manager.start(workspace).await, ActionOutcome::Done);
    settles(&manager, workspace, WorkspaceState::StorageLocked).await;
    assert!(rig.state_changes("mhq-maxhq").is_empty());
    assert!(manager
        .record(workspace)
        .unwrap()
        .blocked()
        .unwrap()
        .contains("its storage is locked"));

    // Once it unlocks, a resync shows it stopped and a start runs it.
    rig.fake.state().locked_pools.clear();
    manager.resync().await;
    assert_eq!(
        manager.record(workspace).unwrap().state,
        WorkspaceState::Stopped
    );
    assert_eq!(manager.start(workspace).await, ActionOutcome::Done);
    settles(&manager, workspace, WorkspaceState::Running).await;
    assert_eq!(rig.state_changes("mhq-maxhq"), ["start"]);
}

#[tokio::test]
async fn start_runs_the_probe_and_updates_in_place_when_no_agent_is_busy() {
    let rig = Rig::new();
    let mut keys = keys_for("mhq-maxhq", 1, true);
    keys.agents = vec![summary("a")];
    rig.add("mhq-maxhq", "Stopped", &keys);
    let manager = rig.manager();
    manager.rebuild().await;
    let workspace = workspace_of("mhq-maxhq");
    rig.hub.apply([Change::Upsert {
        value: agent("a", workspace, Activity::Busy),
    }]);

    // An agent is at work, so the update waits.
    assert_eq!(manager.start(workspace).await, ActionOutcome::Done);
    settles(&manager, workspace, WorkspaceState::Running).await;
    assert_eq!(
        rig.config_key("mhq-maxhq", key::RECIPE).as_deref(),
        Some("1")
    );
    let updated = ("mhq-maxhq".to_owned(), UPDATED.to_owned());
    assert!(!rig.guest.lock().unwrap().paths.contains(&updated));
    let record = manager.record(workspace).unwrap();
    assert!(matches!(record.egress, EgressCheck::Passed { .. }));
    assert_eq!(record.host_boot.as_deref(), Some(BOOT));

    assert_eq!(manager.stop(workspace).await, ActionOutcome::Done);
    settles(&manager, workspace, WorkspaceState::Stopped).await;
    assert!(matches!(
        manager.record(workspace).unwrap().guest,
        GuestState::Down { .. }
    ));
    assert!(rig.links.closed.lock().unwrap().contains(&workspace));

    // With the agent idle, the next start runs only the newer step.
    rig.hub.apply([Change::Upsert {
        value: agent("a", workspace, Activity::Idle),
    }]);
    assert_eq!(manager.start(workspace).await, ActionOutcome::Done);
    settles(&manager, workspace, WorkspaceState::Running).await;
    assert_eq!(
        rig.config_key("mhq-maxhq", key::RECIPE).as_deref(),
        Some("2")
    );
    assert_eq!(manager.record(workspace).unwrap().keys.recipe, 2);
    let guest = rig.guest.lock().unwrap();
    assert!(guest.paths.contains(&updated));
    assert!(!guest
        .paths
        .contains(&("mhq-maxhq".to_owned(), MARKER.to_owned())));
    drop(guest);

    // Never recreated, and the probe ran at each start.
    assert_eq!(
        rig.changes(),
        [
            "PUT /1.0/instances/mhq-maxhq/state",
            "PUT /1.0/instances/mhq-maxhq/state",
            "PUT /1.0/instances/mhq-maxhq/state",
            "PATCH /1.0/instances/mhq-maxhq",
        ]
    );
    let probes = rig
        .fake
        .state()
        .commands()
        .iter()
        .filter(|c| c.starts_with(&format!("{MAXD} guest probe")))
        .count();
    assert_eq!(probes, 2);
    assert_eq!(rig.state_changes("mhq-maxhq"), ["start", "stop", "start"]);
}

#[tokio::test]
async fn a_stop_answers_once_the_container_has_stopped() {
    let rig = Rig::new();
    rig.add("mhq-maxhq", "Stopped", &keys_for("mhq-maxhq", 2, true));
    let manager = rig.manager();
    manager.rebuild().await;
    let workspace = workspace_of("mhq-maxhq");
    assert_eq!(manager.start(workspace).await, ActionOutcome::Done);
    settles(&manager, workspace, WorkspaceState::Running).await;

    // The container takes its time to shut down, and the answer waits.
    rig.fake.state().state_delay = Duration::from_millis(200);
    assert_eq!(manager.stop(workspace).await, ActionOutcome::Done);
    assert_eq!(rig.fake.state().instances["mhq-maxhq"]["status"], "Stopped");
    assert_eq!(
        manager.record(workspace).unwrap().state,
        WorkspaceState::Stopped
    );
    assert!(!manager.changing(workspace));

    // So a start sent on the answer runs.
    assert_eq!(manager.start(workspace).await, ActionOutcome::Done);
    settles(&manager, workspace, WorkspaceState::Running).await;
    assert_eq!(rig.state_changes("mhq-maxhq"), ["start", "stop", "start"]);
}

#[tokio::test]
async fn a_start_sent_while_a_stop_runs_follows_it() {
    let rig = Rig::new();
    rig.add("mhq-maxhq", "Stopped", &keys_for("mhq-maxhq", 2, true));
    let manager = rig.manager_timed(|timing| timing.stop_answer = Duration::from_millis(20));
    manager.rebuild().await;
    let workspace = workspace_of("mhq-maxhq");
    assert_eq!(manager.start(workspace).await, ActionOutcome::Done);
    settles(&manager, workspace, WorkspaceState::Running).await;

    // The stop outlasts its answer, so the start comes while it runs.
    rig.fake.state().state_delay = Duration::from_millis(300);
    assert_eq!(manager.stop(workspace).await, ActionOutcome::Done);
    assert_eq!(
        manager.record(workspace).unwrap().state,
        WorkspaceState::Stopping
    );
    assert_eq!(manager.start(workspace).await, ActionOutcome::Done);
    settles(&manager, workspace, WorkspaceState::Running).await;
    assert_eq!(rig.state_changes("mhq-maxhq"), ["start", "stop", "start"]);

    // A start while another start runs still says to wait.
    rig.fake.state().instances.get_mut("mhq-maxhq").unwrap()["status"] = json!("Stopped");
    manager.set_state(workspace, WorkspaceState::Stopped);
    assert_eq!(manager.start(workspace).await, ActionOutcome::Done);
    let busy = refusal(manager.start(workspace).await);
    assert!(busy.contains("is already being changed"), "{busy}");
    settles(&manager, workspace, WorkspaceState::Running).await;
}

#[tokio::test]
async fn a_stop_that_fails_says_why() {
    let rig = Rig::new();
    rig.add("mhq-maxhq", "Stopped", &keys_for("mhq-maxhq", 2, true));
    let manager = rig.manager();
    manager.rebuild().await;
    let workspace = workspace_of("mhq-maxhq");
    assert_eq!(manager.start(workspace).await, ActionOutcome::Done);
    settles(&manager, workspace, WorkspaceState::Running).await;

    rig.fake.state().down = true;
    let reason = refusal(manager.stop(workspace).await);
    assert!(reason.starts_with("Stopping it failed: "), "{reason}");
    assert_eq!(
        manager.record(workspace).unwrap().state,
        WorkspaceState::Failed { reason }
    );
}

#[tokio::test]
async fn boot_starts_two_at_a_time() {
    let rig = Rig::with_links(FakeLinks {
        delay: Duration::from_millis(40),
        ..FakeLinks::default()
    });
    let marked = ["mhq-a", "mhq-b", "mhq-c", "mhq-d"];
    for name in marked {
        rig.add(name, "Stopped", &keys_for(name, 2, true));
    }
    rig.add("mhq-manual", "Stopped", &keys_for("mhq-manual", 2, false));
    rig.add("mhq-done", "Stopped", &keys_for("mhq-done", 2, true));
    let manager = rig.manager();
    let running = manager.rebuild().await;
    assert!(running.is_empty());
    // Someone stopped this one after it started in this boot.
    manager.update(workspace_of("mhq-done"), |record| {
        record.host_boot = Some(BOOT.to_owned());
    });

    manager.clone().boot(running).await;

    for name in marked {
        settles(&manager, workspace_of(name), WorkspaceState::Running).await;
        assert_eq!(rig.state_changes(name), ["start"], "{name}");
        assert_eq!(
            manager
                .record(workspace_of(name))
                .unwrap()
                .host_boot
                .as_deref(),
            Some(BOOT)
        );
    }
    assert_eq!(rig.links.most.load(Ordering::SeqCst), 2);
    for name in ["mhq-manual", "mhq-done"] {
        assert_eq!(
            manager.record(workspace_of(name)).unwrap().state,
            WorkspaceState::Stopped
        );
        assert!(rig.state_changes(name).is_empty(), "{name}");
    }

    // A second pass in the same boot starts nothing.
    manager.clone().boot(Vec::new()).await;
    for name in marked {
        assert_eq!(rig.state_changes(name), ["start"], "{name}");
    }
}

#[tokio::test]
async fn boot_checks_a_running_workspace_without_starting_it() {
    let rig = Rig::new();
    rig.add("mhq-maxhq", "Running", &keys_for("mhq-maxhq", 2, true));
    let manager = rig.manager();
    let running = manager.rebuild().await;
    let workspace = workspace_of("mhq-maxhq");
    assert_eq!(running, [workspace]);

    manager.clone().boot(running).await;
    settles(&manager, workspace, WorkspaceState::Running).await;
    assert!(rig.state_changes("mhq-maxhq").is_empty());
    assert!(rig
        .fake
        .state()
        .commands()
        .iter()
        .any(|c| c.starts_with(&format!("{MAXD} guest probe"))));
    assert_eq!(rig.links.opened.lock().unwrap().as_slice(), [workspace]);
}

#[tokio::test]
async fn boot_tries_again_until_the_storage_unlocks() {
    let rig = Rig::new();
    rig.add("mhq-maxhq", "Stopped", &keys_for("mhq-maxhq", 2, true));
    rig.fake.state().locked_pools.insert("agents".to_owned());
    let manager = rig.manager();
    let running = manager.rebuild().await;
    let workspace = workspace_of("mhq-maxhq");

    let boot = tokio::spawn(manager.clone().boot(running));
    // Several tries pass with the pool locked.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        manager.record(workspace).unwrap().state,
        WorkspaceState::StorageLocked
    );
    assert!(rig.state_changes("mhq-maxhq").is_empty());
    assert!(!boot.is_finished());

    rig.fake.state().locked_pools.clear();
    boot.await.unwrap();
    settles(&manager, workspace, WorkspaceState::Running).await;
    assert_eq!(rig.state_changes("mhq-maxhq"), ["start"]);
}

#[tokio::test]
async fn lifecycle_events_update_the_state() {
    let rig = Rig::new();
    rig.add("mhq-maxhq", "Stopped", &keys_for("mhq-maxhq", 2, true));
    let manager = rig.manager();
    manager.rebuild().await;
    let workspace = workspace_of("mhq-maxhq");
    tokio::spawn(manager.clone().watch_events());
    until("the event stream", || rig.fake.listeners() > 0).await;

    // Someone started it with the incus command.
    rig.fake.state().instances.get_mut("mhq-maxhq").unwrap()["status"] = json!("Running");
    rig.fake.send_event("mhq-maxhq", "instance-started");
    settles(&manager, workspace, WorkspaceState::Running).await;
    assert!(matches!(
        manager.record(workspace).unwrap().egress,
        EgressCheck::Passed { .. }
    ));

    rig.fake.state().instances.get_mut("mhq-maxhq").unwrap()["status"] = json!("Stopped");
    rig.fake.send_event("mhq-maxhq", "instance-stopped");
    settles(&manager, workspace, WorkspaceState::Stopped).await;

    rig.fake.state().instances.remove("mhq-maxhq");
    rig.fake.send_event("mhq-maxhq", "instance-deleted");
    forgotten(&manager, workspace).await;
    assert!(rig.hub.get(&EntityKey::Workspace(workspace)).is_none());
    assert!(rig.state_changes("mhq-maxhq").is_empty());
}

#[tokio::test]
async fn record_agents_writes_the_instance_key_or_the_folder_manifest() {
    let rig = Rig::new();
    rig.add("mhq-maxhq", "Running", &keys_for("mhq-maxhq", 2, true));
    rig.project(Some(REMOTE));
    let manager = rig.manager();
    manager.rebuild().await;
    let workspace = workspace_of("mhq-maxhq");
    let agents = vec![summary("a")];

    manager
        .record_agents(workspace, agents.clone())
        .await
        .unwrap();
    let written = rig.config_key("mhq-maxhq", key::AGENTS).unwrap();
    let read: Vec<AgentSummary> = serde_json::from_str(&written).unwrap();
    assert_eq!(read, agents);
    assert_eq!(manager.record(workspace).unwrap().keys.agents, agents);
    let container = manager.container(workspace).unwrap();
    assert_eq!(container.instance, "mhq-maxhq");
    assert_eq!(container.repo, REPO_DIR);

    let folder = WorkspaceId::derive("folder");
    rig.hub.apply([Change::Upsert {
        value: Entity::Workspace(Workspace {
            id: folder,
            project: Some(project_id()),
            device: device_id(),
            location: WorkspaceLocation::Folder {
                path: "/Users/max/Code/MaxHQ".to_owned(),
            },
            managed: None,
            clone: None,
        }),
    }]);
    manager.record_agents(folder, agents.clone()).await.unwrap();
    let manifest = manifest::read(&rig.data(), folder).unwrap().unwrap();
    assert_eq!(manifest.workspace, folder);
    assert_eq!(manifest.device, device_id());
    assert_eq!(manifest.project, Some(project_id()));
    assert_eq!(manifest.title.as_deref(), Some("MaxHQ"));
    assert_eq!(manifest.repo.as_deref(), Some(REMOTE));
    assert_eq!(manifest.path, "/Users/max/Code/MaxHQ");
    assert_eq!(manifest.agents, agents);
    assert!(manager.container(folder).is_none());

    let error = manager
        .record_agents(WorkspaceId::derive("nowhere"), agents)
        .await
        .unwrap_err();
    assert_eq!(error, "That workspace isn't on this device.");
}

#[tokio::test]
async fn workflow_variables_you_allowed_outlast_a_save_until_you_turn_them_off() {
    let rig = Rig::new();
    let manager = rig.manager();
    let allowed = AllowedWorkflowEnv {
        vars: vec![AllowedWorkflowVar {
            name: "EXAMPLE_MODE".to_owned(),
            digest: "d1".to_owned(),
        }],
        at: Timestamp(1),
    };
    let kept = |manager: &Manager| manager.project_settings(project_id()).workflow_env_allowed;
    Workspaces::allow_workflow_env(&manager, project_id(), allowed.clone()).unwrap();
    assert_eq!(kept(&manager), Some(allowed.clone()));
    assert_eq!(
        rig.db.project_settings().unwrap()[0].workflow_env_allowed,
        Some(allowed.clone())
    );
    assert_eq!(
        rig.hub.get(&EntityKey::ProjectSettings {
            project: project_id(),
            device: device_id(),
        }),
        Some(Entity::ProjectSettings(
            manager.project_settings(project_id())
        ))
    );

    // A save from a client keeps what maxd kept, whatever the client sent.
    let mut settings = manager.project_settings(project_id());
    settings.resume_on_restart = false;
    settings.workflow_env_allowed = None;
    assert_eq!(
        manager.set_project_settings(settings.clone()).await,
        ActionOutcome::Done
    );
    assert_eq!(kept(&manager), Some(allowed.clone()));
    assert!(!manager.project_settings(project_id()).resume_on_restart);
    settings.workflow_env_allowed = Some(AllowedWorkflowEnv {
        vars: Vec::new(),
        at: Timestamp(2),
    });
    assert_eq!(
        manager.set_project_settings(settings.clone()).await,
        ActionOutcome::Done
    );
    assert_eq!(kept(&manager), Some(allowed.clone()));

    // Turning them off forgets it, and nothing is kept while they're off.
    settings.workflow_env = false;
    assert_eq!(
        manager.set_project_settings(settings.clone()).await,
        ActionOutcome::Done
    );
    assert_eq!(kept(&manager), None);
    assert_eq!(
        rig.db.project_settings().unwrap()[0].workflow_env_allowed,
        None
    );
    assert!(Workspaces::allow_workflow_env(&manager, project_id(), allowed.clone()).is_err());
    settings.workflow_env = true;
    assert_eq!(
        manager.set_project_settings(settings).await,
        ActionOutcome::Done
    );
    assert_eq!(kept(&manager), None);
}

#[tokio::test]
async fn project_settings_are_saved_shown_and_reach_running_containers() {
    let rig = Rig::new();
    rig.add("mhq-maxhq", "Running", &keys_for("mhq-maxhq", 2, true));
    let manager = rig.manager();
    manager.rebuild().await;
    let workspace = workspace_of("mhq-maxhq");
    manager.start(workspace).await;
    settles(&manager, workspace, WorkspaceState::Running).await;

    assert!(manager.project_settings(project_id()).resume_on_restart);
    let settings = ProjectSettings {
        project: project_id(),
        device: device_id(),
        resume_on_restart: false,
        ignored_ports: vec![5173],
        git_author: Some(GitAuthor {
            name: "Ada".to_owned(),
            email: "ada@example.com".to_owned(),
        }),
        auto_settle_days: None,
        settle_on_merge: true,
        workflow_env: true,
        memory_soft: None,
        workflow_env_allowed: None,
    };
    let elsewhere = ProjectSettings {
        device: DeviceId::derive("mac"),
        ..settings.clone()
    };
    assert_eq!(
        refusal(manager.set_project_settings(elsewhere).await),
        "These settings belong to another device."
    );

    assert_eq!(
        manager.set_project_settings(settings.clone()).await,
        ActionOutcome::Done
    );
    assert_eq!(manager.project_settings(project_id()), settings);
    assert_eq!(
        rig.db.project_settings().unwrap(),
        std::slice::from_ref(&settings)
    );
    assert_eq!(
        rig.hub.get(&EntityKey::ProjectSettings {
            project: project_id(),
            device: device_id(),
        }),
        Some(Entity::ProjectSettings(settings.clone()))
    );
    until("the git author", || {
        rig.fake.state().execs.iter().any(|call| {
            call.user == 1000
                && call.command.ends_with(&[
                    REPO_DIR.to_owned(),
                    "Ada".to_owned(),
                    "ada@example.com".to_owned(),
                ])
        })
    })
    .await;

    // A new maxd has them from the database.
    let again = rig.manager();
    assert_eq!(again.project_settings(project_id()), settings);
}

#[tokio::test]
async fn install_pushes_the_binary_links_it_and_restarts_the_guest() {
    let rig = Rig::new();
    rig.add("mhq-maxhq", "Running", &keys_for("mhq-maxhq", 2, true));
    let incus = Incus::new(&rig.fake.socket);
    let name = InstanceName::new("mhq-maxhq").unwrap();
    let path = rig.dir.path().join("maxd-guest");

    super::install::install(&incus, &name, &path).await.unwrap();
    {
        let state = rig.fake.state();
        let commands = state.commands();
        assert_eq!(commands[0], "mkdir -p /usr/local/lib/maxhq");
        assert!(commands[1].ends_with(&format!(
            "sh /usr/local/lib/maxhq/maxd.new /usr/local/lib/maxhq/maxd {MAXD}"
        )));
        assert!(
            commands[2].ends_with("sh /etc/systemd/system/maxhq-guest.service maxhq-guest.service")
        );
        let pushed = &state.files[&(
            "mhq-maxhq".to_owned(),
            "/usr/local/lib/maxhq/maxd.new".to_owned(),
        )];
        assert_eq!(pushed.bytes, b"guest maxd");
        assert_eq!(pushed.owner, ("0".to_owned(), "0".to_owned()));
    }

    // A missing binary fails, and an unmarked container gets nothing.
    let missing = super::install::install(&incus, &name, &rig.dir.path().join("nope")).await;
    assert!(missing.unwrap_err().to_string().contains("no guest maxd"));
    rig.fake.state().add("mhq-plain", "Running", &[]);
    let plain = InstanceName::new("mhq-plain").unwrap();
    assert!(super::install::install(&incus, &plain, &path)
        .await
        .is_err());
    assert!(!rig
        .fake
        .state()
        .files
        .keys()
        .any(|(instance, _)| instance == "mhq-plain"));
}

#[tokio::test]
async fn a_test_host_is_built_from_its_steps_and_left_at_its_snapshot() {
    let rig = Rig::new();
    rig.place();
    let manager = rig.manager();
    let name = InstanceName::new("mhq-test-linux").unwrap();

    manager
        .build_test_host(&name, "linux", &test_steps(0))
        .await
        .unwrap();
    assert_eq!(
        rig.config_key("mhq-test-linux", MANAGED_KEY).as_deref(),
        Some("true")
    );
    assert_eq!(
        rig.config_key("mhq-test-linux", TEST_HOST_KEY).as_deref(),
        Some("linux")
    );
    assert_eq!(
        rig.config_key("mhq-test-linux", "limits.cpu").as_deref(),
        Some("4")
    );
    assert_eq!(
        rig.config_key("mhq-test-linux", "limits.memory.enforce")
            .as_deref(),
        Some("soft")
    );
    assert!(rig
        .guest
        .lock()
        .unwrap()
        .paths
        .contains(&("mhq-test-linux".to_owned(), UPDATED.to_owned())));
    assert!(
        rig.guest.lock().unwrap().targets.is_some(),
        "the egress check ran"
    );
    assert_eq!(rig.state_changes("mhq-test-linux"), ["start", "stop"]);
    // Its clean snapshot is all it keeps, so Incus takes none of its own.
    assert_eq!(rig.config_key("mhq-test-linux", "snapshots.schedule"), None);
    assert_eq!(
        rig.changes().last().map(String::as_str),
        Some("POST /1.0/instances/mhq-test-linux/snapshots")
    );
    assert_eq!(
        *rig.links.closed.lock().unwrap(),
        [manager.test_link(&name)]
    );

    // It's no workspace, so the rebuild passes over it.
    assert!(manager.rebuild().await.is_empty());
    assert!(manager.shared().records.is_empty());

    // A lease starts it and waits for its guest.
    manager.start_test_host(&name).await.unwrap();
    assert_eq!(
        *rig.links.opened.lock().unwrap(),
        [manager.test_link(&name)]
    );
    let incus = Incus::new(&rig.fake.socket);
    incus
        .push_file(&name, "/home/agent/left", b"x".to_vec(), 0o644)
        .await
        .unwrap();

    // Cleaning stops it and takes it back to the snapshot.
    manager.clean_test_host(&name).await.unwrap();
    assert_eq!(
        rig.state_changes("mhq-test-linux"),
        ["start", "stop", "start", "stop"]
    );
    assert_eq!(
        rig.changes().last().map(String::as_str),
        Some("PUT /1.0/instances/mhq-test-linux")
    );
    assert!(!rig
        .fake
        .state()
        .files
        .contains_key(&("mhq-test-linux".to_owned(), "/home/agent/left".to_owned())));

    // A second build replaces it.
    manager
        .build_test_host(&name, "linux", &test_steps(0))
        .await
        .unwrap();
    assert!(rig
        .changes()
        .contains(&"DELETE /1.0/instances/mhq-test-linux".to_owned()));
}

#[tokio::test]
async fn a_test_host_leaves_an_instance_it_didnt_make() {
    let rig = Rig::new();
    rig.place();
    let manager = rig.manager();
    let name = InstanceName::new("mhq-test-linux").unwrap();
    rig.add(
        "mhq-test-linux",
        "Running",
        &keys_for("mhq-test-linux", 2, true),
    );
    rig.fake.state().add("mhq-plain", "Stopped", &[]);
    let plain = InstanceName::new("mhq-plain").unwrap();

    for result in [
        manager
            .build_test_host(&name, "linux", &test_steps(0))
            .await,
        manager.start_test_host(&name).await,
        manager.clean_test_host(&name).await,
        manager.clean_test_host(&plain).await,
    ] {
        assert!(result.unwrap_err().contains("isn't a test host"));
    }
    assert!(rig.changes().is_empty());

    // Nothing there to start or clean.
    let gone = InstanceName::new("mhq-gone").unwrap();
    assert!(manager
        .start_test_host(&gone)
        .await
        .unwrap_err()
        .contains("Build it again"));
}

#[test]
fn slugs_make_container_names() {
    assert_eq!(slug("MaxHQ"), "maxhq");
    assert_eq!(slug("My Cool  App!"), "my-cool-app");
    assert_eq!(slug("--"), "workspace");
    assert_eq!(slug("émoji 🚀 app"), "moji-app");
    assert_eq!(slug(&"a".repeat(70)), "a".repeat(59));
    assert_eq!(slug(&format!("{}-b", "a".repeat(58))), "a".repeat(58));
    for title in ["MaxHQ", "My Cool  App!", "--", &"a".repeat(70)] {
        let name = format!("mhq-{}", slug(title));
        assert!(InstanceName::new(&name).is_ok(), "{name}");
    }
}

const GIB: u64 = 1 << 30;

/// Caps with a parent cgroup under `root`, a temp tree standing in for
/// `/sys/fs/cgroup`, and a 3 GiB soft limit for each workspace.
fn capped(root: &std::path::Path) -> super::Caps {
    super::Caps {
        spec: maxhq_core::host_spec::Caps {
            parent: Some("agent-workspaces".to_owned()),
            memory_high: Some(maxhq_core::host_spec::Size(12 * GIB)),
            cpu_max: Some(8),
            workspace_memory: Some(maxhq_core::host_spec::Size(3 * GIB)),
            ..maxhq_core::host_spec::Caps::default()
        },
        snapshots: maxhq_core::host_spec::Snapshots::default(),
        root: root.to_owned(),
    }
}

/// Makes the parent cgroup in `root`, with its limits set.
fn make_parent(root: &std::path::Path) {
    let parent = root.join("agent-workspaces");
    std::fs::create_dir_all(&parent).unwrap();
    std::fs::write(parent.join("memory.high"), "12884901888\n").unwrap();
    std::fs::write(parent.join("cpu.max"), "800000 100000\n").unwrap();
}

fn shown(rig: &Rig, workspace: WorkspaceId) -> ManagedWorkspace {
    match rig.hub.get(&EntityKey::Workspace(workspace)) {
        Some(Entity::Workspace(shown)) => shown.managed.expect("a managed workspace"),
        other => panic!("not shown: {other:?}"),
    }
}

#[tokio::test]
async fn a_create_runs_it_under_the_parent_with_soft_memory_and_no_cpu_limit() {
    let rig = Rig::new();
    rig.place();
    rig.project(Some(REMOTE));
    let cgroups = TempDir::new();
    make_parent(cgroups.path());
    let manager = rig.manager_capped(capped(cgroups.path()));

    let workspace = workspace_of("mhq-maxhq");
    let outcome = manager.create(rig.spec(None)).await;
    assert_eq!(outcome, ActionOutcome::Created { workspace });
    settles(&manager, workspace, WorkspaceState::Running).await;

    assert_eq!(
        rig.config_key("mhq-maxhq", "raw.lxc").as_deref(),
        Some(
            "lxc.cgroup.dir.container = agent-workspaces/mhq-maxhq\n\
             lxc.cgroup.dir.monitor = agent-workspaces.monitor/mhq-maxhq\n"
        )
    );
    assert_eq!(
        rig.config_key("mhq-maxhq", "limits.memory").as_deref(),
        Some("3GiB")
    );
    assert_eq!(
        rig.config_key("mhq-maxhq", "limits.memory.enforce")
            .as_deref(),
        Some("soft")
    );
    assert_eq!(rig.config_key("mhq-maxhq", "limits.cpu"), None);
    assert_eq!(shown(&rig, workspace).memory_soft, Some(3 * GIB));
}

#[tokio::test]
async fn a_create_or_start_refuses_without_the_parent_cgroup() {
    let rig = Rig::new();
    rig.place();
    rig.project(Some(REMOTE));
    rig.add("mhq-other", "Stopped", &keys_for("mhq-other", 2, true));
    let cgroups = TempDir::new();
    let manager = rig.manager_capped(capped(cgroups.path()));
    manager.rebuild().await;

    let refusal = ActionOutcome::Refused {
        reason: "The workspace cgroup isn't set up on this device. Run maxd host check.".to_owned(),
    };
    assert_eq!(manager.create(rig.spec(None)).await, refusal);
    assert!(!rig.fake.state().instances.contains_key("mhq-maxhq"));

    let other = workspace_of("mhq-other");
    assert_eq!(manager.start(other).await, refusal);
    assert!(rig.state_changes("mhq-other").is_empty());
    assert_eq!(
        manager.record(other).unwrap().state,
        WorkspaceState::Stopped
    );
}

#[tokio::test]
async fn new_caps_wait_for_the_next_start_and_a_running_one_never_restarts() {
    let rig = Rig::new();
    // M3's guard limits, from before the spec named a parent.
    rig.add("mhq-maxhq", "Running", &keys_for("mhq-maxhq", 2, true));
    let cgroups = TempDir::new();
    make_parent(cgroups.path());
    let manager = rig.manager_capped(capped(cgroups.path()));
    let running = manager.rebuild().await;
    let workspace = workspace_of("mhq-maxhq");
    assert_eq!(shown(&rig, workspace).memory_soft, Some(8 * GIB));

    // Boot finds it running, so its keys stay until it stops.
    manager.clone().boot(running).await;
    settles(&manager, workspace, WorkspaceState::Running).await;
    assert!(rig.state_changes("mhq-maxhq").is_empty());
    assert!(rig.changes().is_empty());
    assert_eq!(
        rig.config_key("mhq-maxhq", "limits.cpu").as_deref(),
        Some("4")
    );

    // The next start puts them in first.
    assert_eq!(manager.stop(workspace).await, ActionOutcome::Done);
    settles(&manager, workspace, WorkspaceState::Stopped).await;
    assert_eq!(manager.start(workspace).await, ActionOutcome::Done);
    settles(&manager, workspace, WorkspaceState::Running).await;
    assert_eq!(rig.state_changes("mhq-maxhq"), ["stop", "start"]);
    let key = |name: &str| rig.config_key("mhq-maxhq", name).unwrap_or_default();
    assert_eq!(key("limits.cpu"), "");
    assert_eq!(key("limits.memory"), "3GiB");
    assert!(key("raw.lxc").contains("agent-workspaces/mhq-maxhq"));
    assert_eq!(shown(&rig, workspace).memory_soft, Some(3 * GIB));
    let patches = rig
        .changes()
        .into_iter()
        .filter(|change| change == "PATCH /1.0/instances/mhq-maxhq")
        .count();
    assert_eq!(patches, 1);

    // A start with every key in place leaves them be.
    assert_eq!(manager.stop(workspace).await, ActionOutcome::Done);
    settles(&manager, workspace, WorkspaceState::Stopped).await;
    assert_eq!(manager.start(workspace).await, ActionOutcome::Done);
    settles(&manager, workspace, WorkspaceState::Running).await;
    let patches = rig
        .changes()
        .into_iter()
        .filter(|change| change == "PATCH /1.0/instances/mhq-maxhq")
        .count();
    assert_eq!(patches, 1);
}

#[tokio::test]
async fn the_latest_unsaved_report_shows_and_a_stopped_workspace_keeps_it() {
    use maxhq_protocol::guest::GuestUnsaved;

    let rig = Rig::new();
    rig.add("mhq-maxhq", "Stopped", &keys_for("mhq-maxhq", 2, true));
    let manager = rig.manager();
    manager.rebuild().await;
    let workspace = workspace_of("mhq-maxhq");
    assert_eq!(manager.start(workspace).await, ActionOutcome::Done);
    settles(&manager, workspace, WorkspaceState::Running).await;
    assert_eq!(shown(&rig, workspace).unsaved, None);

    let checkout = |path: &str, changed, commits, no_upstream| GuestUnsaved {
        path: path.to_owned(),
        changed,
        commits,
        no_upstream,
    };
    manager.set_unsaved(
        workspace,
        &[
            checkout("/home/agent/work/ws-alpha", 3, 0, false),
            checkout("/home/agent/work/.worktrees/fix-x", 1, 2, true),
        ],
    );
    let first = shown(&rig, workspace).unsaved.expect("a summary");
    assert_eq!((first.changed, first.commits, first.no_upstream), (4, 2, 1));

    // A later report replaces it.
    manager.set_unsaved(
        workspace,
        &[checkout("/home/agent/work/ws-alpha", 1, 0, false)],
    );
    let latest = shown(&rig, workspace).unsaved.expect("a summary");
    assert_eq!(
        (latest.changed, latest.commits, latest.no_upstream),
        (1, 0, 0)
    );
    assert!(latest.read_at >= first.read_at);

    // Stopped, and after maxd restarts, it keeps the last one, whose read
    // time says how old it is.
    assert_eq!(manager.stop(workspace).await, ActionOutcome::Done);
    settles(&manager, workspace, WorkspaceState::Stopped).await;
    assert_eq!(shown(&rig, workspace).unsaved, Some(latest));
    let again = rig.manager();
    again.rebuild().await;
    assert_eq!(again.record(workspace).unwrap().unsaved, Some(latest));
}

#[tokio::test]
async fn with_no_parent_a_start_leaves_an_m3_container_s_keys_alone() {
    let rig = Rig::new();
    rig.add("mhq-maxhq", "Stopped", &keys_for("mhq-maxhq", 2, true));
    let manager = rig.manager();
    manager.rebuild().await;
    let workspace = workspace_of("mhq-maxhq");
    assert_eq!(manager.start(workspace).await, ActionOutcome::Done);
    settles(&manager, workspace, WorkspaceState::Running).await;

    assert!(!rig
        .changes()
        .contains(&"PATCH /1.0/instances/mhq-maxhq".to_owned()));
    assert_eq!(
        rig.config_key("mhq-maxhq", "limits.cpu").as_deref(),
        Some("4")
    );
    assert_eq!(
        rig.config_key("mhq-maxhq", "limits.memory").as_deref(),
        Some("8GiB")
    );
    assert_eq!(rig.config_key("mhq-maxhq", "raw.lxc"), None);
    assert_eq!(shown(&rig, workspace).memory_soft, Some(8 * GIB));
}

#[tokio::test]
async fn a_project_s_memory_replaces_the_default_in_either_scheme() {
    let settings = |memory: u64| ProjectSettings {
        project: project_id(),
        device: device_id(),
        resume_on_restart: true,
        ignored_ports: Vec::new(),
        git_author: None,
        auto_settle_days: None,
        settle_on_merge: true,
        workflow_env: true,
        memory_soft: Some(memory),
        workflow_env_allowed: None,
    };

    // Under a parent.
    let rig = Rig::new();
    rig.place();
    rig.project(Some(REMOTE));
    let cgroups = TempDir::new();
    make_parent(cgroups.path());
    let manager = rig.manager_capped(capped(cgroups.path()));
    manager.set_project_settings(settings(5 * GIB)).await;
    manager.create(rig.spec(None)).await;
    let workspace = workspace_of("mhq-maxhq");
    settles(&manager, workspace, WorkspaceState::Running).await;
    assert_eq!(
        rig.config_key("mhq-maxhq", "limits.memory").as_deref(),
        Some("5GiB")
    );
    assert_eq!(rig.config_key("mhq-maxhq", "limits.cpu"), None);
    assert_eq!(shown(&rig, workspace).memory_soft, Some(5 * GIB));

    // With no parent, it replaces only the guard's 8 GiB, at the next
    // start.
    let rig = Rig::new();
    rig.add("mhq-maxhq", "Stopped", &keys_for("mhq-maxhq", 2, true));
    let manager = rig.manager();
    manager.rebuild().await;
    manager.set_project_settings(settings(2 * GIB)).await;
    assert_eq!(manager.start(workspace).await, ActionOutcome::Done);
    settles(&manager, workspace, WorkspaceState::Running).await;
    assert_eq!(
        rig.config_key("mhq-maxhq", "limits.memory").as_deref(),
        Some("2GiB")
    );
    assert_eq!(
        rig.config_key("mhq-maxhq", "limits.cpu").as_deref(),
        Some("4")
    );
    assert_eq!(rig.config_key("mhq-maxhq", "raw.lxc"), None);
}

/// The writes that touched `instance` itself, as "METHOD target".
fn writes_to(rig: &Rig, instance: &str) -> Vec<String> {
    let own = format!("/1.0/instances/{instance}");
    rig.changes()
        .into_iter()
        .filter(|change| {
            let target = change.split_once(' ').map_or("", |(_, target)| target);
            target == own || target.starts_with(&format!("{own}/"))
        })
        .collect()
}

#[tokio::test]
async fn the_snapshot_keys_go_on_at_create_and_a_start_puts_back_only_what_differs() {
    let rig = Rig::new();
    rig.place();
    rig.project(Some(REMOTE));
    let manager = rig.manager();
    let workspace = workspace_of("mhq-maxhq");
    manager.create(rig.spec(None)).await;
    settles(&manager, workspace, WorkspaceState::Running).await;
    let key = |name: &str| rig.config_key("mhq-maxhq", name);
    assert_eq!(key("snapshots.schedule").as_deref(), Some("@hourly"));
    assert_eq!(key("snapshots.expiry").as_deref(), Some("7d"));
    assert_eq!(key("snapshots.schedule.stopped").as_deref(), Some("false"));
    assert_eq!(
        key("snapshots.pattern").as_deref(),
        Some("auto-{{ creation_date|date:'2006-01-02-1504' }}")
    );

    // A container from before the keys gets them at its next start.
    let rig = Rig::new();
    rig.add("mhq-older", "Stopped", &keys_for("mhq-older", 2, true));
    for name in ["snapshots.schedule", "snapshots.expiry"] {
        let mut state = rig.fake.state();
        let instance = state.instances.get_mut("mhq-older").unwrap();
        instance["config"].as_object_mut().unwrap().remove(name);
    }
    let manager = rig.manager_with(|config| {
        config.caps.snapshots.schedule = "@daily".to_owned();
    });
    manager.rebuild().await;
    let older = workspace_of("mhq-older");
    assert_eq!(manager.start(older).await, ActionOutcome::Done);
    settles(&manager, older, WorkspaceState::Running).await;
    let patch = rig
        .fake
        .state()
        .log
        .iter()
        .find(|l| l.method == "PATCH" && l.target == "/1.0/instances/mhq-older")
        .map(|l| l.body.clone())
        .expect("the start set the keys");
    assert_eq!(
        patch,
        json!({ "config": { "snapshots.schedule": "@daily", "snapshots.expiry": "7d" } })
    );
}

#[tokio::test]
async fn restore_as_new_makes_a_workspace_and_touches_nothing_existing() {
    let rig = Rig::new();
    rig.add("mhq-maxhq", "Running", &keys_for("mhq-maxhq", 2, true));
    let manager = rig.manager();
    let running = manager.rebuild().await;
    manager.clone().boot(running).await;
    let workspace = workspace_of("mhq-maxhq");
    settles(&manager, workspace, WorkspaceState::Running).await;
    let original = InstanceName::new("mhq-maxhq").unwrap();
    let incus = manager.incus().clone();
    incus
        .push_file(&original, "/kept", b"then".to_vec(), 0o644)
        .await
        .unwrap();
    incus
        .snapshot(&original, "auto-2026-11-02-1400")
        .await
        .unwrap();
    incus
        .push_file(&original, "/kept", b"now".to_vec(), 0o644)
        .await
        .unwrap();
    let before = writes_to(&rig, "mhq-maxhq");
    let instance_before = rig.fake.state().instances["mhq-maxhq"].clone();

    // A snapshot it doesn't have.
    let reason = refusal(
        manager
            .restore_snapshot(workspace, "auto-gone".to_owned(), true, false)
            .await,
    );
    assert!(reason.contains("no snapshot called auto-gone"), "{reason}");

    let restored = workspace_of("mhq-maxhq-restored");
    let outcome = manager
        .restore_snapshot(workspace, "auto-2026-11-02-1400".to_owned(), true, false)
        .await;
    assert_eq!(
        outcome,
        ActionOutcome::Created {
            workspace: restored
        }
    );
    settles(&manager, restored, WorkspaceState::Running).await;

    // The copy has the files then, its own keys and its own socket.
    {
        let state = rig.fake.state();
        let file = |instance: &str| {
            state.files[&(instance.to_owned(), "/kept".to_owned())]
                .bytes
                .clone()
        };
        assert_eq!(file("mhq-maxhq-restored"), b"then");
        assert_eq!(file("mhq-maxhq"), b"now");
        let copy = &state.instances["mhq-maxhq-restored"];
        assert_eq!(
            copy["config"][key::WORKSPACE].as_str(),
            Some(restored.to_string().as_str())
        );
        assert_eq!(copy["config"][key::AGENTS].as_str(), Some("[]"));
        let socket = manager.socket(&InstanceName::new("mhq-maxhq-restored").unwrap());
        assert_eq!(
            copy["devices"]["maxhq-guest"]["listen"],
            format!("unix:{}", socket.display())
        );
        assert_eq!(state.instances["mhq-maxhq"], instance_before);
    }
    assert_eq!(writes_to(&rig, "mhq-maxhq"), before);
    assert_eq!(
        manager.record(workspace).map(|r| r.state),
        Some(WorkspaceState::Running)
    );
    assert_eq!(rig.state_changes("mhq-maxhq-restored"), ["start"]);

    // The rebuild sees two workspaces, and a second copy gets the next
    // name.
    manager.rebuild().await;
    assert!(manager.record(workspace).is_some() && manager.record(restored).is_some());
    let again = manager
        .restore_snapshot(workspace, "auto-2026-11-02-1400".to_owned(), true, false)
        .await;
    let second = workspace_of("mhq-maxhq-restored-2");
    assert_eq!(again, ActionOutcome::Created { workspace: second });
    settles(&manager, second, WorkspaceState::Running).await;
}

#[tokio::test]
async fn roll_back_takes_a_named_snapshot_first_and_stops_on_unsaved_work_until_confirmed() {
    use maxhq_protocol::guest::GuestUnsaved;

    let rig = Rig::new();
    rig.add("mhq-maxhq", "Stopped", &keys_for("mhq-maxhq", 2, true));
    let manager = rig.manager();
    manager.rebuild().await;
    let workspace = workspace_of("mhq-maxhq");
    assert_eq!(manager.start(workspace).await, ActionOutcome::Done);
    settles(&manager, workspace, WorkspaceState::Running).await;
    let name = InstanceName::new("mhq-maxhq").unwrap();
    let incus = manager.incus().clone();
    incus
        .push_file(&name, "/kept", b"then".to_vec(), 0o644)
        .await
        .unwrap();
    incus.snapshot(&name, "auto-2026-11-02-1400").await.unwrap();
    incus
        .push_file(&name, "/kept", b"now".to_vec(), 0o644)
        .await
        .unwrap();
    let back =
        || manager.restore_snapshot(workspace, "auto-2026-11-02-1400".to_owned(), false, false);

    // No report yet, then work that isn't saved: it stops each time.
    let reason = refusal(back().await);
    assert!(reason.contains("hasn't heard"), "{reason}");
    manager.set_unsaved(
        workspace,
        &[GuestUnsaved {
            path: REPO_DIR.to_owned(),
            changed: 2,
            commits: 1,
            no_upstream: false,
        }],
    );
    let reason = refusal(back().await);
    assert!(
        reason.contains("2 changed files not committed, 1 commit not pushed"),
        "{reason}"
    );
    assert_eq!(rig.state_changes("mhq-maxhq"), ["start"]);
    assert_eq!(
        rig.fake.state().files[&("mhq-maxhq".to_owned(), "/kept".to_owned())].bytes,
        b"now"
    );

    // Confirmed, it goes: stop, a named snapshot, the restore, a start.
    let outcome = manager
        .restore_snapshot(workspace, "auto-2026-11-02-1400".to_owned(), false, true)
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    settles(&manager, workspace, WorkspaceState::Running).await;
    assert_eq!(rig.state_changes("mhq-maxhq"), ["start", "stop", "start"]);
    let order: Vec<String> = writes_to(&rig, "mhq-maxhq")
        .into_iter()
        .filter(|w| w.ends_with("/snapshots") || w == "PUT /1.0/instances/mhq-maxhq")
        .skip(1)
        .collect();
    assert_eq!(
        order,
        [
            "POST /1.0/instances/mhq-maxhq/snapshots",
            "PUT /1.0/instances/mhq-maxhq"
        ]
    );
    {
        let state = rig.fake.state();
        assert_eq!(
            state.files[&("mhq-maxhq".to_owned(), "/kept".to_owned())].bytes,
            b"then"
        );
        let (_, before) = state
            .snapshots
            .iter()
            .find(|((_, snapshot), _)| snapshot.starts_with("mhq-before-rollback-"))
            .expect("the named snapshot");
        assert_eq!(before.files["/kept"].bytes, b"now");
        // The keys stay this workspace's.
        let config = &state.instances["mhq-maxhq"]["config"];
        assert_eq!(
            config[key::WORKSPACE].as_str(),
            Some(workspace.to_string().as_str())
        );
    }

    // The named one is kept for good, and the list is oldest first.
    let ActionOutcome::Snapshots { snapshots } = manager.list_snapshots(workspace).await else {
        panic!("expected the snapshots");
    };
    assert_eq!(snapshots[0].name, "auto-2026-11-02-1400");
    assert!(snapshots[1].name.starts_with("mhq-before-rollback-"));
    assert_eq!(snapshots[1].expires_at, None);

    // With nothing unsaved it needs no confirm.
    manager.set_unsaved(
        workspace,
        &[GuestUnsaved {
            path: REPO_DIR.to_owned(),
            changed: 0,
            commits: 0,
            no_upstream: false,
        }],
    );
    assert_eq!(back().await, ActionOutcome::Done);
    settles(&manager, workspace, WorkspaceState::Running).await;
}

#[tokio::test]
async fn snapshots_are_only_for_workspaces_maxd_made() {
    let rig = Rig::new();
    rig.fake.state().add("ws-alpha", "Running", &[]);
    let manager = rig.manager();
    manager.rebuild().await;
    let workspace = workspace_of("ws-alpha");
    let reason = refusal(manager.list_snapshots(workspace).await);
    assert!(reason.contains("no workspace like that"), "{reason}");
    let reason = refusal(
        manager
            .restore_snapshot(workspace, "auto-1".to_owned(), false, true)
            .await,
    );
    assert!(reason.contains("no workspace like that"), "{reason}");
    assert!(rig.changes().is_empty());
}

#[tokio::test]
async fn a_confirmed_roll_back_over_unsaved_work_takes_the_named_snapshot_first() {
    use maxhq_protocol::guest::GuestUnsaved;

    let rig = Rig::new();
    rig.add("mhq-maxhq", "Stopped", &keys_for("mhq-maxhq", 2, true));
    let manager = rig.manager();
    manager.rebuild().await;
    let workspace = workspace_of("mhq-maxhq");
    assert_eq!(manager.start(workspace).await, ActionOutcome::Done);
    settles(&manager, workspace, WorkspaceState::Running).await;
    let name = InstanceName::new("mhq-maxhq").unwrap();
    manager.incus().snapshot(&name, "auto-1").await.unwrap();
    manager.set_unsaved(
        workspace,
        &[GuestUnsaved {
            path: REPO_DIR.to_owned(),
            changed: 1,
            commits: 0,
            no_upstream: true,
        }],
    );
    let outcome = manager
        .restore_snapshot(workspace, "auto-1".to_owned(), false, true)
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    settles(&manager, workspace, WorkspaceState::Running).await;

    let state = rig.fake.state();
    let snapshot =
        |l: &&fake::Logged| l.method == "POST" && l.target == "/1.0/instances/mhq-maxhq/snapshots";
    let taken: Vec<&fake::Logged> = state.log.iter().filter(snapshot).collect();
    let before = taken.last().expect("the named snapshot");
    let name = before.body["name"].as_str().unwrap_or_default();
    assert!(name.starts_with("mhq-before-rollback-"), "{name}");
    assert_eq!(before.body["expires_at"], crate::incus::NEVER);
    let at = |pick: &dyn Fn(&fake::Logged) -> bool| state.log.iter().position(pick);
    let snapshotted = at(&|l| l.body["name"].as_str() == Some(name)).unwrap();
    let restored = at(&|l| l.method == "PUT" && l.body["restore"] == "auto-1").unwrap();
    let stopped = at(&|l| l.body["action"] == "stop").unwrap();
    assert!(stopped < snapshotted && snapshotted < restored);
}
