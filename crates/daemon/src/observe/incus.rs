// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Watches agent workspaces in Incus containers on this device.
//!
//! Each workspace is a container named `ws-*` whose agent CLI reports
//! itself through a hook. The hook writes the agent's status to
//! `/run/agent-state/status.json` on every event, and the session ID and
//! folder to a session file that outlives restarts. The container's
//! `user.*` keys hold its title, repository and vendor. The observer lists
//! instances over the Incus socket, reads both files through the file API,
//! and listens to the event stream so starts and stops show up before the
//! next poll. Every call is a GET or a HEAD, so nothing here can change a
//! container.

mod api;
mod events;
// The fake serves on a Unix socket, as Incus does.
#[cfg(all(test, unix))]
mod fake;
pub(crate) mod look;
mod report;
mod time;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use hyper::body::Bytes;
use maxhq_protocol::ids::{AgentId, DeviceId};
use maxhq_protocol::model::{AgentLink, WorkspaceLocation};
use maxhq_protocol::state::Entity;
use maxhq_protocol::time::Timestamp;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use self::api::{ApiError, Client, Instance, Version};
use self::events::Lifecycle;
use self::report::{Seen, SessionFile, StatusFile};
use crate::hub::{FirstPass, Hub};

/// The Incus daemon's socket, which is `root:incus-admin 0660`.
const SOCKET: &str = "/var/lib/incus/unix.socket";

/// Where the hook writes the agent's status, on a tmpfs.
pub(crate) const STATUS_PATH: &str = "/run/agent-state/status.json";

/// Where the hook writes the session ID and folder.
pub(crate) const SESSION_PATH: &str = "/home/agent/.local/state/agent-workspace/session.json";

/// How often the observer looks again without an event. With the time a
/// pass takes, a change shows within the 3 seconds M1 allows.
const POLL: Duration = Duration::from_secs(2);

/// How long after a file's modification time a read must start before the
/// observer trusts the file's version. Incus reports the time to the
/// second, so a second write in the same second as a read looks unchanged
/// until then.
const SETTLE: Duration = Duration::from_secs(2);

/// Reports what it sees to the hub until the process exits, and says so
/// through `first` after the first pass that read every container. Returns
/// at once when this device has nothing to watch.
pub async fn run(hub: Hub, first: FirstPass) {
    watch(hub, Path::new(SOCKET), POLL, first).await;
}

/// Polls the Incus daemon at `socket` every `poll`, and on every lifecycle
/// event. Returns at once when there's no socket, as on a Mac.
async fn watch(hub: Hub, socket: &Path, poll: Duration, mut first: FirstPass) {
    if !socket.exists() {
        tracing::debug!(socket = %socket.display(), "no Incus here, so no containers to watch");
        first.done();
        return;
    }
    let (sender, mut events) = mpsc::channel(64);
    let _listener = AbortOnDrop(tokio::spawn(events::watch(socket.to_owned(), sender)));
    let mut observer = Observer::new(hub, socket);
    let mut tick = tokio::time::interval(poll);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            Some(event) = events.recv() => {
                observer.note(event);
                while let Ok(event) = events.try_recv() {
                    observer.note(event);
                }
                tick.reset();
            }
        }
        if observer.pass(Timestamp::now()).await {
            first.done();
        }
    }
}

/// Whether an instance name is a workspace's.
fn is_workspace_name(name: &str) -> bool {
    name.starts_with("ws-")
}

/// Stops a task when dropped, so the event listener ends with the observer.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The last copy of a file read from a container.
#[derive(Clone)]
struct Cached {
    /// The version read. None when the file didn't exist.
    version: Option<Version>,
    /// The contents. None when the file didn't exist.
    body: Option<Bytes>,
    /// Whether a later write would change the version, so a version that
    /// matches means the contents do too.
    settled: bool,
}

impl Cached {
    /// The copy for a file that doesn't exist.
    fn missing() -> Self {
        Self {
            version: None,
            body: None,
            settled: true,
        }
    }
}

/// The two files read from one container.
struct Read {
    session: SessionFile,
    status: Option<StatusFile>,
    fresh: bool,
}

