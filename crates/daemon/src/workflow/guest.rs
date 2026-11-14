// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Runs in containers. The host sends the guest the container's own
//! agents, the guest follows the runs in its worktrees with them, and
//! sends the host each run whole when it changes, with the leads it bound.
//! The host trusts none of it: it checks every size, keeps only the
//! container's own agents, and only links to the container's own ports.
//! A node's details come from the guest too, on request.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use maxhq_protocol::guest::{GuestAgent, GuestAgentState, GuestToHost, HostToGuest};
use maxhq_protocol::ids::{AgentId, DeviceId, ProjectId, WorkflowRunId, WorkspaceId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::workflow::{NodeDetail, RunKey, RunSnapshot, WorkflowRun};
use maxhq_workflow::{AgentFacts, AgentState, Agents, Files, NoAgents};
use tokio::sync::watch;
use url::Url;

use super::disk::Disk;
use super::env::Envs;
use super::follow::{Detail, Follower, Root, World, TICK};
use crate::guestlink::wire::Outbox;
use crate::guestlink::GuestSender;
use crate::session::clock::BoxFuture;

/// The most bytes in any one string a guest sends.
const MAX_TEXT: usize = 4096;
/// The most nodes, edges or groups in a run.
const MAX_NODES: usize = 500;
/// The most items in any other list.
pub const MAX_LIST: usize = 64;
/// How long a guest has to answer for a node's details.
pub const DETAIL_WAIT: Duration = Duration::from_secs(5);

/// Whether a string from a guest is small enough to take.
fn text(t: &str) -> bool {
    t.len() <= MAX_TEXT
}

/// Whether a snapshot from a guest is small enough to take.
pub fn check(s: &RunSnapshot) -> Result<(), &'static str> {
    let maybe = |t: &Option<String>| t.as_deref().is_none_or(text);
    let shape = &s.shape;
    let header = &s.header;
    let ok = [&s.key.workflow, &s.key.run, &shape.hash, &header.label]
        .into_iter()
        .all(|t| text(t))
        && shape.nodes.len() <= MAX_NODES
        && shape.edges.len() <= MAX_NODES * 4
        && shape.groups.len() <= MAX_NODES
        && s.nodes.len() <= MAX_NODES
        && s.loops.len() <= MAX_LIST
        && header.lines.len() <= MAX_LIST
        && header.lines.iter().all(|b| text(&b.text))
        && s.notice.as_ref().is_none_or(|n| text(&n.text))
        && shape.nodes.iter().all(|n| {
            text(&n.id) && text(&n.label) && maybe(&n.group) && maybe(&n.lane) && maybe(&n.host)
        })
        && shape.edges.iter().all(|e| text(&e.from) && text(&e.to))
        && shape.groups.iter().all(|g| text(&g.id) && text(&g.label))
        && s.loops
            .iter()
            .all(|l| text(&l.from) && text(&l.to) && maybe(&l.label))
        && s.nodes.iter().all(|n| {
            text(&n.id)
                && maybe(&n.note)
                && n.badges.len() <= MAX_LIST
                && n.badges.iter().all(|b| text(&b.text))
                && n.attempts.len() <= MAX_LIST
                && n.agents.len() <= MAX_LIST
                && n.links.len() <= MAX_LIST
                && n.links.iter().all(|l| text(&l.label) && text(&l.url))
        });
    if ok {
        Ok(())
    } else {
        Err("the guest sent a run over the size limits")
    }
}

/// Whether a node's details from a guest are small enough to take.
pub fn check_detail(d: &NodeDetail) -> Result<(), &'static str> {
    let ok = d.lists.len() <= MAX_LIST
        && d.lists.iter().all(|l| {
            text(&l.label) && l.items.len() <= MAX_NODES && l.items.iter().all(|t| text(t))
        });
    if ok {
        Ok(())
    } else {
        Err("the guest sent details over the size limits")
    }
}

/// A checked snapshot with what the host can't take from a guest taken
/// out: agents but the container's `own`, and links anywhere but the
/// container's own ports.
pub fn trusted(mut s: RunSnapshot, ports: &[u16], own: &[AgentId]) -> RunSnapshot {
    for node in &mut s.nodes {
        node.agents.retain(|a| own.contains(a));
        for attempt in &mut node.attempts {
            attempt.agent = attempt.agent.filter(|a| own.contains(a));
        }
        node.links.retain(|l| own_port(&l.url, ports));
    }
    s
}

