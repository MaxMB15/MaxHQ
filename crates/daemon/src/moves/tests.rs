// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Whole moves against the Incus fake, with a host that only records what
//! it was asked.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use maxhq_protocol::hosts::{MoveState, MoveStep};
use maxhq_protocol::ids::{BootId, DeviceId, MoveId, WorkspaceId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{Device, Vendor};
use maxhq_protocol::time::Timestamp;
use serde_json::json;

use super::record::Found;
use super::*;
use crate::incus::fake::{FakeIncus, Pushed, State};
use crate::incus::moves::{AUTOSTART, MOVE_SNAPSHOT, OLD_REPO};
use crate::incus::{InstanceName, MANAGED_KEY};
use crate::observe::incus::look::{SESSION_PATH, STATUS_PATH};

const SESSION: &str = "5d0c6a52-3a1e-4c8e-9d1b-7f2a9e0c1b3d";
const REPO_DIR: &str = "/home/agent/widgets";
const HEAD: &str = "/home/agent/widgets/.git/HEAD";

fn device() -> Device {
    Device {
        id: DeviceId::derive("devbox"),
        name: "devbox".into(),
        os: "linux".into(),
        boot_id: BootId("boot-1".into()),
        maxd_version: "0.1.0".into(),
        winding_down_until: None,
        workspace_host: None,
        build: None,
        target: None,
        wsl: None,
    }
}

fn file(text: &str) -> Pushed {
    Pushed {
        kind: "file".into(),
        mode: "0644".into(),
        owner: ("1000".into(), "1000".into()),
        bytes: text.as_bytes().to_vec(),
    }
}

/// A container the old scripts made, with an idle Claude session.
fn old_container(state: &mut State, name: &str) {
    state.add(name, "Running", &[(AUTOSTART, "true")]);
    let instance = state.instances.get_mut(name).unwrap();
    instance["profiles"] = json!(["default", "agent-ws"]);
    instance["devices"] = json!({ "root": { "type": "disk", "path": "/", "pool": "default" } });
    instance["expanded_devices"] = json!({
        "root": { "type": "disk", "path": "/", "pool": "default" },
        "eth0": { "type": "nic", "network": "agentbr0" },
        "creds": { "type": "disk", "source": "agent-creds", "path": "/opt/agent-creds" },
    });
    let files = [
        (
            SESSION_PATH,
            json!({ "agent": "claude", "session_id": SESSION, "cwd": REPO_DIR,
                    "updated": "2026-11-02T08:00:00+00:00" })
            .to_string(),
        ),
        (
            STATUS_PATH,
            json!({ "agent": "claude", "status": "idle", "event": "Stop" }).to_string(),
        ),
        (
            "/home/agent/widgets/.git/config",
            "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = https://github.com/example-user/widgets.git\n"
                .to_owned(),
        ),
        (HEAD, "ref: refs/heads/main\n".to_owned()),
    ];
    for (path, text) in files {
        state
            .files
            .insert((name.to_owned(), path.to_owned()), file(&text));
    }
    state.disk_used.insert(name.to_owned(), 4_000_000_000);
}

/// What the host was asked.
#[derive(Default)]
struct Asked {
    calls: Vec<String>,
    resumed: Vec<Resume>,
    unsaved: Vec<String>,
    /// Loses the snapshot after the setup, so the copy fails.
    lose_snapshot: bool,
}

struct FakeHost {
    asked: Mutex<Asked>,
    sockets: PathBuf,
    incus: Incus,
}

impl FakeHost {
    fn asked(&self) -> std::sync::MutexGuard<'_, Asked> {
        self.asked.lock().unwrap()
    }

    fn note(&self, call: &str) {
        self.asked().calls.push(call.to_owned());
    }
}

impl Host for FakeHost {
    fn copy_setup<'a>(&'a self, record: &'a Record) -> BoxFuture<'a, Result<CopySetup, String>> {
        Box::pin(async move {
            self.note("copy_setup");
            if self.asked().lose_snapshot {
                self.incus
                    .drop_snapshot_for_move(&record.source, MOVE_SNAPSHOT, record)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            let workspace = WorkspaceId::derive(&record.target);
            let config = BTreeMap::from([
                ("user.maxhq.workspace".to_owned(), workspace.to_string()),
                (AUTOSTART.to_owned(), "false".to_owned()),
            ]);
            Ok(CopySetup {
                workspace,
                config,
                network: "agentbr0".into(),
                guest_socket: self.sockets.join(format!("{}.sock", record.target)),
            })
        })
    }

    fn converge<'a>(&'a self, _: &'a Record) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.note("converge");
            Ok(())
        })
    }

    /// Deletes a copy made before, as the real host does.
    fn drop_copy<'a>(&'a self, record: &'a Record) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.note("drop_copy");
            let name = InstanceName::new(&record.target).map_err(|e| e.to_string())?;
            if self
                .incus
                .instance(&name)
                .await
                .map_err(|e| e.to_string())?
                .is_some()
            {
                self.incus.delete(&name).await.map_err(|e| e.to_string())?;
            }
            Ok(())
        })
    }

    fn runs_with(&self, _: Vendor) -> String {
        "in Full access mode, on the account Personal".to_owned()
    }

    fn resume(&self, resume: Resume) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            self.note("resume");
            self.asked().resumed.push(resume);
            Ok(())
        })
    }

    fn stop_agents(&self, _: WorkspaceId) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            self.note("stop_agents");
            Ok(())
        })
    }

    fn unsaved(&self, _: WorkspaceId) -> BoxFuture<'_, Vec<String>> {
        Box::pin(async move { self.asked().unsaved.clone() })
    }
}

