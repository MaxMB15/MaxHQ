// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Watches Claude Code sessions on this device.
//!
//! Each running session writes `<config>/sessions/<pid>.json` with its
//! status. Once a second a pass reads those files, keeps the ones whose
//! process still runs, and fills in the context, cache warmth and running
//! sub-agents from the session's transcript. Nothing here writes to a config
//! folder or talks to a session.

pub mod config;
pub(crate) mod process;
pub(crate) mod session;
mod subagent;
pub mod time;
pub(crate) mod transcript;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use maxhq_core::names;
use maxhq_protocol::ids::{AccountId, AgentId, DeviceId, WorkspaceId};
use maxhq_protocol::model::{Activity, Agent, AgentLink, Control, Vendor};
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use tokio::time::MissedTickBehavior;

use crate::hub::{FirstPass, Hub};
use config::ConfigDir;
use process::Processes;
use session::{Session, Status};
use subagent::{Metas, Subagent};
use transcript::{LastTurn, Tail};

/// How often to read the session files.
const EVERY: Duration = Duration::from_secs(1);

/// How long to wait before looking again for a transcript that wasn't there.
const LOOK_AGAIN: Duration = Duration::from_secs(5);

/// The title of a sub-agent whose meta file names no task or kind.
const SUBAGENT_TITLE: &str = "Sub-agent";

/// Reports what it sees to the hub until the process exits, and says so
/// through `first` after its first pass. Returns at once when this device
/// has nothing to watch.
pub async fn run(hub: Hub, mut first: FirstPass) {
    let configs = config::config_dirs();
    if configs.is_empty() {
        first.done();
        return;
    }
    let device = hub.device().id;
    let mut observer = Some(Observer::new(device, configs.clone()));
    let mut owned = HashSet::new();
    let mut tick = tokio::time::interval(EVERY);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let mut current = observer
            .take()
            .unwrap_or_else(|| Observer::new(device, configs.clone()));
        let handle = hub.clone();
        // A pass reads files and runs `ps`, so it runs off the async threads.
        let pass = tokio::task::spawn_blocking(move || {
            let entities = current.observe(Timestamp::now(), &mut Live(&handle));
            (current, entities)
        })
        .await;
        match pass {
            Ok((current, entities)) => {
                observer = Some(current);
                publish(&hub, &mut owned, entities);
                first.done();
            }
            // The next pass starts over with a fresh observer.
            Err(error) => tracing::error!(%error, "a pass over the Claude sessions failed"),
        }
    }
}

/// Makes the hub hold `entities` and drops whatever the last pass reported
/// that this one didn't, keeping a session that ended as an ended agent.
/// The scope is the last pass's own entities rather than every Claude agent
/// on this device, since the Incus observer reports Claude agents here too.
fn publish(hub: &Hub, owned: &mut HashSet<EntityKey>, entities: Vec<Entity>) {
    let keys: HashSet<EntityKey> = entities.iter().map(Entity::key).collect();
    hub.replace_sessions(|entity| owned.contains(&entity.key()), entities);
    *owned = keys;
}

/// What a pass needs from outside the config folders, so tests can stand in
/// for it.
pub trait World {
    /// Which of `pids` run, and since when.
    fn processes(&self, pids: &[u32]) -> Processes;
    /// Whether `maxd` started `pid` itself. The usage probe is a `claude`
    /// process, and it writes a session file like any other while it runs.
    fn started_here(&self, pid: u32) -> bool;
    /// The workspace for a folder, created the first time.
    fn locate(&mut self, folder: &Path) -> WorkspaceId;
}

/// The real world: `ps`, the usage probe and the hub.
struct Live<'a>(&'a Hub);

impl World for Live<'_> {
    fn processes(&self, pids: &[u32]) -> Processes {
        Processes::lookup(pids)
    }

    fn started_here(&self, pid: u32) -> bool {
        crate::usage::claude::is_probe(pid)
    }

    fn locate(&mut self, folder: &Path) -> WorkspaceId {
        self.0.locate_folder(folder)
    }
}