/// The leads a guest bound that are the container's `own` agents, once
/// each.
pub fn own_only(mut leads: Vec<AgentId>, own: &[AgentId]) -> Vec<AgentId> {
    let mut seen = HashSet::new();
    leads.retain(|a| own.contains(a) && seen.insert(*a));
    leads
}

/// What the engine needs to know of an agent the host sent.
pub fn facts_of(a: &GuestAgent) -> AgentFacts {
    AgentFacts {
        id: a.id,
        lead: a.lead,
        title: a.title.clone(),
        pid: a.pid,
        state: match a.state {
            GuestAgentState::Busy => AgentState::Busy,
            GuestAgentState::Waiting => AgentState::Waiting,
            GuestAgentState::Idle => AgentState::Idle,
            GuestAgentState::Gone => AgentState::Gone,
        },
        started: a.started,
        last_write: a.last_write,
        tool_open: false,
    }
}

/// How an agent stands, in the guest link's words.
pub fn guest_state(state: AgentState) -> GuestAgentState {
    match state {
        AgentState::Busy => GuestAgentState::Busy,
        AgentState::Waiting => GuestAgentState::Waiting,
        AgentState::Idle => GuestAgentState::Idle,
        AgentState::Gone => GuestAgentState::Gone,
    }
}

/// The host's side of one container's link, as the follower uses it.
pub trait GuestLink: Send + Sync {
    /// Whether the link is still up. Once false, it stays so.
    fn is_up(&self) -> bool;
    /// Sends the container's agents, in order with earlier sends. A send
    /// not yet made is replaced.
    fn agents(&self, agents: Vec<GuestAgent>);
    /// Asks the guest for a node's details, waiting up to `wait`.
    fn ask(
        &self,
        key: &RunKey,
        node: &str,
        wait: Duration,
    ) -> BoxFuture<Result<GuestToHost, String>>;
}

/// A real link to a container's guest.
struct Linked {
    sender: GuestSender,
    agents: watch::Sender<Vec<GuestAgent>>,
}

/// The follower's link to a container, which sends the agents it's given
/// from a task of its own until the link goes down.
pub fn linked(workspace: WorkspaceId, sender: GuestSender) -> Arc<dyn GuestLink> {
    let (agents, mut changed) = watch::channel(Vec::new());
    changed.mark_unchanged();
    let to = sender.clone();
    tokio::spawn(async move {
        while changed.changed().await.is_ok() {
            let message = HostToGuest::WorkflowAgents {
                agents: changed.borrow_and_update().clone(),
            };
            if let Err(err) = to.send(&message).await {
                tracing::debug!(%workspace, "couldn't send the guest its agents: {err}");
                if !to.is_up() {
                    return;
                }
            }
        }
    });
    Arc::new(Linked { sender, agents })
}

impl GuestLink for Linked {
    fn is_up(&self) -> bool {
        self.sender.is_up()
    }

    fn agents(&self, agents: Vec<GuestAgent>) {
        self.agents.send_replace(agents);
    }

    fn ask(
        &self,
        key: &RunKey,
        node: &str,
        wait: Duration,
    ) -> BoxFuture<Result<GuestToHost, String>> {
        let sender = self.sender.clone();
        let (workflow, run, node) = (key.workflow.clone(), key.run.clone(), node.to_owned());
        Box::pin(async move {
            sender
                .request(wait, |req| HostToGuest::WorkflowDetail {
                    req,
                    workflow,
                    run,
                    node,
                })
                .await
                .map_err(|err| err.to_string())
        })
    }
}

/// The links up to each container, and the agents each guest last got.
#[derive(Default)]
pub struct Links {
    up: HashMap<WorkspaceId, (Arc<dyn GuestLink>, Option<LastSent>)>,
}

/// The agents a guest last got.
type LastSent = Vec<GuestAgent>;

impl Links {
    /// A container's link came up, so its guest gets its agents afresh.
    pub fn up(&mut self, workspace: WorkspaceId, link: Arc<dyn GuestLink>) {
        self.up.insert(workspace, (link, None));
    }