/// What the observer keeps between passes.
struct Observer {
    hub: Hub,
    client: Client,
    /// The last copy of each file, by instance and path.
    files: HashMap<(String, &'static str), Cached>,
    /// The workspace containers that ran at the last pass.
    running: HashSet<String>,
    /// When each stopped workspace container went down, if the observer
    /// saw it go.
    down: HashMap<String, Option<Timestamp>>,
    /// Each agent's last reported activity time.
    history: HashMap<AgentId, Timestamp>,
    /// The agents reported at the last pass, which the next replace covers.
    owned: HashSet<AgentId>,
    /// The entities reported at the last pass, to report again with the
    /// links unknown while Incus doesn't answer.
    last: Vec<Entity>,
    /// The failure logged last, so one that lasts is logged once.
    failure: Option<String>,
    /// Whether a file couldn't be read this pass.
    unread: bool,
}

impl Observer {
    /// An observer that reports into `hub` and reads Incus at `socket`.
    fn new(hub: Hub, socket: &Path) -> Self {
        Self {
            hub,
            client: Client::new(socket),
            files: HashMap::new(),
            running: HashSet::new(),
            down: HashMap::new(),
            history: HashMap::new(),
            owned: HashSet::new(),
            last: Vec::new(),
            failure: None,
            unread: false,
        }
    }

    /// Takes in a lifecycle event before the pass it triggers. A stop
    /// carries the time the container went down.
    fn note(&mut self, event: Lifecycle) {
        if let Some(at) = event.stopped_at {
            self.down.insert(event.instance, Some(at));
        }
    }

    /// Reads every workspace container and replaces what this observer
    /// reported before. Returns whether Incus answered and every file
    /// read, so no container's agent is missing for want of a read.
    async fn pass(&mut self, now: Timestamp) -> bool {
        let instances = match self.client.instances().await {
            Ok(instances) => instances,
            Err(error) => {
                self.unreachable(&error);
                return false;
            }
        };
        self.unread = false;
        if self.failure.take().is_some() {
            tracing::info!("Incus answers again");
        }
        let workspaces: Vec<&Instance> = instances
            .iter()
            .filter(|i| i.kind == "container" && is_workspace_name(&i.name))
            .collect();

        let device = self.hub.device().id;
        let mut entities = Vec::new();
        let mut history = HashMap::new();
        for instance in &workspaces {
            let down_since = self.down_since(instance, now);
            let Some(read) = self.read(instance).await else {
                continue;
            };
            let seen = Seen {
                instance,
                session: &read.session,
                status: read.status.as_ref(),
                fresh: read.fresh,
                down_since,
            };
            let Some(report) = report::report(device, &seen, &self.history, now) else {
                continue;
            };
            history.insert(report.agent.id, report.agent.last_activity_at);
            entities.push(Entity::Project(report.project));
            entities.push(Entity::Workspace(report.workspace));
            entities.push(Entity::Agent(report.agent));
            entities.extend(report.attention.map(Entity::Attention));
        }

        let present = |name: &str| workspaces.iter().any(|i| i.name == name);
        self.files.retain(|(name, _), _| present(name));
        self.down.retain(|name, _| present(name));
        self.running = workspaces
            .iter()
            .filter(|i| i.is_running())
            .map(|i| i.name.clone())
            .collect();
        self.history = history;
        self.last.clone_from(&entities);
        self.publish(entities);
        !self.unread
    }

    /// Reports the last pass again with every link unknown, since the app
    /// says unknown when it can't tell rather than calling agents gone.
    fn unreachable(&mut self, error: &ApiError) {
        let message = error.to_string();
        if self.failure.as_ref() != Some(&message) {
            tracing::warn!(%error, "can't list Incus instances");
            self.failure = Some(message);
        }
        let entities = self
            .last
            .iter()
            .cloned()
            .map(|entity| match entity {
                Entity::Agent(mut agent) => {
                    agent.link = AgentLink::Unknown;
                    Entity::Agent(agent)
                }
                other => other,
            })
            .collect();
        self.publish(entities);
    }

    /// Replaces this observer's entities as a set, so agents in deleted
    /// containers and prompts that were answered go away.
    fn publish(&mut self, entities: Vec<Entity>) {
        let current: HashSet<AgentId> = entities
            .iter()
            .filter_map(|entity| match entity {
                Entity::Agent(agent) => Some(agent.id),
                _ => None,
            })
            .collect();
        let owned: HashSet<AgentId> = self.owned.union(&current).copied().collect();
        let device = self.hub.device().id;
        self.hub
            .replace(|entity| owns(entity, device, &owned), entities);
        self.owned = current;
    }

    /// When a stopped container went down: the time its stop event gave,
    /// or the pass that first saw it stopped. None for a container that
    /// was already stopped when the observer started, or that runs.
    fn down_since(&mut self, instance: &Instance, now: Timestamp) -> Option<Timestamp> {
        if instance.is_running() {
            self.down.remove(&instance.name);
            return None;
        }
        let saw_it_run = self.running.contains(&instance.name);
        *self
            .down
            .entry(instance.name.clone())
            .or_insert(saw_it_run.then_some(now))
    }

