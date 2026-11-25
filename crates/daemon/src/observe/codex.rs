// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Watches Codex sessions on this device. Every 2 seconds it lists the
//! `codex` processes you started, asks which rollout files each holds open,
//! and reads what was added to those files since the last look. Each open
//! rollout is one thread, so one agent.

mod clock;
mod process;
pub(crate) mod rollout;

use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::time::Duration;

use maxhq_core::names;
use maxhq_protocol::ids::{AccountId, AgentId, DeviceId};
use maxhq_protocol::model::{Activity, Agent, AgentLink, Control, Vendor};
use maxhq_protocol::state::Entity;
use maxhq_protocol::time::Timestamp;
use serde::Deserialize;
use tokio::time::MissedTickBehavior;

use crate::hub::{FirstPass, Hub};
use crate::usage::codex::{self as usage, limits::Reading, Home};
use clock::SystemClock;
use rollout::{Rollout, Source, Tail, Tailed};

/// How often the observer looks.
pub const POLL_EVERY: Duration = Duration::from_secs(2);

/// The title of a session with no name and no prompt yet.
const UNTITLED: &str = "Codex";

/// How deep sub-agents can nest before the observer stops following
/// parents. Codex allows far fewer.
const MAX_DEPTH: usize = 16;

/// Reports what it sees to the hub until the process exits, and says so
/// through `first` after its first pass. Returns at once when this device
/// has nothing to watch.
pub async fn run(hub: Hub, mut first: FirstPass) {
    let homes = usage::homes();
    if homes.is_empty() {
        first.done();
        return;
    }
    let mut watcher = Some(Watcher::new(homes));
    let mut ticks = tokio::time::interval(POLL_EVERY);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        ticks.tick().await;
        let Some(mut current) = watcher.take() else {
            return;
        };
        let hub = hub.clone();
        // `ps` and `lsof` block, so each pass runs off the async threads.
        let pass = tokio::task::spawn_blocking(move || {
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                current.pass(&hub, Timestamp::now());
            }));
            if result.is_err() {
                tracing::warn!("codex observer pass panicked");
            }
            (current, result.is_ok())
        });
        match pass.await {
            Ok((current, passed)) => {
                watcher = Some(current);
                if passed {
                    first.done();
                }
            }
            // Only a runtime shutting down cancels a blocking task.
            Err(_) => return,
        }
    }
}

/// One rollout a process holds open.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Open {
    /// The rollout file.
    path: PathBuf,
    /// The process that holds it.
    pid: u32,
    /// That process's working folder.
    cwd: Option<PathBuf>,
}

/// Each open thread's rollout and the process holding it, by thread ID.
type Threads<'a> = HashMap<String, (&'a Open, &'a Rollout)>;

/// A rollout being read.
#[derive(Debug, Default)]
struct Tracked {
    /// Where the last read stopped.
    tail: Tail,
    /// What the lines so far say.
    rollout: Rollout,
}

/// The thread names in one home's `session_index.jsonl`, which Codex
/// appends to when you name a thread.
#[derive(Debug, Default)]
struct Titles {
    /// Where the last read stopped.
    tail: Tail,
    /// Each thread's latest name, by thread ID.
    names: HashMap<String, String>,
}

/// One line of `session_index.jsonl`.
#[derive(Deserialize)]
struct IndexLine {
    /// The thread ID.
    id: String,
    /// The name you gave it.
    thread_name: Option<String>,
}

/// What the observer remembers between passes.
struct Watcher {
    /// The `CODEX_HOME` folders `maxd` has accounts for.
    homes: Vec<Home>,
    /// `maxd` itself, whose app-servers aren't sessions.
    own_pid: u32,
    /// The open rollouts, by path.
    rollouts: HashMap<PathBuf, Tracked>,
    /// The thread names in each home that has an open rollout.
    titles: HashMap<PathBuf, Titles>,
    /// The agents the last pass reported, which this observer owns.
    reported: HashSet<AgentId>,
    /// Turns the local times in limit messages into instants.
    clock: SystemClock,
}

impl Watcher {
    /// A watcher that has seen nothing yet.
    fn new(homes: Vec<Home>) -> Self {
        Self {
            homes,
            own_pid: std::process::id(),
            rollouts: HashMap::new(),
            titles: HashMap::new(),
            reported: HashSet::new(),
            clock: SystemClock,
        }
    }