    /// Forgets links that went down, and sends each guest its own agents
    /// when they changed.
    pub fn tick(&mut self, world: &dyn World) {
        self.up.retain(|_, (link, _)| link.is_up());
        for (workspace, (link, sent)) in &mut self.up {
            let agents = world.guest_agents(*workspace);
            if sent.as_ref() != Some(&agents) {
                link.agents(agents.clone());
                *sent = Some(agents);
            }
        }
    }

    /// A container's link, while it's up.
    pub fn get(&self, workspace: WorkspaceId) -> Option<Arc<dyn GuestLink>> {
        self.up
            .get(&workspace)
            .map(|(link, _)| link.clone())
            .filter(|link| link.is_up())
    }
}

/// Asks a container's guest for a node's details, and checks them.
pub async fn ask_detail(
    link: Option<Arc<dyn GuestLink>>,
    key: &RunKey,
    node: &str,
    wait: Duration,
) -> ActionOutcome {
    let refused = |reason: &str| ActionOutcome::Refused {
        reason: reason.to_owned(),
    };
    let Some(link) = link else {
        return refused("The container isn't connected.");
    };
    let reply = match tokio::time::timeout(wait, link.ask(key, node, wait)).await {
        Ok(Ok(reply)) => reply,
        Ok(Err(err)) => return refused(&format!("The container didn't answer: {err}.")),
        Err(_) => return refused("The container didn't answer in time."),
    };
    match reply {
        GuestToHost::WorkflowDetail {
            result: Ok(detail), ..
        } => match check_detail(&detail) {
            Ok(()) => ActionOutcome::NodeDetail { detail },
            Err(why) => refused(&format!("The container's answer was refused: {why}.")),
        },
        GuestToHost::WorkflowDetail {
            result: Err(reason),
            ..
        } if text(&reason) => refused(&reason),
        GuestToHost::WorkflowDetail { .. } => {
            refused("The container's answer was refused: its reason was too long.")
        }
        other => refused(&crate::guestlink::unexpected(&other)),
    }
}

/// Whether `url` goes to one of the container's ports, as the host
/// forwards them to `localhost`.
fn own_port(url: &str, ports: &[u16]) -> bool {
    let Ok(url) = Url::parse(url) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && matches!(url.host_str(), Some("localhost" | "127.0.0.1"))
        && url.port().is_some_and(|p| ports.contains(&p))
}

/// The guest's side: the repo and each of its worktrees, with the agents
/// the host sent, once it has.
struct Worktrees {
    work: PathBuf,
    agents: Option<Vec<GuestAgent>>,
}

/// The guest names no device or project; the host fills in its own.
fn guest_project() -> ProjectId {
    ProjectId::derive("guest")
}

impl World for Worktrees {
    fn roots(&self, _open: &HashSet<ProjectId>) -> Vec<Root> {
        let project = guest_project();
        let mut paths = vec![self.work.clone()];
        if let Ok(entries) = Disk.list(&self.work.join(crate::guest::worktree::WORKTREES)) {
            paths.extend(entries.into_iter().filter(|e| e.dir).map(|e| {
                self.work
                    .join(crate::guest::worktree::WORKTREES)
                    .join(e.name)
            }));
        }
        // A lead works in its branch's worktree, or else in the repo.
        let agents = self.agents.as_deref().unwrap_or_default();
        let home = |a: &GuestAgent| {
            a.branch
                .as_deref()
                .map(|b| crate::guest::worktree::path(&self.work, b))
                .filter(|p| paths.contains(p))
                .unwrap_or_else(|| self.work.clone())
        };
        paths
            .iter()
            .map(|path| Root {
                path: path.clone(),
                project,
                leads: agents
                    .iter()
                    .filter(|a| a.lead.is_none() && home(a) == *path)
                    .map(|a| a.id)
                    .collect(),
            })
            .collect()
    }

    fn agents(&self, _project: ProjectId) -> Box<dyn Agents + '_> {
        match &self.agents {
            Some(agents) => Box::new(Sent(agents)),
            None => Box::new(NoAgents),
        }
    }

    fn guest_project(&self, _workspace: WorkspaceId) -> Option<ProjectId> {
        None
    }

    fn forwarded(&self, _workspace: WorkspaceId) -> Vec<u16> {
        Vec::new()
    }

    fn guest_agents(&self, _workspace: WorkspaceId) -> Vec<GuestAgent> {
        Vec::new()
    }

    fn publish(&self, _runs: Vec<WorkflowRun>) {}
}

/// The agents the host sent, as the engine reads them.
struct Sent<'a>(&'a [GuestAgent]);

