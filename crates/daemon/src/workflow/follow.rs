// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The follower: finds the runs in followed folders, reads each again when
//! its files or agents change, keeps a summary entity per run, and streams
//! open runs to the clients that opened them.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use maxhq_protocol::guest::GuestAgent;
use maxhq_protocol::ids::{AgentId, DeviceId, ProjectId, WorkflowRunId, WorkspaceId};
use maxhq_protocol::message::{ActionOutcome, ServerMessage};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::workflow::{RunKey, RunSnapshot, WorkflowRun, WorkflowUpdate};
use maxhq_workflow::descriptor::Descriptor;
use maxhq_workflow::{
    fallback_leads, lead_state, summarize, AgentFacts, AgentState, Agents, Catalog, Diff, Entry,
    Files, Run, Scan, Stat, AGENTS_ID,
};
use tokio::sync::mpsc;

use super::env::{self, Envs};
use super::guest;

/// How often followed runs are checked.
pub const TICK: Duration = Duration::from_secs(1);
/// How often folders are scanned again, and resting runs checked.
const PASS: Duration = Duration::from_secs(30);
/// How long a run may go without activity before it rests.
const RESTS_AFTER: Duration = Duration::from_secs(6 * 60 * 60);
/// The most runs followed per folder, newest first.
pub const MAX_RUNS: usize = 100;
/// The most files one run's derive is watched by.
const MAX_WATCHED: usize = 64;

/// A folder to follow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
    /// The folder, on this device.
    pub path: PathBuf,
    /// Its project.
    pub project: ProjectId,
    /// The lead agents working in it.
    pub leads: Vec<AgentId>,
}

/// What the follower reads besides files, and where its summaries go.
pub trait World {
    /// The folders to follow: those with an agent, and those of `open`
    /// projects.
    fn roots(&self, open: &HashSet<ProjectId>) -> Vec<Root>;
    /// The project's agents, leads and sub-agents.
    fn agents(&self, project: ProjectId) -> Box<dyn Agents + '_>;
    /// The project of a container workspace.
    fn guest_project(&self, workspace: WorkspaceId) -> Option<ProjectId>;
    /// The ports a container workspace has open.
    fn forwarded(&self, workspace: WorkspaceId) -> Vec<u16>;
    /// A container workspace's own agents, as its guest gets them.
    fn guest_agents(&self, workspace: WorkspaceId) -> Vec<GuestAgent>;
    /// Makes the summary entities exactly `runs`.
    fn publish(&self, runs: Vec<WorkflowRun>);
}

/// Names a client's stream.
pub type ViewerId = u64;
/// Where a client's workflow messages go.
pub type Sink = mpsc::UnboundedSender<ServerMessage>;

/// Where a followed run comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Source {
    File(PathBuf),
    Lead(AgentId),
}

/// A run on this device.
struct Followed {
    root: PathBuf,
    project: ProjectId,
    source: Source,
    desc: Arc<Descriptor>,
    run: Run,
    /// What the last derive read, as it stood then.
    watched: Vec<(PathBuf, Option<Stat>)>,
    /// The agents as the last derive saw them.
    facts: Vec<AgentFacts>,
    last: RunSnapshot,
    /// Whether the tick checks it, rather than only the pass.
    active: bool,
}

/// A run a container's guest follows.
struct GuestRun {
    last: RunSnapshot,
    /// Its leads, each one of the container's own agents.
    leads: Vec<AgentId>,
}

/// Where a node's details come from.
pub enum Detail {
    /// Read here.
    Here(ActionOutcome),
    /// The run is in this container, whose guest reads them.
    Guest(WorkspaceId),
}

/// A run some clients have open.
struct Open {
    key: RunKey,
    sinks: Vec<(ViewerId, Sink)>,
}

/// Every run this device follows, and who has which open.
pub struct Follower {
    device: DeviceId,
    catalog: Catalog,
    envs: Envs,
    retry_pause: Duration,
    /// Each followed folder's last scan.
    scans: HashMap<PathBuf, (Root, Scan)>,
    runs: BTreeMap<WorkflowRunId, Followed>,
    guests: BTreeMap<WorkflowRunId, (WorkspaceId, GuestRun)>,
    opens: HashMap<WorkflowRunId, Open>,
    published: Option<Vec<WorkflowRun>>,
    next_pass: Option<Instant>,
}