struct Rig {
    fake: FakeIncus,
    host: Arc<FakeHost>,
    mover: Mover,
    now: Arc<AtomicU64>,
    dir: PathBuf,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const START: u64 = 1_800_000_000_000;

fn rig(names: &[&str]) -> Rig {
    let mut state = State::default();
    for name in names {
        old_container(&mut state, name);
    }
    let fake = FakeIncus::start(state);
    let dir = std::env::temp_dir().join(format!("maxd-moves-{}", uuid::Uuid::now_v7()));
    let sockets = dir.join("guests");
    std::fs::create_dir_all(&sockets).unwrap();
    let host = Arc::new(FakeHost {
        asked: Mutex::default(),
        sockets,
        incus: Incus::new(&fake.socket),
    });
    let now = Arc::new(AtomicU64::new(START));
    let clock = now.clone();
    let mover = Mover::open(Config {
        incus: Incus::new(&fake.socket),
        hub: Hub::new(device(), HashMap::new()),
        db: Db::open_in_memory().unwrap(),
        device: device().id,
        host: host.clone(),
        resume_flags: dir.join("resume"),
        uptime: dir.join("uptime"),
        state_timeout: Duration::from_secs(5),
        now: Arc::new(move || Timestamp(clock.load(Ordering::SeqCst))),
    });
    Rig {
        fake,
        host,
        mover,
        now,
        dir,
    }
}

impl Rig {
    fn id(&self, source: &str) -> MoveId {
        MoveId::derive(&format!("move:{}:{source}", device().id))
    }

    fn record(&self, source: &str) -> Record {
        self.mover.record(self.id(source)).expect("the move")
    }

    fn plan(&self, source: &str) {
        assert_eq!(self.mover.plan(source), ActionOutcome::Done);
    }

    /// Runs a step and waits for it to end. Says why it was refused, if it
    /// was.
    async fn run(&self, source: &str, step: MoveStep) -> Result<Record, String> {
        let outcome = self.mover.run_step(self.id(source), step).await;
        self.ended(source, outcome).await
    }

    async fn back(&self, source: &str) -> Result<Record, String> {
        let outcome = self.mover.move_back(self.id(source)).await;
        self.ended(source, outcome).await
    }

    async fn retire(&self, source: &str) -> Result<Record, String> {
        let outcome = self.mover.retire(self.id(source)).await;
        self.ended(source, outcome).await
    }

    async fn ended(&self, source: &str, outcome: ActionOutcome) -> Result<Record, String> {
        if let ActionOutcome::Refused { reason } = outcome {
            return Err(reason);
        }
        for _ in 0..500 {
            let record = self.record(source);
            match &record.state {
                MoveState::Running { .. } => tokio::time::sleep(Duration::from_millis(10)).await,
                MoveState::Failed { reason } => return Err(format!("failed: {reason}")),
                _ => return Ok(record),
            }
        }
        panic!("the step didn't end");
    }

    /// Runs the move up to the keep.
    async fn switched(&self, source: &str) -> Record {
        self.plan(source);
        for step in [
            MoveStep::Look,
            MoveStep::Copy,
            MoveStep::Converge,
            MoveStep::Switch,
        ] {
            self.run(source, step)
                .await
                .unwrap_or_else(|reason| panic!("{step:?}: {reason}"));
        }
        self.record(source)
    }