impl Sent<'_> {
    fn find(&self, agent: AgentId) -> Option<&GuestAgent> {
        self.0.iter().find(|a| a.id == agent)
    }
}

impl Agents for Sent<'_> {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }

    fn list(&self) -> Vec<AgentFacts> {
        self.0.iter().map(facts_of).collect()
    }

    fn prompt_head(&self, agent: AgentId) -> Option<String> {
        self.find(agent).and_then(|a| a.prompt_head.clone())
    }

    fn each_command(&self, agent: AgentId, visit: &mut dyn FnMut(&str)) {
        for command in self.find(agent).iter().flat_map(|a| &a.commands) {
            visit(command);
        }
    }
}

/// The runs a guest follows, and the revs and leads it last sent the host.
pub struct GuestRuns {
    follower: Follower,
    world: Worktrees,
    sent: HashMap<WorkflowRunId, (u64, Vec<AgentId>)>,
}

impl GuestRuns {
    /// Follows the runs under `work`.
    pub fn new(work: PathBuf, retry_pause: Duration) -> Self {
        Self {
            follower: Follower::new(DeviceId::derive("guest"), Envs::default(), retry_pause),
            world: Worktrees { work, agents: None },
            sent: HashMap::new(),
        }
    }

    /// Takes the container's agents, as the host sent them.
    pub fn agents(&mut self, agents: Vec<GuestAgent>) {
        self.world.agents = Some(agents);
    }

    /// Checks the runs, and gives each that changed since it was last
    /// sent, with its leads.
    pub fn tick(&mut self, files: &dyn Files, now: Instant) -> Vec<(RunSnapshot, Vec<AgentId>)> {
        self.follower.tick(files, &self.world, now);
        let mut changed = Vec::new();
        let mut sent = HashMap::new();
        for (snapshot, leads) in self.follower.followed() {
            let id = snapshot.key.id();
            let now = (snapshot.rev, leads.to_vec());
            if self.sent.get(&id) != Some(&now) {
                changed.push((snapshot.clone(), now.1.clone()));
            }
            sent.insert(id, now);
        }
        self.sent = sent;
        changed
    }

    /// The descriptor's lists for one node of a run here.
    pub fn detail(
        &self,
        files: &dyn Files,
        workflow: String,
        run: String,
        node: &str,
    ) -> Result<NodeDetail, String> {
        let key = RunKey {
            device: DeviceId::derive("guest"),
            project: guest_project(),
            workflow,
            run,
        };
        match self.follower.detail(files, &key, node) {
            Detail::Here(ActionOutcome::NodeDetail { detail }) => Ok(detail),
            Detail::Here(ActionOutcome::Refused { reason }) => Err(reason),
            _ => Err("That run isn't followed in this container.".to_owned()),
        }
    }
}

/// Stops a link's runner when dropped, and hands it what the host sends
/// about workflows.
pub struct Following(mpsc::Sender<HostToGuest>);

impl Following {
    /// Hands the runner a `WorkflowAgents` or `WorkflowDetail`.
    pub fn take(&self, message: HostToGuest) {
        let _ = self.0.send(message);
    }
}