impl Follower {
    /// A follower for this device. `retry_pause` is how long a run file
    /// that doesn't parse waits before it's read again.
    pub fn new(device: DeviceId, envs: Envs, retry_pause: Duration) -> Self {
        Self {
            device,
            catalog: envs.catalog(),
            envs,
            retry_pause,
            scans: HashMap::new(),
            runs: BTreeMap::new(),
            guests: BTreeMap::new(),
            opens: HashMap::new(),
            published: None,
            next_pass: None,
        }
    }

    /// Checks the followed runs, and on the pass scans folders again.
    pub fn tick(&mut self, files: &dyn Files, world: &dyn World, now: Instant) {
        let pass = self.next_pass.is_none_or(|at| now >= at);
        if pass {
            self.next_pass = Some(now + PASS);
        }
        let open: HashSet<ProjectId> = self.opens.values().map(|o| o.key.project).collect();
        let roots = world.roots(&open);
        self.scans
            .retain(|path, _| roots.iter().any(|r| &r.path == path));
        for path in self.envs_to_forget(&roots) {
            self.envs.forget(&path);
        }
        self.runs
            .retain(|_, f| roots.iter().any(|r| r.path == f.root));

        let mut projects: Vec<ProjectId> = roots.iter().map(|r| r.project).collect();
        projects.sort();
        projects.dedup();
        for project in projects {
            let agents = world.agents(project);
            let facts = agents.list();
            // Runs a scan starts are read then, so only those before it
            // are read again here.
            let ids: Vec<WorkflowRunId> = self
                .runs
                .iter()
                .filter(|(_, f)| f.project == project && (f.active || pass))
                .map(|(id, _)| *id)
                .collect();
            for root in roots.iter().filter(|r| r.project == project) {
                match self.scans.get_mut(&root.path) {
                    Some((known, _)) if !pass => known.leads = root.leads.clone(),
                    _ => self.scan(files, &*agents, &facts, root.clone()),
                }
            }
            for id in ids {
                let due = self.runs.get(&id).is_some_and(|f| {
                    pass || f.facts != facts
                        || f.watched.iter().any(|(p, s)| files.stat(p).ok() != *s)
                });
                if due {
                    self.derive(id, files, &*agents, &facts);
                }
            }
            self.fallback(files, project, &*agents, &facts);
        }
        self.publish(world);
    }

    /// Scans a folder, and starts following the runs it has that aren't
    /// followed yet.
    fn scan(&mut self, files: &dyn Files, agents: &dyn Agents, facts: &[AgentFacts], root: Root) {
        let scan = self.catalog.scan(files, &root.path);
        let mut refs = scan.runs(files);
        if refs.len() > MAX_RUNS {
            refs.sort_by_cached_key(|r| {
                std::cmp::Reverse(files.stat(&r.file).map(|s| s.modified).ok())
            });
            refs.truncate(MAX_RUNS);
        }
        self.envs.put(&root.path, env::of_scan(&scan, &refs));
        // Runs whose file went, or whose workflow did, stop.
        self.runs.retain(|_, f| {
            f.root != root.path
                || match &f.source {
                    Source::File(file) => refs.iter().any(|r| &r.file == file),
                    Source::Lead(_) => true,
                }
        });
        for f in self.runs.values_mut().filter(|f| f.root == root.path) {
            if let Some(workflow) = scan.workflow(&f.desc.id) {
                f.desc = workflow.descriptor.clone();
            }
            if let Source::Lead(_) = f.source {
                f.run.set_notice(scan.fallback_notice());
            }
        }
        for r in refs {
            let followed = self
                .runs
                .values()
                .any(|f| f.source == Source::File(r.file.clone()));
            let Some(workflow) = scan.workflow(&r.workflow) else {
                continue;
            };
            if followed {
                continue;
            }
            let desc = workflow.descriptor.clone();
            let file = r.file.clone();
            let run = Run::new(self.device, root.project, r);
            self.start(files, agents, facts, &root, Source::File(file), desc, run);
        }
        self.scans.insert(root.path.clone(), (root, scan));
    }