    /// Looks at the device once and reports what it found.
    fn pass(&mut self, hub: &Hub, now: Timestamp) {
        let processes = process::list();
        let pids = process::sessions(&processes, self.own_pid);
        let files = process::open_files(&pids);
        let mut open: Vec<Open> = pids
            .iter()
            .filter_map(|pid| Some((*pid, files.get(pid)?)))
            .flat_map(|(pid, files)| {
                files.rollouts.iter().map(move |path| Open {
                    path: path.clone(),
                    pid,
                    cwd: files.cwd.clone(),
                })
            })
            .collect();
        open.sort();
        open.dedup_by(|a, b| a.path == b.path);
        self.report(hub, &open, now);
    }

    /// Reads the open rollouts and makes the hub's Codex agents match them.
    fn report(&mut self, hub: &Hub, open: &[Open], now: Timestamp) {
        self.read_rollouts(open);
        self.read_titles();
        let agents = self.agents(hub, open, now);
        let ids: HashSet<AgentId> = agents.iter().map(|agent| agent.id).collect();
        // Other observers report Codex agents too, such as those in
        // containers, so only the ones this observer reported are its own.
        let owned: HashSet<AgentId> = self.reported.union(&ids).copied().collect();
        hub.replace_sessions(
            |entity| matches!(entity, Entity::Agent(agent) if owned.contains(&agent.id)),
            agents.into_iter().map(Entity::Agent),
        );
        self.reported = ids;
        self.report_usage(hub.device().id);
    }

    /// Reads what was added to each open rollout and forgets the closed
    /// ones.
    fn read_rollouts(&mut self, open: &[Open]) {
        let wanted: HashSet<&Path> = open.iter().map(|entry| entry.path.as_path()).collect();
        self.rollouts
            .retain(|path, _| wanted.contains(path.as_path()));
        let clock = self.clock;
        for entry in open {
            let tracked = self.rollouts.entry(entry.path.clone()).or_default();
            let read = tracked.tail.read(&entry.path, |read| match read {
                Tailed::Restart => tracked.rollout = Rollout::default(),
                Tailed::Line(text) => tracked.rollout.read_line(text, &clock),
            });
            if let Err(error) = read {
                tracing::debug!(path = %entry.path.display(), %error, "couldn't read a rollout");
                self.rollouts.remove(&entry.path);
            }
        }
    }

    /// Reads the thread names in each home that has an open rollout.
    fn read_titles(&mut self) {
        let homes: HashSet<PathBuf> = self
            .rollouts
            .keys()
            .filter_map(|path| home_of(path))
            .map(Path::to_path_buf)
            .collect();
        self.titles.retain(|home, _| homes.contains(home));
        for home in homes {
            let titles = self.titles.entry(home.clone()).or_default();
            let names = &mut titles.names;
            let read = titles
                .tail
                .read(&home.join("session_index.jsonl"), |read| match read {
                    Tailed::Restart => names.clear(),
                    Tailed::Line(text) => {
                        if let Ok(line) = serde_json::from_str::<IndexLine>(text) {
                            match line.thread_name.filter(|name| !name.trim().is_empty()) {
                                Some(name) => names.insert(line.id, name),
                                None => names.remove(&line.id),
                            };
                        }
                    }
                });
            if read.is_err() {
                // No index yet, so no thread has a name.
                *titles = Titles::default();
            }
        }
    }

    /// The agents the open rollouts stand for.
    fn agents(&self, hub: &Hub, open: &[Open], now: Timestamp) -> Vec<Agent> {
        let threads: Threads<'_> = open
            .iter()
            .filter_map(|entry| {
                let thread = rollout::thread_id(&entry.path)?;
                let tracked = self.rollouts.get(&entry.path)?;
                Some((thread, (entry, &tracked.rollout)))
            })
            .collect();
        let mut agents: Vec<Agent> = threads
            .keys()
            .filter_map(|thread| self.agent(hub, &threads, thread, now))
            .collect();
        agents.sort_by_key(|agent| agent.id);
        mark_waiting_parents(&mut agents);
        agents
    }