/// What one pass learns that the next can reuse.
pub struct Observer {
    /// This device.
    device: DeviceId,
    /// The config folders to read.
    configs: Vec<ConfigDir>,
    /// What's known about each running session's transcript, by session ID.
    leads: HashMap<String, Lead>,
    /// Each running sub-agent's transcript, by path.
    subagents: HashMap<PathBuf, Tail>,
    /// The sub-agent meta files read so far.
    metas: Metas,
}

/// What the observer keeps for one session.
#[derive(Debug, Default)]
struct Lead {
    /// Its transcript, once found.
    tail: Option<Tail>,
    /// When the transcript was last looked for.
    looked_at: Option<Instant>,
    /// The last unknown status logged, so each one is logged once.
    unknown: Option<String>,
}

/// What a pass has used, so the caches can drop the rest.
#[derive(Default)]
struct Used {
    /// Sub-agent transcripts.
    tails: HashSet<PathBuf>,
    /// `subagents` folders.
    dirs: HashSet<PathBuf>,
}

impl Observer {
    /// An observer for these config folders on `device`.
    pub fn new(device: DeviceId, configs: Vec<ConfigDir>) -> Self {
        Self {
            device,
            configs,
            leads: HashMap::new(),
            subagents: HashMap::new(),
            metas: Metas::default(),
        }
    }

    /// One pass: every running session and running sub-agent as an agent,
    /// and every waiting session's attention event.
    pub fn observe(&mut self, now: Timestamp, world: &mut impl World) -> Vec<Entity> {
        let found: Vec<(usize, Session)> = self
            .configs
            .iter()
            .enumerate()
            .flat_map(|(index, config)| {
                session::list(&config.path.join("sessions"))
                    .into_iter()
                    .map(move |session| (index, session))
            })
            .collect();
        let pids: Vec<u32> = found.iter().map(|(_, session)| session.pid).collect();
        let processes = world.processes(&pids);

        // A resumed session can leave the file of a process that died
        // without cleaning up, so keep the newest file per session.
        let mut running: HashMap<String, (usize, Session)> = HashMap::new();
        for (index, session) in found {
            if world.started_here(session.pid)
                || !processes.runs(session.pid, session.proc_start.as_deref())
            {
                continue;
            }
            let newer = running
                .get(&session.session_id)
                .is_none_or(|(_, kept)| session.freshness() > kept.freshness());
            if newer {
                running.insert(session.session_id.clone(), (index, session));
            }
        }
        let mut running: Vec<(usize, Session)> = running.into_values().collect();
        running.sort_by(|a, b| a.1.session_id.cmp(&b.1.session_id));

        let mut entities = Vec::new();
        let mut used = Used::default();
        for (index, session) in &running {
            let config = self.configs[*index].clone();
            self.report(&config, session, now, world, &mut used, &mut entities);
        }
        self.leads
            .retain(|id, _| running.iter().any(|(_, session)| session.session_id == *id));
        self.subagents.retain(|path, _| used.tails.contains(path));
        self.metas.retain(|dir| used.dirs.contains(dir));
        entities
    }