    /// The fallback's runs: one for each lead with sub-agents that no run
    /// bound, in the folder it works in.
    fn fallback(
        &mut self,
        files: &dyn Files,
        project: ProjectId,
        agents: &dyn Agents,
        facts: &[AgentFacts],
    ) {
        let taken: Vec<AgentId> = self
            .runs
            .values()
            .filter(|f| f.project == project && matches!(f.source, Source::File(_)))
            .flat_map(|f| f.run.orchestrators().iter().copied())
            .collect();
        let leads = fallback_leads(facts, &taken);
        let roots: Vec<(
            Root,
            Option<Arc<Descriptor>>,
            Option<maxhq_protocol::workflow::RunNotice>,
        )> = self
            .scans
            .values()
            .filter(|(root, _)| root.project == project)
            .map(|(root, scan)| {
                let desc = scan.workflow(AGENTS_ID).map(|w| w.descriptor.clone());
                (root.clone(), desc, scan.fallback_notice())
            })
            .collect();
        for (root, desc, notice) in roots {
            let wanted: Vec<AgentId> = leads
                .iter()
                .copied()
                .filter(|l| root.leads.contains(l))
                .collect();
            // A run already drawn stays while its lead does, so the sub-agents
            // it reached stay on the canvas once none of them runs any more.
            let stays = |lead: &AgentId| {
                wanted.contains(lead)
                    || (root.leads.contains(lead)
                        && !taken.contains(lead)
                        && facts.iter().any(|f| f.id == *lead && f.lead.is_none()))
            };
            self.runs.retain(|_, f| {
                f.root != root.path
                    || match f.source {
                        Source::Lead(lead) => stays(&lead),
                        Source::File(_) => true,
                    }
            });
            let Some(desc) = desc else {
                continue;
            };
            for lead in wanted {
                if self.runs.values().any(|f| f.source == Source::Lead(lead)) {
                    continue;
                }
                let mut run = Run::lead(self.device, project, &root.path, lead);
                run.set_notice(notice.clone());
                self.start(
                    files,
                    agents,
                    facts,
                    &root,
                    Source::Lead(lead),
                    desc.clone(),
                    run,
                );
            }
        }
    }

    /// Reads a new run for the first time and follows it.
    #[allow(clippy::too_many_arguments)]
    fn start(
        &mut self,
        files: &dyn Files,
        agents: &dyn Agents,
        facts: &[AgentFacts],
        root: &Root,
        source: Source,
        desc: Arc<Descriptor>,
        mut run: Run,
    ) {
        run.set_retry_pause(self.retry_pause);
        let seen = Seen::new(files);
        let last = run.derive(&desc, &seen, agents);
        let id = last.key.id();
        // Two runs with one key: the first found keeps it.
        if self.runs.contains_key(&id) {
            return;
        }
        let mut followed = Followed {
            root: root.path.clone(),
            project: root.project,
            source,
            desc,
            run,
            watched: seen.watched(files),
            facts: facts.to_vec(),
            last,
            active: true,
        };
        followed.active = active(&followed, agents.now());
        self.send(&id, WorkflowUpdate::Reset(followed.last.clone()));
        self.runs.insert(id, followed);
    }

    /// Reads a run again, and sends what changed to those who have it open.
    fn derive(
        &mut self,
        id: WorkflowRunId,
        files: &dyn Files,
        agents: &dyn Agents,
        facts: &[AgentFacts],
    ) {
        let Some(mut f) = self.runs.remove(&id) else {
            return;
        };
        let seen = Seen::new(files);
        let newer = f.run.derive(&f.desc, &seen, agents);
        f.watched = seen.watched(files);
        f.facts = facts.to_vec();
        let update = (newer.rev != f.last.rev).then(|| match f.last.diff(&newer) {
            Some(patch) => WorkflowUpdate::Patch(patch),
            None => WorkflowUpdate::Reset(newer.clone()),
        });
        f.last = newer;
        f.active = active(&f, agents.now());
        let new_id = f.last.key.id();
        if new_id != id && self.runs.contains_key(&new_id) {
            return;
        }
        if let Some(update) = update {
            let update = match (new_id == id, update) {
                (true, update) => update,
                // A run whose id changed is a new run to its viewers.
                (false, _) => WorkflowUpdate::Reset(f.last.clone()),
            };
            self.send(&new_id, update);
        }
        self.runs.insert(new_id, f);
    }