    /// Reads a container's session and status files. None when it has no
    /// session file, or none that reads, since then there's no agent to
    /// report.
    async fn read(&mut self, instance: &Instance) -> Option<Read> {
        let (session, session_fresh) = self.file(instance, SESSION_PATH).await;
        let session: SessionFile = match serde_json::from_slice(&session?) {
            Ok(session) => session,
            Err(error) => {
                tracing::debug!(%error, instance = %instance.name, "unreadable session file");
                return None;
            }
        };
        // A stopped container has no status, since the file is on a tmpfs.
        let (status, status_fresh) = if instance.is_running() {
            self.file(instance, STATUS_PATH).await
        } else {
            (None, true)
        };
        // The hook replaces the file whole, so one that doesn't read is
        // treated as not written yet.
        let status = status.and_then(|body| serde_json::from_slice(&body).ok());
        Some(Read {
            session,
            status,
            fresh: session_fresh && status_fresh,
        })
    }

    /// A file's contents, and whether they were read this pass. When the
    /// read fails, the last copy stands in.
    async fn file(&mut self, instance: &Instance, path: &'static str) -> (Option<Bytes>, bool) {
        let key = (instance.name.clone(), path);
        let cached = self.files.get(&key).cloned();
        match self.fetch(instance, path, cached.clone()).await {
            Ok(copy) => {
                let body = copy.body.clone();
                self.files.insert(key, copy);
                (body, true)
            }
            Err(error) => {
                tracing::debug!(%error, instance = %instance.name, path, "can't read a workspace file");
                self.unread = true;
                (cached.and_then(|copy| copy.body), false)
            }
        }
    }