    /// The agent for one open thread. None for a helper, or a rollout that
    /// hasn't said what it is yet.
    fn agent(
        &self,
        hub: &Hub,
        threads: &Threads<'_>,
        thread: &str,
        now: Timestamp,
    ) -> Option<Agent> {
        let (entry, rollout) = *threads.get(thread)?;
        let meta = rollout.meta.as_ref()?;
        let parent = match &meta.source {
            Source::Helper => return None,
            Source::Lead => None,
            Source::Subagent { parent, .. } => Some(agent_id(parent, threads, 0)),
        };
        let id = match parent {
            Some(parent) => names::subagent(parent, thread),
            None => names::agent(Vendor::Codex, thread),
        };
        // The session's own folder, since `codex -C` doesn't change the
        // process's.
        let cwd = meta.cwd.as_deref().or(entry.cwd.as_deref())?;
        let home = home_of(&entry.path);
        let named = home
            .and_then(|home| self.titles.get(home))
            .and_then(|titles| titles.names.get(thread))
            .cloned();
        let given = match &meta.source {
            Source::Subagent { nickname, role, .. } => nickname.clone().or_else(|| role.clone()),
            _ => None,
        };
        let title = named
            .or(given)
            .or_else(|| rollout.first_prompt.clone())
            .unwrap_or_else(|| UNTITLED.to_owned());
        let started_at = meta.started_at.or(rollout.active_at).unwrap_or(now);
        Some(Agent {
            id,
            workspace: hub.locate_folder(cwd),
            parent,
            vendor: Vendor::Codex,
            account: home.and_then(|home| self.account(home, hub.device().id)),
            model: None,
            permission: None,
            session: None,
            vendor_session_id: Some(thread.to_owned()),
            control: Control::Observed,
            pid: parent.is_none().then_some(entry.pid),
            link: AgentLink::Live,
            title,
            activity: rollout.activity(now),
            context: rollout.context,
            started_at,
            last_activity_at: rollout.active_at.unwrap_or(started_at).max(started_at),
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

    /// The account for a `CODEX_HOME`, when it's one `maxd` watches.
    fn account(&self, home: &Path, device: DeviceId) -> Option<AccountId> {
        self.homes
            .iter()
            .find(|known| known.is(home))
            .map(|known| known.account(device))
    }

    /// Passes the newest rate limits any rollout showed to the usage task
    /// for its account.
    fn report_usage(&self, device: DeviceId) {
        let mut newest: HashMap<AccountId, &Reading> = HashMap::new();
        for (path, tracked) in &self.rollouts {
            let Some(reading) = &tracked.rollout.rate_limits else {
                continue;
            };
            let Some(account) = home_of(path).and_then(|home| self.account(home, device)) else {
                continue;
            };
            let slot = newest.entry(account).or_insert(reading);
            if reading.checked_at > slot.checked_at {
                *slot = reading;
            }
        }
        for (account, reading) in newest {
            usage::report_rollout(account, reading.clone());
        }
    }
}

/// The agent ID of a thread. A sub-agent's ID comes from its parent's, so
/// this follows the chain up.
fn agent_id(thread: &str, threads: &Threads<'_>, depth: usize) -> AgentId {
    let source = threads
        .get(thread)
        .and_then(|(_, rollout)| rollout.meta.as_ref())
        .map(|meta| &meta.source);
    match source {
        Some(Source::Subagent { parent, .. }) if depth < MAX_DEPTH => {
            names::subagent(agent_id(parent, threads, depth + 1), thread)
        }
        _ => names::agent(Vendor::Codex, thread),
    }
}

/// Shows an agent that's working while a sub-agent of its own also works
/// as busy in sub-agents.
fn mark_waiting_parents(agents: &mut [Agent]) {
    let parents: HashMap<AgentId, AgentId> = agents
        .iter()
        .filter_map(|agent| Some((agent.id, agent.parent?)))
        .collect();
    let mut waiting = HashSet::new();
    for agent in agents
        .iter()
        .filter(|agent| agent.activity == Activity::Busy)
    {
        let mut current = agent.parent;
        for _ in 0..MAX_DEPTH {
            let Some(parent) = current else {
                break;
            };
            if !waiting.insert(parent) {
                break;
            }
            current = parents.get(&parent).copied();
        }
    }
    for agent in agents {
        if agent.activity == Activity::Busy && waiting.contains(&agent.id) {
            agent.activity = Activity::BusyInSubagents;
        }
    }
}

/// The `CODEX_HOME` a rollout lives in, from its path
/// `<home>/sessions/YYYY/MM/DD/rollout-*.jsonl`.
fn home_of(rollout: &Path) -> Option<&Path> {
    let sessions = rollout.ancestors().nth(4)?;
    if sessions.file_name()? != "sessions" {
        return None;
    }
    sessions.parent()
}

#[cfg(test)]
mod tests {
    use super::*;
    use maxhq_protocol::ids::BootId;
    use maxhq_protocol::model::{ContextUsage, Device};
    use maxhq_protocol::state::Change;

    const LEAD: &str = "0199f0a1-7c1e-7a52-9d1e-3f4a5b6c7d8e";
    const SUB: &str = "0199f0b2-1111-7222-8333-944455566677";
    const HELPER: &str = "0199f0d4-3333-7444-8555-b66677788899";

    fn hub() -> Hub {
        Hub::new(
            Device {
                id: DeviceId::derive("test"),
                name: "mac".into(),
                os: "macos".into(),
                boot_id: BootId("b".into()),
                maxd_version: "0.1.0".into(),
                winding_down_until: None,
                workspace_host: None,
                build: None,
                target: None,
                wsl: None,
            },
            HashMap::new(),
        )
    }

    fn at(text: &str) -> Timestamp {
        clock::parse_utc(text).unwrap()
    }

    /// A `CODEX_HOME` in a temporary folder, removed afterwards.
    struct TempHome {
        root: PathBuf,
        home: Home,
        day: PathBuf,
    }

    impl TempHome {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("maxd-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let dir = root.join(".codex");
            let day = dir.join("sessions/2026/10/24");
            std::fs::create_dir_all(&day).unwrap();
            let real = dir.canonicalize().unwrap();
            let day = real.join("sessions/2026/10/24");
            Self {
                root,
                home: Home { dir, real },
                day,
            }
        }

        /// Writes a rollout for `thread` and returns its path, as `lsof`
        /// would show it.
        fn rollout(&self, time: &str, thread: &str, text: &str) -> PathBuf {
            let path = self
                .day
                .join(format!("rollout-2026-10-24T{time}-{thread}.jsonl"));
            std::fs::write(&path, text).unwrap();
            path
        }

        fn append(&self, path: &Path, text: &str) {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
            file.write_all(text.as_bytes()).unwrap();
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn agents(hub: &Hub) -> Vec<Agent> {
        let mut agents: Vec<Agent> = hub
            .snapshot()
            .entities
            .into_iter()
            .filter_map(|entity| match entity {
                Entity::Agent(agent) => Some(agent),
                _ => None,
            })
            .collect();
        agents.sort_by_key(|agent| agent.title.clone());
        agents
    }

    fn open(path: &Path, pid: u32) -> Open {
        Open {
            path: path.to_path_buf(),
            pid,
            cwd: Some(PathBuf::from("/Users/dev")),
        }
    }

    #[test]
    fn open_rollouts_become_agents_and_closed_ones_go() {
        let temp = TempHome::new("codex-observe");
        let lead = temp.rollout("09-12-03", LEAD, include_str!("codex/fixtures/lead.jsonl"));
        let sub = temp.rollout(
            "09-30-41",
            SUB,
            include_str!("codex/fixtures/subagent.jsonl"),
        );
        // A review helper the sub-agent's rollout stands in for.
        let helper_text = include_str!("codex/fixtures/subagent.jsonl").replacen(
            &format!(
                r#""source":{{"subagent":{{"thread_spawn":{{"parent_thread_id":"{LEAD}","depth":1,"agent_path":null,"agent_nickname":"Ada","agent_role":"explorer"}}}}}}"#
            ),
            r#""source":{"subagent":"review"}"#,
            1,
        );
        assert!(helper_text.contains(r#""source":{"subagent":"review"}"#));
        let helper = temp.rollout("09-31-00", HELPER, &helper_text);
        std::fs::write(
            temp.home.real.join("session_index.jsonl"),
            include_str!("codex/fixtures/session_index.jsonl"),
        )
        .unwrap();
        // The lead starts a new turn and waits on its sub-agent.
        temp.append(
            &lead,
            "{\"timestamp\":\"2026-10-24T13:30:40.000Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\",\"model_context_window\":258400}}\n",
        );

        let hub = hub();
        let device = hub.device().id;
        // Another observer's Codex agent, which this one must leave alone.
        let elsewhere = AgentId::derive("in a container");
        let other = Agent {
            id: elsewhere,
            workspace: hub.locate_folder(Path::new("/srv/other")),
            parent: None,
            vendor: Vendor::Codex,
            account: None,
            model: None,
            permission: None,
            session: None,
            vendor_session_id: Some("x".into()),
            control: Control::Observed,
            pid: Some(7),
            link: AgentLink::Live,
            title: "Elsewhere".into(),
            activity: Activity::Idle,
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
        };
        hub.apply([Change::Upsert {
            value: Entity::Agent(other.clone()),
        }]);

        let mut watcher = Watcher::new(vec![temp.home.clone()]);
        let now = at("2026-10-24T13:31:00Z");
        watcher.report(
            &hub,
            &[open(&lead, 50100), open(&sub, 50100), open(&helper, 50100)],
            now,
        );

        let lead_id = names::agent(Vendor::Codex, LEAD);
        let account = temp.home.account(device);
        let workspace = hub.locate_folder(Path::new("/Users/dev/src/my app"));
        let shown = agents(&hub);
        let titles: Vec<&str> = shown.iter().map(|agent| agent.title.as_str()).collect();
        assert_eq!(
            titles,
            ["Ada", "Dark mode toggle", "Elsewhere"],
            "the helper is hidden"
        );

        let [sub_agent, lead_agent, still_other] = &shown[..] else {
            unreachable!()
        };
        assert_eq!(lead_agent.id, lead_id);
        assert_eq!(lead_agent.parent, None);
        assert_eq!(lead_agent.pid, Some(50100));
        assert_eq!(lead_agent.account, Some(account));
        assert_eq!(lead_agent.workspace, workspace);
        assert_eq!(lead_agent.control, Control::Observed);
        assert_eq!(lead_agent.vendor_session_id.as_deref(), Some(LEAD));
        assert_eq!(lead_agent.activity, Activity::BusyInSubagents);
        assert_eq!(lead_agent.started_at, at("2026-10-24T13:12:03.100Z"));
        assert_eq!(lead_agent.last_activity_at, at("2026-10-24T13:30:40Z"));

        assert_eq!(sub_agent.id, names::subagent(lead_id, SUB));
        assert_eq!(sub_agent.parent, Some(lead_id));
        assert_eq!(sub_agent.pid, None);
        assert_eq!(sub_agent.account, Some(account));
        assert_eq!(sub_agent.activity, Activity::Busy);
        assert_eq!(
            sub_agent.context,
            Some(ContextUsage {
                used_tokens: 9_100,
                window_tokens: 128_000,
            })
        );
        assert_eq!(still_other, &other);

        // The sub-agent's rate limits are newer than the lead's, so they
        // go to the usage task.
        let reading = usage::rollout_reading(account).map(|reading| reading.checked_at);
        assert_eq!(reading, Some(at("2026-10-24T13:30:44Z")));

        // The lead finishes. The sub-agent's thread closes, and its
        // rollout with it.
        temp.append(
            &lead,
            "{\"timestamp\":\"2026-10-24T13:31:10.000Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n",
        );
        let later = at("2026-10-24T13:31:12Z");
        watcher.report(&hub, &[open(&lead, 50100)], later);
        let shown = agents(&hub);
        let titles: Vec<&str> = shown.iter().map(|agent| agent.title.as_str()).collect();
        assert_eq!(titles, ["Dark mode toggle", "Elsewhere"]);
        assert_eq!(
            shown[0].activity,
            Activity::DoneUnread,
            "a finished turn you haven't seen"
        );

        // The process exits. The lead stays, ended, so you can continue
        // it.
        watcher.report(&hub, &[], later);
        let shown = agents(&hub);
        assert_eq!(shown.len(), 2);
        assert_eq!(shown[0].id, lead_id);
        assert!(matches!(shown[0].link, AgentLink::Down { .. }));
        assert_eq!(shown[0].pid, None);
        assert_eq!(shown[1], other);
    }

    #[test]
    fn a_rollout_names_its_home() {
        assert_eq!(
            home_of(Path::new("/h/.codex/sessions/2026/10/24/rollout-x.jsonl")),
            Some(Path::new("/h/.codex"))
        );
        assert_eq!(
            home_of(Path::new("/h/.codex/archived/2026/10/24/rollout-x.jsonl")),
            None
        );
        assert_eq!(home_of(Path::new("/rollout-x.jsonl")), None);
    }

    /// Runs one pass against this device and prints what it found, with no
    /// titles, prompts or paths. Run it with
    /// `cargo test -p maxd -- --ignored real_codex_sessions --nocapture`.
    #[test]
    #[ignore = "reads this device's processes and Codex rollouts"]
    fn real_codex_sessions() {
        let processes = process::list();
        let named = processes
            .iter()
            .filter(|process| process.command.rsplit('/').next() == Some("codex"))
            .count();
        let pids = process::sessions(&processes, std::process::id());
        let files = process::open_files(&pids);
        let rollouts: usize = files.values().map(|files| files.rollouts.len()).sum();
        println!(
            "codex processes: {named}, sessions: {}, open rollouts: {rollouts}",
            pids.len()
        );
        let hub = hub();
        let mut watcher = Watcher::new(usage::homes());
        watcher.pass(&hub, Timestamp::now());
        for agent in agents(&hub) {
            println!(
                "agent: sub-agent {}, {:?}, context {:?}, account {}",
                agent.parent.is_some(),
                agent.activity,
                agent.context,
                agent.account.is_some()
            );
        }
    }
}