    /// Sends `update` to each client that has the run open, and forgets
    /// clients that went.
    fn send(&mut self, id: &WorkflowRunId, update: WorkflowUpdate) {
        let Some(open) = self.opens.get_mut(id) else {
            return;
        };
        let message = ServerMessage::Workflow {
            run: open.key.clone(),
            update: Box::new(update),
        };
        open.sinks
            .retain(|(_, sink)| sink.send(message.clone()).is_ok());
    }

    /// Updates the summary entities when any changed. A container run
    /// needs you when its leads wait, as the host sees them.
    fn publish(&mut self, world: &dyn World) {
        let mut facts: HashMap<WorkspaceId, Vec<AgentFacts>> = HashMap::new();
        let mut runs: Vec<WorkflowRun> =
            self.runs.values().filter_map(|f| f.run.summary()).collect();
        for (workspace, g) in self.guests.values() {
            let facts = facts.entry(*workspace).or_insert_with(|| {
                let agents = world.guest_agents(*workspace);
                agents.iter().map(guest::facts_of).collect()
            });
            let waiting = lead_state(&g.leads, facts) == AgentState::Waiting;
            runs.push(summarize(&g.last, waiting, g.leads.clone(), None));
        }
        runs.sort_by_key(|r| r.id);
        if self.published.as_ref() != Some(&runs) {
            world.publish(runs.clone());
            self.published = Some(runs);
        }
    }

    /// The last snapshot of each run on this device, with its leads.
    pub fn followed(&self) -> impl Iterator<Item = (&RunSnapshot, &[AgentId])> {
        self.runs.values().map(|f| (&f.last, f.run.orchestrators()))
    }

    /// Starts sending a run to a client, with the whole run first. A run
    /// not found yet is sent once a scan finds it.
    pub fn open(&mut self, viewer: ViewerId, key: RunKey, sink: Sink) {
        let id = key.id();
        let open = self.opens.entry(id).or_insert_with(|| Open {
            key: key.clone(),
            sinks: Vec::new(),
        });
        open.sinks.retain(|(v, _)| *v != viewer);
        let snapshot = self
            .runs
            .get(&id)
            .map(|f| f.last.clone())
            .or_else(|| self.guests.get(&id).map(|(_, g)| g.last.clone()));
        match snapshot {
            Some(snapshot) => {
                let message = ServerMessage::Workflow {
                    run: key,
                    update: Box::new(WorkflowUpdate::Reset(snapshot)),
                };
                if sink.send(message).is_ok() {
                    open.sinks.push((viewer, sink));
                }
            }
            None => {
                open.sinks.push((viewer, sink));
                // Its project may not be followed yet.
                self.next_pass = None;
            }
        }
    }

    /// Stops sending a run to a client.
    pub fn close(&mut self, viewer: ViewerId, key: &RunKey) {
        let id = key.id();
        if let Some(open) = self.opens.get_mut(&id) {
            open.sinks.retain(|(v, _)| *v != viewer);
            if open.sinks.is_empty() {
                self.opens.remove(&id);
            }
        }
    }

    /// Stops sending anything to a client that went.
    pub fn gone(&mut self, viewer: ViewerId) {
        self.opens.retain(|_, open| {
            open.sinks.retain(|(v, _)| *v != viewer);
            !open.sinks.is_empty()
        });
    }

    /// The descriptor's lists for one node of a run, or the container
    /// whose guest has them.
    pub fn detail(&self, files: &dyn Files, key: &RunKey, node: &str) -> Detail {
        if let Some((workspace, _)) = self.guests.get(&key.id()) {
            return Detail::Guest(*workspace);
        }
        Detail::Here(match self.runs.get(&key.id()) {
            Some(f) => ActionOutcome::NodeDetail {
                detail: f.run.detail(&f.desc, files, node),
            },
            None => ActionOutcome::Refused {
                reason: "That run isn't followed on this device.".to_owned(),
            },
        })
    }

