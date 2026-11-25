// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The session manager's tests. Each one runs a manager over fake holders
//! and scripted drivers, with a clock that moves only when the test moves
//! it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use maxhq_core::names;
use maxhq_protocol::attention::{
    AnswerOutcome, AttentionAnswer, AttentionEvent, AttentionKind, AttentionOption,
};
use maxhq_protocol::context::{InlineRef, QuoteSource};
use maxhq_protocol::ids::{
    AccountId, AcpAgentId, AgentId, AttachmentId, AttentionId, BootId, ClientId, DeviceId,
    IdempotencyKey, PhoneId, ProjectId, SessionId,
};
use maxhq_protocol::message::{
    ActionOutcome, AgentPlace, AgentSpec, AttachmentSource, ClientMessage, SendMode, ServerMessage,
    UserInput,
};
use maxhq_protocol::model::{
    Account, Activity, Agent, AgentLink, CloneProgress, CloneStage, CloneState, CommandRule,
    CommandRules, Control, Device, DeviceSettings, EditKind, HiddenProjects, LoginKind,
    ModelChoice, PauseReason, PermissionMode, ProjectSettings, Usage, UsagePeriod, UsageSource,
    UsageWindow, Vendor, WorkspaceLocation,
};
use maxhq_protocol::state::{Change, Entity, EntityKey, Seq};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{
    AttachmentKind, AttachmentRef, Delivery, ItemBody, ItemId, ItemStatus, TimelineEvent,
    TimelineItem, TimelineUpdate,
};
use uuid::Uuid;

use super::clock::{Clock, FakeClock};
use super::drivers::STOP_GRACE;
use super::fake::{
    FakeCredentials, FakeFiles, FakeHolders, FakeLeases, FakePeers, FakePlaces, FakePower,
    FakeUnsaved, FakeVendors, FakeWorkspaces, Say,
};
use super::held::{Held, HoldRecord};
use super::{start, Client, Config, Owners, Sessions, CLIENT_BACKLOG};
use crate::db::{Db, HolderRow};
use crate::hub::{self, Command, Hub};
use crate::testing::TempDir;

/// How long a test waits for something before it fails.
const WAIT: Duration = Duration::from_secs(5);

/// How long a test waits to be sure something didn't happen.
const QUIET: Duration = Duration::from_millis(50);

mod agents;
mod automated;
mod container_reach;
mod deletes;
mod dispatcher;
mod dispatcher_chats;
mod dispatcher_guards;
mod finds;
mod folders;
mod idle;
mod inbox;
mod limits;
mod loading;
mod lookups;
mod memory;
mod moves;
mod notes;
mod notes_card;
mod pace;
mod phone_switch;
mod pulls;
mod renames;
mod restarts;
mod running;
mod switch;
mod take_over;
mod tasks;
mod terminals;
mod user_sessions;
mod workflow_env;

/// The client most tests act as.
fn phone() -> ClientId {
    ClientId::derive("phone")
}

fn device() -> Device {
    Device {
        id: DeviceId::derive("test"),
        name: "mac".into(),
        os: "macos".into(),
        boot_id: BootId("boot-1".into()),
        maxd_version: "0.1.0".into(),
        winding_down_until: None,
        workspace_host: None,
        build: None,
        target: None,
        wsl: None,
    }
}

/// A hub for `host` with a Claude and a Codex account.
fn make_hub(dir: &Path, db: &Db, host: &Device) -> Hub {
    let hub = Hub::new(host.clone(), db.seen().unwrap_or_default());
    hub.remember_deleted(db);
    let accounts = [
        (AccountId::derive("claude"), Vendor::Claude, "claude"),
        (AccountId::derive("codex"), Vendor::Codex, "codex"),
    ];
    hub.apply(accounts.map(|(id, vendor, name)| Change::Upsert {
        value: Entity::Account(Account {
            id,
            device: host.id,
            vendor,
            label: "Personal".into(),
            config_dir: dir.join(name).display().to_string(),
            email: None,
            organization: None,
            plan: None,
            models: Vec::new(),
            usage: None,
            usage_failure: None,
            acp: None,
            login: LoginKind::ConfigDir,
            origin: None,
            model_server: None,
        }),
    }));
    hub
}

/// What a manager runs over besides its store and clock. Clones share
/// their state, so the fakes outlive a restart.
#[derive(Clone)]
struct Fakes {
    places: FakePlaces,
    vendors: FakeVendors,
    workspaces: FakeWorkspaces,
    credentials: FakeCredentials,
    files: FakeFiles,
    power: FakePower,
    unsaved: FakeUnsaved,
    peers: FakePeers,
    hooks: tasks::FakeHooks,
    leases: FakeLeases,
    workflows: crate::workflow::Shared,
    /// The port watch, where a test says what holders run.
    ports: crate::ports::HolderWatch,
    /// Whether the manager has a dispatcher, as on the Mac.
    dispatcher: bool,
    terminals: terminals::FakeTerminals,
}

/// Starts a manager over the test's fakes.
fn manager(dir: &Path, hub: &Hub, db: &Db, fakes: &Fakes, clock: &Arc<FakeClock>) -> Sessions {
    let sessions = start(Config {
        hub: hub.clone(),
        db: db.clone(),
        holders: Arc::new(fakes.places.clone()),
        vendors: Arc::new(fakes.vendors.clone()),
        clock: clock.clone(),
        attachments: dir.join("attachments"),
        workspaces: Arc::new(fakes.workspaces.clone()),
        credentials: Arc::new(fakes.credentials.clone()),
        files: Arc::new(fakes.files.clone()),
        resume: dir.join("resume"),
        timelines: dir.join("timelines"),
        power: Arc::new(fakes.power.clone()),
        owners: Owners::default(),
        unsaved: Arc::new(fakes.unsaved.clone()),
        ports: fakes.ports.clone(),
        peers: Arc::new(fakes.peers.clone()),
        leases: Arc::new(fakes.leases.clone()),
        keys: dir.join("keys"),
        hooks: Arc::new(fakes.hooks.clone()),
        funnel: fakes.hooks.reach.subscribe(),
        workflows: fakes.workflows.clone(),
        notes: crate::notes::Notes::new(db.clone(), hub.clone()),
        terminals: fakes.terminals.wiring(),
        dispatcher: fakes
            .dispatcher
            .then(|| super::dispatcher::home(hub, &dir.join("data")))
            .flatten(),
    });
    fakes.peers.answer_with(&sessions, hub.device().id);
    sessions
}

/// A client of the manager that keeps the messages it read but no test
/// has taken yet.
struct Inbox {
    client: Client,
    kept: Vec<ServerMessage>,
}

impl Inbox {
    fn new(sessions: &Sessions, by: ClientId) -> Self {
        Self {
            client: sessions.client(by),
            kept: Vec::new(),
        }
    }

    /// A client on `phone`, whose switch is off unless the shared phone
    /// store turned it on.
    fn from_phone(sessions: &Sessions, phone: PhoneId) -> Self {
        Self {
            client: sessions.phone_client(ClientId::derive("iphone"), phone),
            kept: Vec::new(),
        }
    }

    fn send(&self, message: ClientMessage) {
        self.client.send(message);
    }

    /// The oldest message `pick` takes, waiting for one if none came yet.
    async fn take<T>(
        &mut self,
        what: &str,
        mut pick: impl FnMut(&ServerMessage) -> Option<T>,
    ) -> T {
        if let Some(at) = self.kept.iter().position(|message| pick(message).is_some()) {
            let message = self.kept.remove(at);
            return pick(&message).expect("the message matched a moment ago");
        }
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            match tokio::time::timeout_at(deadline, self.client.recv()).await {
                Ok(Some(message)) => match pick(&message) {
                    Some(found) => return found,
                    None => self.kept.push(message),
                },
                Ok(None) => panic!("the manager ended before {what} came"),
                Err(_) => panic!("{what} never came. Other messages: {:#?}", self.kept),
            }
        }
    }

    /// Reads for a moment, then checks that nothing `pick` matches came.
    async fn quiet(&mut self, what: &str, mut pick: impl FnMut(&ServerMessage) -> bool) {
        let deadline = tokio::time::Instant::now() + QUIET;
        while let Ok(Some(message)) = tokio::time::timeout_at(deadline, self.client.recv()).await {
            self.kept.push(message);
        }
        assert!(
            !self.kept.iter().any(&mut pick),
            "{what} came: {:#?}",
            self.kept
        );
    }

    async fn result(&mut self, key: IdempotencyKey) -> ActionOutcome {
        self.take("an action result", |message| match message {
            ServerMessage::ActionResult { key: got, outcome } if *got == key => {
                Some(outcome.clone())
            }
            _ => None,
        })
        .await
    }

    async fn answered(&mut self, key: IdempotencyKey) -> AnswerOutcome {
        self.take("an answer result", |message| match message {
            ServerMessage::AnswerResult {
                idempotency_key,
                outcome,
            } if *idempotency_key == key => Some(outcome.clone()),
            _ => None,
        })
        .await
    }

    async fn timeline(&mut self, agent: AgentId) -> TimelineUpdate {
        self.take("a timeline update", |message| match message {
            ServerMessage::Timeline { agent: got, update } if *got == agent => Some(update.clone()),
            _ => None,
        })
        .await
    }
}

/// Waits until `check` passes.
async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !check() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what} never happened"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn started(outcome: ActionOutcome) -> AgentId {
    match outcome {
        ActionOutcome::Started { agent } => agent,
        other => panic!("expected a started agent, got {other:?}"),
    }
}