    /// Adds one running session's agent, its attention event and its
    /// running sub-agents to `out`.
    fn report(
        &mut self,
        config: &ConfigDir,
        session: &Session,
        now: Timestamp,
        world: &mut impl World,
        used: &mut Used,
        out: &mut Vec<Entity>,
    ) {
        let id = names::agent(Vendor::Claude, &session.session_id);
        let account = config.account(self.device);
        let workspace = world.locate(Path::new(&session.cwd));
        let lead = self.leads.entry(session.session_id.clone()).or_default();
        let last = lead.read(&config.path.join("projects"), session);
        let subagents_dir = lead
            .tail
            .as_ref()
            .and_then(|tail| tail.path().parent())
            .map(|dir| dir.join(&session.session_id).join("subagents"));

        let started_at = Timestamp(session.started_at.or(session.updated_at).unwrap_or(0));
        let running = match subagents_dir {
            // Before 2.1.301 an ended sub-agent looks the same as a running
            // one, so those versions show none.
            Some(dir) if subagent::marks_finished(session.version.as_deref()) => {
                let open = self.metas.open(&dir);
                used.dirs.insert(dir);
                open.into_iter()
                    .filter(|sub| sub.runs(started_at))
                    .collect()
            }
            _ => Vec::new(),
        };
        let lead_unknown = &mut lead.unknown;

        let attention = session.attention(id);
        let activity = match (&attention, session.status()) {
            (Some(event), _) => Activity::NeedsInput {
                attention: event.id,
            },
            (None, Status::Busy) if !running.is_empty() => Activity::BusyInSubagents,
            (None, Status::Busy | Status::Shell | Status::Waiting) => Activity::Busy,
            (None, Status::Idle) => match last.warm_until() {
                Some(warm_until) if now < warm_until => Activity::IdleCacheWarm { warm_until },
                _ => Activity::Idle,
            },
            (None, Status::Unknown) => {
                if *lead_unknown != session.status {
                    tracing::debug!(
                        status = session.status.as_deref().unwrap_or("none"),
                        session = %session.session_id,
                        "unknown Claude session status, shown as idle"
                    );
                    lead_unknown.clone_from(&session.status);
                }
                Activity::Idle
            }
        };
        let changed_at = session.status_changed_at();
        out.push(Entity::Agent(Agent {
            id,
            workspace,
            parent: None,
            vendor: Vendor::Claude,
            account: Some(account),
            model: None,
            permission: None,
            session: None,
            vendor_session_id: Some(session.session_id.clone()),
            control: Control::Observed,
            pid: Some(session.pid),
            link: AgentLink::Live,
            title: title(session),
            activity,
            context: last.context,
            started_at,
            last_activity_at: last.ended_at.map_or(changed_at, |at| at.max(changed_at)),
            branch: None,
            inbox: Default::default(),
            origin: None,
            pulls: Vec::new(),
            taking_over: false,
            asleep: false,
            effort: None,
            running: None,
            moved: None,
        }));
        if let Some(event) = attention {
            out.push(Entity::Attention(event));
        }
        for sub in &running {
            let agent = self.subagent(sub, &running, id, account, workspace, started_at, world);
            used.tails.insert(sub.transcript.clone());
            out.push(Entity::Agent(agent));
        }
    }

    /// The agent for one running sub-agent of the lead `lead`, which runs
    /// beside the other sub-agents in `running`.
    #[allow(clippy::too_many_arguments)]
    fn subagent(
        &mut self,
        sub: &Subagent,
        running: &[Subagent],
        lead: AgentId,
        account: AccountId,
        workspace: WorkspaceId,
        lead_started: Timestamp,
        world: &mut impl World,
    ) -> Agent {
        // A sub-agent started by another one sits under it while that one
        // runs, and under the lead otherwise.
        let parent = sub
            .meta
            .parent_agent_id
            .as_deref()
            .filter(|parent| running.iter().any(|other| other.id == *parent))
            .map_or(lead, |parent| names::subagent(lead, parent));
        let workspace = sub
            .meta
            .worktree_path
            .as_deref()
            .map(Path::new)
            .filter(|path| path.is_absolute())
            .map_or(workspace, |path| world.locate(path));
        let tail = self
            .subagents
            .entry(sub.transcript.clone())
            .or_insert_with(|| Tail::new(sub.transcript.clone(), false));
        // No transcript yet is fine. It shows up on a later pass.
        let last = tail.poll().cloned().unwrap_or_default();
        let touched = sub.touched_at.unwrap_or(lead_started);
        Agent {
            id: names::subagent(lead, &sub.id),
            workspace,
            parent: Some(parent),
            vendor: Vendor::Claude,
            account: Some(account),
            model: None,
            permission: None,
            session: None,
            // Its ID names its transcript, as a held sub-agent's task ID does.
            vendor_session_id: Some(sub.id.clone()),
            control: Control::Observed,
            pid: None,
            link: AgentLink::Live,
            title: sub
                .meta
                .description
                .clone()
                .or_else(|| sub.meta.agent_type.clone())
                .unwrap_or_else(|| SUBAGENT_TITLE.to_owned()),
            activity: Activity::Busy,
            context: last.context,
            started_at: sub.created_at.unwrap_or(touched),
            last_activity_at: last.ended_at.map_or(touched, |at| at.max(touched)),
            branch: None,
            inbox: Default::default(),
            origin: None,
            pulls: Vec::new(),
            taking_over: false,
            asleep: false,
            effort: None,
            running: None,
            moved: None,
        }
    }
}