    /// Takes in a snapshot a container's guest sent, with the leads it
    /// bound, once it passes the host's checks.
    pub fn take_guest(
        &mut self,
        world: &dyn World,
        workspace: WorkspaceId,
        snapshot: RunSnapshot,
        leads: Vec<AgentId>,
    ) -> Result<(), &'static str> {
        guest::check(&snapshot)?;
        let project = world
            .guest_project(workspace)
            .ok_or("the workspace has no project")?;
        let own: Vec<AgentId> = world.guest_agents(workspace).iter().map(|a| a.id).collect();
        let mut snapshot = guest::trusted(snapshot, &world.forwarded(workspace), &own);
        let leads = guest::own_only(leads, &own);
        snapshot.key.device = self.device;
        snapshot.key.project = project;
        let id = snapshot.key.id();
        let update = match self.guests.get(&id) {
            Some((at, _)) if *at != workspace => return Err("another workspace has that run"),
            Some((_, known)) => {
                snapshot.rev = known.last.rev;
                if same(&known.last, &snapshot) {
                    None
                } else {
                    snapshot.rev += 1;
                    Some(match known.last.diff(&snapshot) {
                        Some(patch) => WorkflowUpdate::Patch(patch),
                        None => WorkflowUpdate::Reset(snapshot.clone()),
                    })
                }
            }
            None => {
                let here = self
                    .guests
                    .values()
                    .filter(|(at, _)| *at == workspace)
                    .count();
                if here >= MAX_RUNS {
                    return Err("the workspace sent too many runs");
                }
                snapshot.rev = 1;
                Some(WorkflowUpdate::Reset(snapshot.clone()))
            }
        };
        self.guests.insert(
            id,
            (
                workspace,
                GuestRun {
                    last: snapshot,
                    leads,
                },
            ),
        );
        if let Some(update) = update {
            self.send(&id, update);
        }
        self.publish(world);
        Ok(())
    }

    /// The folders whose variables no longer apply.
    fn envs_to_forget(&self, roots: &[Root]) -> Vec<PathBuf> {
        self.scans
            .keys()
            .filter(|p| !roots.iter().any(|r| &r.path == *p))
            .cloned()
            .collect()
    }
}

/// Whether two snapshots say the same, whatever their revs.
fn same(a: &RunSnapshot, b: &RunSnapshot) -> bool {
    a.key == b.key
        && a.shape == b.shape
        && a.nodes == b.nodes
        && a.loops == b.loops
        && a.header == b.header
        && a.notice == b.notice
}

/// Whether a run is checked every tick: it isn't done, and something
/// happened in the last six hours.
fn active(f: &Followed, now: Timestamp) -> bool {
    let Some(summary) = f.run.summary() else {
        return true;
    };
    let rests = RESTS_AFTER.as_millis() as u64;
    !summary.done
        && summary
            .last_activity
            .is_none_or(|at| now.0.saturating_sub(at.0) < rests)
}

/// The files, noting each path a derive looks at.
struct Seen<'a> {
    files: &'a dyn Files,
    paths: RefCell<Vec<PathBuf>>,
}

impl<'a> Seen<'a> {
    fn new(files: &'a dyn Files) -> Self {
        Self {
            files,
            paths: RefCell::new(Vec::new()),
        }
    }

    fn note(&self, path: &Path) {
        let mut paths = self.paths.borrow_mut();
        if paths.len() < MAX_WATCHED && !paths.iter().any(|p| p == path) {
            paths.push(path.to_owned());
        }
    }

    /// Each path looked at, with how it stands now.
    fn watched(self, files: &dyn Files) -> Vec<(PathBuf, Option<Stat>)> {
        self.paths
            .into_inner()
            .into_iter()
            .map(|p| {
                let stat = files.stat(&p).ok();
                (p, stat)
            })
            .collect()
    }
}