fn reason(outcome: &ActionOutcome) -> &str {
    match outcome {
        ActionOutcome::Refused { reason } => reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn reset(update: TimelineUpdate) -> (Seq, Vec<TimelineItem>, bool) {
    match update {
        TimelineUpdate::Reset {
            seq,
            items,
            has_older,
        } => (seq, items, has_older),
        other => panic!("expected a reset, got {other:?}"),
    }
}

fn events(update: TimelineUpdate) -> (Seq, Vec<TimelineEvent>) {
    match update {
        TimelineUpdate::Events { seq, events } => (seq, events),
        other => panic!("expected events, got {other:?}"),
    }
}

fn older(update: TimelineUpdate) -> (Vec<TimelineItem>, bool) {
    match update {
        TimelineUpdate::Older { items, has_older } => (items, has_older),
        other => panic!("expected older items, got {other:?}"),
    }
}

fn item(agent: AgentId, id: &str, status: ItemStatus, body: ItemBody) -> TimelineItem {
    TimelineItem {
        id: ItemId(id.into()),
        agent,
        turn: None,
        parent: None,
        ordinal: 0,
        status,
        started_at: None,
        completed_at: None,
        body,
    }
}

fn command(agent: AgentId, id: &str, line: &str, status: ItemStatus) -> TimelineItem {
    let body = ItemBody::Command {
        command: line.into(),
        cwd: None,
        exit_code: None,
        output: String::new(),
        output_truncated: false,
        background: false,
    };
    item(agent, id, status, body)
}

fn reply(agent: AgentId, id: &str, text: &str, status: ItemStatus) -> TimelineItem {
    let body = ItemBody::AssistantMessage {
        text: text.into(),
        streaming: status == ItemStatus::Running,
    };
    item(agent, id, status, body)
}

/// A message waiting in the agent's queue.
fn queued(agent: AgentId, id: &str, text: &str) -> TimelineItem {
    let body = ItemBody::UserMessage {
        text: text.into(),
        attachments: Vec::new(),
        refs: Vec::new(),
        delivery: Delivery::Queued,
    };
    item(agent, id, ItemStatus::Pending, body)
}

fn upsert(agent: AgentId, item: TimelineItem) -> (AgentId, TimelineEvent) {
    (agent, TimelineEvent::Upsert { item })
}

fn ids(items: &[TimelineItem]) -> Vec<&str> {
    items.iter().map(|item| item.id.0.as_str()).collect()
}

fn event_ids(events: &[TimelineEvent]) -> Vec<&str> {
    events
        .iter()
        .map(|event| match event {
            TimelineEvent::Upsert { item } => item.id.0.as_str(),
            TimelineEvent::Append { id, .. } | TimelineEvent::Remove { id } => id.0.as_str(),
        })
        .collect()
}

fn is_notice(item: &TimelineItem, text: &str) -> bool {
    matches!(&item.body, ItemBody::Notice { message, .. } if message.contains(text))
}

fn answer(key: IdempotencyKey, event: AttentionId) -> ClientMessage {
    ClientMessage::Answer {
        idempotency_key: key,
        event,
        answer: AttentionAnswer::Option { id: "allow".into() },
        refs: Vec::new(),
    }
}

/// A manager over fakes, and the fakes, which outlive a restart.
struct Harness {
    dir: TempDir,
    folder: PathBuf,
    db: Db,
    holders: FakeHolders,
    vendors: FakeVendors,
    fakes: Fakes,
    /// The device as its hub starts. A test changes its boot ID to restart
    /// it.
    host: Device,
    /// Entities a restarted `maxd` gets back from the rest of the daemon,
    /// such as workspaces and project settings.
    kept: Vec<Entity>,
    clock: Arc<FakeClock>,
    /// The wall time the test started at.
    epoch: Timestamp,
    claude: AccountId,
    codex: AccountId,
    hub: Hub,
    sessions: Sessions,
    phone: Inbox,
}

impl Harness {
    fn new() -> Self {
        Self::on(device())
    }

    /// A harness on a device of its own, such as one that isn't a Mac.
    fn on(host: Device) -> Self {
        let dir = TempDir::new();
        let folder = dir.path().join("project");
        fs::create_dir_all(&folder).expect("the project folder can be made");
        let db = Db::open_in_memory().expect("the database opens");
        let holders = FakeHolders::new();
        let clock = Arc::new(FakeClock::new());
        let vendors = FakeVendors::new(clock.clone());
        let fakes = Fakes {
            places: FakePlaces::new(holders.clone(), vendors.clone()),
            vendors: vendors.clone(),
            workspaces: FakeWorkspaces::default(),
            credentials: FakeCredentials::default(),
            files: FakeFiles::new(dir.path().join("attachments")),
            power: FakePower::default(),
            unsaved: FakeUnsaved::default(),
            peers: FakePeers::default(),
            hooks: tasks::FakeHooks::default(),
            leases: FakeLeases::default(),
            workflows: crate::workflow::Shared::default(),
            ports: crate::ports::HolderWatch::default(),
            dispatcher: false,
            terminals: terminals::FakeTerminals::default(),
        };
        let epoch = clock.wall();
        let hub = make_hub(dir.path(), &db, &host);
        let sessions = manager(dir.path(), &hub, &db, &fakes, &clock);
        let phone = Inbox::new(&sessions, phone());
        Self {
            dir,
            folder,
            db,
            holders,
            vendors,
            fakes,
            host,
            kept: Vec::new(),
            clock,
            epoch,
            claude: AccountId::derive("claude"),
            codex: AccountId::derive("codex"),
            hub,
            sessions,
            phone,
        }
    }

    /// Ends the manager as `maxd` does when it quits.
    async fn stop(&self) {
        self.sessions.shutdown().await;
    }

    /// Starts a new manager over the same holders, store and clock, as a
    /// restarted `maxd` does.
    fn boot(&mut self) {
        self.hub = make_hub(self.dir.path(), &self.db, &self.host);
        self.hub.apply(
            self.kept
                .iter()
                .map(|value| Change::Upsert {
                    value: value.clone(),
                })
                .collect::<Vec<_>>(),
        );
        self.sessions = manager(
            self.dir.path(),
            &self.hub,
            &self.db,
            &self.fakes,
            &self.clock,
        );
        self.phone = Inbox::new(&self.sessions, phone());
    }

    /// Puts an entity in the hub, and again in each restarted one.
    fn keep(&mut self, value: Entity) {
        self.hub.apply([Change::Upsert {
            value: value.clone(),
        }]);
        self.kept.push(value);
    }

    async fn restart(&mut self) {
        self.stop().await;
        self.boot();
    }

    /// Sends an action under a new key and waits for its outcome.
    async fn act(&mut self, make: impl FnOnce(IdempotencyKey) -> ClientMessage) -> ActionOutcome {
        let key = IdempotencyKey::generate();
        self.phone.send(make(key));
        self.phone.result(key).await
    }

    /// Turns this device's "Agents can read other agents" switch on or off.
    async fn read_others(&mut self, on: bool) {
        let settings = DeviceSettings {
            read_other_agents: on,
            ..hub::default_settings(&device())
        };
        let outcome = self
            .act(|key| ClientMessage::SetDeviceSettings { key, settings })
            .await;
        assert_eq!(outcome, ActionOutcome::Done);
    }

    async fn send(&mut self, agent: AgentId, text: &str) -> ActionOutcome {
        self.send_with(agent, text, Vec::new(), SendMode::Queue)
            .await
    }

    /// Sends a message with these attachments, the way `mode` says.
    async fn send_with(
        &mut self,
        agent: AgentId,
        text: &str,
        attachments: Vec<AttachmentId>,
        mode: SendMode,
    ) -> ActionOutcome {
        let text = text.to_owned();
        self.act(|key| ClientMessage::SendInput {
            key,
            agent,
            input: UserInput {
                text,
                attachments,
                refs: Vec::new(),
            },
            mode,
        })
        .await
    }

    /// Stages pasted bytes as `name`.
    async fn stage_bytes(&mut self, name: &str, bytes: &[u8]) -> AttachmentRef {
        let source = AttachmentSource::Bytes {
            name: name.into(),
            mime: super::attach::mime_for(name).unwrap_or("text/plain").into(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        };
        let outcome = self
            .act(|key| ClientMessage::StageAttachment { key, source })
            .await;
        let ActionOutcome::Staged { attachment } = outcome else {
            panic!("the bytes weren't staged: {outcome:?}");
        };
        attachment
    }

    /// The paths the last message sent to the CLI named.
    fn attached(&self, session: SessionId) -> Vec<PathBuf> {
        let writes = self.holders.writes(session);
        let line: serde_json::Value = serde_json::from_str(&writes.last().unwrap().0).unwrap();
        line["attachments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|path| PathBuf::from(path.as_str().unwrap()))
            .collect()
    }

    async fn open(&mut self, agent: AgentId, since: Option<Seq>, limit: u32) -> TimelineUpdate {
        self.phone.send(ClientMessage::OpenTimeline {
            agent,
            since,
            limit,
            lead: None,
        });
        self.phone.timeline(agent).await
    }

    /// Opens a sub-agent's timeline, naming the lead it ran under.
    async fn open_under(&mut self, agent: AgentId, lead: AgentId) -> TimelineUpdate {
        self.phone.send(ClientMessage::OpenTimeline {
            agent,
            since: None,
            limit: 50,
            lead: Some(lead),
        });
        self.phone.timeline(agent).await
    }

    /// The agent as clients see it.
    fn agent(&self, id: AgentId) -> Agent {
        match self.hub.get(&EntityKey::Agent(id)) {
            Some(Entity::Agent(agent)) => agent,
            other => panic!("the hub has no agent {id}: {other:?}"),
        }
    }

    fn activity(&self, id: AgentId) -> Activity {
        self.agent(id).activity
    }

    /// The agent as stored for the next run of `maxd`.
    fn stored(&self, id: AgentId) -> Held {
        let rows: Vec<Held> = self.db.held().expect("the held agents can be read");
        rows.into_iter()
            .find(|held| held.agent.id == id)
            .unwrap_or_else(|| panic!("{id} isn't stored"))
    }

    fn spec(&self, vendor: Vendor, first: Option<&str>) -> AgentSpec {
        AgentSpec {
            effort: None,
            vendor,
            account: match vendor {
                Vendor::Claude => self.claude,
                Vendor::Codex => self.codex,
                Vendor::Cursor => AccountId::derive("cursor"),
                Vendor::Acp => AccountId::derive("acp"),
            },
            place: AgentPlace::Folder {
                path: self.folder.display().to_string(),
            },
            model: None,
            permission: None,
            title: None,
            first: first.map(|text| UserInput {
                text: text.into(),
                attachments: Vec::new(),
                refs: Vec::new(),
            }),
            branch: None,
            origin: None,
        }
    }

    /// Starts a Claude agent. Returns it and its holder once the first
    /// message, if any, reached the CLI.
    async fn start_claude(&mut self, first: Option<&str>) -> (AgentId, SessionId) {
        let spec = self.spec(Vendor::Claude, first);
        let agent = started(
            self.act(|key| ClientMessage::StartAgent { key, spec })
                .await,
        );
        let session = self.holders.last();
        if let Some(first) = first {
            let first = first.to_owned();
            eventually("the first message", || self.sent(session).contains(&first)).await;
        }
        (agent, session)
    }

    /// Gives the Claude account one model, which lists `efforts` and runs
    /// at `default` when told none.
    fn claude_model(&mut self, efforts: &[&str], default: &str) {
        let Some(Entity::Account(mut account)) = self.hub.get(&EntityKey::Account(self.claude))
        else {
            panic!("the Claude account is gone");
        };
        account.models = vec![ModelChoice {
            id: "opus".into(),
            label: "Opus".into(),
            default: true,
            efforts: efforts.iter().copied().map(String::from).collect(),
            default_effort: Some(default.into()),
            no_images: false,
        }];
        self.keep(Entity::Account(account));
    }

    /// The effort level a holder's CLI was started at.
    fn launched_effort(&self, session: SessionId) -> Option<String> {
        let args: Vec<String> = self
            .holders
            .launch(session)
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let at = args.iter().position(|arg| arg == "--effort")?;
        args.get(at + 1).cloned()
    }

    /// Adds this device's Cursor account, signed in as `login` says.
    fn cursor_account(&mut self, login: LoginKind) -> AccountId {
        let id = AccountId::derive("cursor");
        self.keep(Entity::Account(Account {
            id,
            device: self.host.id,
            vendor: Vendor::Cursor,
            label: "Cursor".into(),
            config_dir: self.dir.path().join("cursor").display().to_string(),
            email: None,
            organization: None,
            plan: None,
            models: Vec::new(),
            usage: None,
            usage_failure: None,
            acp: None,
            login,
            origin: None,
            model_server: None,
        }));
        id
    }

    /// A local ACP agent defined on this device, and its account. Its
    /// variable `API_KEY` has [`ACP_VALUE`] in the secret store.
    fn acp_agent(&mut self, containers: bool) -> AccountId {
        let agent = maxhq_protocol::acp::AcpAgent {
            id: AcpAgentId::derive("gemini"),
            device: self.host.id,
            name: "Gemini".into(),
            program: "gemini".into(),
            args: vec!["--experimental-acp".into()],
            env: vec!["API_KEY".into()],
            containers,
            problem: None,
            skills: Default::default(),
        };
        let account = crate::acp_agents::account_of(&agent);
        self.fakes
            .credentials
            .set_acp_value(agent.id, "API_KEY", ACP_VALUE);
        self.keep(Entity::AcpAgent(agent));
        self.keep(Entity::Account(Account {
            id: account,
            device: self.host.id,
            vendor: Vendor::Acp,
            label: "Gemini".into(),
            config_dir: String::new(),
            email: None,
            organization: None,
            plan: None,
            models: Vec::new(),
            usage: None,
            usage_failure: None,
            acp: None,
            login: LoginKind::AcpCommand,
            origin: None,
            model_server: None,
        }));
        account
    }

    /// A Codex agent as its app-server reports it.
    fn codex_agent(&self, thread: &str, activity: Activity) -> Agent {
        Agent {
            id: names::agent(Vendor::Codex, thread),
            workspace: self.hub.locate_folder(&self.folder),
            parent: None,
            vendor: Vendor::Codex,
            account: Some(self.codex),
            model: None,
            permission: None,
            session: None,
            vendor_session_id: Some(thread.into()),
            control: Control::Held,
            pid: None,
            link: AgentLink::Live,
            title: "Ship it".into(),
            activity,
            context: None,
            started_at: self.epoch,
            last_activity_at: self.epoch,
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
        }
    }

    /// Starts a busy Codex agent on `thread`. Returns it and its
    /// app-server's holder.
    async fn start_codex(&mut self, thread: &str) -> (AgentId, SessionId) {
        let spec = self.spec(Vendor::Codex, Some("Ship it"));
        self.start_codex_with(thread, spec).await
    }

    /// Starts a busy Codex agent on `thread` from `spec`.
    async fn start_codex_with(&mut self, thread: &str, spec: AgentSpec) -> (AgentId, SessionId) {
        let key = IdempotencyKey::generate();
        self.phone.send(ClientMessage::StartAgent { key, spec });
        self.logged(&format!("start {key}")).await;
        let server = self.codex_server();
        let agent = self.codex_agent(thread, Activity::Busy);
        let id = agent.id;
        let say = Say {
            agents: vec![agent],
            results: vec![(key, ActionOutcome::Started { agent: id })],
            ..Say::default()
        };
        self.say(server, &say).await;
        assert_eq!(
            self.phone.result(key).await,
            ActionOutcome::Started { agent: id }
        );
        (id, server)
    }

    /// The newest Codex app-server's holder.
    fn codex_server(&self) -> SessionId {
        self.holders
            .sessions()
            .into_iter()
            .rev()
            .find(|session| self.holders.launch(*session).program == Path::new("fake-codex"))
            .expect("an app-server was started")
    }

    /// The agent as clients see it, doing `activity`, for a driver to
    /// report.
    fn reported(&self, id: AgentId, activity: Activity) -> Agent {
        Agent {
            activity,
            ..self.agent(id)
        }
    }

    /// The CLI prints a scripted line. Waits for the driver to read it.
    async fn say(&self, session: SessionId, say: &Say) -> u64 {
        let from = self.vendors.log().len();
        let seq = self.holders.say(session, say);
        self.read(from, seq).await;
        seq
    }

    /// The CLI prints any line. Waits for the driver to read it.
    async fn print(&self, session: SessionId, text: &str) -> u64 {
        let from = self.vendors.log().len();
        let seq = self.holders.print(session, text);
        self.read(from, seq).await;
        seq
    }

    /// The CLI asks `prompt` and waits for the answer.
    async fn ask(&self, session: SessionId, prompt: &AttentionEvent) -> u64 {
        let from = self.vendors.log().len();
        let say = Say {
            open: vec![prompt.clone()],
            ..Say::default()
        };
        let seq = self.holders.ask(session, &prompt.id.to_string(), &say);
        self.read(from, seq).await;
        seq
    }

    /// Waits for a driver to read line `seq` live, after log entry `from`.
    /// Holders share seqs, so earlier entries don't count.
    async fn read(&self, from: usize, seq: u64) {
        let entry = format!("line {seq} replayed=false");
        eventually(&entry, || self.vendors.log()[from..].contains(&entry)).await;
    }

    /// What each message sent to the CLI said.
    fn sent(&self, session: SessionId) -> Vec<String> {
        self.holders
            .writes(session)
            .into_iter()
            .filter_map(|(line, _)| {
                let value: serde_json::Value = serde_json::from_str(&line).ok()?;
                value.get("send")?.as_str().map(str::to_owned)
            })
            .collect()
    }

    /// Waits for a driver to log `entry`.
    async fn logged(&self, entry: &str) {
        eventually(entry, || {
            self.vendors.log().iter().any(|logged| logged == entry)
        })
        .await;
    }

    /// How many log entries start with `prefix`.
    fn count(&self, prefix: &str) -> usize {
        self.vendors
            .log()
            .iter()
            .filter(|logged| logged.starts_with(prefix))
            .count()
    }

    /// A permission prompt from `agent`.
    fn prompt(&self, agent: AgentId, summary: &str) -> AttentionEvent {
        AttentionEvent {
            id: AttentionId::generate(),
            agent,
            kind: AttentionKind::Permission,
            summary: summary.into(),
            options: vec![AttentionOption {
                id: "allow".into(),
                label: "Allow".into(),
                kind: None,
            }],
            accepts_text: false,
            created_at: self.epoch,
            expires_at: None,
        }
    }

    /// A Claude session that runs outside MaxHQ, as the scanner reports
    /// it.
    fn outside(&self, session: &str, activity: Activity) -> AgentId {
        let id = names::agent(Vendor::Claude, session);
        let agent = Agent {
            id,
            workspace: self.hub.locate_folder(&self.folder),
            parent: None,
            vendor: Vendor::Claude,
            account: Some(self.claude),
            model: None,
            permission: None,
            session: None,
            vendor_session_id: Some(session.into()),
            control: Control::Observed,
            pid: None,
            link: AgentLink::Live,
            title: "Outside".into(),
            activity,
            context: None,
            started_at: self.epoch,
            last_activity_at: self.epoch,
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
        };
        self.hub.apply([Change::Upsert {
            value: Entity::Agent(agent),
        }]);
        id
    }

    /// Crashes a holder's CLI, then waits for the resume to start holder
    /// number `next`.
    async fn crash(&self, session: SessionId, next: usize) -> SessionId {
        self.holders.crash(session);
        eventually("the resumed holder", || {
            self.holders.sessions().len() == next
        })
        .await;
        self.holders.last()
    }

    /// As [`Harness::crash`], with the agent in a turn the crash cuts off.
    async fn busy_crash(&self, agent: AgentId, session: SessionId, next: usize) -> SessionId {
        let say = Say {
            agents: vec![self.reported(agent, Activity::Busy)],
            ..Say::default()
        };
        self.say(session, &say).await;
        self.crash(session, next).await
    }
}

#[tokio::test]
async fn a_claude_agent_starts_in_a_holder_and_gets_its_first_message() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(Some("Fix the build\nand test it")).await;

    let launch = h.holders.launch(session);
    assert_eq!(launch.program, PathBuf::from("fake-claude"));
    assert_eq!(launch.cwd, h.folder);
    let launches = h.vendors.launches();
    assert_eq!(launches.len(), 1);
    let (uuid, resume) = launches[0];
    assert!(!resume);
    assert_eq!(agent, names::agent(Vendor::Claude, &uuid.to_string()));
    assert_eq!(h.sent(session), ["Fix the build\nand test it"]);

    let shown = h.agent(agent);
    assert_eq!(shown.control, Control::Held);
    assert_eq!(shown.link, AgentLink::Live);
    assert_eq!(shown.session, Some(session));
    assert_eq!(shown.pid, Some(1000));
    assert_eq!(shown.activity, Activity::Starting);
    assert_eq!(shown.title, "Fix the build");
    assert_eq!(
        shown.permission.map(|permission| permission.requested),
        Some(PermissionMode::Auto)
    );

    let stored = h.stored(agent);
    assert_eq!(stored.vendor_session, uuid.to_string());
    assert_eq!(stored.cwd, h.folder);
    assert_eq!(stored.session, Some(session));
    let holders: Vec<HolderRow<HoldRecord>> = h.db.holders().unwrap();
    assert_eq!(holders.len(), 1);
    assert_eq!(holders[0].session, session);
    assert_eq!(holders[0].record.agent, Some(agent));
    assert_eq!(h.vendors.log()[0], "connected fresh=true pending=0");
    assert_eq!(h.holders.connects(session), [Some(0)]);
}

#[tokio::test]
async fn a_message_whose_pills_dont_fit_it_is_refused() {
    let mut h = Harness::new();
    let quote = InlineRef::Quote {
        agent: AgentId::derive("other"),
        agent_title: "api refactor".into(),
        item: ItemId("claude:msg_01".into()),
        text: "the cache never expires".into(),
        position: 0,
        source: QuoteSource::Reply,
        from: None,
    };
    let mut spec = h.spec(Vendor::Claude, Some("Explain this"));
    spec.first.as_mut().unwrap().refs = vec![quote.clone()];
    let outcome = h.act(|key| ClientMessage::StartAgent { key, spec }).await;
    assert_eq!(
        reason(&outcome),
        "the pills in this message don't match its text"
    );

    let mut spec = h.spec(Vendor::Claude, Some("Explain \u{FFFC}\nin detail"));
    spec.first.as_mut().unwrap().refs = vec![quote];
    let agent = started(h.act(|key| ClientMessage::StartAgent { key, spec }).await);
    assert_eq!(h.agent(agent).title, "Explain \"the cache never expires\"");

    let missing = AttachmentId::derive("missing");
    let outcome = h
        .act(|key| ClientMessage::SendInput {
            key,
            agent,
            input: UserInput {
                text: "see \u{FFFC}".into(),
                attachments: Vec::new(),
                refs: vec![InlineRef::Attachment { id: missing }],
            },
            mode: SendMode::Queue,
        })
        .await;
    assert_eq!(
        reason(&outcome),
        "a file pill names a file this message doesn't carry"
    );
}

#[tokio::test]
async fn an_agent_waits_for_its_folder_to_finish_cloning() {
    let mut h = Harness::new();
    let cloning = CloneProgress {
        state: CloneState::Cloning {
            stage: CloneStage::Receiving,
            percent: Some(40),
            stalled: false,
        },
    };
    h.hub
        .show_clone(&h.folder, ProjectId::derive("project"), cloning);
    let location = WorkspaceLocation::Folder {
        path: h.folder.display().to_string(),
    };
    let workspace = names::workspace(h.hub.device().id, &location);

    // Neither the folder, a folder inside it nor its workspace takes an
    // agent until git is done.
    let spec = h.spec(Vendor::Claude, None);
    let outcome = h.act(|key| ClientMessage::StartAgent { key, spec }).await;
    assert_eq!(reason(&outcome), "this folder is still being cloned");
    let inside = h.folder.join("src");
    fs::create_dir_all(&inside).unwrap();
    let mut spec = h.spec(Vendor::Claude, None);
    spec.place = AgentPlace::Folder {
        path: inside.display().to_string(),
    };
    let outcome = h.act(|key| ClientMessage::StartAgent { key, spec }).await;
    assert_eq!(reason(&outcome), "this folder is still being cloned");
    let mut spec = h.spec(Vendor::Claude, None);
    spec.place = AgentPlace::Workspace { workspace };
    let outcome = h.act(|key| ClientMessage::StartAgent { key, spec }).await;
    assert_eq!(reason(&outcome), "this folder is still being cloned");

    h.hub.cloned(&h.folder);
    h.start_claude(None).await;
}

#[tokio::test]
async fn a_codex_agent_starts_once_its_app_server_reports_the_thread() {
    let mut h = Harness::new();
    let key = IdempotencyKey::generate();
    h.phone.send(ClientMessage::StartAgent {
        key,
        spec: h.spec(Vendor::Codex, Some("Ship it")),
    });
    h.logged(&format!("start {key}")).await;
    let server = h.codex_server();
    h.phone
        .quiet("a result before the thread exists", |message| {
            matches!(message, ServerMessage::ActionResult { .. })
        })
        .await;

    let agent = h.codex_agent("thread-1", Activity::Busy);
    let id = agent.id;
    let say = Say {
        agents: vec![agent],
        results: vec![(key, ActionOutcome::Started { agent: id })],
        ..Say::default()
    };
    h.say(server, &say).await;
    assert_eq!(
        h.phone.result(key).await,
        ActionOutcome::Started { agent: id }
    );
    let shown = h.agent(id);
    assert_eq!(shown.control, Control::Held);
    assert_eq!(shown.link, AgentLink::Live);
    assert_eq!(shown.session, Some(server));
    assert_eq!(shown.activity, Activity::Busy);
    assert_eq!(h.stored(id).vendor_session, "thread-1");

    // A second agent shares the app-server. One the outcome names before
    // the server describes it still shows at once.
    let key = IdempotencyKey::generate();
    h.phone.send(ClientMessage::StartAgent {
        key,
        spec: h.spec(Vendor::Codex, None),
    });
    h.logged(&format!("start {key}")).await;
    let second = names::agent(Vendor::Codex, "thread-2");
    let say = Say {
        results: vec![(key, ActionOutcome::Started { agent: second })],
        ..Say::default()
    };
    h.say(server, &say).await;
    assert_eq!(
        h.phone.result(key).await,
        ActionOutcome::Started { agent: second }
    );
    let shown = h.agent(second);
    assert_eq!(shown.activity, Activity::Starting);
    assert_eq!(shown.title, "New agent");
    assert_eq!(h.holders.sessions(), [server]);
}

#[tokio::test]
async fn a_claude_agent_keeps_the_title_you_gave_it_through_a_restart() {
    let mut h = Harness::new();
    let spec = AgentSpec {
        title: Some("  Login fix ".into()),
        ..h.spec(Vendor::Claude, Some("Fix the build"))
    };
    let agent = started(h.act(|key| ClientMessage::StartAgent { key, spec }).await);
    let session = h.holders.last();
    assert_eq!(h.agent(agent).title, "Login fix");

    // Claude names the session from its first message, but the title you
    // gave stands over it.
    let named = Say {
        agents: vec![Agent {
            title: "Fix the build".into(),
            ..h.reported(agent, Activity::Busy)
        }],
        ..Say::default()
    };
    h.say(session, &named).await;
    let shown = h.agent(agent);
    assert_eq!(shown.activity, Activity::Busy);
    assert_eq!(shown.title, "Login fix");
    assert_eq!(h.stored(agent).title.as_deref(), Some("Login fix"));

    // A restarted `maxd` reads it back from the held agent's record.
    h.restart().await;
    eventually("the agent back with its title", || {
        matches!(
            h.hub.get(&EntityKey::Agent(agent)),
            Some(Entity::Agent(shown)) if shown.title == "Login fix"
        )
    })
    .await;
    eventually("a second connect", || {
        h.holders.connects(session).len() == 2
    })
    .await;
    h.say(session, &named).await;
    assert_eq!(h.agent(agent).title, "Login fix");

    // An agent you didn't name stores no title, as a record from before
    // titles were kept, and its record still loads.
    let (other, _) = h.start_claude(Some("Ship it")).await;
    let record = serde_json::to_value(h.stored(other)).unwrap();
    assert!(record.get("title").is_none(), "{record:#}");
    assert_eq!(h.stored(other).title, None);
    assert_eq!(h.agent(other).title, "Ship it");
}

#[tokio::test]
async fn a_held_agents_folder_and_project_come_back_after_a_restart() {
    let mut h = Harness::new();
    let (claude, _) = h.start_claude(Some("Fix the build")).await;
    let (codex, _) = h.start_codex("thread-1").await;
    let workspace = h.agent(claude).workspace;
    assert_eq!(h.agent(codex).workspace, workspace);
    let Some(Entity::Workspace(shown)) = h.hub.get(&EntityKey::Workspace(workspace)) else {
        panic!("the agents' folder has no workspace");
    };
    let project = shown.project.expect("the folder has a project");
    let named = h.hub.get(&EntityKey::Project(project));
    assert!(matches!(named, Some(Entity::Project(_))), "{named:?}");

    // No observer reads the config folders MaxHQ runs its own agents
    // under, so only the held agents can bring the folder back.
    h.restart().await;
    eventually("the agents back", || {
        h.hub.get(&EntityKey::Agent(claude)).is_some()
            && h.hub.get(&EntityKey::Agent(codex)).is_some()
    })
    .await;
    assert_eq!(h.agent(claude).workspace, workspace);
    assert_eq!(h.agent(codex).workspace, workspace);
    assert_eq!(
        h.hub.get(&EntityKey::Workspace(workspace)),
        Some(Entity::Workspace(shown))
    );
    assert_eq!(h.hub.get(&EntityKey::Project(project)), named);
}

#[tokio::test]
async fn a_restart_hands_each_new_driver_its_agents_as_stored() {
    let mut h = Harness::new();
    let (claude, s1) = h.start_claude(Some("Fix the build")).await;
    h.cursor_account(LoginKind::ConfigDir);
    let spec = h.spec(Vendor::Cursor, Some("Fix the build"));
    let cursor = started(h.act(|key| ClientMessage::StartAgent { key, spec }).await);
    let s2 = h.holders.last();
    let (codex, s3) = h.start_codex("thread-1").await;
    let mut stored = Vec::new();
    for (n, (agent, session)) in [(claude, s1), (cursor, s2), (codex, s3)]
        .into_iter()
        .enumerate()
    {
        let started_at = Timestamp(h.epoch.0 + n as u64 * 1000);
        let last_activity_at = Timestamp(started_at.0 + 60_000);
        let say = Say {
            agents: vec![Agent {
                started_at,
                last_activity_at,
                ..h.reported(agent, Activity::Idle)
            }],
            ..Say::default()
        };
        h.say(session, &say).await;
        stored.push((agent, started_at, last_activity_at));
    }
    h.stop().await;
    for (agent, started_at, last) in &stored {
        let held = h.stored(*agent).agent;
        assert_eq!(
            (held.started_at, held.last_activity_at),
            (*started_at, *last)
        );
    }

    // Claude's CLI ends while `maxd` is down, so a resume starts it again.
    // The other two holders still run, and the new drivers take them up.
    h.holders.crash(s1);
    h.boot();
    eventually("each new driver handed its agent", || {
        let met = h.vendors.met();
        stored.iter().all(|agent| met.contains(agent))
    })
    .await;
    // The resume can come a step after the agent is met, so wait for it.
    eventually("Claude's CLI resumed", || {
        h.vendors.launches().last().map(|(_, resume)| *resume) == Some(true)
    })
    .await;
}

#[tokio::test]
async fn actions_reach_the_agents_driver() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;

    assert_eq!(h.send(agent, "hello").await, ActionOutcome::Done);
    assert_eq!(h.sent(session), ["hello"]);
    let outcome = h.act(|key| ClientMessage::Interrupt { key, agent }).await;
    assert_eq!(outcome, ActionOutcome::Done);
    let outcome = h
        .act(|key| ClientMessage::SetPermission {
            key,
            agent,
            mode: PermissionMode::AcceptEdits,
        })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    let outcome = h
        .act(|key| ClientMessage::SetModel {
            key,
            agent,
            model: "opus".into(),
            account: None,
        })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    let outcome = h
        .act(|key| ClientMessage::SetEffort {
            key,
            agent,
            effort: Some("max".into()),
        })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    let outcome = h
        .act(|key| ClientMessage::CancelQueued {
            key,
            agent,
            item: ItemId("q1".into()),
        })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);

    let log = h.vendors.log();
    for entry in [
        "send hello",
        "interrupt",
        "mode AcceptEdits",
        "model opus",
        "effort max",
        "cancel q1",
    ] {
        assert!(
            log.iter().any(|logged| logged == entry),
            "no {entry} in {log:#?}"
        );
    }
    let shown = h.agent(agent);
    assert_eq!(
        shown.permission.map(|permission| permission.requested),
        Some(PermissionMode::AcceptEdits)
    );
    assert_eq!(shown.model.as_deref(), Some("opus"));
    assert_eq!(shown.effort.as_deref(), Some("max"));
    let stored = h.stored(agent);
    assert_eq!(stored.mode, PermissionMode::AcceptEdits);
    assert_eq!(stored.model.as_deref(), Some("opus"));
    assert_eq!(stored.effort.as_deref(), Some("max"));

    let nobody = AgentId::generate();
    let outcome = h
        .act(|key| ClientMessage::Interrupt { key, agent: nobody })
        .await;
    assert_eq!(outcome, ActionOutcome::UnknownAgent);
    let outside = h.outside(&Uuid::now_v7().to_string(), Activity::Idle);
    let outcome = h.send(outside, "hi").await;
    assert!(reason(&outcome).contains("outside MaxHQ"), "{outcome:?}");
}

#[tokio::test]
async fn agents_start_at_a_level_their_model_takes() {
    let mut h = Harness::new();
    h.claude_model(&["low", "medium", "high"], "medium");

    // With no device default, the CLI is told the model's own default.
    let (agent, session) = h.start_claude(None).await;
    assert_eq!(h.agent(agent).effort.as_deref(), Some("medium"));
    assert_eq!(h.stored(agent).effort.as_deref(), Some("medium"));
    assert_eq!(h.launched_effort(session).as_deref(), Some("medium"));

    // Asking for the default names it too.
    let outcome = h
        .act(|key| ClientMessage::SetEffort {
            key,
            agent,
            effort: None,
        })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    assert!(
        h.vendors
            .log()
            .iter()
            .any(|logged| logged == "effort medium"),
        "{:#?}",
        h.vendors.log()
    );
    assert_eq!(h.agent(agent).effort.as_deref(), Some("medium"));

    // A device default the model can't take comes down to one it can.
    let settings = DeviceSettings {
        claude_effort: Some("xhigh".into()),
        ..hub::default_settings(&device())
    };
    h.keep(Entity::DeviceSettings(settings));
    let (agent, session) = h.start_claude(None).await;
    assert_eq!(h.agent(agent).effort.as_deref(), Some("high"));
    assert_eq!(h.launched_effort(session).as_deref(), Some("high"));
}

#[tokio::test]
async fn an_agent_without_a_level_comes_back_at_its_models_default() {
    let mut h = Harness::new();
    // Before the account's models are known, nothing names a level.
    let (agent, first) = h.start_claude(None).await;
    assert_eq!(h.agent(agent).effort, None);
    assert_eq!(h.launched_effort(first), None);

    h.claude_model(&["low", "medium", "high", "xhigh"], "xhigh");
    let second = h.crash(first, 2).await;
    assert_eq!(h.launched_effort(second).as_deref(), Some("xhigh"));
    assert_eq!(h.agent(agent).effort.as_deref(), Some("xhigh"));
    assert_eq!(h.stored(agent).effort.as_deref(), Some("xhigh"));
}

#[tokio::test]
async fn every_effect_a_driver_reports_is_carried_out() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(None).await;
    let (_, items, _) = reset(h.open(lead, None, 50).await);
    assert!(items.is_empty());

    let sub = Agent {
        id: AgentId::generate(),
        parent: Some(lead),
        title: "Helper".into(),
        ..h.reported(lead, Activity::Busy)
    };
    let ask = h.prompt(lead, "Run cargo test?");
    let usage = Usage {
        checked_at: h.epoch,
        source: UsageSource::ClaudeProbe,
        windows: vec![UsageWindow {
            scope: None,
            name: None,
            window_minutes: 300,
            used_percent: 12.5,
            resets_at: None,
            period: UsagePeriod::Rolling,
            spend: None,
        }],
        reset_credits: None,
    };
    let models = vec![ModelChoice {
        id: "opus".into(),
        label: "Opus".into(),
        default: true,
        efforts: Vec::new(),
        default_effort: None,
        no_images: false,
    }];
    let say = Say {
        agents: vec![h.reported(lead, Activity::Busy), sub.clone()],
        timeline: vec![upsert(
            lead,
            reply(lead, "r1", "Working", ItemStatus::Running),
        )],
        open: vec![ask.clone()],
        usage: vec![(h.claude, usage.clone())],
        models: vec![(h.claude, models.clone())],
        writes: vec![r#"{"ack":1}"#.into()],
        wake_ms: Some(500),
        ..Say::default()
    };
    h.say(session, &say).await;

    assert_eq!(h.activity(lead), Activity::Busy);
    let shown = h.agent(sub.id);
    assert_eq!(shown.parent, Some(lead));
    assert_eq!(shown.control, Control::Held);
    assert_eq!(shown.session, Some(session));
    let (_, got) = events(h.phone.timeline(lead).await);
    assert_eq!(event_ids(&got), ["r1"]);
    assert!(matches!(
        h.hub.get(&EntityKey::Attention(ask.id)),
        Some(Entity::Attention(_))
    ));
    let Some(Entity::Account(account)) = h.hub.get(&EntityKey::Account(h.claude)) else {
        panic!("the account is gone");
    };
    assert_eq!(account.usage, Some(usage));
    assert_eq!(account.models, models);
    assert!(h
        .holders
        .writes(session)
        .iter()
        .any(|(line, _)| line == r#"{"ack":1}"#));

    // The tick comes once the clock reaches it.
    assert_eq!(h.count("tick"), 0);
    h.clock.advance(Duration::from_millis(500));
    h.logged("tick").await;

    // History goes before what this process said, and viewers start over.
    let say = Say {
        close: vec![ask.id],
        gone: vec![sub.id],
        history: vec![(
            lead,
            vec![reply(lead, "h1", "Earlier", ItemStatus::Completed)],
        )],
        ..Say::default()
    };
    h.say(session, &say).await;
    let (_, items, _) = reset(h.phone.timeline(lead).await);
    assert_eq!(ids(&items), ["h1", "r1"]);
    assert!(h.hub.get(&EntityKey::Attention(ask.id)).is_none());
    assert!(h.hub.get(&EntityKey::Agent(sub.id)).is_none());

    // A driver can end its own CLI.
    let say = Say {
        stop: true,
        ..Say::default()
    };
    h.say(session, &say).await;
    let paused = Activity::Paused {
        reason: PauseReason::ByYou,
    };
    eventually("the agent pausing", || h.activity(lead) == paused).await;
    eventually("the stop", || h.holders.stops(session) == [STOP_GRACE]).await;
    eventually("the holder's files going", || h.holders.forgotten(session)).await;
    assert_eq!(h.vendors.launches().len(), 1);
}

#[tokio::test]
async fn a_dropped_connection_replays_from_the_last_seq() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    h.print(session, "{}").await;
    h.print(session, "{}").await;

    h.holders.drop_connections(session, &["{}", "{}"]);
    h.logged("line 4 replayed=true").await;
    assert_eq!(h.holders.connects(session), [Some(0), Some(2)]);
    let log = h.vendors.log();
    assert!(log.contains(&"connected fresh=false pending=0".to_owned()));
    assert!(log.contains(&"line 3 replayed=true".to_owned()));
    assert_eq!(h.count("line 1 "), 1);
    assert_eq!(h.count("line 2 "), 1);

    assert_eq!(h.print(session, "{}").await, 5);
    assert_eq!(h.agent(agent).link, AgentLink::Live);
}

#[tokio::test]
async fn a_restart_reads_the_open_turn_again() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    h.print(session, "{}").await;
    let turn = Say {
        turn: Some(true),
        ..Say::default()
    };
    assert_eq!(h.say(session, &turn).await, 2);
    h.print(session, "{}").await;

    // The new driver reads the turn from its first line.
    let from = h.vendors.log().len();
    h.restart().await;
    h.logged("line 3 replayed=true").await;
    assert_eq!(h.holders.connects(session), [Some(0), Some(1)]);
    let log = h.vendors.log()[from..].to_vec();
    assert!(log.contains(&"line 2 replayed=true".to_owned()), "{log:#?}");
    assert!(!log.iter().any(|logged| logged.starts_with("line 1 ")));
    assert_eq!(h.print(session, "{}").await, 4);
    assert_eq!(h.agent(agent).link, AgentLink::Live);

    // Once the turn ends, the next restart reads on from the last line.
    let done = Say {
        turn: Some(false),
        ..Say::default()
    };
    assert_eq!(h.say(session, &done).await, 5);
    let from = h.vendors.log().len();
    h.restart().await;
    eventually("a third connect", || h.holders.connects(session).len() == 3).await;
    assert_eq!(h.holders.connects(session), [Some(0), Some(1), Some(5)]);
    assert_eq!(h.print(session, "{}").await, 6);
    let log = h.vendors.log()[from..].to_vec();
    assert!(
        !log.iter().any(|logged| logged.ends_with("replayed=true")),
        "{log:#?}"
    );
}

#[tokio::test]
async fn a_restart_reads_on_from_the_last_line_once_the_turn_is_trimmed() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let turn = Say {
        turn: Some(true),
        ..Say::default()
    };
    h.say(session, &turn).await;
    h.print(session, "{}").await;
    h.print(session, "{}").await;

    // The holder no longer keeps the turn's first line, so the lines the
    // last run read aren't read again.
    h.stop().await;
    h.holders.trim(session, 1);
    let from = h.vendors.log().len();
    h.boot();
    eventually("a second connect", || {
        h.holders.connects(session).len() == 2
    })
    .await;
    assert_eq!(h.holders.connects(session), [Some(0), Some(0)]);
    assert_eq!(h.print(session, "{}").await, 4);
    let log = h.vendors.log()[from..].to_vec();
    assert!(
        !log.iter().any(|logged| logged.ends_with("replayed=true")),
        "{log:#?}"
    );
    let (_, items, _) = reset(h.open(agent, None, 50).await);
    assert!(!items.iter().any(|item| is_notice(item, "")), "{items:#?}");
}