impl Lead {
    /// What the session's transcript says now. A transcript that isn't
    /// there yet is looked for again every [`LOOK_AGAIN`].
    fn read(&mut self, projects: &Path, session: &Session) -> LastTurn {
        let look =
            self.tail.is_none() && self.looked_at.is_none_or(|at| at.elapsed() >= LOOK_AGAIN);
        if look {
            self.looked_at = Some(Instant::now());
            self.tail = find_transcript(projects, session).map(|path| Tail::new(path, true));
        }
        let Some(tail) = &mut self.tail else {
            return LastTurn::default();
        };
        match tail.poll() {
            Ok(last) => last.clone(),
            Err(error) => {
                tracing::debug!(%error, path = %tail.path().display(), "lost a Claude transcript");
                self.tail = None;
                LastTurn::default()
            }
        }
    }
}

/// A session's transcript: `projects/<sanitized cwd>/<sessionId>.jsonl`, or
/// failing that, the same file name in any folder under `projects`. Claude
/// Code shortens and hashes the folder name of a long path, and this doesn't
/// copy the hash.
fn find_transcript(projects: &Path, session: &Session) -> Option<PathBuf> {
    let file = format!("{}.jsonl", session.session_id);
    let direct = projects.join(sanitize(&session.cwd)).join(&file);
    if direct.is_file() {
        return Some(direct);
    }
    fs::read_dir(projects)
        .ok()?
        .flatten()
        .map(|entry| entry.path().join(&file))
        .find(|path| path.is_file())
}

/// The folder name Claude Code gives a working folder under `projects`. It
/// turns each UTF-16 unit that isn't an ASCII letter or digit into `-`.
fn sanitize(cwd: &str) -> String {
    let mut name = String::with_capacity(cwd.len());
    for c in cwd.chars() {
        if c.is_ascii_alphanumeric() {
            name.push(c);
        } else {
            name.extend(std::iter::repeat_n('-', c.len_utf16()));
        }
    }
    name
}

/// A session's title: its name, or the folder it runs in.
fn title(session: &Session) -> String {
    session
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .or_else(|| {
            Path::new(&session.cwd)
                .file_name()
                .and_then(|name| name.to_str())
        })
        .unwrap_or("Claude")
        .to_owned()
}

#[cfg(test)]
pub(crate) mod tests {
    use std::fs::File;
    use std::ops::Deref;
    use std::time::{SystemTime, UNIX_EPOCH};

    use maxhq_protocol::attention::AttentionKind;
    use maxhq_protocol::ids::{AttentionId, BootId};
    use maxhq_protocol::model::{ContextUsage, Device, WorkspaceLocation};
    use maxhq_protocol::state::Change;
    use uuid::Uuid;

    use super::*;

    /// A file or folder under `tests/fixtures/claude`.
    pub(crate) fn fixture(path: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/claude")
            .join(path)
    }

    /// An empty folder that's removed when the test ends.
    pub(crate) struct Scratch(PathBuf);