    fn status(&self, name: &str) -> String {
        self.fake.state().instances[name]["status"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn autostart(&self, name: &str) -> String {
        self.fake.state().instances[name]["config"][AUTOSTART]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    fn change(&self, source: &str, path: &str, text: &str) {
        self.fake
            .state()
            .files
            .insert((source.to_owned(), path.to_owned()), file(text));
    }

    /// Every write the original took, as `METHOD tail`.
    fn writes_to(&self, source: &str) -> Vec<String> {
        let prefix = format!("/1.0/instances/{source}");
        self.fake
            .state()
            .writes()
            .into_iter()
            .filter_map(|logged| {
                let tail = logged.target.strip_prefix(&prefix)?;
                (tail.is_empty() || tail.starts_with('/'))
                    .then(|| format!("{} {tail}", logged.method))
            })
            .collect()
    }
}

#[tokio::test]
async fn a_move_runs_each_step_you_approve() {
    let rig = rig(&["ws-alpha"]);
    rig.plan("ws-alpha");
    assert_eq!(rig.record("ws-alpha").target, "mhq-alpha");

    let looked = rig.run("ws-alpha", MoveStep::Look).await.unwrap();
    assert_eq!(looked.step, MoveStep::Copy);
    assert_eq!(looked.found.vendor, Some(Vendor::Claude));
    assert_eq!(looked.found.session.as_deref(), Some(SESSION));
    assert_eq!(
        looked.found.repo.as_deref(),
        Some("https://github.com/example-user/widgets.git")
    );
    assert_eq!(looked.found.pool.as_deref(), Some("default"));
    assert_eq!(looked.found.autostart, "true");
    let plan = looked.plan.unwrap();
    assert_eq!(plan.look.repo_path.as_deref(), Some(REPO_DIR));
    assert_eq!(plan.look.agent_state.as_deref(), Some("idle"));
    assert_eq!(plan.look.disk_used, Some(4_000_000_000));
    assert!(plan.look.profiles.contains(&"agent-ws".to_owned()));
    assert!(!plan.look.resume_flag);
    assert_eq!(plan.steps.len(), 5);
    // The look only reads.
    assert!(rig.writes_to("ws-alpha").is_empty());

    let copied = rig.run("ws-alpha", MoveStep::Copy).await.unwrap();
    assert_eq!(copied.step, MoveStep::Converge);
    assert_eq!(copied.workspace, Some(WorkspaceId::derive("mhq-alpha")));
    {
        let state = rig.fake.state();
        assert!(state
            .snapshots
            .contains_key(&("ws-alpha".into(), MOVE_SNAPSHOT.into())));
        let copy = &state.instances["mhq-alpha"];
        assert_eq!(copy["status"], "Stopped");
        assert_eq!(copy["profiles"], json!([]));
        assert_eq!(copy["config"][crate::incus::MANAGED_KEY], "true");
        assert_eq!(copy["config"][AUTOSTART], "false");
        assert_eq!(copy["devices"]["root"]["pool"], "default");
        assert_eq!(copy["devices"]["eth0"]["network"], "agentbr0");
        assert!(copy["devices"]["maxhq-guest"].is_object());
        // The copy has the original's files as they were.
        assert!(state.files.contains_key(&("mhq-alpha".into(), HEAD.into())));
    }
    assert_eq!(rig.status("ws-alpha"), "Running");

    let converged = rig.run("ws-alpha", MoveStep::Converge).await.unwrap();
    assert_eq!(converged.step, MoveStep::Switch);

    let switched = rig.run("ws-alpha", MoveStep::Switch).await.unwrap();
    assert_eq!(switched.step, MoveStep::Keep);
    assert_eq!(switched.state, MoveState::Ready);
    assert_eq!(rig.status("ws-alpha"), "Stopped");
    assert_eq!(rig.autostart("ws-alpha"), "false");
    assert_eq!(
        switched.kept_until,
        Some(Timestamp(START + KEEP.as_millis() as u64))
    );
    assert_eq!(
        rig.host.asked().resumed,
        [Resume {
            workspace: WorkspaceId::derive("mhq-alpha"),
            vendor: Vendor::Claude,
            session: SESSION.into(),
            cwd: "/home/agent/work/widgets".into(),
        }]
    );
    assert_eq!(
        rig.host.asked().calls,
        ["drop_copy", "copy_setup", "converge", "resume"]
    );

    // The original took only the move's own writes, and no command ran in
    // it.
    assert_eq!(
        rig.writes_to("ws-alpha"),
        [
            "DELETE /snapshots/mhq-move",
            "POST /snapshots",
            "PATCH ",
            "PUT /state",
        ]
    );
    assert!(rig.fake.state().execs.is_empty());
    // The move is published.
    let published = rig
        .mover
        .config()
        .hub
        .get(&maxhq_protocol::state::EntityKey::Move(switched.id));
    assert!(matches!(published, Some(Entity::Move(found)) if found.step == MoveStep::Keep));
}

#[tokio::test]
async fn a_step_out_of_order_is_refused() {
    let rig = rig(&["ws-alpha"]);
    assert!(rig
        .mover
        .run_step(rig.id("ws-alpha"), MoveStep::Look)
        .await
        .is_refused());
    rig.plan("ws-alpha");
    let early = rig.run("ws-alpha", MoveStep::Copy).await.unwrap_err();
    assert_eq!(early, "The copy can't run yet. The look comes first.");
    rig.run("ws-alpha", MoveStep::Look).await.unwrap();
    for step in [MoveStep::Converge, MoveStep::Switch, MoveStep::Retire] {
        let early = rig.run("ws-alpha", step).await.unwrap_err();
        assert!(early.ends_with("The copy comes first."), "{early}");
    }
    let back = rig.back("ws-alpha").await.unwrap_err();
    assert!(back.contains("Switch first"), "{back}");
    let retire = rig.retire("ws-alpha").await.unwrap_err();
    assert!(retire.contains("Switch first"), "{retire}");
    rig.run("ws-alpha", MoveStep::Copy).await.unwrap();
    let early = rig.run("ws-alpha", MoveStep::Switch).await.unwrap_err();
    assert!(early.ends_with("The setup comes first."), "{early}");
    // Nothing the refusals asked for reached the original.
    assert_eq!(
        rig.writes_to("ws-alpha"),
        ["DELETE /snapshots/mhq-move", "POST /snapshots"]
    );
    assert!(rig.mover.plan("mhq-alpha").is_refused());
}

#[tokio::test]
async fn a_change_since_the_snapshot_asks_for_a_copy_again() {
    let rig = rig(&["ws-alpha"]);
    rig.plan("ws-alpha");
    for step in [MoveStep::Look, MoveStep::Copy, MoveStep::Converge] {
        rig.run("ws-alpha", step).await.unwrap();
    }
    rig.change("ws-alpha", HEAD, "0123456789abcdef\n");
    let changed = rig.run("ws-alpha", MoveStep::Switch).await.unwrap_err();
    assert_eq!(
        changed,
        "ws-alpha changed since the copy. Copy again first, then switch."
    );
    let record = rig.record("ws-alpha");
    assert!(record.source_changed);
    assert!(record.entity().source_changed);
    assert_eq!(rig.status("ws-alpha"), "Running");

    let again = rig.run("ws-alpha", MoveStep::Copy).await.unwrap();
    assert_eq!(again.step, MoveStep::Converge);
    assert!(!again.source_changed);
    assert_eq!(
        rig.fake.state().files[&("mhq-alpha".to_owned(), HEAD.to_owned())].bytes,
        b"0123456789abcdef\n"
    );
    rig.run("ws-alpha", MoveStep::Converge).await.unwrap();
    rig.run("ws-alpha", MoveStep::Switch).await.unwrap();
    assert_eq!(
        rig.host.asked().calls,
        [
            "drop_copy",
            "copy_setup",
            "converge",
            "drop_copy",
            "copy_setup",
            "converge",
            "resume"
        ]
    );
}

#[tokio::test]
async fn the_switch_waits_for_an_idle_agent_and_the_old_resume() {
    let rig = rig(&["ws-alpha"]);
    rig.plan("ws-alpha");
    for step in [MoveStep::Look, MoveStep::Copy, MoveStep::Converge] {
        rig.run("ws-alpha", step).await.unwrap();
    }
    rig.change(
        "ws-alpha",
        STATUS_PATH,
        &json!({ "agent": "claude", "status": "working" }).to_string(),
    );
    let busy = rig.run("ws-alpha", MoveStep::Switch).await.unwrap_err();
    assert!(busy.contains("at work"), "{busy}");
    rig.change(
        "ws-alpha",
        STATUS_PATH,
        &json!({ "agent": "claude", "status": "idle" }).to_string(),
    );
    let flags = rig.dir.join("resume");
    std::fs::create_dir_all(&flags).unwrap();
    std::fs::write(flags.join("ws-alpha"), "").unwrap();
    let flagged = rig.run("ws-alpha", MoveStep::Switch).await.unwrap_err();
    assert!(flagged.contains("resume ws-alpha"), "{flagged}");
    std::fs::remove_file(flags.join("ws-alpha")).unwrap();
    assert_eq!(rig.status("ws-alpha"), "Running");
    rig.run("ws-alpha", MoveStep::Switch).await.unwrap();
}

#[tokio::test]
async fn a_move_back_restarts_the_original_and_a_second_switch_works() {
    let rig = rig(&["ws-alpha"]);
    rig.switched("ws-alpha").await;
    rig.host.asked().unsaved = vec!["1 changed file not committed".into()];

    let back = rig.back("ws-alpha").await.unwrap();
    assert_eq!(back.state, MoveState::MovedBack);
    assert_eq!(back.unsaved, ["1 changed file not committed"]);
    assert_eq!(back.kept_until, None);
    assert_eq!(rig.status("ws-alpha"), "Running");
    assert_eq!(rig.autostart("ws-alpha"), "true");
    assert!(rig
        .host
        .asked()
        .calls
        .ends_with(&["stop_agents".to_owned()]));
    let over = rig.run("ws-alpha", MoveStep::Copy).await.unwrap_err();
    assert!(over.contains("Plan it again"), "{over}");

    // Planned again, the same move starts over and switches a second time.
    rig.host.asked().unsaved.clear();
    let second = rig.switched("ws-alpha").await;
    assert_eq!(second.id, back.id);
    assert_eq!(second.step, MoveStep::Keep);
    assert!(second.unsaved.is_empty());
    assert_eq!(rig.status("ws-alpha"), "Stopped");
    assert_eq!(rig.autostart("ws-alpha"), "false");
    assert_eq!(rig.host.asked().resumed.len(), 2);
}

#[tokio::test]
async fn a_retire_shows_unsaved_work_first() {
    let rig = rig(&["ws-alpha", "ws-bravo"]);
    rig.host.asked().unsaved = vec!["2 commits not pushed".into()];
    rig.switched("ws-alpha").await;
    rig.host.asked().unsaved.clear();

    let warned = rig.retire("ws-alpha").await.unwrap_err();
    assert_eq!(
        warned,
        "ws-alpha has 2 commits not pushed. Retiring deletes it. Save what you need first, \
         or retire again to delete it anyway."
    );
    assert_eq!(rig.record("ws-alpha").unsaved, ["2 commits not pushed"]);
    assert!(rig.fake.state().instances.contains_key("ws-alpha"));

    let retired = rig.retire("ws-alpha").await.unwrap();
    assert_eq!(retired.state, MoveState::Retired);
    {
        let state = rig.fake.state();
        assert!(!state.instances.contains_key("ws-alpha"));
        assert!(!state
            .snapshots
            .contains_key(&("ws-alpha".into(), MOVE_SNAPSHOT.into())));
        assert!(state.instances.contains_key("mhq-alpha"));
    }
    // ws-bravo is still there, so the checklist waits.
    assert!(retired.checklist.is_empty());
    assert!(rig.mover.plan("ws-alpha").is_refused());
    assert!(rig.back("ws-alpha").await.is_err());
}

#[tokio::test]
async fn the_last_retire_lists_what_is_left_by_hand() {
    let rig = rig(&["ws-alpha"]);
    rig.switched("ws-alpha").await;
    let retired = rig.retire("ws-alpha").await.unwrap();
    assert_eq!(retired.checklist, plan::CHECKLIST);
    assert_eq!(rig.record("ws-alpha").entity().checklist.len(), 9);
    assert!(retired
        .checklist
        .iter()
        .any(|line| line.contains(".git-credentials") && line.contains(".git/config")));
}

/// What the plan says the copy step does, for an original whose git config
/// has `origin` at `remote`.
async fn converge_changes(remote: &str) -> Vec<String> {
    let rig = rig(&["ws-alpha"]);
    rig.fake.state().files.insert(
        (
            "ws-alpha".to_owned(),
            "/home/agent/widgets/.git/config".to_owned(),
        ),
        file(&format!("[remote \"origin\"]\n\turl = {remote}\n")),
    );
    rig.plan("ws-alpha");
    let looked = rig.run("ws-alpha", MoveStep::Look).await.unwrap();
    looked
        .plan
        .unwrap()
        .steps
        .into_iter()
        .find(|step| step.step == MoveStep::Converge)
        .unwrap()
        .changes
}

#[tokio::test]
async fn the_plan_names_the_repo_a_copy_gets_github_access_to() {
    for remote in [
        "https://github.com/someone-else/secrets.git",
        "git@github.com:someone-else/secrets.git",
    ] {
        let changes = converge_changes(remote).await;
        assert!(
            changes.iter().any(|change| change
                == "Gives mhq-alpha GitHub write access to \
                    https://github.com/someone-else/secrets.git, the origin remote in ws-alpha's \
                    .git/config or the repo the old scripts named on it, and puts it in that \
                    repo's project. Anything running in ws-alpha can change that file, so check \
                    it's the repo you expect."),
            "{changes:?}"
        );
    }
}

#[tokio::test]
async fn a_session_with_no_git_config_in_its_folder_finds_the_repo_the_old_scripts_named() {
    for cwd in [
        "/home/agent",
        "/home/agent/widgets/app",
        "/home/agent/widgets-two",
    ] {
        let rig = rig(&["ws-alpha"]);
        {
            let mut state = rig.fake.state();
            state.instances.get_mut("ws-alpha").unwrap()["expanded_config"][OLD_REPO] =
                json!("example-user/widgets");
            state.files.insert(
                ("ws-alpha".to_owned(), SESSION_PATH.to_owned()),
                file(
                    &json!({ "agent": "claude", "session_id": SESSION, "cwd": cwd,
                             "updated": "2026-11-02T08:00:00+00:00" })
                    .to_string(),
                ),
            );
        }
        rig.plan("ws-alpha");
        let looked = rig.run("ws-alpha", MoveStep::Look).await.unwrap();
        assert_eq!(
            looked.found.repo.as_deref(),
            Some("example-user/widgets"),
            "{cwd}"
        );
        let changes = looked
            .plan
            .unwrap()
            .steps
            .into_iter()
            .find(|step| step.step == MoveStep::Converge)
            .unwrap()
            .changes;
        assert!(
            changes.iter().any(|change| change
                .contains("GitHub write access to https://github.com/example-user/widgets.git")),
            "{cwd}: {changes:?}"
        );
    }
}

#[tokio::test]
async fn a_session_in_a_linked_checkout_copies() {
    const LINKED_HEAD: &str = "/home/agent/widgets/.git/worktrees/widgets-two/HEAD";
    let rig = rig(&["ws-alpha"]);
    {
        let mut state = rig.fake.state();
        state.instances.get_mut("ws-alpha").unwrap()["expanded_config"][OLD_REPO] =
            json!("example-user/widgets");
        for (path, text) in [
            (
                SESSION_PATH,
                json!({ "agent": "claude", "session_id": SESSION,
                        "cwd": "/home/agent/widgets-two",
                        "updated": "2026-11-02T08:00:00+00:00" })
                .to_string(),
            ),
            (
                "/home/agent/widgets-two/.git",
                "gitdir: /home/agent/widgets/.git/worktrees/widgets-two\n".to_owned(),
            ),
            (LINKED_HEAD, "ref: refs/heads/two\n".to_owned()),
        ] {
            state
                .files
                .insert(("ws-alpha".to_owned(), path.to_owned()), file(&text));
        }
    }
    rig.plan("ws-alpha");
    let looked = rig.run("ws-alpha", MoveStep::Look).await.unwrap();
    assert_eq!(looked.found.repo.as_deref(), Some("example-user/widgets"));

    let copied = rig.run("ws-alpha", MoveStep::Copy).await.unwrap();
    assert_eq!(copied.step, MoveStep::Converge);
    assert_eq!(copied.marks["/home/agent/widgets-two/.git/HEAD"], None);
    assert!(copied.marks[LINKED_HEAD].is_some());
}

#[tokio::test]
async fn a_look_runs_again_until_there_is_a_copy() {
    let rig = rig(&["ws-alpha"]);
    rig.plan("ws-alpha");
    rig.run("ws-alpha", MoveStep::Look).await.unwrap();
    rig.fake.state().files.insert(
        (
            "ws-alpha".to_owned(),
            "/home/agent/widgets/.git/config".to_owned(),
        ),
        file("[remote \"origin\"]\n\turl = https://github.com/example-user/gadgets.git\n"),
    );
    let looked = rig.run("ws-alpha", MoveStep::Look).await.unwrap();
    assert_eq!(looked.step, MoveStep::Copy);
    assert_eq!(
        looked.found.repo.as_deref(),
        Some("https://github.com/example-user/gadgets.git")
    );

    rig.run("ws-alpha", MoveStep::Copy).await.unwrap();
    let again = rig.run("ws-alpha", MoveStep::Look).await.unwrap_err();
    assert_eq!(again, "The look ran already.");
}

#[tokio::test]
async fn the_origin_remote_outranks_the_repo_the_old_scripts_named() {
    let rig = rig(&["ws-alpha"]);
    rig.fake.state().instances.get_mut("ws-alpha").unwrap()["expanded_config"][OLD_REPO] =
        json!("example-user/renamed-since");
    rig.plan("ws-alpha");
    let looked = rig.run("ws-alpha", MoveStep::Look).await.unwrap();
    assert_eq!(
        looked.found.repo.as_deref(),
        Some("https://github.com/example-user/widgets.git")
    );
}

#[tokio::test]
async fn the_plan_shows_nothing_of_a_remote_it_cant_read_as_github() {
    for remote in [
        "https://someone:hunter2@github.com/example-user/widgets.git",
        "https://example.com/hunter2/widgets.git",
    ] {
        let changes = converge_changes(remote).await;
        assert!(
            changes.iter().any(|change| change
                == "Finds no GitHub repo in ws-alpha's origin remote, so mhq-alpha gets no \
                    GitHub access."),
            "{changes:?}"
        );
        assert!(!changes.join(" ").contains("hunter2"), "{changes:?}");
    }
}

#[tokio::test]
async fn the_original_is_kept_for_seven_days() {
    let rig = rig(&["ws-alpha"]);
    rig.switched("ws-alpha").await;
    let keep = rig.run("ws-alpha", MoveStep::Keep).await.unwrap_err();
    assert!(keep.contains("kept until"), "{keep}");

    let day = 24 * 60 * 60 * 1000;
    rig.now.store(START + 7 * day - 60_000, Ordering::SeqCst);
    rig.mover.keep_ended();
    assert_eq!(rig.record("ws-alpha").step, MoveStep::Keep);
    rig.now.store(START + 7 * day, Ordering::SeqCst);
    rig.mover.keep_ended();
    let ended = rig.record("ws-alpha");
    assert_eq!(ended.step, MoveStep::Retire);
    assert_eq!(ended.state, MoveState::Ready);
    // Nothing is deleted until you say so.
    assert!(rig.fake.state().instances.contains_key("ws-alpha"));
    // You can still go back after the keep ends, until the retire runs.
    let retired = rig.run("ws-alpha", MoveStep::Retire).await.unwrap();
    assert_eq!(retired.state, MoveState::Retired);
    assert!(!rig.fake.state().instances.contains_key("ws-alpha"));
}

#[tokio::test]
async fn a_step_maxd_stopped_in_fails_when_it_opens() {
    let rig = rig(&["ws-alpha"]);
    rig.plan("ws-alpha");
    let mut record = rig.record("ws-alpha");
    record.state = MoveState::Running {
        doing: "Copying".into(),
    };
    record.approved = Some(Approval::Step(MoveStep::Copy));
    let db = rig.mover.config().db.clone();
    db.put_move(record.id, &record).unwrap();
    let reopened = Mover::open(Config {
        incus: Incus::new(&rig.fake.socket),
        hub: Hub::new(device(), HashMap::new()),
        db,
        device: device().id,
        host: rig.host.clone(),
        resume_flags: rig.dir.join("resume"),
        uptime: rig.dir.join("uptime"),
        state_timeout: Duration::from_secs(5),
        now: Arc::new(|| Timestamp(START)),
    });
    let opened = reopened.record(record.id).unwrap();
    assert_eq!(
        opened.state,
        MoveState::Failed {
            reason: "maxd stopped while it was copying".into()
        }
    );
    assert_eq!(opened.approved, None);
}

#[tokio::test]
async fn copying_again_after_a_move_back_warns_about_the_copys_work() {
    let rig = rig(&["ws-alpha"]);
    rig.switched("ws-alpha").await;
    rig.host.asked().unsaved = vec!["3 commits not pushed".into()];
    rig.back("ws-alpha").await.unwrap();
    rig.plan("ws-alpha");
    rig.run("ws-alpha", MoveStep::Look).await.unwrap();

    let warned = rig.run("ws-alpha", MoveStep::Copy).await.unwrap_err();
    assert_eq!(
        warned,
        "mhq-alpha holds 3 commits not pushed from while the agent ran there. Copying again \
         deletes it. Save it first, or copy again to delete it anyway."
    );
    let record = rig.record("ws-alpha");
    assert_eq!(record.step, MoveStep::Copy);
    assert_eq!(record.state, MoveState::Ready);
    assert_eq!(record.unsaved, ["3 commits not pushed"]);
    assert!(rig.fake.state().instances.contains_key("mhq-alpha"));
    assert!(!rig.host.asked().calls.ends_with(&["drop_copy".to_owned()]));

    // The second approval goes ahead and makes a fresh copy.
    let copied = rig.run("ws-alpha", MoveStep::Copy).await.unwrap();
    assert_eq!(copied.step, MoveStep::Converge);
    assert_eq!(copied.warned, None);
    assert!(!copied.copy_used);
    // That work went with the old copy, so the card stops listing it.
    assert!(copied.unsaved.is_empty());
    // A copy the agent never ran in holds nothing of its own to warn about.
    rig.change("ws-alpha", HEAD, "0123456789abcdef\n");
    rig.run("ws-alpha", MoveStep::Converge).await.unwrap();
    assert!(rig.run("ws-alpha", MoveStep::Switch).await.is_err());
    rig.run("ws-alpha", MoveStep::Copy).await.unwrap();
}

#[tokio::test]
async fn a_retire_lists_the_snapshots_that_go_with_the_original() {
    let rig = rig(&["ws-alpha"]);
    rig.switched("ws-alpha").await;
    {
        let mut state = rig.fake.state();
        let taken = state.snapshots[&("ws-alpha".to_owned(), MOVE_SNAPSHOT.to_owned())].clone();
        for name in ["before-upgrade", "nightly"] {
            state
                .snapshots
                .insert(("ws-alpha".to_owned(), name.to_owned()), taken.clone());
        }
    }
    let warned = rig.retire("ws-alpha").await.unwrap_err();
    assert_eq!(
        warned,
        "Its own snapshots go with it: before-upgrade, nightly. Retiring deletes it. Save what \
         you need first, or retire again to delete it anyway."
    );
    assert_eq!(rig.record("ws-alpha").state, MoveState::Ready);
    assert!(rig.fake.state().instances.contains_key("ws-alpha"));

    let retired = rig.run("ws-alpha", MoveStep::Retire).await;
    // The keep hasn't ended, so the step itself waits; RetireMove goes.
    assert!(retired.is_err());
    let retired = rig.retire("ws-alpha").await.unwrap();
    assert_eq!(retired.state, MoveState::Retired);
    assert!(!rig.fake.state().instances.contains_key("ws-alpha"));
}

#[tokio::test]
async fn two_requests_at_once_run_the_step_once() {
    let rig = rig(&["ws-alpha"]);
    rig.plan("ws-alpha");
    for step in [MoveStep::Look, MoveStep::Copy, MoveStep::Converge] {
        rig.run("ws-alpha", step).await.unwrap();
    }
    let id = rig.id("ws-alpha");
    let (first, second) = tokio::join!(
        rig.mover.run_step(id, MoveStep::Switch),
        rig.mover.run_step(id, MoveStep::Switch)
    );
    let outcomes = [first, second];
    assert_eq!(
        outcomes
            .iter()
            .filter(|o| **o == ActionOutcome::Done)
            .count(),
        1,
        "{outcomes:?}"
    );
    assert!(outcomes
        .iter()
        .any(|o| matches!(o, ActionOutcome::Refused { reason } if reason.contains("busy"))));
    rig.ended("ws-alpha", ActionOutcome::Done).await.unwrap();
    assert_eq!(rig.host.asked().resumed.len(), 1);
    assert_eq!(rig.record("ws-alpha").step, MoveStep::Keep);

    // A move back and a retire at once: only one goes.
    let (back, retire) = tokio::join!(rig.mover.move_back(id), rig.mover.retire(id));
    assert!(
        back.is_refused() != retire.is_refused(),
        "{back:?} {retire:?}"
    );
}

#[tokio::test]
async fn a_refused_switch_gives_its_claim_back() {
    let rig = rig(&["ws-alpha"]);
    rig.plan("ws-alpha");
    for step in [MoveStep::Look, MoveStep::Copy, MoveStep::Converge] {
        rig.run("ws-alpha", step).await.unwrap();
    }
    rig.change("ws-alpha", HEAD, "0123456789abcdef\n");
    assert!(rig.run("ws-alpha", MoveStep::Switch).await.is_err());
    let record = rig.record("ws-alpha");
    assert_eq!(record.state, MoveState::Ready);
    assert_eq!(record.approved, None);
    assert!(record.source_changed);
}

trait Refused {
    fn is_refused(&self) -> bool;
}

impl Refused for ActionOutcome {
    fn is_refused(&self) -> bool {
        matches!(self, ActionOutcome::Refused { .. })
    }
}

#[tokio::test]
async fn a_copy_leaves_an_instance_that_isnt_its_own() {
    let other = WorkspaceId::derive("someone-else").to_string();
    let keys: [&[(&str, &str)]; 2] = [
        &[(MANAGED_KEY, "true")],
        &[
            (MANAGED_KEY, "true"),
            ("user.maxhq.workspace", other.as_str()),
        ],
    ];
    for keys in keys {
        let rig = rig(&["ws-alpha"]);
        rig.plan("ws-alpha");
        rig.run("ws-alpha", MoveStep::Look).await.unwrap();
        rig.fake.state().add("mhq-alpha", "Running", keys);

        let refused = rig.run("ws-alpha", MoveStep::Copy).await.unwrap_err();
        assert!(refused.contains("isn't this move's copy"), "{refused}");
        assert_eq!(rig.status("mhq-alpha"), "Running");
        let asked = rig.host.asked().calls.clone();
        assert!(asked.is_empty(), "{asked:?}");
    }
}

#[tokio::test]
async fn a_copy_that_failed_part_way_can_go_again() {
    let rig = rig(&["ws-alpha"]);
    rig.plan("ws-alpha");
    rig.run("ws-alpha", MoveStep::Look).await.unwrap();
    rig.host.asked().lose_snapshot = true;
    let failed = rig.run("ws-alpha", MoveStep::Copy).await.unwrap_err();
    assert!(failed.contains("Snapshot"), "{failed}");

    // What a copy that broke later would leave behind.
    let ours = WorkspaceId::derive("mhq-alpha").to_string();
    rig.fake.state().add(
        "mhq-alpha",
        "Stopped",
        &[(MANAGED_KEY, "true"), ("user.maxhq.workspace", &ours)],
    );
    rig.host.asked().lose_snapshot = false;
    let again = rig.run("ws-alpha", MoveStep::Copy).await.unwrap();
    assert_eq!(again.step, MoveStep::Converge);
}

#[test]
fn the_repo_folder_stays_in_the_agents_home() {
    let in_cwd = |cwd: &str| {
        plan::repo_path(&Found {
            cwd: Some(cwd.into()),
            ..Found::default()
        })
    };
    assert_eq!(in_cwd(REPO_DIR).as_deref(), Some(REPO_DIR));
    assert_eq!(in_cwd("/home/agent/../../etc"), None);
    assert_eq!(in_cwd("/home/agentx/foo"), None);
    let from_url = |repo: &str| {
        plan::repo_path(&Found {
            repo: Some(repo.into()),
            ..Found::default()
        })
    };
    assert_eq!(
        from_url("https://github.com/example-user/widgets.git").as_deref(),
        Some(REPO_DIR)
    );
    assert_eq!(from_url("https://github.com/example-user/.."), None);
}

#[tokio::test]
async fn an_odd_session_file_names_nothing_the_move_acts_on() {
    let rig = rig(&["ws-alpha"]);
    rig.plan("ws-alpha");
    rig.change(
        "ws-alpha",
        SESSION_PATH,
        &json!({ "agent": "claude", "session_id": "--dangerous/../../shadow",
                 "cwd": "/home/agent/../../root",
                 "updated": "2026-11-02T08:00:00+00:00" })
        .to_string(),
    );

    let record = rig.run("ws-alpha", MoveStep::Look).await.unwrap();

    assert_eq!(record.found.session, None);
    assert_eq!(record.found.cwd, None);
    let plan = serde_json::to_string(&record.plan).unwrap();
    assert!(!plan.contains("shadow"), "{plan}");
    assert!(!plan.contains("/root"), "{plan}");
}

#[tokio::test]
async fn the_plan_says_how_the_resumed_agent_runs() {
    let rig = rig(&["ws-alpha"]);
    rig.plan("ws-alpha");
    let record = rig.run("ws-alpha", MoveStep::Look).await.unwrap();
    let plan = serde_json::to_string(&record.plan).unwrap();
    assert!(plan.contains("in Full access mode"), "{plan}");
    assert!(plan.contains("on the account Personal"), "{plan}");
}