#[tokio::test]
async fn a_prompt_survives_a_reconnect_and_takes_one_answer() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let first = h.prompt(agent, "Run cargo test?");
    h.ask(session, &first).await;
    h.holders.drop_connections(session, &[]);
    h.logged("connected fresh=false pending=1").await;
    assert!(h.hub.get(&EntityKey::Attention(first.id)).is_some());

    let key = IdempotencyKey::generate();
    h.phone.send(answer(key, first.id));
    assert_eq!(h.phone.answered(key).await, AnswerOutcome::Accepted);
    let line = serde_json::json!({ "answer": first.id.to_string() }).to_string();
    assert_eq!(
        h.holders.writes(session),
        [(line, Some(first.id.to_string()))]
    );
    assert!(h.hub.get(&EntityKey::Attention(first.id)).is_none());

    // A second answer from another client is refused and names the first.
    let mut laptop = Inbox::new(&h.sessions, ClientId::derive("laptop"));
    let other = IdempotencyKey::generate();
    laptop.send(answer(other, first.id));
    match laptop.answered(other).await {
        AnswerOutcome::AlreadyAnswered { answered_by, .. } => assert_eq!(answered_by, phone()),
        outcome => panic!("expected the first answer to win, got {outcome:?}"),
    }
    assert_eq!(h.holders.writes(session).len(), 1);
    // The first answer sent again gets its outcome again.
    h.phone.send(answer(key, first.id));
    assert_eq!(h.phone.answered(key).await, AnswerOutcome::Accepted);

    // An answer typed in the CLI's own terminal wins as well.
    let second = h.prompt(agent, "Push to main?");
    h.ask(session, &second).await;
    h.holders.answer_elsewhere(session, &second.id.to_string());
    let key = IdempotencyKey::generate();
    h.phone.send(answer(key, second.id));
    match h.phone.answered(key).await {
        AnswerOutcome::AlreadyAnswered { answered_by, .. } => {
            assert_eq!(answered_by, ClientId::derive("elsewhere"));
        }
        outcome => panic!("expected the terminal's answer to win, got {outcome:?}"),
    }
    assert!(h.hub.get(&EntityKey::Attention(second.id)).is_none());

    // Answers outlive a restart.
    drop(laptop);
    h.restart().await;
    let mut laptop = Inbox::new(&h.sessions, ClientId::derive("laptop"));
    let late = IdempotencyKey::generate();
    laptop.send(answer(late, first.id));
    match laptop.answered(late).await {
        AnswerOutcome::AlreadyAnswered { answered_by, .. } => assert_eq!(answered_by, phone()),
        outcome => panic!("expected the first answer to win, got {outcome:?}"),
    }
}