/// Follows the runs under `work` for one host link, sending every run when
/// it starts and each change after, until the guard drops or the link
/// does.
pub fn follow(work: &Path, outbox: Outbox) -> Following {
    let (inputs, rx) = mpsc::channel();
    let runtime = tokio::runtime::Handle::current();
    let work = work.to_owned();
    let spawned = std::thread::Builder::new()
        .name("workflows".to_owned())
        .spawn(move || {
            let mut runs = GuestRuns::new(work, super::RETRY_PAUSE);
            let send = |message: GuestToHost| runtime.block_on(outbox.send(&message)).is_ok();
            let mut next = Instant::now();
            loop {
                match rx.recv_timeout(next.saturating_duration_since(Instant::now())) {
                    Ok(HostToGuest::WorkflowAgents { agents }) => {
                        runs.agents(agents);
                        next = Instant::now();
                    }
                    Ok(HostToGuest::WorkflowDetail {
                        req,
                        workflow,
                        run,
                        node,
                    }) => {
                        let result = runs.detail(&Disk, workflow, run, &node);
                        if !send(GuestToHost::WorkflowDetail { req, result }) {
                            return;
                        }
                    }
                    Ok(_) => {}
                    Err(RecvTimeoutError::Timeout) => {
                        for (snapshot, leads) in runs.tick(&Disk, Instant::now()) {
                            let message = GuestToHost::WorkflowSnapshot {
                                snapshot: Box::new(snapshot),
                                leads,
                            };
                            if !send(message) {
                                return;
                            }
                        }
                        next = Instant::now() + TICK;
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        });
    if let Err(err) = spawned {
        tracing::warn!("couldn't start following workflow runs: {err}");
    }
    Following(inputs)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use maxhq_protocol::workflow::{DetailList, Link, NodeShape};

    use super::*;
    use crate::workflow::follow::tests::{guest_agent, snapshot, world, write_run};

    #[test]
    fn a_snapshot_over_the_caps_is_refused() {
        let mut s = snapshot();
        assert_eq!(check(&s), Ok(()));
        s.header.label = "x".repeat(MAX_TEXT + 1);
        assert!(check(&s).is_err());

        let mut s = snapshot();
        s.shape.nodes = (0..=MAX_NODES)
            .map(|i| NodeShape {
                id: i.to_string(),
                label: String::new(),
                group: None,
                lane: None,
                host: None,
            })
            .collect();
        assert!(check(&s).is_err());

        let mut s = snapshot();
        s.nodes[0].links = vec![
            Link {
                label: "x".to_owned(),
                url: "http://localhost:1".to_owned(),
            };
            MAX_LIST + 1
        ];
        assert!(check(&s).is_err());
    }

    #[test]
    fn only_links_to_the_containers_ports_stay() {
        let mut s = snapshot();
        let link = |url: &str| Link {
            label: "x".to_owned(),
            url: url.to_owned(),
        };
        s.nodes[0].links = vec![
            link("http://localhost:3000/"),
            link("https://127.0.0.1:3000"),
            link("http://localhost:4000/"),
            link("https://example.com/"),
            link("javascript:alert(1)"),
        ];
        let s = trusted(s, &[3000], &[]);
        let urls: Vec<&str> = s.nodes[0].links.iter().map(|l| l.url.as_str()).collect();
        assert_eq!(urls, ["http://localhost:3000/", "https://127.0.0.1:3000"]);
    }

    #[test]
    fn only_the_containers_own_agents_stay() {
        let (own, other) = (AgentId::derive("own"), AgentId::derive("other"));
        let mut s = snapshot();
        s.nodes[0].agents = vec![own, other];
        let s = trusted(s, &[], &[own]);
        assert_eq!(s.nodes[0].agents, [own]);
        assert_eq!(own_only(vec![other, own, own], &[own]), [own]);
    }

    /// A link that keeps what it was sent, and answers with `answer`.
    struct FakeLink {
        sent: Mutex<Vec<Vec<GuestAgent>>>,
        answer: Option<GuestToHost>,
    }

    fn fake(answer: Option<GuestToHost>) -> Arc<FakeLink> {
        Arc::new(FakeLink {
            sent: Mutex::new(Vec::new()),
            answer,
        })
    }

    impl GuestLink for FakeLink {
        fn is_up(&self) -> bool {
            true
        }
        fn agents(&self, agents: Vec<GuestAgent>) {
            self.sent.lock().unwrap().push(agents);
        }
        fn ask(&self, _: &RunKey, _: &str, _: Duration) -> BoxFuture<Result<GuestToHost, String>> {
            match self.answer.clone() {
                Some(answer) => Box::pin(async move { Ok(answer) }),
                None => Box::pin(std::future::pending()),
            }
        }
    }

    #[test]
    fn each_guest_gets_its_own_agents_when_they_change() {
        let mut world = world();
        world.guest_agents = vec![guest_agent("lead", None, GuestAgentState::Busy)];
        let (box_, other) = (fake(None), fake(None));
        let mut links = Links::default();
        links.up(WorkspaceId::derive("box"), box_.clone());
        links.up(WorkspaceId::derive("other"), other.clone());
        links.tick(&world);
        links.tick(&world);
        assert_eq!(*box_.sent.lock().unwrap(), [world.guest_agents.clone()]);
        assert_eq!(*other.sent.lock().unwrap(), [Vec::new()]);

        world.guest_agents[0].state = GuestAgentState::Waiting;
        links.tick(&world);
        assert_eq!(box_.sent.lock().unwrap().len(), 2);
        assert_eq!(other.sent.lock().unwrap().len(), 1);
    }

    fn detail(lists: Vec<DetailList>) -> GuestToHost {
        GuestToHost::WorkflowDetail {
            req: 1,
            result: Ok(NodeDetail { lists }),
        }
    }

    async fn ask(link: Option<Arc<FakeLink>>) -> ActionOutcome {
        let link = link.map(|l| l as Arc<dyn GuestLink>);
        let key = snapshot().key;
        ask_detail(link, &key, "plan", Duration::from_millis(20)).await
    }

    #[tokio::test]
    async fn a_detail_request_is_answered_checked_or_timed_out() {
        let files = vec![DetailList {
            label: "Files".to_owned(),
            items: vec!["a.rs".to_owned()],
        }];
        assert_eq!(
            ask(Some(fake(Some(detail(files.clone()))))).await,
            ActionOutcome::NodeDetail {
                detail: NodeDetail { lists: files }
            }
        );

        let too_many = vec![
            DetailList {
                label: String::new(),
                items: Vec::new(),
            };
            MAX_LIST + 1
        ];
        let long = vec![DetailList {
            label: String::new(),
            items: vec!["x".repeat(MAX_TEXT + 1)],
        }];
        for lists in [too_many, long] {
            let outcome = ask(Some(fake(Some(detail(lists))))).await;
            assert!(
                matches!(outcome, ActionOutcome::Refused { reason } if reason.contains("size limits"))
            );
        }

        let refused = |reason: &str| ActionOutcome::Refused {
            reason: reason.to_owned(),
        };
        assert_eq!(
            ask(Some(fake(None))).await,
            refused("The container didn't answer in time.")
        );
        assert_eq!(ask(None).await, refused("The container isn't connected."));
        let no = GuestToHost::WorkflowDetail {
            req: 1,
            result: Err("No such node.".to_owned()),
        };
        assert_eq!(ask(Some(fake(Some(no)))).await, refused("No such node."));
    }

    #[test]
    fn the_guest_sends_each_run_once_and_then_its_changes() {
        let dir = crate::testing::TempDir::new();
        let work = dir.path().to_owned();
        let tree = work.join(crate::guest::worktree::WORKTREES).join("feature");
        std::fs::create_dir_all(&tree).unwrap();
        write_run(&tree, "one", "doing");
        let mut runs = GuestRuns::new(work, Duration::ZERO);
        let now = Instant::now();
        let first = runs.tick(&Disk, now);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].0.key.run, "one");
        assert!(first[0].1.is_empty());
        assert!(runs.tick(&Disk, now + TICK).is_empty());
    }

    #[test]
    fn the_guest_binds_the_agents_the_host_sent() {
        let dir = crate::testing::TempDir::new();
        let work = dir.path().to_owned();
        let tree = crate::guest::worktree::path(&work, "feature/x");
        std::fs::create_dir_all(&tree).unwrap();
        write_run(&tree, "one", "doing");
        let mut runs = GuestRuns::new(work, Duration::ZERO);
        let now = Instant::now();
        runs.tick(&Disk, now);

        let mut lead = guest_agent("lead", None, GuestAgentState::Waiting);
        lead.branch = Some("feature/x".to_owned());
        let plan = guest_agent("plan", Some(lead.id), GuestAgentState::Busy);
        runs.agents(vec![lead.clone(), plan.clone()]);
        let sent = runs.tick(&Disk, now + TICK);
        assert_eq!(sent.len(), 1);
        let (snapshot, leads) = &sent[0];
        let node = snapshot.nodes.iter().find(|n| n.id == "plan").unwrap();
        assert_eq!(node.agents, [plan.id]);
        assert_eq!(*leads, [lead.id]);
    }

    #[test]
    fn the_guest_answers_for_a_nodes_details() {
        let dir = crate::testing::TempDir::new();
        let work = dir.path().to_owned();
        write_run(&work, "one", "doing");
        let mut runs = GuestRuns::new(work, Duration::ZERO);
        runs.tick(&Disk, Instant::now());
        let detail = runs.detail(&Disk, "example".to_owned(), "one".to_owned(), "plan");
        assert_eq!(detail, Ok(NodeDetail { lists: Vec::new() }));
        let missing = runs.detail(&Disk, "example".to_owned(), "two".to_owned(), "plan");
        assert!(missing.is_err());
    }
}