impl Files for Seen<'_> {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.note(path);
        self.files.read(path)
    }

    fn read_from(&self, path: &Path, offset: u64, max: usize) -> io::Result<Vec<u8>> {
        self.note(path);
        self.files.read_from(path, offset, max)
    }

    fn stat(&self, path: &Path) -> io::Result<Stat> {
        self.note(path);
        self.files.stat(path)
    }

    fn list(&self, dir: &Path) -> io::Result<Vec<Entry>> {
        self.note(dir);
        self.files.list(dir)
    }

    fn alive(&self, pid: u32) -> bool {
        self.files.alive(pid)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::RefCell;

    use maxhq_protocol::guest::GuestAgentState;
    use maxhq_protocol::workflow::NodeStatus;
    use maxhq_workflow::fake::{FakeAgents, MemFiles};

    use super::*;

    const ROOT: &str = "/work/example-app";
    const DESCRIPTOR: &str = "format: maxhq.workflow/1.0
id: example
runs:
  glob: .runs/*/run.yaml
bind: { node: [{ title: '^(?<node>\\w+)$' }] }
";

    /// A run file in the reference shape, with `plan` in `status`.
    fn run_file(id: &str, status: &str) -> String {
        format!(
            "run: {{ id: {id}, label: Run {id} }}
nodes:
  - {{ id: plan, status: {status} }}
  - {{ id: build, needs: [plan] }}
"
        )
    }

    /// Writes the descriptor and a run into a real folder.
    pub(crate) fn write_run(root: &Path, id: &str, status: &str) {
        let workflows = root.join(maxhq_workflow::WORKFLOWS_DIR);
        std::fs::create_dir_all(&workflows).unwrap();
        std::fs::write(workflows.join("example.yaml"), DESCRIPTOR).unwrap();
        let run = root.join(".runs").join(id);
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(run.join("run.yaml"), run_file(id, status)).unwrap();
    }

    /// A snapshot of a small run, as a guest would send it.
    pub(crate) fn snapshot() -> RunSnapshot {
        let mut files = MemFiles::default();
        files.put(format!("{ROOT}/.maxhq/workflows/example.yaml"), DESCRIPTOR);
        files.put(
            format!("{ROOT}/.runs/one/run.yaml"),
            run_file("one", "busy"),
        );
        let mut runs = follower();
        runs.tick(&files, &world(), Instant::now());
        runs.followed().next().unwrap().0.clone()
    }

    fn device() -> DeviceId {
        DeviceId::derive("devbox")
    }

    fn project() -> ProjectId {
        ProjectId::derive("git:github.com/owner/repo")
    }

    fn follower() -> Follower {
        Follower::new(device(), Envs::default(), Duration::ZERO)
    }

    pub(crate) struct FakeWorld {
        roots: Vec<Root>,
        agents: FakeAgents,
        /// The agents in the container `box`.
        pub(crate) guest_agents: Vec<GuestAgent>,
        published: RefCell<Vec<Vec<WorkflowRun>>>,
    }

    pub(crate) fn world() -> FakeWorld {
        FakeWorld {
            roots: vec![Root {
                path: ROOT.into(),
                project: project(),
                leads: Vec::new(),
            }],
            agents: FakeAgents::default(),
            guest_agents: Vec::new(),
            published: RefCell::default(),
        }
    }

    impl FakeWorld {
        fn last(&self) -> Vec<WorkflowRun> {
            self.published.borrow().last().cloned().unwrap_or_default()
        }
    }

    struct Borrowed<'a>(&'a FakeAgents);

    impl Agents for Borrowed<'_> {
        fn now(&self) -> Timestamp {
            self.0.now()
        }
        fn list(&self) -> Vec<AgentFacts> {
            self.0.list()
        }
        fn prompt_head(&self, agent: AgentId) -> Option<String> {
            self.0.prompt_head(agent)
        }
        fn each_command(&self, agent: AgentId, visit: &mut dyn FnMut(&str)) {
            self.0.each_command(agent, visit);
        }
    }

    impl World for FakeWorld {
        fn roots(&self, _open: &HashSet<ProjectId>) -> Vec<Root> {
            self.roots.clone()
        }
        fn agents(&self, _project: ProjectId) -> Box<dyn Agents + '_> {
            Box::new(Borrowed(&self.agents))
        }
        fn guest_project(&self, workspace: WorkspaceId) -> Option<ProjectId> {
            (workspace == WorkspaceId::derive("box")).then(project)
        }
        fn forwarded(&self, _workspace: WorkspaceId) -> Vec<u16> {
            vec![3000]
        }
        fn guest_agents(&self, workspace: WorkspaceId) -> Vec<GuestAgent> {
            if workspace == WorkspaceId::derive("box") {
                self.guest_agents.clone()
            } else {
                Vec::new()
            }
        }
        fn publish(&self, runs: Vec<WorkflowRun>) {
            self.published.borrow_mut().push(runs);
        }
    }

    fn files(status: &str) -> MemFiles {
        let mut files = MemFiles::default();
        files.put(format!("{ROOT}/.maxhq/workflows/example.yaml"), DESCRIPTOR);
        files.put(
            format!("{ROOT}/.runs/one/run.yaml"),
            run_file("one", status),
        );
        files
    }

    fn key() -> RunKey {
        RunKey {
            device: device(),
            project: project(),
            workflow: "example".into(),
            run: "one".into(),
        }
    }

    fn update(rx: &mut mpsc::UnboundedReceiver<ServerMessage>) -> Option<WorkflowUpdate> {
        match rx.try_recv() {
            Ok(ServerMessage::Workflow { run, update }) => {
                assert_eq!(run, key());
                Some(*update)
            }
            Ok(other) => panic!("unexpected {other:?}"),
            Err(_) => None,
        }
    }

    #[test]
    fn an_open_run_comes_whole_then_as_patches_until_closed() {
        let mut files = files("done");
        let world = world();
        let mut runs = follower();
        let now = Instant::now();
        runs.tick(&files, &world, now);
        let (tx, mut rx) = mpsc::unbounded_channel();
        runs.open(1, key(), tx.clone());
        let Some(WorkflowUpdate::Reset(first)) = update(&mut rx) else {
            panic!("expected the whole run");
        };
        assert_eq!(first.nodes[0].status, NodeStatus::Done);

        files.put(
            format!("{ROOT}/.runs/one/run.yaml"),
            run_file("one", "failed"),
        );
        runs.tick(&files, &world, now + TICK);
        let Some(WorkflowUpdate::Patch(patch)) = update(&mut rx) else {
            panic!("expected a patch");
        };
        assert_eq!(patch.rev, first.rev + 1);
        assert!(patch
            .nodes
            .iter()
            .any(|n| n.id == "plan" && n.status == NodeStatus::Failed));

        // Opening it again, as after a reconnect, sends it whole again.
        runs.open(1, key(), tx);
        let Some(WorkflowUpdate::Reset(again)) = update(&mut rx) else {
            panic!("expected the whole run again");
        };
        assert_eq!(again.rev, patch.rev);

        runs.close(1, &key());
        files.put(
            format!("{ROOT}/.runs/one/run.yaml"),
            run_file("one", "skipped"),
        );
        runs.tick(&files, &world, now + TICK * 2);
        assert_eq!(update(&mut rx), None);
    }

    #[test]
    fn a_run_opened_before_it_is_found_comes_once_it_is() {
        let files = files("busy");
        let world = world();
        let mut runs = follower();
        let (tx, mut rx) = mpsc::unbounded_channel();
        runs.open(1, key(), tx);
        assert_eq!(update(&mut rx), None);
        runs.tick(&files, &world, Instant::now());
        assert!(matches!(update(&mut rx), Some(WorkflowUpdate::Reset(_))));
    }

    #[test]
    fn a_quiet_tick_reads_no_file() {
        let files = files("busy");
        let world = world();
        let mut runs = follower();
        let now = Instant::now();
        runs.tick(&files, &world, now);
        let reads = files.reads();
        assert!(reads > 0);
        runs.tick(&files, &world, now + TICK);
        runs.tick(&files, &world, now + TICK * 2);
        assert_eq!(files.reads(), reads);
        // The pass reads everything again.
        runs.tick(&files, &world, now + PASS);
        assert!(files.reads() > reads);
    }

    #[test]
    fn each_run_has_a_summary_until_its_file_goes() {
        let mut files = files("busy");
        let world = world();
        let mut runs = follower();
        let now = Instant::now();
        runs.tick(&files, &world, now);
        let published = world.last();
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].key, key());
        assert!(!published[0].done);
        let count = world.published.borrow().len();
        runs.tick(&files, &world, now + TICK);
        assert_eq!(world.published.borrow().len(), count, "nothing changed");

        files.remove(format!("{ROOT}/.runs/one/run.yaml"));
        runs.tick(&files, &world, now + PASS);
        assert!(world.last().is_empty());
    }

    #[test]
    fn a_run_binds_agents_as_they_come() {
        let files = files("busy");
        let mut world = world();
        let mut runs = follower();
        let now = Instant::now();
        runs.tick(&files, &world, now);
        let lead = world.agents.add("lead", None);
        let build = world.agents.add("build", Some(lead));
        runs.tick(&files, &world, now + TICK);
        let (s, _) = runs.followed().next().unwrap();
        let node = s.nodes.iter().find(|n| n.id == "build").unwrap();
        assert_eq!(node.agents, [build]);
    }

    #[test]
    fn a_lead_with_sub_agents_and_no_run_gets_the_fallback() {
        let mut files = MemFiles::default();
        files.put(format!("{ROOT}/README.md"), "hello");
        let mut world = world();
        let lead = world.agents.add("lead", None);
        world.agents.add("helper", Some(lead));
        world.roots[0].leads = vec![lead];
        let mut runs = follower();
        runs.tick(&files, &world, Instant::now());
        let published = world.last();
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].key.workflow, AGENTS_ID);
        assert_eq!(published[0].orchestrators, [lead]);
    }

    #[test]
    fn a_fallback_run_stays_after_its_sub_agents_leave() {
        let mut files = MemFiles::default();
        files.put(format!("{ROOT}/README.md"), "hello");
        let mut world = world();
        let lead = world.agents.add("lead", None);
        let helper = world.agents.add("helper", Some(lead));
        world.roots[0].leads = vec![lead];
        let mut runs = follower();
        let start = Instant::now();
        runs.tick(&files, &world, start);
        assert_eq!(world.last().len(), 1);

        // Claude drops a sub-agent from its list once it ends.
        world.agents.remove(helper);
        runs.tick(&files, &world, start + Duration::from_secs(60));
        let published = world.last();
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].progress.done, 1);
        assert_eq!(published[0].progress.total, 1);

        // The run goes with its lead.
        world.agents.remove(lead);
        runs.tick(&files, &world, start + Duration::from_secs(120));
        assert!(world.last().is_empty());
    }

    #[test]
    fn a_guest_run_is_keyed_here_counted_here_and_checked() {
        let world = world();
        let mut runs = follower();
        let box_ = WorkspaceId::derive("box");
        let mut sent = snapshot();
        sent.key.device = DeviceId::derive("guest");
        sent.key.project = ProjectId::derive("guest");
        sent.rev = 40;
        let (tx, mut rx) = mpsc::unbounded_channel();
        runs.open(1, key(), tx);

        runs.take_guest(&world, box_, sent.clone(), Vec::new())
            .unwrap();
        let Some(WorkflowUpdate::Reset(first)) = update(&mut rx) else {
            panic!("expected the whole run");
        };
        assert_eq!((first.key.clone(), first.rev), (key(), 1));
        assert_eq!(world.last().len(), 1);

        // The same run again changes nothing.
        runs.take_guest(&world, box_, sent.clone(), Vec::new())
            .unwrap();
        assert_eq!(update(&mut rx), None);

        sent.nodes[0].status = NodeStatus::Done;
        runs.take_guest(&world, box_, sent.clone(), Vec::new())
            .unwrap();
        let Some(WorkflowUpdate::Patch(patch)) = update(&mut rx) else {
            panic!("expected a patch");
        };
        assert_eq!(patch.rev, 2);

        sent.header.label = "x".repeat(10_000);
        assert!(runs
            .take_guest(&world, box_, sent.clone(), Vec::new())
            .is_err());
        assert!(runs
            .take_guest(
                &world,
                WorkspaceId::derive("elsewhere"),
                snapshot(),
                Vec::new()
            )
            .is_err());
        assert!(matches!(
            runs.detail(&MemFiles::default(), &key(), "plan"),
            Detail::Guest(w) if w == box_
        ));
    }

    /// One agent in the container, as the host sends it.
    pub(crate) fn guest_agent(
        name: &str,
        lead: Option<AgentId>,
        state: GuestAgentState,
    ) -> GuestAgent {
        GuestAgent {
            id: AgentId::derive(name),
            lead,
            branch: None,
            title: Some(name.to_owned()),
            pid: None,
            state,
            started: Some(Timestamp(1)),
            last_write: Some(Timestamp(2)),
            prompt_head: None,
            commands: Vec::new(),
        }
    }

    #[test]
    fn a_container_run_needs_you_when_its_lead_waits() {
        let mut world = world();
        let lead = guest_agent("lead", None, GuestAgentState::Busy);
        world.guest_agents = vec![lead.clone()];
        let stranger = AgentId::derive("stranger");
        let mut runs = follower();
        let box_ = WorkspaceId::derive("box");
        runs.take_guest(&world, box_, snapshot(), vec![lead.id, stranger])
            .unwrap();
        let published = world.last();
        assert_eq!(published[0].orchestrators, [lead.id]);
        assert!(!published[0].needs_you);

        // The lead's state is the host's, and counts on the next tick.
        world.guest_agents[0].state = GuestAgentState::Waiting;
        runs.tick(&MemFiles::default(), &world, Instant::now());
        assert!(world.last()[0].needs_you);
    }
}