#[tokio::test]
async fn the_pills_in_a_typed_answer_reach_the_agent_as_in_a_message() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let asked = h.prompt(agent, "Run cargo test?");
    h.ask(session, &asked).await;
    let pill = InlineRef::Agent {
        agent: AgentId::derive("cache"),
        title: "cache fix".into(),
        vendor: Vendor::Claude,
        device: DeviceId::derive("mac"),
        excerpt: None,
    };
    let typed = "No, do what \u{FFFC} did";
    let key = IdempotencyKey::generate();
    h.phone.send(ClientMessage::Answer {
        idempotency_key: key,
        event: asked.id,
        answer: AttentionAnswer::Text { text: typed.into() },
        refs: vec![pill.clone()],
    });
    assert_eq!(h.phone.answered(key).await, AnswerOutcome::Accepted);
    let text = maxhq_core::context::encode(typed, &[pill], &[], agent).text;
    assert!(text.starts_with("No, do what [Agent: cache fix] did\n\n<maxhq_context>"));
    let sent = serde_json::json!(AttentionAnswer::Text { text });
    h.logged(&format!("answered {sent}")).await;
}

#[tokio::test]
async fn a_timeline_catches_up_from_a_seq_or_starts_over() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let (opened, items, has_older) = reset(h.open(agent, None, 50).await);
    assert!(items.is_empty());
    assert!(!has_older);

    let say = Say {
        timeline: vec![upsert(
            agent,
            reply(agent, "r1", "One", ItemStatus::Completed),
        )],
        ..Say::default()
    };
    h.say(session, &say).await;
    let (seq, got) = events(h.phone.timeline(agent).await);
    assert_eq!(seq, opened.next());
    assert_eq!(event_ids(&got), ["r1"]);

    h.phone.send(ClientMessage::CloseTimeline { agent });
    let say = Say {
        timeline: vec![upsert(
            agent,
            reply(agent, "r2", "Two", ItemStatus::Completed),
        )],
        ..Say::default()
    };
    h.say(session, &say).await;
    h.phone
        .quiet("an update to a closed timeline", |message| {
            matches!(message, ServerMessage::Timeline { .. })
        })
        .await;

    // A client that kept up gets only what it missed.
    let (seq, got) = events(h.open(agent, Some(opened.next()), 50).await);
    assert_eq!(seq, opened.next().next());
    assert_eq!(event_ids(&got), ["r2"]);

    // One too far behind starts over.
    let (_, items, _) = reset(h.open(agent, Some(Seq(5)), 50).await);
    assert_eq!(ids(&items), ["r1", "r2"]);

    // Lines that rotated away before maxd read them reset the timeline
    // from history.
    let uuid = h.stored(agent).vendor_session;
    let history = vec![
        reply(agent, "h1", "Before", ItemStatus::Completed),
        reply(agent, "r1", "One", ItemStatus::Completed),
    ];
    h.vendors.set_history(&uuid, history);
    h.holders.skip(session, 3);
    h.print(session, "{}").await;
    let (_, items, _) = reset(h.phone.timeline(agent).await);
    assert_eq!(ids(&items), ["h1", "r1", "r2"]);
}

#[tokio::test]
async fn older_items_load_a_page_at_a_time() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let timeline = (0..10)
        .map(|n| {
            upsert(
                agent,
                reply(agent, &format!("r{n}"), "Hi", ItemStatus::Completed),
            )
        })
        .collect();
    let say = Say {
        timeline,
        ..Say::default()
    };
    h.say(session, &say).await;

    let (_, items, has_older) = reset(h.open(agent, None, 4).await);
    assert_eq!(ids(&items), ["r6", "r7", "r8", "r9"]);
    assert!(has_older);
    h.phone.send(ClientMessage::LoadOlder {
        agent,
        before: items[0].ordinal,
        limit: 4,
    });
    let (items, has_older) = older(h.phone.timeline(agent).await);
    assert_eq!(ids(&items), ["r2", "r3", "r4", "r5"]);
    assert!(has_older);
    h.phone.send(ClientMessage::LoadOlder {
        agent,
        before: items[0].ordinal,
        limit: 4,
    });
    let (items, has_older) = older(h.phone.timeline(agent).await);
    assert_eq!(ids(&items), ["r0", "r1"]);
    assert!(!has_older);
}

#[tokio::test]
async fn a_repeated_action_runs_once() {
    let mut h = Harness::new();
    let key = IdempotencyKey::generate();
    let spec = h.spec(Vendor::Claude, None);
    h.phone.send(ClientMessage::StartAgent {
        key,
        spec: spec.clone(),
    });
    h.phone.send(ClientMessage::StartAgent { key, spec });
    let first = started(h.phone.result(key).await);
    let second = started(h.phone.result(key).await);
    assert_eq!(first, second);
    assert_eq!(h.holders.sessions().len(), 1);
    let session = h.holders.last();

    let key = IdempotencyKey::generate();
    let message = ClientMessage::SendInput {
        key,
        agent: first,
        input: UserInput {
            text: "once".into(),
            attachments: Vec::new(),
            refs: Vec::new(),
        },
        mode: SendMode::Queue,
    };
    for _ in 0..3 {
        h.phone.send(message.clone());
    }
    for _ in 0..3 {
        assert_eq!(h.phone.result(key).await, ActionOutcome::Done);
    }
    assert_eq!(h.sent(session), ["once"]);

    // A repeat that comes while the first is still running waits for it.
    let key = IdempotencyKey::generate();
    let spec = h.spec(Vendor::Codex, None);
    h.phone.send(ClientMessage::StartAgent {
        key,
        spec: spec.clone(),
    });
    h.phone.send(ClientMessage::StartAgent { key, spec });
    h.logged(&format!("start {key}")).await;
    let server = h.codex_server();
    let agent = names::agent(Vendor::Codex, "thread-1");
    let say = Say {
        results: vec![(key, ActionOutcome::Started { agent })],
        ..Say::default()
    };
    h.say(server, &say).await;
    for _ in 0..2 {
        assert_eq!(h.phone.result(key).await, ActionOutcome::Started { agent });
    }
    assert_eq!(h.count(&format!("start {key}")), 1);
}

#[tokio::test]
async fn attachments_are_staged_then_moved_when_sent() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let notes = h.dir.path().join("notes.txt");
    fs::write(&notes, "remember").unwrap();

    let path = notes.display().to_string();
    let outcome = h
        .act(|key| ClientMessage::StageAttachment {
            key,
            source: AttachmentSource::Path { path },
        })
        .await;
    let ActionOutcome::Staged { attachment: copied } = outcome else {
        panic!("the path wasn't staged: {outcome:?}");
    };
    let data = base64::engine::general_purpose::STANDARD.encode(b"png!");
    let outcome = h
        .act(|key| ClientMessage::StageAttachment {
            key,
            source: AttachmentSource::Bytes {
                name: "shot.png".into(),
                mime: "image/png".into(),
                data,
            },
        })
        .await;
    let ActionOutcome::Staged {
        attachment: written,
    } = outcome
    else {
        panic!("the bytes weren't staged: {outcome:?}");
    };
    assert_eq!(written.name, "shot.png");
    let staged = h.dir.path().join("attachments").join("staged");
    assert!(staged.join(copied.id.to_string()).exists());
    assert!(staged.join(written.id.to_string()).exists());

    let attachments = vec![copied.id, written.id];
    let outcome = h
        .act(|key| ClientMessage::SendInput {
            key,
            agent,
            input: UserInput {
                text: "look".into(),
                attachments,
                refs: Vec::new(),
            },
            mode: SendMode::Queue,
        })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    let writes = h.holders.writes(session);
    let line: serde_json::Value = serde_json::from_str(&writes.last().unwrap().0).unwrap();
    let paths: Vec<PathBuf> = line["attachments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|path| PathBuf::from(path.as_str().unwrap()))
        .collect();
    let into = h.dir.path().join("attachments").join(agent.to_string());
    assert_eq!(
        paths,
        [
            into.join(copied.id.to_string()).join("notes.txt"),
            into.join(written.id.to_string()).join("shot.png"),
        ]
    );
    assert_eq!(fs::read_to_string(&paths[0]).unwrap(), "remember");
    assert_eq!(fs::read(&paths[1]).unwrap(), b"png!");
    assert!(!staged.join(copied.id.to_string()).exists());
    assert!(!staged.join(written.id.to_string()).exists());

    // The same agent may name a sent file again, and it stays put.
    let outcome = h
        .send_with(agent, "again", vec![copied.id], SendMode::Queue)
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    assert_eq!(h.attached(session), [paths[0].clone()]);
    assert_eq!(fs::read_to_string(&paths[0]).unwrap(), "remember");

    // A staged file goes to one agent only.
    let (other, other_session) = h.start_claude(None).await;
    let outcome = h
        .send_with(other, "mine?", vec![copied.id], SendMode::Queue)
        .await;
    assert_eq!(reason(&outcome), super::attach::GONE, "{outcome:?}");
    assert!(h
        .holders
        .writes(other_session)
        .iter()
        .all(|write| !write.0.contains("mine?")));
    assert!(paths[0].exists());

    // Staged files nobody sent are removed when maxd starts.
    let old = staged.join("old");
    fs::create_dir_all(&old).unwrap();
    fs::write(old.join("file"), "x").unwrap();
    let day_ago = SystemTime::now() - Duration::from_secs(25 * 60 * 60);
    maxhq_os::fsmode::set_modified(&old, day_ago).unwrap();
    let fresh = staged.join("fresh");
    fs::create_dir_all(&fresh).unwrap();
    h.restart().await;
    eventually("the stale copy going", || !old.exists()).await;
    assert!(fresh.exists());
}