    /// Reads a file only when it may have changed. A running container's
    /// file gets a HEAD first, and a GET only when its version moved. A
    /// stopped container's files can't change, so one read lasts until it
    /// starts again.
    async fn fetch(
        &mut self,
        instance: &Instance,
        path: &'static str,
        cached: Option<Cached>,
    ) -> Result<Cached, ApiError> {
        let name = instance.name.as_str();
        if !instance.is_running() {
            if let Some(copy) = cached.filter(|copy| copy.settled) {
                return Ok(copy);
            }
            let read = self.client.read(name, path).await?;
            return Ok(read.map_or_else(Cached::missing, |file| Cached {
                version: Some(file.version),
                body: Some(file.body),
                settled: true,
            }));
        }
        let version = self.client.version(name, path).await?;
        if let Some(copy) = cached.filter(|copy| copy.settled && copy.version == version) {
            return Ok(copy);
        }
        if version.is_none() {
            return Ok(Cached::missing());
        }
        let started = Timestamp::now();
        let Some(file) = self.client.read(name, path).await? else {
            return Ok(Cached::missing());
        };
        let settled = file
            .version
            .modified
            .is_some_and(|modified| started >= modified.after(SETTLE));
        Ok(Cached {
            version: Some(file.version),
            body: Some(file.body),
            settled,
        })
    }
}

/// Whether an entity is this observer's: a `ws-*` container workspace on
/// this device, or an agent it reported or its prompt. The `mhq-*`
/// workspaces belong to [`crate::workspace`].
fn owns(entity: &Entity, device: DeviceId, agents: &HashSet<AgentId>) -> bool {
    match entity {
        Entity::Workspace(workspace) => {
            workspace.device == device
                && matches!(
                    &workspace.location,
                    WorkspaceLocation::Container { instance, .. } if is_workspace_name(instance)
                )
        }
        Entity::Agent(agent) => agents.contains(&agent.id),
        Entity::Attention(event) => agents.contains(&event.agent),
        _ => false,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::time::Instant;

    use maxhq_core::names;
    use maxhq_protocol::ids::BootId;
    use maxhq_protocol::model::{Activity, Agent, Device, Vendor};
    use serde_json::Value;

    use super::fake::{FakeFile, FakeIncus, State};
    use super::*;

    const INSTANCES: &str = include_str!("../../tests/fixtures/incus/instances.json");

    fn hub() -> Hub {
        Hub::new(
            Device {
                id: DeviceId::derive("devbox"),
                name: "devbox".into(),
                os: "linux".into(),
                boot_id: BootId("b".into()),
                maxd_version: "0.1.0".into(),
                winding_down_until: None,
                workspace_host: None,
                build: None,
                target: None,
            },
            HashMap::new(),
        )
    }

    fn file(modified: &str, body: &str) -> FakeFile {
        FakeFile {
            modified: modified.into(),
            body: body.as_bytes().to_vec(),
        }
    }

    /// The fixture containers, with the files each one holds. `ws-quill`
    /// has no status file, as on the real host.
    fn fixtures() -> State {
        let modified = "2026-10-24 09:01:03 +0000 UTC";
        let files = [
            (
                "ws-harbor",
                SESSION_PATH,
                include_str!("../../tests/fixtures/incus/session-harbor.json"),
            ),
            (
                "ws-harbor",
                STATUS_PATH,
                include_str!("../../tests/fixtures/incus/status-harbor.json"),
            ),
            (
                "ws-lantern",
                SESSION_PATH,
                include_str!("../../tests/fixtures/incus/session-lantern.json"),
            ),
            (
                "ws-lantern",
                STATUS_PATH,
                include_str!("../../tests/fixtures/incus/status-lantern.json"),
            ),
            (
                "ws-quill",
                SESSION_PATH,
                include_str!("../../tests/fixtures/incus/session-quill.json"),
            ),
            (
                "ws-tidepool",
                SESSION_PATH,
                include_str!("../../tests/fixtures/incus/session-tidepool.json"),
            ),
        ];
        State {
            instances: serde_json::from_str(INSTANCES).unwrap(),
            files: files
                .into_iter()
                .map(|(instance, path, body)| {
                    ((instance.to_owned(), path.to_owned()), file(modified, body))
                })
                .collect(),
            ..State::default()
        }
    }

    fn agents(hub: &Hub) -> HashMap<String, Agent> {
        hub.snapshot()
            .entities
            .into_iter()
            .filter_map(|entity| match entity {
                Entity::Agent(agent) => Some((agent.title.clone(), agent)),
                _ => None,
            })
            .collect()
    }

    /// Waits up to five seconds for `done` to hold.
    async fn until(mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < deadline, "timed out");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn set_status(state: &mut State, name: &str, status: &str) {
        let list = state.instances["metadata"].as_array_mut().unwrap();
        let instance = list.iter_mut().find(|i| i["name"] == name).unwrap();
        instance["status"] = Value::from(status);
    }

    #[tokio::test]
    async fn returns_at_once_without_incus() {
        let missing = std::env::temp_dir().join("maxd-no-incus-here.sock");
        let hub = hub();
        let first = hub.first_pass();
        tokio::time::timeout(
            Duration::from_secs(1),
            watch(hub.clone(), &missing, POLL, first),
        )
        .await
        .expect("watch returned");
        assert!(hub.observers_reported(), "there was nothing to report");
    }

    #[tokio::test]
    async fn a_pass_reports_everything_only_when_every_file_reads() {
        let incus = FakeIncus::start(fixtures());
        let lantern = ("ws-lantern".to_owned(), SESSION_PATH.to_owned());
        incus.state().broken.insert(lantern.clone());
        let hub = hub();
        let mut observer = Observer::new(hub.clone(), &incus.socket);
        assert!(
            !observer.pass(Timestamp::now()).await,
            "lantern's agent may be missing"
        );
        assert!(!agents(&hub).contains_key("lantern"));

        incus.state().broken.remove(&lantern);
        assert!(observer.pass(Timestamp::now()).await);
        assert!(agents(&hub).contains_key("lantern"));

        let socket = incus.socket.clone();
        drop(incus);
        observer.client = Client::new(&socket);
        assert!(
            !observer.pass(Timestamp::now()).await,
            "not while Incus doesn't answer"
        );
    }

    #[tokio::test]
    async fn reports_every_workspace_and_follows_events() {
        let incus = FakeIncus::start(fixtures());
        let hub = hub();
        // An hour between polls, so only events start passes after the
        // first.
        let task = tokio::spawn({
            let hub = hub.clone();
            let socket = incus.socket.clone();
            let first = hub.first_pass();
            async move { watch(hub, &socket, Duration::from_secs(3600), first).await }
        });
        until(|| agents(&hub).len() == 4).await;

        let found = agents(&hub);
        let lantern = &found["lantern"];
        assert_eq!(lantern.link, AgentLink::Live);
        assert_eq!(
            lantern.id,
            names::agent(Vendor::Claude, "0c9b8a7d-6e5f-4a3b-8c2d-1e0f9a8b7c6d")
        );
        assert_eq!(found["Harbor"].activity, Activity::Idle);
        assert_eq!(found["Harbor"].link, AgentLink::Live);
        assert_eq!(found["quill"].activity, Activity::Idle);
        assert_eq!(found["quill"].link, AgentLink::Unknown);
        assert_eq!(found["TIDEPOOL"].activity, Activity::NotAccessible);
        assert_eq!(
            found["TIDEPOOL"].link,
            AgentLink::Down {
                reason: "container stopped".into(),
                since: None,
            }
        );
        let snapshot = hub.snapshot();
        let workspaces = snapshot
            .entities
            .iter()
            .filter(|e| matches!(e, Entity::Workspace(_)))
            .count();
        let projects = snapshot
            .entities
            .iter()
            .filter(|e| matches!(e, Entity::Project(_)))
            .count();
        assert_eq!((workspaces, projects), (4, 4));

        // Every file was read once.
        let gets = incus.state().gets.clone();
        assert_eq!(gets.len(), 6);
        assert!(gets.values().all(|&n| n == 1), "{gets:?}");

        // Harbor stops, and its event starts a pass.
        until(|| incus.listeners() == 1).await;
        set_status(&mut incus.state(), "ws-harbor", "Stopped");
        let heads = incus.state().heads;
        let stopped = include_str!("../../tests/fixtures/incus/events.jsonl")
            .lines()
            .nth(1)
            .unwrap();
        incus.send_event(stopped);
        until(|| matches!(agents(&hub)["Harbor"].link, AgentLink::Down { .. })).await;
        assert_eq!(
            agents(&hub)["Harbor"].link,
            AgentLink::Down {
                reason: "container stopped".into(),
                since: Some(Timestamp(1_792_891_025_402)),
            }
        );
        // The pass checked the running containers' files with HEADs and
        // read nothing again.
        assert!(incus.state().heads > heads);
        assert_eq!(incus.state().gets, gets);

        // Quill's container goes away, and its agent and workspace with it.
        {
            let mut state = incus.state();
            let list = state.instances["metadata"].as_array_mut().unwrap();
            list.retain(|i| i["name"] != "ws-quill");
        }
        incus.send_event(
            r#"{"type":"lifecycle","timestamp":"2026-10-25T02:20:00+01:00","metadata":{"action":"instance-deleted","name":"ws-quill"}}"#,
        );
        until(|| agents(&hub).len() == 3).await;
        let workspaces = hub
            .snapshot()
            .entities
            .iter()
            .filter(|e| matches!(e, Entity::Workspace(_)))
            .count();
        assert_eq!(workspaces, 3);

        task.abort();
    }

    #[tokio::test]
    async fn a_lost_daemon_leaves_the_links_unknown() {
        let incus = FakeIncus::start(fixtures());
        let hub = hub();
        let mut observer = Observer::new(hub.clone(), &incus.socket);
        observer.pass(Timestamp::now()).await;
        assert_eq!(agents(&hub)["lantern"].link, AgentLink::Live);

        let socket = incus.socket.clone();
        drop(incus);
        observer.client = Client::new(&socket);
        observer.pass(Timestamp::now()).await;
        let found = agents(&hub);
        assert_eq!(found.len(), 4);
        assert!(found.values().all(|a| a.link == AgentLink::Unknown));
    }

    #[tokio::test]
    async fn a_new_status_is_read_again() {
        let incus = FakeIncus::start(fixtures());
        let hub = hub();
        let mut observer = Observer::new(hub.clone(), &incus.socket);
        observer.pass(Timestamp::now()).await;

        let working = r#"{"agent":"claude","status":"working","event":"UserPromptSubmit","notification":"","updated":"2026-10-24T09:05:00+00:00"}"#;
        incus.state().files.insert(
            ("ws-lantern".into(), STATUS_PATH.into()),
            file("2026-10-24 09:05:00 +0000 UTC", working),
        );
        observer.pass(Timestamp::now()).await;
        assert_eq!(agents(&hub)["lantern"].activity, Activity::Busy);
        let key = ("ws-lantern".to_owned(), STATUS_PATH.to_owned());
        assert_eq!(incus.state().gets[&key], 2);
    }

    #[test]
    fn owns_only_its_own_entities() {
        let device = DeviceId::derive("devbox");
        let workspace = |location| {
            Entity::Workspace(maxhq_protocol::model::Workspace {
                id: names::workspace(device, &location),
                project: None,
                device,
                location,
                managed: None,
                clone: None,
            })
        };
        let none = HashSet::new();
        let container = WorkspaceLocation::Container {
            instance: "ws-a".into(),
            path: "/home/agent".into(),
        };
        let folder = WorkspaceLocation::Folder {
            path: "/home/dev/a".into(),
        };
        let managed = WorkspaceLocation::Container {
            instance: "mhq-a".into(),
            path: "/home/agent/work/a".into(),
        };
        assert!(owns(&workspace(container), device, &none));
        assert!(!owns(&workspace(folder), device, &none));
        assert!(!owns(&workspace(managed), device, &none));
    }
}