    impl Deref for Scratch {
        type Target = Path;

        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A new [`Scratch`] folder.
    pub(crate) fn scratch() -> Scratch {
        let dir = std::env::temp_dir().join(format!("maxd-claude-{}", Uuid::now_v7()));
        fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    /// Copies a folder tree, giving every file the modification time `at`.
    pub(crate) fn copy_tree(from: &Path, to: &Path, at: SystemTime) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap().flatten() {
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &target, at);
            } else {
                fs::copy(entry.path(), &target).unwrap();
                File::options()
                    .write(true)
                    .open(&target)
                    .unwrap()
                    .set_modified(at)
                    .unwrap();
            }
        }
    }

    const LEAD: &str = "6f1d2c3b-0a4e-4b8f-9c11-2d3e4f5a6b7c";
    const WAITING: &str = "9a8b7c6d-5e4f-4a3b-8c2d-1e0f9a8b7c6d";
    const IDLE: &str = "3c4d5e6f-7a8b-4c9d-8e0f-1a2b3c4d5e6f";
    const SHELL: &str = "0d1e2f3a-4b5c-4d6e-8f70-8192a3b4c5d6";

    /// When the fixture's files were last written: after every lead
    /// started, as a running sub-agent's transcript would be.
    fn written() -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(1_792_889_000_000)
    }

    /// The idle session's last reply, from its transcript.
    fn idle_ended() -> Timestamp {
        time::parse_iso8601("2026-10-25T00:30:00.000Z").unwrap()
    }

    fn device() -> DeviceId {
        DeviceId::derive("test")
    }

    /// The process table and the hub, made up.
    struct Fake {
        /// Each running PID and its start time.
        running: HashMap<u32, String>,
        /// PIDs `maxd` started.
        own: HashSet<u32>,
        /// The hub to locate folders through, if any.
        hub: Option<Hub>,
    }

    impl Fake {
        /// Every session in the fixture running as the process it names.
        fn all() -> Self {
            let running = [
                (41001, "Sat Oct 24 22:13:20 2026"),
                (41002, "Sun Oct 25 00:20:00 2026"),
                (41003, "Sat Oct 24 23:00:00 2026"),
                (41004, "Sun Oct 25 00:40:00 2026"),
            ];
            Self {
                running: running
                    .into_iter()
                    .map(|(pid, start)| (pid, start.to_owned()))
                    .collect(),
                own: HashSet::new(),
                hub: None,
            }
        }
    }

    impl World for Fake {
        fn processes(&self, pids: &[u32]) -> Processes {
            let table: String = pids
                .iter()
                .filter_map(|pid| Some(format!("{pid} {}\n", self.running.get(pid)?)))
                .collect();
            Processes::parse(&table)
        }

        fn started_here(&self, pid: u32) -> bool {
            self.own.contains(&pid)
        }

        fn locate(&mut self, folder: &Path) -> WorkspaceId {
            match &self.hub {
                Some(hub) => hub.locate_folder(folder),
                None => names::workspace(
                    device(),
                    &WorkspaceLocation::Folder {
                        path: folder.to_string_lossy().into_owned(),
                    },
                ),
            }
        }
    }

    /// The fixture config folder, copied so its times can be set.
    fn config() -> (Scratch, Observer) {
        let dir = scratch();
        copy_tree(&fixture("config"), &dir.join("claude"), written());
        let config = ConfigDir::custom(dir.join("claude"));
        let observer = Observer::new(device(), vec![config]);
        (dir, observer)
    }

    fn agents(entities: &[Entity]) -> HashMap<AgentId, Agent> {
        entities
            .iter()
            .filter_map(|entity| match entity {
                Entity::Agent(agent) => Some((agent.id, agent.clone())),
                _ => None,
            })
            .collect()
    }

    fn lead(session: &str) -> AgentId {
        names::agent(Vendor::Claude, session)
    }

    #[test]
    fn running_sessions_become_agents() {
        let (dir, mut observer) = config();
        let now = idle_ended().after(Duration::from_secs(120));
        let entities = observer.observe(now, &mut Fake::all());
        let agents = agents(&entities);

        let busy = &agents[&lead(LEAD)];
        assert_eq!(busy.title, "observer refactor");
        assert_eq!(busy.pid, Some(41001));
        assert_eq!(busy.parent, None);
        assert_eq!(busy.control, Control::Observed);
        assert_eq!(busy.link, AgentLink::Live);
        assert_eq!(busy.vendor_session_id.as_deref(), Some(LEAD));
        assert_eq!(busy.started_at, Timestamp(1_792_880_000_000));
        assert_eq!(
            busy.account,
            Some(ConfigDir::custom(dir.join("claude")).account(device()))
        );
        assert_eq!(busy.workspace, Fake::all().locate(Path::new("/work/maxhq")));
        assert_eq!(
            busy.context,
            Some(ContextUsage {
                used_tokens: 6 + 81_250 + 1_874,
                window_tokens: 200_000,
            })
        );
        assert_eq!(
            busy.last_activity_at,
            time::parse_iso8601("2026-10-25T00:49:20.281Z").unwrap()
        );
        assert_eq!(busy.activity, Activity::BusyInSubagents);

        assert_eq!(agents[&lead(SHELL)].activity, Activity::Busy);
        assert_eq!(
            agents[&lead(SHELL)].title,
            "maxhq",
            "no name, so the folder"
        );
        assert_eq!(agents[&lead(SHELL)].context, None, "no transcript yet");
        assert_eq!(agents[&lead(IDLE)].title, "notes");
    }

    #[test]
    fn a_waiting_session_needs_input() {
        let (_dir, mut observer) = config();
        let entities = observer.observe(Timestamp::now(), &mut Fake::all());
        let id = lead(WAITING);
        let event = entities
            .iter()
            .find_map(|entity| match entity {
                Entity::Attention(event) => Some(event.clone()),
                _ => None,
            })
            .expect("an attention event");
        assert_eq!(event.agent, id);
        assert_eq!(event.kind, AttentionKind::Permission);
        assert_eq!(event.summary, "permission prompt");
        assert_eq!(
            event.id,
            AttentionId::derive(&format!("{id}:1792889000000"))
        );
        assert_eq!(
            agents(&entities)[&id].activity,
            Activity::NeedsInput {
                attention: event.id
            }
        );
    }

    #[test]
    fn an_idle_session_stays_warm_for_its_cache_ttl() {
        let (_dir, mut observer) = config();
        let id = lead(IDLE);
        let warm = observer.observe(
            idle_ended().after(Duration::from_secs(120)),
            &mut Fake::all(),
        );
        assert_eq!(
            agents(&warm)[&id].activity,
            Activity::IdleCacheWarm {
                warm_until: idle_ended().after(Duration::from_secs(300)),
            }
        );
        let cold = observer.observe(
            idle_ended().after(Duration::from_secs(301)),
            &mut Fake::all(),
        );
        assert_eq!(agents(&cold)[&id].activity, Activity::Idle);
    }

    #[test]
    fn running_sub_agents_sit_under_their_parent() {
        let (_dir, mut observer) = config();
        let entities = observer.observe(Timestamp::now(), &mut Fake::all());
        let agents = agents(&entities);
        let parent = lead(LEAD);
        let first = names::subagent(parent, "a1f00d0000000001");
        let nested = names::subagent(parent, "a1f00d0000000004");
        let subs: Vec<&Agent> = agents.values().filter(|a| a.parent.is_some()).collect();
        assert_eq!(subs.len(), 2, "the finished and the stopped ones are gone");

        let first = &agents[&first];
        assert_eq!(first.parent, Some(parent));
        assert_eq!(first.title, "Map the session files");
        assert_eq!(first.activity, Activity::Busy);
        assert_eq!(first.pid, None);
        assert_eq!(first.vendor_session_id.as_deref(), Some("a1f00d0000000001"));
        assert_eq!(first.account, agents[&parent].account);
        assert_eq!(first.workspace, agents[&parent].workspace);
        assert_eq!(first.context.unwrap().used_tokens, 4 + 12_000 + 900);

        let nested = &agents[&nested];
        assert_eq!(nested.parent, Some(first.id));
        assert_eq!(nested.title, "Explore");
        assert_eq!(
            nested.workspace,
            Fake::all().locate(Path::new("/work/maxhq-wt"))
        );
    }

    #[test]
    fn sub_agents_end_with_their_lead() {
        let (_dir, mut observer) = config();
        let mut world = Fake::all();
        world.running.remove(&41001);
        let entities = observer.observe(Timestamp::now(), &mut world);
        assert!(agents(&entities)
            .values()
            .all(|agent| agent.parent.is_none() && agent.id != lead(LEAD)));
    }

    #[test]
    fn a_reused_pid_and_the_usage_probe_are_not_sessions() {
        let (_dir, mut observer) = config();
        let mut world = Fake::all();
        // A later process got the waiting session's PID.
        world
            .running
            .insert(41002, "Sun Oct 25 03:00:00 2026".to_owned());
        world.own.insert(41003);
        let entities = observer.observe(Timestamp::now(), &mut world);
        let agents = agents(&entities);
        assert!(!agents.contains_key(&lead(WAITING)));
        assert!(!agents.contains_key(&lead(IDLE)));
        assert!(agents.contains_key(&lead(LEAD)));
        assert!(!entities.iter().any(|e| matches!(e, Entity::Attention(_))));
    }

    #[test]
    fn an_unknown_status_is_idle() {
        let (dir, mut observer) = config();
        let path = dir.join("claude/sessions/41004.json");
        let text = fs::read_to_string(&path).unwrap();
        fs::write(&path, text.replace("\"shell\"", "\"pondering\"")).unwrap();
        let entities = observer.observe(Timestamp::now(), &mut Fake::all());
        assert_eq!(agents(&entities)[&lead(SHELL)].activity, Activity::Idle);
    }

    #[test]
    fn the_newest_file_for_a_session_wins() {
        let (dir, mut observer) = config();
        let sessions = dir.join("claude/sessions");
        let text = fs::read_to_string(sessions.join("41003.json")).unwrap();
        let older = text
            .replace("41003", "41009")
            .replace(
                "\"updatedAt\": 1792889400000",
                "\"updatedAt\": 1792880000000",
            )
            .replace("\"idle\"", "\"busy\"");
        fs::write(sessions.join("41009.json"), older).unwrap();
        let mut world = Fake::all();
        world
            .running
            .insert(41009, "Sat Oct 24 22:00:00 2026".to_owned());
        let entities = observer.observe(Timestamp::now(), &mut world);
        assert_eq!(agents(&entities)[&lead(IDLE)].pid, Some(41003));
    }

    #[test]
    fn ended_sessions_stay_until_settled() {
        let hub = Hub::new(
            Device {
                id: device(),
                name: "mac".into(),
                os: "macos".into(),
                boot_id: BootId("b".into()),
                maxd_version: "0.1.0".into(),
                winding_down_until: None,
                workspace_host: None,
                build: None,
                target: None,
            },
            HashMap::new(),
        );
        // Another observer's Claude agent, such as one in a container.
        let mut other = Fake::all();
        let elsewhere = Agent {
            id: lead("in-a-container"),
            workspace: other.locate(Path::new("/elsewhere")),
            parent: None,
            vendor: Vendor::Claude,
            account: None,
            model: None,
            permission: None,
            session: None,
            vendor_session_id: Some("in-a-container".into()),
            control: Control::Observed,
            pid: Some(7),
            link: AgentLink::Live,
            title: "elsewhere".into(),
            activity: Activity::Busy,
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
        };
        hub.apply([Change::Upsert {
            value: Entity::Agent(elsewhere),
        }]);

        let (_dir, mut observer) = config();
        let mut owned = HashSet::new();
        let mut world = Fake::all();
        world.hub = Some(hub.clone());
        publish(
            &hub,
            &mut owned,
            observer.observe(Timestamp::now(), &mut world),
        );
        let held = |hub: &Hub| -> Vec<EntityKey> {
            hub.snapshot()
                .entities
                .iter()
                .filter(|e| matches!(e, Entity::Agent(_) | Entity::Attention(_)))
                .map(Entity::key)
                .collect()
        };
        assert_eq!(held(&hub).len(), 1 + 4 + 2 + 1);

        world.running.remove(&41002);
        world.running.remove(&41001);
        publish(
            &hub,
            &mut owned,
            observer.observe(Timestamp::now(), &mut world),
        );
        // The two ended sessions stay, without their sub-agents and
        // attention events, so you can continue them.
        let left = held(&hub);
        assert_eq!(left.len(), 1 + 2 + 2, "{left:?}");
        assert!(left.contains(&EntityKey::Agent(lead("in-a-container"))));
        assert!(left.contains(&EntityKey::Agent(lead(IDLE))));
        assert!(left.contains(&EntityKey::Agent(lead(SHELL))));
        let ended = |hub: &Hub, session| match hub.get(&EntityKey::Agent(lead(session))) {
            Some(Entity::Agent(agent)) => agent,
            other => panic!("no agent: {other:?}"),
        };
        let shown = ended(&hub, LEAD);
        assert!(
            matches!(shown.link, AgentLink::Down { .. }),
            "{:?}",
            shown.link
        );
        assert_eq!((shown.pid, shown.activity), (None, Activity::Idle));
        assert_eq!(shown.control, Control::Observed);

        // Another pass keeps them, and settling one lets it go.
        publish(
            &hub,
            &mut owned,
            observer.observe(Timestamp::now(), &mut world),
        );
        assert_eq!(held(&hub).len(), 5);
        let settled = maxhq_protocol::model::Inbox {
            settled: Some(maxhq_protocol::model::Settled {
                at: Timestamp(3),
                by: maxhq_protocol::model::Settler::You,
            }),
            ..Default::default()
        };
        hub.set_inbox(lead(LEAD), settled);
        let left = held(&hub);
        assert_eq!(left.len(), 4, "{left:?}");
        assert!(!left.contains(&EntityKey::Agent(lead(LEAD))));
        assert_eq!(ended(&hub, WAITING).link, shown.link);

        // A session resumed in a terminal runs again.
        world
            .running
            .insert(41002, "Sun Oct 25 00:20:00 2026".to_owned());
        publish(
            &hub,
            &mut owned,
            observer.observe(Timestamp::now(), &mut world),
        );
        assert_eq!(ended(&hub, WAITING).link, AgentLink::Live);
    }

    #[test]
    fn sanitizes_folders_like_claude_code() {
        assert_eq!(sanitize("/work/maxhq"), "-work-maxhq");
        assert_eq!(
            sanitize("/Users/max/Code/Max AI_Manager.v2"),
            "-Users-max-Code-Max-AI-Manager-v2"
        );
        assert_eq!(sanitize("/tmp/café"), "-tmp-caf-");
        assert_eq!(sanitize("/tmp/🦀"), "-tmp---");
    }

    #[test]
    fn finds_a_transcript_outside_its_usual_folder() {
        let dir = scratch();
        let projects = dir.join("projects");
        let moved = projects.join("-work-maxhq-shortened-1x2y3z");
        fs::create_dir_all(&moved).unwrap();
        fs::write(moved.join(format!("{LEAD}.jsonl")), "").unwrap();
        let session =
            Session::parse(&fs::read(fixture("config/sessions/41001.json")).unwrap()).unwrap();
        assert_eq!(
            find_transcript(&projects, &session),
            Some(moved.join(format!("{LEAD}.jsonl")))
        );
        let found = find_transcript(&fixture("config/projects"), &session).unwrap();
        assert!(found.ends_with(format!("-work-maxhq/{LEAD}.jsonl")));
    }

    /// One pass over this device's real Claude config folders, printing
    /// the agents and attention events the hub ends up with. Titles and
    /// attention text are left out, since they can quote your prompts.
    #[test]
    #[ignore = "reads this device's real Claude sessions"]
    fn real_sessions() {
        let hub = Hub::new(
            Device {
                id: device(),
                name: "this device".into(),
                os: std::env::consts::OS.into(),
                boot_id: BootId("test".into()),
                maxd_version: env!("CARGO_PKG_VERSION").into(),
                winding_down_until: None,
                workspace_host: None,
                build: None,
                target: None,
            },
            HashMap::new(),
        );
        let configs = config::config_dirs();
        println!("config folders: {}", configs.len());
        let mut observer = Observer::new(device(), configs);
        let mut owned = HashSet::new();
        // The first pass reads each transcript, and later ones only what
        // was added.
        for pass in 1..=3 {
            let started = Instant::now();
            let entities = observer.observe(Timestamp::now(), &mut Live(&hub));
            println!("pass {pass} took {:?}", started.elapsed());
            publish(&hub, &mut owned, entities);
        }
        let entities = hub.snapshot().entities;
        let agents: Vec<&Agent> = entities
            .iter()
            .filter_map(|entity| match entity {
                Entity::Agent(agent) => Some(agent),
                _ => None,
            })
            .collect();
        let number = |id: AgentId| agents.iter().position(|agent| agent.id == id);
        for (index, agent) in agents.iter().enumerate() {
            println!(
                "#{index} parent={:?} pid={:?} activity={:?} context={:?} title=<{} chars> started_at={} last_activity_at={}",
                agent.parent.and_then(number),
                agent.pid,
                agent.activity,
                agent.context,
                agent.title.chars().count(),
                agent.started_at.0,
                agent.last_activity_at.0,
            );
        }
        for entity in &entities {
            if let Entity::Attention(event) = entity {
                println!(
                    "attention for #{:?} kind={:?} summary=<{} chars>",
                    number(event.agent),
                    event.kind,
                    event.summary.chars().count()
                );
            }
        }
    }
}