#[tokio::test]
async fn a_queued_message_taken_back_goes_again_with_its_picture() {
    let mut h = Harness::new();
    let (claude, claude_session) = h.start_claude(None).await;
    let (codex, codex_server) = h.start_codex("thread-1").await;
    let account = h.acp_agent(false);
    let mut spec = h.spec(Vendor::Acp, None);
    spec.account = account;
    let acp = started(h.act(|key| ClientMessage::StartAgent { key, spec }).await);
    let acp_session = h.holders.last();

    for (agent, session) in [
        (claude, claude_session),
        (codex, codex_server),
        (acp, acp_session),
    ] {
        let picture = h.stage_bytes("pasted-image.png", b"png!").await;
        let outcome = h
            .send_with(agent, "look", vec![picture.id], SendMode::Queue)
            .await;
        assert_eq!(outcome, ActionOutcome::Done);
        let sent = h.attached(session);
        let into = h.dir.path().join("attachments").join(agent.to_string());
        assert_eq!(
            sent,
            [into.join(picture.id.to_string()).join("pasted-image.png")]
        );

        // Steer now: the app takes the queued message back and sends it
        // again at once, naming the same picture.
        let cancel = |key| ClientMessage::CancelQueued {
            key,
            agent,
            item: ItemId("q1".into()),
        };
        assert_eq!(h.act(cancel).await, ActionOutcome::Done);
        let outcome = h
            .send_with(agent, "look", vec![picture.id], SendMode::Steer)
            .await;
        assert_eq!(outcome, ActionOutcome::Done, "{:?}", h.agent(agent).vendor);
        assert_eq!(h.attached(session), sent);

        // Edit: taken back, changed and sent again.
        assert_eq!(h.act(cancel).await, ActionOutcome::Done);
        let outcome = h
            .send_with(agent, "look closer", vec![picture.id], SendMode::Queue)
            .await;
        assert_eq!(outcome, ActionOutcome::Done, "{:?}", h.agent(agent).vendor);
        assert_eq!(h.attached(session), sent);
        assert_eq!(fs::read(&sent[0]).unwrap(), b"png!");
        assert!(h.sent(session).contains(&"look closer".to_owned()));
    }

    // Another agent can't name a picture sent to the first.
    let picture = h.stage_bytes("pasted-image.png", b"png!").await;
    let outcome = h
        .send_with(claude, "look", vec![picture.id], SendMode::Queue)
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    let outcome = h
        .send_with(codex, "and you", vec![picture.id], SendMode::Steer)
        .await;
    assert_eq!(reason(&outcome), super::attach::GONE);
}

#[tokio::test]
async fn a_tool_call_runs_in_its_leads_folders_and_goes_back_to_the_driver() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(None).await;
    let sub = Agent {
        id: AgentId::generate(),
        parent: Some(lead),
        title: "Helper".into(),
        ..h.reported(lead, Activity::Busy)
    };
    let say = Say {
        agents: vec![h.reported(lead, Activity::Busy), sub.clone()],
        ..Say::default()
    };
    h.say(session, &say).await;

    let say = Say {
        sends: vec![
            ("req-1".into(), sub.id, vec!["out/chart.png".into()]),
            ("req-2".into(), AgentId::generate(), vec!["x.txt".into()]),
        ],
        ..Say::default()
    };
    h.say(session, &say).await;
    // A sub-agent sends from its lead's folders.
    let into = h.dir.path().join("attachments").join(lead.to_string());
    let sent = into.join("req-1").join("chart.png");
    h.logged(&format!("tool req-1 sent {}", sent.display()))
        .await;
    h.logged("tool req-2 failed MaxHQ doesn't hold this agent any more.")
        .await;
    let collected = h.fakes.files.collected();
    let [(place, call, paths)] = collected.as_slice() else {
        panic!("expected one send, got {collected:?}");
    };
    assert_eq!(place.attachments, into);
    assert_eq!(call, "req-1");
    assert_eq!(paths, &["out/chart.png"]);
}

#[tokio::test]
async fn an_open_from_a_sub_agent_checks_its_leads_folders() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(None).await;
    let sub = Agent {
        id: AgentId::generate(),
        parent: Some(lead),
        title: "Helper".into(),
        ..h.reported(lead, Activity::Busy)
    };
    let say = Say {
        agents: vec![h.reported(lead, Activity::Busy), sub.clone()],
        ..Say::default()
    };
    h.say(session, &say).await;
    let open = |key: &str| Say {
        tools: vec![(
            key.into(),
            sub.id,
            "open_file".into(),
            serde_json::json!({"path": "notes/plan.md", "line": 3}),
        )],
        ..Say::default()
    };

    h.say(session, &open("open-1")).await;
    h.logged("tool open-1 failed There's no file at notes/plan.md. Check the path and try again.")
        .await;
    let looked = h.fakes.files.looked();
    let [(place, path)] = looked.as_slice() else {
        panic!("expected one open, got {looked:?}");
    };
    assert_eq!(path, "notes/plan.md");
    let into = h.dir.path().join("attachments").join(lead.to_string());
    assert_eq!(place.attachments, into);

    // Once the file is in the lead's folder, the open shows it.
    let cwd = place.cwd.clone();
    fs::create_dir_all(cwd.join("notes")).unwrap();
    fs::write(cwd.join("notes/plan.md"), "# Plan").unwrap();
    h.say(session, &open("open-2")).await;
    let real = cwd.canonicalize().unwrap().join("notes/plan.md");
    h.logged(&format!(
        "tool open-2 opened {} line=Some(3) outside=false",
        real.display()
    ))
    .await;
}

#[tokio::test]
async fn a_show_copies_its_page_and_checks_what_it_replaces() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(None).await;
    let show = |key: &str, arguments: serde_json::Value| Say {
        tools: vec![(key.into(), lead, "show_visual".into(), arguments)],
        ..Say::default()
    };
    let into = h.dir.path().join("attachments").join(lead.to_string());

    // A page given whole is named for its title.
    h.say(
        session,
        &show(
            "show-1",
            serde_json::json!({"title": "Weekly usage", "html": "<p>hi</p>"}),
        ),
    )
    .await;
    let page = into.join("show-1").join("visual-weekly-usage.html");
    h.logged(&format!(
        "tool show-1 showed Weekly usage at {} replaces=None stretches=false",
        page.display()
    ))
    .await;

    // A page in a file comes from the agent's folder, and one that fills
    // the window is caught.
    let shown = h.fakes.files.shown();
    let cwd = shown[0].0.cwd.clone();
    fs::create_dir_all(cwd.join("out")).unwrap();
    fs::write(
        cwd.join("out/usage.html"),
        "<style>body{height:100vh}</style>",
    )
    .unwrap();
    h.say(
        session,
        &show(
            "show-2",
            serde_json::json!({"title": "Usage", "path": "out/usage.html"}),
        ),
    )
    .await;
    let page = into.join("show-2").join("visual-usage.html");
    h.logged(&format!(
        "tool show-2 showed Usage at {} replaces=None stretches=true",
        page.display()
    ))
    .await;

    // A file over the limit is refused, and says the limit.
    let big = "x".repeat(maxhq_core::tools::MAX_VISUAL_BYTES + 1);
    fs::write(cwd.join("out/big.html"), big).unwrap();
    h.say(
        session,
        &show(
            "show-3",
            serde_json::json!({"title": "Big", "path": "out/big.html"}),
        ),
    )
    .await;
    let failed = "tool show-3 failed The page is 513 KB, over the 512 KB a visual holds";
    eventually(failed, || h.count(failed) == 1).await;

    // `replaces` must name a visual on the agent's timeline.
    h.say(
        session,
        &show(
            "show-4",
            serde_json::json!({"title": "Usage", "html": "<p/>", "replaces": "claude:x:visual"}),
        ),
    )
    .await;
    let failed = "tool show-4 failed `replaces` names no visual you showed";
    eventually(failed, || h.count(failed) == 1).await;
    let earlier = item(
        lead,
        "claude:x:visual",
        ItemStatus::Completed,
        ItemBody::AgentVisual {
            title: "Usage".into(),
            attachment: AttachmentRef {
                id: AttachmentId::derive("earlier"),
                name: "visual-usage.html".into(),
                kind: AttachmentKind::File,
                bytes: 4,
                mime: None,
                path: page.display().to_string(),
                thumbnail: None,
            },
            replaces: None,
        },
    );
    let say = Say {
        timeline: vec![upsert(lead, earlier)],
        ..Say::default()
    };
    h.say(session, &say).await;
    h.say(
        session,
        &show(
            "show-5",
            serde_json::json!({"title": "Usage", "html": "<p/>", "replaces": "claude:x:visual"}),
        ),
    )
    .await;
    let page = into.join("show-5").join("visual-usage.html");
    h.logged(&format!(
        "tool show-5 showed Usage at {} replaces=Some(\"claude:x:visual\") stretches=false",
        page.display()
    ))
    .await;
}

#[tokio::test]
async fn device_settings_are_stored_and_published() {
    let mut h = Harness::new();
    let settings = DeviceSettings {
        device: device().id,
        default_permission: PermissionMode::AcceptEdits,
        workspace_network: Some("agentbr0".into()),
        workspace_pool: Some("agents".into()),
        auto_settle_days: 5,
        snooze_at_limit: false,
        continue_claude_at_limit: false,
        continue_codex_at_limit: false,
        claude_effort: Some("high".into()),
        codex_effort: None,
        placement: Default::default(),
        tool_updates: Vec::new(),
        cursor_usage: false,
        keep_awake: Default::default(),
        test_hosts: Default::default(),
        skill_library: None,
        network: Default::default(),
        idle_release_minutes: 60,
        memory_items: 256,
        history_mib: 256,
        claude_setup_token: false,
        shell_integration: true,
        windows_shell: Default::default(),
        read_other_agents: true,
        dispatcher_acts: true,
        chat_gap_minutes: 30,
        local_models: Default::default(),
        claude_artifacts: true,
    };
    let outcome = h
        .act(|key| ClientMessage::SetDeviceSettings {
            key,
            settings: settings.clone(),
        })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    let key = EntityKey::DeviceSettings(device().id);
    assert_eq!(
        h.hub.get(&key),
        Some(Entity::DeviceSettings(settings.clone()))
    );
    assert_eq!(
        h.db.device_settings(device().id).unwrap(),
        Some(settings.clone())
    );

    let elsewhere = DeviceSettings {
        device: DeviceId::derive("other"),
        ..settings.clone()
    };
    let outcome = h
        .act(|key| ClientMessage::SetDeviceSettings {
            key,
            settings: elsewhere,
        })
        .await;
    assert!(reason(&outcome).contains("another device"), "{outcome:?}");

    // New agents run in the device's mode, at its effort for their vendor.
    let (agent, _) = h.start_claude(None).await;
    assert_eq!(
        h.agent(agent)
            .permission
            .map(|permission| permission.requested),
        Some(PermissionMode::AcceptEdits)
    );
    assert_eq!(h.agent(agent).effort.as_deref(), Some("high"));
    assert_eq!(h.stored(agent).effort.as_deref(), Some("high"));

    h.restart().await;
    eventually("the settings coming back", || {
        h.hub.get(&key) == Some(Entity::DeviceSettings(settings.clone()))
    })
    .await;
}

#[tokio::test]
async fn hidden_projects_are_stored_published_and_survive_a_restart() {
    let mut h = Harness::new();
    let key = EntityKey::HiddenProjects(device().id);
    let hidden = |projects: &[ProjectId]| {
        Some(Entity::HiddenProjects(HiddenProjects {
            device: device().id,
            projects: projects.to_vec(),
        }))
    };
    // An empty list shows from the start, so clients know to send here.
    assert_eq!(h.hub.get(&key), hidden(&[]));

    let (a, b) = (ProjectId::derive("a"), ProjectId::derive("b"));
    let set = |project, hidden| {
        move |key| ClientMessage::SetProjectHidden {
            key,
            device: device().id,
            project,
            hidden,
        }
    };
    assert_eq!(h.act(set(a, true)).await, ActionOutcome::Done);
    assert_eq!(h.act(set(b, true)).await, ActionOutcome::Done);
    assert_eq!(h.act(set(b, false)).await, ActionOutcome::Done);
    assert_eq!(h.hub.get(&key), hidden(&[a]));

    let elsewhere = h
        .act(|key| ClientMessage::SetProjectHidden {
            key,
            device: DeviceId::derive("other"),
            project: b,
            hidden: true,
        })
        .await;
    assert!(
        reason(&elsewhere).contains("another device"),
        "{elsewhere:?}"
    );

    h.restart().await;
    assert_eq!(h.hub.get(&key), hidden(&[a]));
}

#[tokio::test]
async fn command_rules_show_per_project_and_go_when_removed() {
    let mut h = Harness::new();
    let (a, b) = (ProjectId::derive("a"), ProjectId::derive("b"));
    let key = |project| EntityKey::CommandRules {
        project,
        device: device().id,
    };
    let rule = |command: &str, exact| CommandRule {
        command: command.into(),
        exact,
    };
    // A project without rules has no list.
    assert_eq!(h.hub.get(&key(a)), None);
    h.db.add_command_rule(a, &rule("git show", false)).unwrap();
    h.db.add_command_rule(a, &rule("rm -rf build", true))
        .unwrap();
    h.db.add_command_rule(b, &rule("ls", false)).unwrap();
    h.restart().await;
    let rules = |project, rules: &[CommandRule]| {
        Some(Entity::CommandRules(CommandRules {
            project,
            device: device().id,
            rules: rules.to_vec(),
            edits: Vec::new(),
        }))
    };
    assert_eq!(
        h.hub.get(&key(a)),
        rules(a, &[rule("git show", false), rule("rm -rf build", true)])
    );

    let remove = |project, rule: CommandRule| {
        move |key| ClientMessage::RemoveCommandRule {
            key,
            device: device().id,
            project,
            rule,
        }
    };
    let gone = rule("rm -rf build", true);
    assert_eq!(h.act(remove(a, gone)).await, ActionOutcome::Done);
    assert_eq!(h.hub.get(&key(a)), rules(a, &[rule("git show", false)]));
    assert_eq!(
        h.act(remove(b, rule("ls", false))).await,
        ActionOutcome::Done
    );
    assert_eq!(h.hub.get(&key(b)), None);

    let elsewhere = h
        .act(|key| ClientMessage::RemoveCommandRule {
            key,
            device: DeviceId::derive("other"),
            project: a,
            rule: rule("git show", false),
        })
        .await;
    assert!(
        reason(&elsewhere).contains("another device"),
        "{elsewhere:?}"
    );

    h.restart().await;
    assert_eq!(h.hub.get(&key(a)), rules(a, &[rule("git show", false)]));
    assert_eq!(h.hub.get(&key(b)), None);
}

#[tokio::test]
async fn rules_a_driver_saves_go_to_its_project_and_its_drivers() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let project = match h.hub.get(&EntityKey::Workspace(h.agent(agent).workspace)) {
        Some(Entity::Workspace(workspace)) => workspace.project.expect("a folder has a project"),
        other => panic!("no workspace: {other:?}"),
    };
    let rule = CommandRule {
        command: "git show".into(),
        exact: false,
    };
    let say = Say {
        command_rules: vec![rule.clone(), rule.clone()],
        ..Say::default()
    };
    h.say(session, &say).await;
    let key = EntityKey::CommandRules {
        project,
        device: device().id,
    };
    let saved = Some(Entity::CommandRules(CommandRules {
        project,
        device: device().id,
        rules: vec![rule.clone()],
        edits: Vec::new(),
    }));
    eventually("the rule shows", || h.hub.get(&key) == saved).await;
    assert!(h.vendors.log().contains(&"rules git show".to_owned()));
    assert_eq!(h.db.command_rules().unwrap(), [(project, rule)]);

    // A new driver gets them as it connects.
    h.restart().await;
    eventually("the rules again", || {
        h.vendors
            .log()
            .iter()
            .filter(|entry| *entry == "rules git show")
            .count()
            == 2
    })
    .await;
}

#[tokio::test]
async fn kinds_of_file_a_card_remembers_reach_its_project_until_turned_off() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let project_of = |h: &Harness, agent| match h
        .hub
        .get(&EntityKey::Workspace(h.agent(agent).workspace))
    {
        Some(Entity::Workspace(workspace)) => workspace.project.expect("a folder has a project"),
        other => panic!("no workspace: {other:?}"),
    };
    let project = project_of(&h, agent);
    let say = Say {
        allowed_edits: vec![EditKind::GitHooks, EditKind::GitHooks],
        ..Say::default()
    };
    h.say(session, &say).await;
    let key = EntityKey::CommandRules {
        project,
        device: device().id,
    };
    let kept = Some(Entity::CommandRules(CommandRules {
        project,
        device: device().id,
        rules: Vec::new(),
        edits: vec![EditKind::GitHooks],
    }));
    eventually("the kind shows", || h.hub.get(&key) == kept).await;
    assert_eq!(
        h.db.allowed_edits().unwrap(),
        [(project, EditKind::GitHooks)]
    );
    let given = |h: &Harness, entry: &str| h.vendors.log().iter().filter(|e| *e == entry).count();
    assert_eq!(given(&h, "edits git_hooks"), 1);

    // A new agent in the project gets it as it connects, and one in
    // another project doesn't.
    h.start_claude(None).await;
    eventually("the second agent's", || given(&h, "edits git_hooks") == 2).await;
    let other = h.folder.with_file_name("other");
    fs::create_dir_all(&other).unwrap();
    let mut spec = h.spec(Vendor::Claude, None);
    spec.place = AgentPlace::Folder {
        path: other.display().to_string(),
    };
    let elsewhere = started(h.act(|key| ClientMessage::StartAgent { key, spec }).await);
    h.say(h.holders.last(), &Say::default()).await;
    assert_ne!(project_of(&h, elsewhere), project);
    assert_eq!(given(&h, "edits git_hooks"), 2);

    let remove = |device| {
        move |key| ClientMessage::RemoveAllowedEdit {
            key,
            device,
            project,
            kind: EditKind::GitHooks,
        }
    };
    let refused = h.act(remove(DeviceId::derive("other"))).await;
    assert!(reason(&refused).contains("another device"), "{refused:?}");
    assert_eq!(h.hub.get(&key), kept);

    // Turned off, the project's drivers hear so and the list goes.
    assert_eq!(h.act(remove(device().id)).await, ActionOutcome::Done);
    assert_eq!(h.hub.get(&key), None);
    assert_eq!(given(&h, "edits"), 2);
    assert_eq!(h.db.allowed_edits().unwrap(), []);
    h.restart().await;
    assert_eq!(h.hub.get(&key), None);
}

#[tokio::test]
async fn turning_cursor_usage_on_checks_usage_now() {
    let mut h = Harness::new();
    let mut commands = h.hub.commands();
    let mut settings = hub::default_settings(&device());
    let save = |key, settings: &DeviceSettings| ClientMessage::SetDeviceSettings {
        key,
        settings: settings.clone(),
    };
    settings.snooze_at_limit = true;
    assert_eq!(h.act(|key| save(key, &settings)).await, ActionOutcome::Done);
    assert!(commands.try_recv().is_err(), "other settings don't check");

    settings.cursor_usage = true;
    assert_eq!(h.act(|key| save(key, &settings)).await, ActionOutcome::Done);
    assert_eq!(commands.try_recv(), Ok(Command::RefreshUsage(None)));
}

#[tokio::test]
async fn continue_here_waits_for_the_other_terminal() {
    let mut h = Harness::new();
    let uuid = Uuid::now_v7();
    let session = uuid.to_string();
    let agent = h.outside(&session, Activity::Busy);
    let outcome = h
        .act(|key| ClientMessage::ContinueHere { key, agent })
        .await;
    assert!(reason(&outcome).contains("another terminal"), "{outcome:?}");

    h.outside(&session, Activity::Idle);
    h.vendors.run_elsewhere(&session, true);
    let outcome = h
        .act(|key| ClientMessage::ContinueHere { key, agent })
        .await;
    assert!(reason(&outcome).contains("another terminal"), "{outcome:?}");
    assert!(h.vendors.launches().is_empty());

    h.vendors.run_elsewhere(&session, false);
    let history = vec![reply(agent, "h1", "Before", ItemStatus::Completed)];
    h.vendors.set_history(&session, history);
    let outcome = h
        .act(|key| ClientMessage::ContinueHere { key, agent })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    assert_eq!(h.vendors.launches(), [(uuid, true)]);
    let shown = h.agent(agent);
    assert_eq!(shown.control, Control::Held);
    assert_eq!(shown.link, AgentLink::Live);
    let (_, items, _) = reset(h.open(agent, None, 50).await);
    assert_eq!(ids(&items), ["h1"]);
}

#[tokio::test]
async fn a_crash_mid_turn_resumes_with_a_continue_message_and_no_replay() {
    let mut h = Harness::new();
    let (agent, first) = h.start_claude(Some("Run the tests")).await;
    let (uuid, _) = h.vendors.launches()[0];
    let say = Say {
        agents: vec![h.reported(agent, Activity::Busy)],
        timeline: vec![upsert(
            agent,
            command(agent, "c1", "cargo test", ItemStatus::Running),
        )],
        ..Say::default()
    };
    h.say(first, &say).await;

    let second = h.crash(first, 2).await;
    assert_eq!(h.vendors.launches().last(), Some(&(uuid, true)));
    eventually("the continue message", || !h.sent(second).is_empty()).await;
    let sent = h.sent(second);
    assert_eq!(sent.len(), 1, "{sent:#?}");
    assert!(
        sent[0].starts_with("Your session stopped unexpectedly"),
        "{}",
        sent[0]
    );
    assert!(sent[0].contains("the command `cargo test`"), "{}", sent[0]);
    assert!(!sent[0].contains("Run the tests"));

    let (_, items, _) = reset(h.open(agent, None, 50).await);
    let cut = items
        .iter()
        .find(|item| item.id.0 == "c1")
        .expect("the cut-off command stays");
    assert_eq!(cut.status, ItemStatus::Interrupted);
    assert!(items.iter().any(|item| is_notice(item, "restarted it")));
    eventually("the old holder's files going", || {
        h.holders.forgotten(first)
    })
    .await;

    // The agent shows as starting until the new CLI says something.
    assert_eq!(h.activity(agent), Activity::Starting);
    h.print(second, "{}").await;
    assert_eq!(h.activity(agent), Activity::Idle);
}

#[tokio::test]
async fn three_resumes_without_progress_pause_the_agent() {
    let mut h = Harness::new();
    let (agent, s1) = h.start_claude(None).await;

    // A crash that cuts nothing off isn't counted. The resumed CLI makes
    // progress during a turn, which resets the count anyway.
    let s2 = h.crash(s1, 2).await;
    let say = Say {
        agents: vec![h.reported(agent, Activity::Busy)],
        ..Say::default()
    };
    h.say(s2, &say).await;
    let say = Say {
        timeline: vec![upsert(
            agent,
            command(agent, "c2", "ls", ItemStatus::Completed),
        )],
        ..Say::default()
    };
    h.say(s2, &say).await;

    // A command that finishes outside a turn isn't progress.
    let s3 = h.busy_crash(agent, s2, 3).await;
    let say = Say {
        timeline: vec![upsert(
            agent,
            command(agent, "c3", "sleep 30", ItemStatus::Completed),
        )],
        ..Say::default()
    };
    h.say(s3, &say).await;
    let s4 = h.busy_crash(agent, s3, 4).await;
    let s5 = h.busy_crash(agent, s4, 5).await;

    // The third resume in a row with no progress was the last.
    let say = Say {
        agents: vec![h.reported(agent, Activity::Busy)],
        ..Say::default()
    };
    h.say(s5, &say).await;
    h.holders.crash(s5);
    let paused = Activity::Paused {
        reason: PauseReason::ResumesMadeNoProgress,
    };
    eventually("the pause", || h.activity(agent) == paused).await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.holders.sessions().len(), 5);
    assert_eq!(h.activity(agent), paused);
    let notice = h.stored(agent).notice.expect("the pause opens a notice");
    match h.hub.get(&EntityKey::Attention(notice)) {
        Some(Entity::Attention(event)) => assert_eq!(event.kind, AttentionKind::Error),
        other => panic!("the notice isn't shown: {other:?}"),
    }
    let (_, items, _) = reset(h.open(agent, None, 50).await);
    assert!(items.iter().any(|item| is_notice(item, "no progress")));

    // A message brings it back, and only the message is sent.
    assert_eq!(h.send(agent, "carry on").await, ActionOutcome::Done);
    assert_eq!(h.holders.sessions().len(), 6);
    let s6 = h.holders.last();
    assert_eq!(
        h.vendors.launches().last().map(|launch| launch.1),
        Some(true)
    );
    assert_eq!(h.sent(s6), ["carry on"]);
    assert!(h.hub.get(&EntityKey::Attention(notice)).is_none());
}

#[tokio::test]
async fn a_new_sub_agents_first_items_count_as_its_leads_progress() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(None).await;
    let say = Say {
        agents: vec![h.reported(lead, Activity::Busy)],
        ..Say::default()
    };
    h.say(session, &say).await;

    // The sub-agent comes in the same effects as a command it finished.
    let sub = Agent {
        id: AgentId::generate(),
        parent: Some(lead),
        title: "Helper".into(),
        ..h.reported(lead, Activity::Busy)
    };
    let say = Say {
        agents: vec![sub.clone()],
        timeline: vec![upsert(
            sub.id,
            command(sub.id, "c1", "ls", ItemStatus::Completed),
        )],
        ..Say::default()
    };
    h.say(session, &say).await;
    assert_eq!(h.agent(sub.id).parent, Some(lead));
    h.stop().await;
    assert!(h.stored(lead).resumes.progress);
}

#[tokio::test]
async fn a_reply_that_ends_the_turn_counts_as_progress() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let say = Say {
        agents: vec![h.reported(agent, Activity::Busy)],
        ..Say::default()
    };
    h.say(session, &say).await;

    // The lead goes idle in the same effects that finish its reply.
    let say = Say {
        agents: vec![h.reported(agent, Activity::Idle)],
        timeline: vec![upsert(
            agent,
            reply(agent, "r1", "Done", ItemStatus::Completed),
        )],
        ..Say::default()
    };
    h.say(session, &say).await;
    assert_eq!(h.activity(agent), Activity::Idle);
    h.stop().await;
    assert!(h.stored(agent).resumes.progress);
}

#[tokio::test]
async fn an_idle_agent_whose_cli_ends_comes_back_without_counting() {
    let mut h = Harness::new();
    let (agent, mut session) = h.start_claude(None).await;

    // The device restarts more times than a pause allows, each time with
    // the agent idle, and its CLI dies with it.
    for next in 2..=5 {
        h.stop().await;
        h.holders.crash(session);
        h.clock.advance(Duration::from_secs(10 * 60));
        h.boot();
        eventually("a resumed holder", || h.holders.sessions().len() == next).await;
        session = h.holders.last();
        tokio::time::sleep(QUIET).await;
        assert!(h.sent(session).is_empty(), "{:?}", h.sent(session));
    }
    // And once more without a restart.
    session = h.crash(session, 6).await;
    tokio::time::sleep(QUIET).await;
    assert!(h.sent(session).is_empty());
    assert!(!matches!(
        h.activity(agent),
        Activity::Paused { .. } | Activity::Error { .. }
    ));
    h.stop().await;
    assert_eq!(h.stored(agent).resumes.count, 0);
}

#[tokio::test]
async fn an_idle_cli_that_keeps_ending_stops_coming_back() {
    let mut h = Harness::new();
    let (agent, mut session) = h.start_claude(None).await;
    for next in 2..=5 {
        session = h.crash(session, next).await;
    }

    // The fifth end in five minutes is one too many.
    h.holders.crash(session);
    eventually("the reason showing", || {
        matches!(
            h.activity(agent),
            Activity::Error { reason } if reason.contains("5 times in 5 minutes")
        )
    })
    .await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.holders.sessions().len(), 5);

    // What you send brings it back.
    assert_eq!(h.send(agent, "carry on").await, ActionOutcome::Done);
    assert_eq!(h.holders.sessions().len(), 6);
    let s6 = h.holders.last();
    assert_eq!(h.sent(s6), ["carry on"]);
}

#[tokio::test]
async fn an_agent_you_stopped_stays_stopped_until_you_send() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let outcome = h.act(|key| ClientMessage::StopAgent { key, agent }).await;
    assert_eq!(outcome, ActionOutcome::Done);
    let paused = Activity::Paused {
        reason: PauseReason::ByYou,
    };
    eventually("the pause", || h.activity(agent) == paused).await;
    eventually("the stop", || h.holders.stops(session) == [STOP_GRACE]).await;
    eventually("the holder's files going", || h.holders.forgotten(session)).await;
    assert!(h.holders.exited(session));
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.holders.sessions(), [session]);

    h.restart().await;
    eventually("the agent coming back paused", || {
        matches!(
            h.hub.get(&EntityKey::Agent(agent)),
            Some(Entity::Agent(shown)) if shown.activity == paused
        )
    })
    .await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.holders.sessions(), [session]);

    assert_eq!(h.send(agent, "go on").await, ActionOutcome::Done);
    let resumed = h.holders.last();
    assert_ne!(resumed, session);
    assert_eq!(
        h.vendors.launches().last().map(|launch| launch.1),
        Some(true)
    );
    assert_eq!(h.sent(resumed), ["go on"]);
    // Stop leaves it paused, and the message took it out: one state, which
    // Resume or a message ends.
    eventually("the pause ending", || h.activity(agent) != paused).await;
}

#[tokio::test]
async fn a_holder_that_vanishes_counts_as_a_crash() {
    let mut h = Harness::new();
    let (agent, s1) = h.start_claude(None).await;
    h.holders.vanish(s1);
    eventually("a resumed holder", || h.holders.sessions().len() == 2).await;
    // The old holder was ended first, named by the process ID its hello
    // gave, so its CLI never ran beside the new one.
    assert_eq!(h.holders.ends(s1), [Some(11_000)]);
    assert!(h.holders.exited(s1));
    assert_eq!(h.holders.overlaps(), 0);
    let s2 = h.holders.last();
    assert_eq!(
        h.vendors.launches().last().map(|launch| launch.1),
        Some(true)
    );
    h.print(s2, "{}").await;
    let shown = h.agent(agent);
    assert_eq!(shown.session, Some(s2));
    assert_eq!(shown.link, AgentLink::Live);
}

#[tokio::test]
async fn a_holder_that_dies_before_its_cli_ends_the_cli_first() {
    let mut h = Harness::new();
    let (agent, s1) = h.start_claude(None).await;
    h.holders.orphan(s1);
    eventually("a resumed holder", || h.holders.sessions().len() == 2).await;
    assert_eq!(h.holders.ends(s1), [Some(11_000)]);
    assert_eq!(h.holders.overlaps(), 0);
    let s2 = h.holders.last();
    h.print(s2, "{}").await;
    assert_eq!(h.agent(agent).session, Some(s2));
}

#[tokio::test]
async fn a_lost_holder_that_cant_be_confirmed_ended_keeps_its_agent_down() {
    let mut h = Harness::new();
    let (agent, s1) = h.start_claude(None).await;
    h.holders.unconfirmable(s1);
    h.holders.vanish(s1);
    let stranded = |activity: &Activity| {
        matches!(
            activity,
            Activity::Error { reason }
                if reason.contains("couldn't make sure it stopped")
                    && reason.contains("doesn't look like its holder")
        )
    };
    eventually("the reason showing", || stranded(&h.activity(agent))).await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.holders.sessions(), [s1]);
    let (_, items, _) = reset(h.open(agent, None, 50).await);
    assert!(items
        .iter()
        .any(|item| is_notice(item, "didn't start another")));

    // A restarted maxd tries to end it again, and still doesn't replace it.
    h.restart().await;
    eventually("a second try", || h.holders.ends(s1).len() == 2).await;
    eventually("the reason showing again", || stranded(&h.activity(agent))).await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.holders.sessions(), [s1]);

    // Once you send, a new holder replaces it for good.
    assert_eq!(h.send(agent, "carry on").await, ActionOutcome::Done);
    assert_eq!(h.holders.sessions().len(), 2);
    let s2 = h.holders.last();
    assert_eq!(h.sent(s2), ["carry on"]);
    h.restart().await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.holders.ends(s1).len(), 2);
    assert_eq!(h.holders.sessions().len(), 2);
}

#[tokio::test]
async fn a_lost_app_server_that_cant_be_confirmed_ended_isnt_restarted() {
    let mut h = Harness::new();
    let (agent, server1) = h.start_codex("thread-1").await;
    let say = Say {
        timeline: vec![upsert(
            agent,
            command(agent, "n1", "npm test", ItemStatus::Running),
        )],
        ..Say::default()
    };
    h.say(server1, &say).await;
    h.holders.unconfirmable(server1);
    h.holders.vanish(server1);
    eventually("the reason showing", || {
        matches!(
            h.activity(agent),
            Activity::Error { reason } if reason.contains("couldn't make sure it stopped")
        )
    })
    .await;
    h.clock.advance(Duration::from_secs(60));
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.holders.sessions(), [server1]);

    // What you send starts a new app-server, which carries on.
    assert_eq!(h.send(agent, "go on").await, ActionOutcome::Done);
    assert_eq!(h.holders.sessions().len(), 2);
    let server2 = h.holders.last();
    let sent = h.sent(server2);
    assert_eq!(sent.last().map(String::as_str), Some("go on"), "{sent:#?}");
}

#[tokio::test]
async fn a_cli_that_cant_start_again_shows_why_until_you_send() {
    let mut h = Harness::new();
    let (agent, s1) = h.start_claude(None).await;
    h.holders.fail_spawns(1);
    h.holders.crash(s1);
    eventually("the error showing", || {
        matches!(
            h.activity(agent),
            Activity::Error { reason } if reason.contains("couldn't restart Claude")
        )
    })
    .await;
    assert_eq!(h.holders.sessions(), [s1]);

    assert_eq!(h.send(agent, "try again").await, ActionOutcome::Done);
    assert_eq!(h.holders.sessions().len(), 2);
    let s2 = h.holders.last();
    assert_eq!(h.sent(s2), ["try again"]);
    assert_eq!(h.activity(agent), Activity::Idle);

    // A start that can't spawn is refused and leaves nothing behind.
    h.holders.fail_spawns(1);
    let spec = h.spec(Vendor::Claude, Some("Hi"));
    let outcome = h.act(|key| ClientMessage::StartAgent { key, spec }).await;
    assert!(
        reason(&outcome).contains("couldn't start Claude"),
        "{outcome:?}"
    );
    assert_eq!(h.holders.sessions().len(), 2);
    assert_eq!(h.db.held::<Held>().unwrap().len(), 1);
}

/// Crashes `agent`'s CLI in `session` and has the new one fail to start,
/// as a holder whose folder is gone, while a message waits for it. Checks
/// that the message and the agent both show the holder's reason, naming
/// `vendor`.
async fn a_restart_into_a_gone_folder_says_why(
    h: &mut Harness,
    agent: AgentId,
    session: SessionId,
    vendor: &str,
) {
    let gone = "the folder /tmp/project is gone";
    let starts = |h: &Harness| h.vendors.launches().len() + h.vendors.acp_launches().len();
    let before = starts(h);
    h.holders.fail_spawn_with(gone);
    h.holders.hold_spawns();
    h.holders.crash(session);
    eventually("the restart", || starts(h) > before).await;
    let key = IdempotencyKey::generate();
    h.phone.send(ClientMessage::SendInput {
        key,
        agent,
        input: UserInput {
            text: "carry on".into(),
            attachments: Vec::new(),
            refs: Vec::new(),
        },
        mode: SendMode::Queue,
    });
    h.phone
        .quiet("an action result", |message| {
            matches!(message, ServerMessage::ActionResult { .. })
        })
        .await;

    h.holders.release_spawns();
    let want = format!("couldn't restart {vendor}: {gone}");
    let outcome = h.phone.result(key).await;
    assert_eq!(reason(&outcome), want);
    eventually("the error showing", || {
        h.activity(agent)
            == Activity::Error {
                reason: want.clone(),
            }
    })
    .await;
}

#[tokio::test]
async fn a_claude_restart_that_cant_start_refuses_what_waited_with_why() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    a_restart_into_a_gone_folder_says_why(&mut h, agent, session, "Claude").await;
}

#[tokio::test]
async fn a_cursor_restart_that_cant_start_refuses_what_waited_with_why() {
    let mut h = Harness::new();
    h.cursor_account(LoginKind::ConfigDir);
    let spec = h.spec(Vendor::Cursor, Some("Fix the build"));
    let agent = started(h.act(|key| ClientMessage::StartAgent { key, spec }).await);
    let session = h.holders.last();
    a_restart_into_a_gone_folder_says_why(&mut h, agent, session, "Cursor").await;
}

#[tokio::test]
async fn an_agent_whose_cli_died_while_maxd_was_down_comes_back() {
    let mut h = Harness::new();
    let (agent, s1) = h.start_claude(Some("Build it")).await;
    let (uuid, _) = h.vendors.launches()[0];
    let say = Say {
        agents: vec![h.reported(agent, Activity::Busy)],
        ..Say::default()
    };
    h.say(s1, &say).await;
    let history = vec![command(agent, "b1", "cargo build", ItemStatus::Running)];
    h.vendors.set_history(&uuid.to_string(), history);

    h.stop().await;
    h.holders.crash(s1);
    h.boot();

    eventually("a resumed holder", || h.holders.sessions().len() == 2).await;
    let s2 = h.holders.last();
    assert_eq!(h.vendors.launches().last(), Some(&(uuid, true)));
    eventually("the continue message", || !h.sent(s2).is_empty()).await;
    let sent = h.sent(s2);
    assert_eq!(sent.len(), 1, "{sent:#?}");
    assert!(sent[0].contains("the command `cargo build`"), "{}", sent[0]);
}

#[tokio::test]
async fn a_crashed_codex_app_server_restarts_with_backoff() {
    let mut h = Harness::new();
    let (agent, server1) = h.start_codex("thread-1").await;
    let say = Say {
        timeline: vec![upsert(
            agent,
            command(agent, "n1", "npm test", ItemStatus::Running),
        )],
        ..Say::default()
    };
    h.say(server1, &say).await;

    // The first restart comes at once. It adopts the thread, then sends
    // the continue message.
    let from = h.vendors.log().len();
    h.holders.crash(server1);
    eventually("a second app-server", || h.holders.sessions().len() == 2).await;
    let server2 = h.holders.last();
    eventually("the continue message", || !h.sent(server2).is_empty()).await;
    let sent = h.sent(server2);
    assert!(sent[0].contains("the command `npm test`"), "{}", sent[0]);
    let log = h.vendors.log()[from..].to_vec();
    let adopt = log.iter().position(|logged| logged == "adopt thread-1");
    let mode = log.iter().position(|logged| logged == "mode Auto");
    let send = log
        .iter()
        .position(|logged| logged.starts_with("send Your session"));
    assert!(adopt.is_some() && adopt < mode && mode < send, "{log:#?}");

    // The second waits a second.
    let say = Say {
        agents: vec![h.reported(agent, Activity::Busy)],
        ..Say::default()
    };
    h.say(server2, &say).await;
    h.holders.crash(server2);
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.holders.sessions().len(), 2);
    h.clock.advance(Duration::from_secs(1));
    eventually("a third app-server", || h.holders.sessions().len() == 3).await;
    let server3 = h.holders.last();

    // One that never says anything is ended after the handshake limit.
    tokio::time::sleep(QUIET).await;
    assert!(h.holders.stops(server3).is_empty());
    h.clock.advance(Duration::from_secs(30));
    eventually("the silent app-server ending", || {
        h.holders.stops(server3) == [Duration::ZERO]
    })
    .await;
}

#[tokio::test]
async fn a_restarted_maxd_reconnects_and_adopts_its_codex_threads() {
    let mut h = Harness::new();
    let (agent, server) = h.start_codex("thread-1").await;
    h.print(server, "{}").await;
    let from = h.vendors.log().len();

    h.restart().await;
    eventually("the thread adopted again", || {
        h.vendors.log()[from..]
            .iter()
            .any(|logged| logged == "adopt thread-1")
    })
    .await;
    assert_eq!(h.holders.connects(server), [Some(0), Some(2)]);
    assert_eq!(h.holders.sessions(), [server]);
    let log = h.vendors.log()[from..].to_vec();
    assert!(log.contains(&"connected fresh=false pending=0".to_owned()));
    tokio::time::sleep(QUIET).await;
    let adopts = h.vendors.log()[from..]
        .iter()
        .filter(|logged| *logged == "adopt thread-1")
        .count();
    assert_eq!(adopts, 1);
    eventually("the agent showing live", || {
        matches!(
            h.hub.get(&EntityKey::Agent(agent)),
            Some(Entity::Agent(shown)) if shown.link == AgentLink::Live
        )
    })
    .await;
}

#[tokio::test]
async fn an_adopted_codex_thread_runs_in_the_agents_mode_and_model() {
    let mut h = Harness::new();
    let mut spec = h.spec(Vendor::Codex, Some("Ship it"));
    spec.permission = Some(PermissionMode::FullAccess);
    spec.model = Some("gpt-5.6-luna".into());
    let (agent, server) = h.start_codex_with("thread-1", spec).await;
    h.print(server, "{}").await;
    let from = h.vendors.log().len();

    // The new driver knows nothing of the thread, so the adopt is followed
    // by the agent's own mode and model, before anything is sent.
    h.restart().await;
    eventually("the thread adopted again", || {
        h.vendors.log()[from..]
            .iter()
            .any(|logged| logged == "adopt thread-1")
    })
    .await;
    assert_eq!(h.send(agent, "next").await, ActionOutcome::Done);
    let log = h.vendors.log()[from..].to_vec();
    let at = |entry: &str| log.iter().position(|logged| logged == entry);
    let (adopt, mode, model, send) = (
        at("adopt thread-1"),
        at("mode FullAccess"),
        at("model gpt-5.6-luna"),
        at("send next"),
    );
    assert!(
        adopt.is_some() && adopt < mode && mode < model && model < send,
        "{log:#?}"
    );
    assert_eq!(h.count("mode "), 1);
    assert_eq!(h.stored(agent).mode, PermissionMode::FullAccess);
}

#[tokio::test]
async fn a_slow_history_read_holds_up_no_other_agent() {
    let mut h = Harness::new();
    let (held, session) = h.start_claude(None).await;
    let uuid = Uuid::now_v7().to_string();
    let outside = h.outside(&uuid, Activity::Idle);
    let history = vec![reply(outside, "h1", "Before", ItemStatus::Completed)];
    h.vendors.set_history(&uuid, history);
    h.vendors.hold_reads(&uuid);
    h.phone.send(ClientMessage::OpenTimeline {
        agent: outside,
        since: None,
        limit: 50,
        lead: None,
    });
    eventually("the history read", || h.vendors.waiting_reads() == 1).await;

    // The read waits on its own thread while the other agent gets its
    // message.
    assert_eq!(h.send(held, "hello").await, ActionOutcome::Done);
    assert!(h.sent(session).contains(&"hello".to_owned()));
    h.phone
        .quiet(
            "the outside timeline",
            |message| matches!(message, ServerMessage::Timeline { agent, .. } if *agent == outside),
        )
        .await;

    h.vendors.release_reads(&uuid);
    let (_, items, _) = reset(h.phone.timeline(outside).await);
    assert_eq!(ids(&items), ["h1"]);
}

#[tokio::test]
async fn a_terminal_sessions_sub_agent_shows_its_own_transcript() {
    let mut h = Harness::new();
    let session = Uuid::now_v7().to_string();
    let lead = h.outside(&session, Activity::Busy);
    let sub = |id: &str, parent: AgentId| Agent {
        id: names::subagent(lead, id),
        parent: Some(parent),
        vendor_session_id: Some(id.into()),
        title: "Helper".into(),
        ..h.agent(lead)
    };
    let helper = sub("a1", lead);
    let inner = sub("a2", helper.id);
    h.hub.apply([&helper, &inner].map(|agent| Change::Upsert {
        value: Entity::Agent(agent.clone()),
    }));
    let (helper, inner) = (helper.id, inner.id);
    h.vendors.set_history(
        &session,
        vec![reply(lead, "l1", "Asking a helper", ItemStatus::Completed)],
    );
    let mut written = vec![reply(helper, "s1", "Looking", ItemStatus::Completed)];
    h.vendors
        .set_history(&format!("{session}/a1"), written.clone());
    h.vendors.set_history(
        &format!("{session}/a2"),
        vec![reply(inner, "n1", "Deep down", ItemStatus::Completed)],
    );

    let (_, items, _) = reset(h.open(helper, None, 50).await);
    assert_eq!(ids(&items), ["s1"]);
    let (_, items, _) = reset(h.open(inner, None, 50).await);
    assert_eq!(ids(&items), ["n1"]);

    // What the sub-agent writes next reaches its open timeline on the next
    // poll.
    written.push(reply(helper, "s2", "Found it", ItemStatus::Completed));
    h.vendors.set_history(&format!("{session}/a1"), written);
    tokio::time::sleep(QUIET).await;
    h.clock.advance(super::POLL_EVERY);
    let (_, events) = events(h.phone.timeline(helper).await);
    assert_eq!(event_ids(&events), ["s2"]);
}

#[tokio::test]
async fn a_terminal_codex_sessions_sub_agent_shows_its_own_thread() {
    let mut h = Harness::new();
    let lead = h.outside("thread-lead", Activity::Busy);
    let lead = Agent {
        vendor: Vendor::Codex,
        account: Some(h.codex),
        ..h.agent(lead)
    };
    let sub = Agent {
        id: AgentId::derive("thread-sub"),
        parent: Some(lead.id),
        vendor_session_id: Some("thread-sub".into()),
        ..lead.clone()
    };
    h.hub
        .apply([lead.clone(), sub.clone()].map(|agent| Change::Upsert {
            value: Entity::Agent(agent),
        }));
    h.vendors.set_history(
        "thread-lead",
        vec![reply(
            lead.id,
            "l1",
            "Asking a helper",
            ItemStatus::Completed,
        )],
    );
    h.vendors.set_history(
        "thread-sub",
        vec![reply(sub.id, "s1", "Looking", ItemStatus::Completed)],
    );
    let (_, items, _) = reset(h.open(sub.id, None, 50).await);
    assert_eq!(ids(&items), ["s1"]);
}

#[tokio::test]
async fn a_finished_sub_agent_opens_from_its_transcript_once() {
    let mut h = Harness::new();
    let session = Uuid::now_v7().to_string();
    let lead = h.outside(&session, Activity::Busy);
    // Only a1 runs still. a2 and the a3 it started finished, so they aren't
    // listed.
    let helper = Agent {
        id: names::subagent(lead, "a1"),
        parent: Some(lead),
        vendor_session_id: Some("a1".into()),
        ..h.agent(lead)
    };
    h.hub.apply([Change::Upsert {
        value: Entity::Agent(helper.clone()),
    }]);
    let (done, deep) = (names::subagent(lead, "a2"), names::subagent(lead, "a3"));
    let mut written = vec![reply(done, "d1", "Found it", ItemStatus::Completed)];
    h.vendors
        .set_history(&format!("{session}/a2"), written.clone());
    h.vendors.set_history(
        &format!("{session}/a3"),
        vec![reply(deep, "n1", "Deep down", ItemStatus::Completed)],
    );

    let (_, items, _) = reset(h.open_under(done, lead).await);
    assert_eq!(ids(&items), ["d1"]);
    // A sub-agent's own sub-agent names its parent, which leads to the same
    // session.
    let (_, items, _) = reset(h.open_under(deep, helper.id).await);
    assert_eq!(ids(&items), ["n1"]);

    // A finished transcript isn't read again while it's open.
    written.push(reply(done, "d2", "Later", ItemStatus::Completed));
    h.vendors.set_history(&format!("{session}/a2"), written);
    tokio::time::sleep(QUIET).await;
    h.clock.advance(super::POLL_EVERY);
    h.phone
        .quiet(
            "a poll of the finished sub-agent",
            |message| matches!(message, ServerMessage::Timeline { agent, .. } if *agent == done),
        )
        .await;
    let read = format!("{session}/a2");
    assert_eq!(h.vendors.reads().iter().filter(|r| **r == read).count(), 1);

    // A client that doesn't name the lead gets what it got before.
    let other = names::subagent(lead, "a9");
    let (_, items, _) = reset(h.open(other, None, 50).await);
    assert!(items.is_empty());
}

#[tokio::test]
async fn a_finished_sub_agent_goes_with_its_lead() {
    let mut h = Harness::new();
    let session = Uuid::now_v7().to_string();
    let lead = h.outside(&session, Activity::Idle);
    let done = names::subagent(lead, "a2");
    h.vendors.set_history(
        &format!("{session}/a2"),
        vec![reply(done, "d1", "Found it", ItemStatus::Completed)],
    );
    h.hub.apply([Change::Remove {
        key: EntityKey::Agent(lead),
    }]);

    let (_, items, _) = reset(h.open_under(done, lead).await);
    assert!(items.is_empty());
    assert!(h.vendors.reads().is_empty());
}

#[tokio::test]
async fn a_held_leads_finished_sub_agent_opens_after_a_restart() {
    let mut h = Harness::new();
    let (lead, _) = h.start_claude(None).await;
    let session = h.stored(lead).vendor_session;
    let done = names::subagent(lead, "t1");
    h.vendors.set_history(
        &format!("{session}/t1"),
        vec![reply(done, "d1", "Found it", ItemStatus::Completed)],
    );
    let (_, items, _) = reset(h.open_under(done, lead).await);
    assert_eq!(ids(&items), ["d1"]);

    h.restart().await;
    eventually("the lead back", || {
        h.hub.get(&EntityKey::Agent(lead)).is_some()
    })
    .await;
    let (_, items, _) = reset(h.open_under(done, lead).await);
    assert_eq!(ids(&items), ["d1"]);
}

#[tokio::test]
async fn a_slow_start_holds_up_no_other_agent() {
    let mut h = Harness::new();
    let (held, session) = h.start_claude(None).await;
    h.holders.hold_spawns();
    let spec = h.spec(Vendor::Claude, Some("Hi"));
    let key = IdempotencyKey::generate();
    h.phone.send(ClientMessage::StartAgent { key, spec });
    eventually("the second holder", || h.holders.sessions().len() == 2).await;
    let new = h.holders.last();

    // Messages to the agent that already runs don't wait for the spawn.
    assert_eq!(h.send(held, "hello").await, ActionOutcome::Done);
    assert!(h.sent(session).contains(&"hello".to_owned()));
    h.phone
        .quiet("the new agent", |message| {
            matches!(message, ServerMessage::ActionResult { key: got, .. } if *got == key)
        })
        .await;
    assert!(h.sent(new).is_empty());

    // Once the holder is up, the new agent starts and gets its first
    // message.
    h.holders.release_spawns();
    started(h.phone.result(key).await);
    eventually("the first message", || {
        h.sent(new).contains(&"Hi".to_owned())
    })
    .await;
}

#[tokio::test]
async fn actions_wait_in_order_for_their_agents_holder() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    h.holders.hold_spawns();
    let next = h.crash(session, 2).await;
    let keys = [IdempotencyKey::generate(), IdempotencyKey::generate()];
    for (key, text) in keys.into_iter().zip(["one", "two"]) {
        h.phone.send(ClientMessage::SendInput {
            key,
            agent,
            input: UserInput {
                text: text.into(),
                attachments: Vec::new(),
                refs: Vec::new(),
            },
            mode: SendMode::Queue,
        });
    }
    h.phone
        .quiet("an action result", |message| {
            matches!(message, ServerMessage::ActionResult { .. })
        })
        .await;

    h.holders.release_spawns();
    for key in keys {
        assert_eq!(h.phone.result(key).await, ActionOutcome::Done);
    }
    assert_eq!(h.sent(next), ["one", "two"]);
}

#[tokio::test]
async fn a_client_that_falls_behind_gets_a_reset_for_what_it_missed() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let (opened, _, _) = reset(h.open(agent, None, 50).await);

    // The phone reads nothing while far more changes come than its line
    // holds, and it sends a message in the middle.
    let from = h.vendors.log().len();
    let count = CLIENT_BACKLOG + 100;
    let mut seq = 0;
    for n in 0..count {
        let say = Say {
            timeline: vec![upsert(
                agent,
                reply(agent, &format!("r{n}"), "Hi", ItemStatus::Completed),
            )],
            ..Say::default()
        };
        seq = h.holders.say(session, &say);
    }
    h.read(from, seq).await;
    let key = IdempotencyKey::generate();
    h.phone.send(ClientMessage::SendInput {
        key,
        agent,
        input: UserInput {
            text: "hello".into(),
            attachments: Vec::new(),
            refs: Vec::new(),
        },
        mode: SendMode::Queue,
    });
    eventually("the message", || {
        h.sent(session).contains(&"hello".to_owned())
    })
    .await;

    // It gets what fit, then one reset that carries the rest.
    let mut last = opened;
    let mut updates = 0;
    let items = loop {
        match h.phone.timeline(agent).await {
            TimelineUpdate::Events { seq, .. } => {
                assert!(seq > last, "{seq:?} came after {last:?}");
                last = seq;
                updates += 1;
            }
            TimelineUpdate::Reset {
                seq,
                items,
                has_older,
            } => {
                assert!(seq > last, "{seq:?} came after {last:?}");
                assert!(has_older);
                break items;
            }
            other => panic!("expected events or a reset, got {other:?}"),
        }
    };
    assert!(updates <= CLIENT_BACKLOG, "{updates} updates");
    let newest = format!("r{}", count - 1);
    assert!(ids(&items).contains(&newest.as_str()), "{:?}", ids(&items));

    // The result waited rather than being dropped.
    assert_eq!(h.phone.result(key).await, ActionOutcome::Done);

    // Caught up, it gets events again.
    let say = Say {
        timeline: vec![upsert(
            agent,
            reply(agent, "after", "Bye", ItemStatus::Completed),
        )],
        ..Say::default()
    };
    h.say(session, &say).await;
    let update = h.phone.timeline(agent).await;
    let (_, got) = events(update);
    assert_eq!(event_ids(&got), ["after"]);
}

#[tokio::test]
async fn a_cursor_agent_runs_in_a_holder_of_its_own_and_loads_its_session_again() {
    let mut h = Harness::new();
    h.cursor_account(LoginKind::ConfigDir);
    let mut spec = h.spec(Vendor::Cursor, Some("Fix the build"));
    spec.title = Some("Build fix".into());
    let agent = started(h.act(|key| ClientMessage::StartAgent { key, spec }).await);
    let session = h.holders.last();
    eventually("the first message", || {
        h.sent(session).contains(&"Fix the build".to_owned())
    })
    .await;

    let launch = h.holders.launch(session);
    assert_eq!(launch.program, PathBuf::from("fake-cursor"));
    assert_eq!(launch.cwd, h.folder);
    // The Mac's CLI uses its own login, and a key from the shell can't
    // stand in for it.
    assert!(launch.secret_env.is_empty());
    assert!(launch
        .env_remove
        .iter()
        .any(|name| name == "CURSOR_API_KEY"));
    assert_eq!(launch.run_as, None);
    let kept = h.vendors.acp_kept();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].session, None);
    assert_eq!(kept[0].title.as_deref(), Some("Build fix"));
    assert_eq!(h.agent(agent).vendor, Vendor::Cursor);
    assert_eq!(h.stored(agent).vendor_session, "");
    assert_eq!(h.agent(agent).vendor_session_id, None);

    // The driver reports the ACP session once it's made.
    let say = Say {
        agents: vec![Agent {
            vendor_session_id: Some("acp-1".into()),
            ..h.reported(agent, Activity::Idle)
        }],
        ..Say::default()
    };
    h.say(session, &say).await;
    eventually("the session kept", || {
        h.agent(agent).vendor_session_id.as_deref() == Some("acp-1")
    })
    .await;

    // A crash brings a new CLI that loads it.
    let next = h.crash(session, 2).await;
    assert_ne!(next, session);
    let kept = h.vendors.acp_kept();
    assert_eq!(kept.len(), 2);
    assert_eq!(kept[1].session.as_deref(), Some("acp-1"));
    assert_eq!(kept[1].title.as_deref(), Some("Build fix"));
    assert_eq!(h.vendors.acp_launches().len(), 2);
    let rows: Vec<HolderRow<HoldRecord>> = h.db.holders().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].record.vendor, Vendor::Cursor);
    assert_eq!(rows[0].record.agent, Some(agent));
    assert_eq!(rows[0].record.vendor_session.as_deref(), Some("acp-1"));
}

/// A variable's value, with nothing real in it.
const ACP_VALUE: &str = "test-value-not-a-real-key";

#[tokio::test]
async fn a_local_acp_agent_runs_its_own_program_with_its_values_as_secrets() {
    let mut h = Harness::new();
    let account = h.acp_agent(false);
    let mut spec = h.spec(Vendor::Acp, Some("Fix the build"));
    spec.account = account;
    let agent = started(h.act(|key| ClientMessage::StartAgent { key, spec }).await);
    let session = h.holders.last();
    eventually("the first message", || {
        h.sent(session).contains(&"Fix the build".to_owned())
    })
    .await;

    let launch = h.holders.launch(session);
    assert_eq!(launch.program, PathBuf::from("gemini"));
    assert_eq!(launch.args, ["--experimental-acp"]);
    assert_eq!(launch.cwd, h.folder);
    assert_eq!(launch.run_as, None);
    assert_eq!(launch.secret_env.len(), 1);
    assert_eq!(launch.secret_env[0].0, "API_KEY");
    assert_eq!(launch.secret_env[0].1.expose(), ACP_VALUE);
    assert!(!format!("{launch:?}").contains(ACP_VALUE));
    // No credential maxd carries for another vendor reaches it.
    for name in [
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CURSOR_API_KEY",
        "OPENAI_API_KEY",
    ] {
        assert!(launch.env_remove.iter().any(|removed| removed == name));
    }
    assert_eq!(h.vendors.acp_launches().len(), 1);

    // It gets the agent tools, as Cursor does.
    let tools = h.vendors.acp_kept()[0].tools.clone();
    assert!(tools.is_some(), "a local agent here gets the agent tools");
    assert!(h.stored(agent).tools_key.is_some());
    assert_eq!(h.agent(agent).vendor, Vendor::Acp);
    let stored = serde_json::to_string(&h.stored(agent)).unwrap();
    assert!(!stored.contains(ACP_VALUE));
    let rows: Vec<HolderRow<HoldRecord>> = h.db.holders().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].record.vendor, Vendor::Acp);
    assert_eq!(rows[0].record.login, LoginKind::AcpCommand);
    assert!(!serde_json::to_string(&rows[0].record)
        .unwrap()
        .contains(ACP_VALUE));

    // A crash brings its own program back, which loads the session.
    let say = Say {
        agents: vec![Agent {
            vendor_session_id: Some("local-1".into()),
            ..h.reported(agent, Activity::Idle)
        }],
        ..Say::default()
    };
    h.say(session, &say).await;
    eventually("the session kept", || {
        h.agent(agent).vendor_session_id.as_deref() == Some("local-1")
    })
    .await;
    let next = h.crash(session, 2).await;
    assert_eq!(h.holders.launch(next).program, PathBuf::from("gemini"));
    assert_eq!(h.vendors.acp_kept()[1].session.as_deref(), Some("local-1"));
}

#[tokio::test]
async fn a_local_acp_agent_that_was_deleted_doesnt_start() {
    let mut h = Harness::new();
    let account = h.acp_agent(false);
    h.hub.apply([Change::Remove {
        key: EntityKey::AcpAgent(AcpAgentId::derive("gemini")),
    }]);
    let mut spec = h.spec(Vendor::Acp, Some("Hi"));
    spec.account = account;
    let outcome = h.act(|key| ClientMessage::StartAgent { key, spec }).await;
    assert!(reason(&outcome).contains("isn't set up"), "{outcome:?}");
    assert!(h.holders.sessions().is_empty());
    assert!(h.db.held::<Held>().unwrap().is_empty());
}

#[tokio::test]
async fn full_access_on_a_cursor_agent_starts_its_cli_again_on_the_same_session() {
    let mut h = Harness::new();
    h.cursor_account(LoginKind::ConfigDir);
    let spec = h.spec(Vendor::Cursor, Some("Fix the build"));
    let agent = started(h.act(|key| ClientMessage::StartAgent { key, spec }).await);
    let session = h.holders.last();
    let report = |h: &Harness, activity| Say {
        agents: vec![Agent {
            vendor_session_id: Some("acp-1".into()),
            ..h.reported(agent, activity)
        }],
        ..Say::default()
    };
    let set = |mode| move |key| ClientMessage::SetPermission { key, agent, mode };
    let sandbox = |launch: &crate::drive::Launch| {
        let args = &launch.args;
        let at = args.iter().position(|arg| arg == "--sandbox").unwrap();
        let on = args[at + 1].to_string_lossy().into_owned();
        (on, args.iter().any(|arg| arg == "--force"))
    };

    // A turn would be cut off, so it waits.
    h.say(session, &report(&h, Activity::Busy)).await;
    let outcome = h.act(set(PermissionMode::FullAccess)).await;
    assert!(
        reason(&outcome).contains("once the turn ends"),
        "{outcome:?}"
    );
    assert!(h.holders.stops(session).is_empty());

    // Idle, the CLI ends and a new one starts unsandboxed and loads it.
    h.say(session, &report(&h, Activity::Idle)).await;
    let outcome = h.act(set(PermissionMode::FullAccess)).await;
    assert_eq!(outcome, ActionOutcome::Done);
    eventually("the new CLI", || h.holders.sessions().len() == 2).await;
    assert_eq!(h.holders.stops(session), [STOP_GRACE]);
    let launches = h.vendors.acp_launches();
    assert_eq!(sandbox(&launches[0]), ("enabled".to_owned(), false));
    assert_eq!(sandbox(&launches[1]), ("disabled".to_owned(), true));
    assert_eq!(h.vendors.acp_kept()[1].session.as_deref(), Some("acp-1"));
    let stored = h.stored(agent);
    assert_eq!(stored.mode, PermissionMode::FullAccess);
    assert_eq!(stored.paused, None);

    // Plan mode is the CLI's own to change, in the same process, once
    // full access is off again.
    let session = h.holders.last();
    h.say(session, &report(&h, Activity::Idle)).await;
    assert_eq!(h.act(set(PermissionMode::Ask)).await, ActionOutcome::Done);
    eventually("the sandboxed CLI", || h.holders.sessions().len() == 3).await;
    let session = h.holders.last();
    h.say(session, &report(&h, Activity::Idle)).await;
    assert_eq!(h.act(set(PermissionMode::Plan)).await, ActionOutcome::Done);
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.holders.sessions().len(), 3);
    assert!(h.holders.stops(session).is_empty());
}

#[tokio::test]
async fn an_updated_codex_app_server_stops_once_idle_and_the_next_send_adopts_again() {
    let mut h = Harness::new();
    let (agent, server1) = h.start_codex("thread-1").await;

    // A busy thread keeps it running.
    h.sessions.recycle_codex();
    tokio::time::sleep(QUIET).await;
    assert!(h.holders.stops(server1).is_empty());

    // Once the thread is idle it stops.
    let say = Say {
        agents: vec![h.reported(agent, Activity::Idle)],
        ..Say::default()
    };
    h.say(server1, &say).await;
    eventually("the stop", || h.holders.stops(server1) == [STOP_GRACE]).await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.holders.sessions(), [server1]);

    // The next send starts a new app-server, which adopts the thread.
    let from = h.vendors.log().len();
    assert_eq!(h.send(agent, "go on").await, ActionOutcome::Done);
    assert_eq!(h.holders.sessions().len(), 2);
    let log = h.vendors.log()[from..].to_vec();
    assert!(
        log.iter().any(|logged| logged == "adopt thread-1"),
        "{log:#?}"
    );
    let server2 = h.holders.last();
    let sent = h.sent(server2);
    assert_eq!(sent.last().map(String::as_str), Some("go on"), "{sent:#?}");
}

/// A tool call showing the picture `id`, without a preview.
fn showing(agent: AgentId, id: &AttachmentId, status: ItemStatus) -> TimelineItem {
    let image = AttachmentRef {
        id: *id,
        name: "image.png".into(),
        kind: AttachmentKind::Image,
        bytes: 1,
        mime: Some("image/png".into()),
        path: "/path/to/image.png".into(),
        thumbnail: None,
    };
    let body = ItemBody::ToolCall {
        server: None,
        name: "screenshot".into(),
        title: None,
        input: None,
        output: None,
        images: vec![image],
        artifact: None,
    };
    item(agent, "shot", status, body)
}

fn thumbnail_shown(event: &TimelineEvent) -> Option<String> {
    let TimelineEvent::Upsert { item } = event else {
        panic!("not an upsert: {event:?}");
    };
    let ItemBody::ToolCall { images, .. } = &item.body else {
        panic!("not a tool call: {item:?}");
    };
    images[0].thumbnail.clone()
}

#[tokio::test]
async fn a_pictures_preview_fills_in_after_its_item_and_stays() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    h.open(agent, None, 50).await;
    let id = AttachmentId::derive("a picture");
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(4, 3, image::Rgb([200, 30, 30]))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    h.say(
        session,
        &Say {
            timeline: vec![(
                agent,
                TimelineEvent::Upsert {
                    item: showing(agent, &id, ItemStatus::Running),
                },
            )],
            previews: vec![(agent, id, png.into_inner())],
            ..Say::default()
        },
    )
    .await;

    // The item comes first, and the preview on its own update after it.
    let (_, first) = events(h.phone.timeline(agent).await);
    assert_eq!(thumbnail_shown(&first[0]), None);
    let (_, second) = events(h.phone.timeline(agent).await);
    assert!(thumbnail_shown(&second[0]).is_some());

    // The driver's copy has no preview, and an update from it keeps the one
    // shown.
    h.say(
        session,
        &Say {
            timeline: vec![(
                agent,
                TimelineEvent::Upsert {
                    item: showing(agent, &id, ItemStatus::Completed),
                },
            )],
            ..Say::default()
        },
    )
    .await;
    let (_, third) = events(h.phone.timeline(agent).await);
    assert!(thumbnail_shown(&third[0]).is_some());
}
