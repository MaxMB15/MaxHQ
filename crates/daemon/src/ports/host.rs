// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The host's side of port detection. Each workspace's ports arrive as a
//! whole list, from a container's guest link or from [`run_local`] for
//! host folder agents. [`HostPorts`] turns them into `ListeningPort`
//! entities, keeps one that goes for [`GONE_AFTER`], and then removes it
//! and closes its forwards.
//!
//! A guest is untrusted, so its list is checked here: at most
//! [`MAX_PORTS`] entries, a session counts only when the host watches it
//! in that workspace, and the agent comes from the host's own map. Names
//! and commands are cut short again.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use maxhq_protocol::guest::{GuestPort, Pace, WatchedHolder, MAX_PORTS};
use maxhq_protocol::ids::{AgentId, PortId, SessionId, WorkspaceId};
use maxhq_protocol::model::{BindAddress, ListeningPort};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use tokio::sync::watch;
use tracing::warn;

use super::{
    bind_of, ignored, interval, process, processes, Debounce, Jobs, Scanner, SharedScanner,
    GONE_AFTER,
};
use crate::hub::Hub;
use crate::tunnel::Tunnels;

/// How often gone ports are checked for removal.
const SWEEP_EVERY: Duration = Duration::from_secs(1);
/// The most ports one workspace keeps, gone ones too. Past it, gone ports
/// go at once.
const MAX_KEPT: usize = 2 * MAX_PORTS;

/// Every workspace's ports. Clone it into the guest links and
/// [`run_local`].
#[derive(Clone)]
pub struct HostPorts {
    hub: Hub,
    tunnels: Tunnels,
    books: Arc<Mutex<HashMap<WorkspaceId, Book>>>,
}

/// One workspace's holders and ports.
#[derive(Default)]
struct Book {
    /// The agent each watched holder runs.
    agents: HashMap<SessionId, Option<AgentId>>,
    /// Each port as the hub has it, by number.
    ports: HashMap<u16, ListeningPort>,
}

/// A host folder agent's holder, watched for ports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalHolder {
    /// The folder workspace the agent runs in.
    pub workspace: WorkspaceId,
    /// The holder.
    pub holder: WatchedHolder,
}

impl HostPorts {
    /// Keeps ports in `hub` and closes their forwards through `tunnels`.
    pub fn new(hub: Hub, tunnels: Tunnels) -> Self {
        Self {
            hub,
            tunnels,
            books: Arc::default(),
        }
    }

    /// The holders watched in `workspace` now, the same list the guest gets
    /// in `WatchPorts`. Only their sessions count in a report.
    pub fn watch(&self, workspace: WorkspaceId, holders: &[WatchedHolder]) {
        let mut books = self.lock();
        books.entry(workspace).or_default().agents = holders
            .iter()
            .map(|holder| (holder.session, holder.agent))
            .collect();
    }

    /// Every port listening in `workspace` now. A shown port missing from
    /// the list is gone, and one that comes back within [`GONE_AFTER`]
    /// keeps its ID.
    pub fn report(&self, workspace: WorkspaceId, ports: Vec<GuestPort>) {
        self.report_at(workspace, ports, Timestamp::now());
    }

    /// Marks every port in `workspace` gone, as when its guest link drops.
    pub fn down(&self, workspace: WorkspaceId) {
        self.report(workspace, Vec::new());
    }

    /// Removes `workspace`'s ports at once and closes their forwards, as
    /// when the workspace is deleted.
    pub fn forget(&self, workspace: WorkspaceId) {
        let removed: Vec<PortId> = {
            let mut books = self.lock();
            let Some(book) = books.remove(&workspace) else {
                return;
            };
            let removed: Vec<PortId> = book.ports.values().map(|port| port.id).collect();
            self.hub.apply(removed.iter().map(|port| removal(*port)));
            removed
        };
        self.close(removed);
    }

    /// Removes the ports gone for [`GONE_AFTER`] and closes their forwards.
    pub fn sweep(&self) {
        self.sweep_at(Timestamp::now());
    }

    /// Calls [`sweep`](Self::sweep) every second, for as long as it runs.
    pub async fn sweep_forever(self) {
        let mut every = tokio::time::interval(SWEEP_EVERY);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            every.tick().await;
            self.sweep();
        }
    }

    fn report_at(&self, workspace: WorkspaceId, ports: Vec<GuestPort>, now: Timestamp) {
        let skip = self.ignored_ports(workspace);
        let removed = {
            let mut books = self.lock();
            let book = books.entry(workspace).or_default();
            let mut fresh: HashMap<u16, ListeningPort> = HashMap::new();
            let mut changes = Vec::new();
            for port in ports.into_iter().take(MAX_PORTS) {
                if port.port == 0 || fresh.contains_key(&port.port) || skip.contains(&port.port) {
                    continue;
                }
                let Some(bind) = checked(&port.bind, port.port) else {
                    continue;
                };
                let agent = port
                    .session
                    .and_then(|session| book.agents.get(&session).copied().flatten());
                let process = port
                    .process
                    .map(|found| process(found.pid, &found.name, &found.command));
                if let (Some(agent), Some(found)) = (agent, &process)
                    && self.is_cli(agent, found.pid)
                {
                    continue;
                }
                let old = book.ports.get(&port.port);
                let entity = ListeningPort {
                    id: old.map_or_else(PortId::generate, |old| old.id),
                    workspace,
                    agent,
                    port: port.port,
                    bind,
                    process,
                    url_hint: old.and_then(|old| old.url_hint.clone()),
                    first_seen: old.map_or(now, |old| old.first_seen),
                    gone_since: None,
                };
                if old != Some(&entity) {
                    changes.push(upsert(&entity));
                }
                fresh.insert(port.port, entity);
            }
            let mut removed = Vec::new();
            for (number, mut old) in book.ports.drain() {
                if fresh.contains_key(&number) {
                    continue;
                }
                if old.gone_since.is_none() {
                    old.gone_since = Some(now);
                    changes.push(upsert(&old));
                }
                fresh.insert(number, old);
            }
            if fresh.len() > MAX_KEPT {
                fresh.retain(|_, port| {
                    let keep = port.gone_since.is_none();
                    if !keep {
                        removed.push(port.id);
                        changes.push(removal(port.id));
                    }
                    keep
                });
            }
            book.ports = fresh;
            self.hub.apply(changes);
            removed
        };
        self.close(removed);
    }

    fn sweep_at(&self, now: Timestamp) {
        let removed = {
            let mut books = self.lock();
            let mut removed = Vec::new();
            for book in books.values_mut() {
                book.ports.retain(|_, port| {
                    let due = port
                        .gone_since
                        .is_some_and(|gone| now >= gone.after(GONE_AFTER));
                    if due {
                        removed.push(port.id);
                    }
                    !due
                });
            }
            books.retain(|_, book| !book.ports.is_empty() || !book.agents.is_empty());
            self.hub.apply(removed.iter().map(|port| removal(*port)));
            removed
        };
        self.close(removed);
    }

    /// Closes the forwards on ports that went for good. It runs without the
    /// lock on the books, so the tunnels' lock is never taken inside it.
    fn close(&self, removed: Vec<PortId>) {
        for port in removed {
            self.tunnels.port_removed(port);
        }
    }

    /// The project's `ignored_ports` for `workspace`, from the settings on
    /// the device that holds it.
    fn ignored_ports(&self, workspace: WorkspaceId) -> HashSet<u16> {
        let Some(Entity::Workspace(workspace)) = self.hub.get(&EntityKey::Workspace(workspace))
        else {
            return HashSet::new();
        };
        let Some(project) = workspace.project else {
            return HashSet::new();
        };
        let key = EntityKey::ProjectSettings {
            project,
            device: workspace.device,
        };
        match self.hub.get(&key) {
            Some(Entity::ProjectSettings(settings)) => settings.ignored_ports.into_iter().collect(),
            _ => HashSet::new(),
        }
    }

    /// Whether `pid` is the agent's CLI itself, whose own listeners never
    /// show.
    fn is_cli(&self, agent: AgentId, pid: u32) -> bool {
        matches!(
            self.hub.get(&EntityKey::Agent(agent)),
            Some(Entity::Agent(agent)) if agent.pid == Some(pid)
        )
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<WorkspaceId, Book>> {
        self.books.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A bind as it shows, or none when it names no address or a port on the
/// fixed ignore list.
fn checked(bind: &BindAddress, port: u16) -> Option<BindAddress> {
    let ip = match bind {
        BindAddress::Loopback => IpAddr::V4(Ipv4Addr::LOCALHOST),
        BindAddress::Any => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        BindAddress::Address { ip } => ip.parse().ok()?,
    };
    (!ignored(ip, port)).then(|| match bind {
        BindAddress::Address { .. } => bind_of(ip),
        other => other.clone(),
    })
}

fn upsert(port: &ListeningPort) -> Change {
    Change::Upsert {
        value: Entity::ListeningPort(port.clone()),
    }
}

fn removal(port: PortId) -> Change {
    Change::Remove {
        key: EntityKey::ListeningPort(port),
    }
}

/// Polls this machine for the ports of host folder agents until `watched`
/// closes, then marks them all gone. The session manager sends the pace
/// and every holder each time either changes, and lists processes through
/// the same `scanner`. Each poll also puts what each one-agent holder runs
/// besides its CLI in `jobs`, which changes only when that does.
pub async fn run_local(
    ports: HostPorts,
    watched: watch::Receiver<(Pace, Vec<LocalHolder>)>,
    scanner: SharedScanner,
    jobs: Arc<watch::Sender<Jobs>>,
) {
    run_with(ports, watched, scanner, jobs, interval).await;
}

/// The scanner for this machine's holders: Linux reads `/proc`, and macOS
/// asks libproc about the holders' trees only.
pub fn local_scanner() -> SharedScanner {
    Arc::new(Mutex::new(scanner_here()))
}

#[cfg(target_os = "linux")]
fn scanner_here() -> Box<dyn Scanner> {
    Box::new(super::Procfs::new("/proc", false))
}

#[cfg(target_os = "macos")]
fn scanner_here() -> Box<dyn Scanner> {
    Box::new(super::Libproc::new())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn scanner_here() -> Box<dyn Scanner> {
    struct Nothing;
    impl Scanner for Nothing {
        fn scan(&mut self, _holders: &[WatchedHolder]) -> Vec<GuestPort> {
            Vec::new()
        }
    }
    Box::new(Nothing)
}

async fn run_with(
    ports: HostPorts,
    mut watched: watch::Receiver<(Pace, Vec<LocalHolder>)>,
    scanner: SharedScanner,
    jobs: Arc<watch::Sender<Jobs>>,
    tick: fn(Pace) -> Duration,
) {
    let mut debounce = Debounce::new();
    let mut reported: HashSet<WorkspaceId> = HashSet::new();
    loop {
        let (pace, holders) = watched.borrow_and_update().clone();
        let mut by_workspace: HashMap<WorkspaceId, Vec<WatchedHolder>> = HashMap::new();
        for local in &holders {
            by_workspace
                .entry(local.workspace)
                .or_default()
                .push(local.holder.clone());
        }
        for (workspace, watched) in &by_workspace {
            ports.watch(*workspace, watched);
        }
        let (found, running) = if holders.is_empty() {
            (Vec::new(), Jobs::new())
        } else {
            let all: Vec<WatchedHolder> =
                holders.iter().map(|local| local.holder.clone()).collect();
            let scanner = scanner.clone();
            match tokio::task::spawn_blocking(move || {
                let mut scanner = scanner.lock().unwrap_or_else(PoisonError::into_inner);
                let found = scanner.scan(&all);
                (found, jobs_of(&mut **scanner, &all))
            })
            .await
            {
                Ok(polled) => polled,
                Err(error) => {
                    warn!(%error, "the port scan failed, so host ports stop");
                    break;
                }
            }
        };
        jobs.send_if_modified(|was| {
            let changed = *was != running;
            if changed {
                *was = running;
            }
            changed
        });
        let workspace_of: HashMap<SessionId, WorkspaceId> = holders
            .iter()
            .map(|local| (local.holder.session, local.workspace))
            .collect();
        let mut split: HashMap<WorkspaceId, Vec<GuestPort>> = by_workspace
            .keys()
            .map(|workspace| (*workspace, Vec::new()))
            .collect();
        for port in debounce.poll(found) {
            if let Some(workspace) = port.session.and_then(|session| workspace_of.get(&session)) {
                split.entry(*workspace).or_default().push(port);
            }
        }
        for workspace in reported
            .iter()
            .filter(|workspace| !split.contains_key(workspace))
        {
            ports.report(*workspace, Vec::new());
        }
        reported = split.keys().copied().collect();
        for (workspace, list) in split {
            ports.report(workspace, list);
        }
        tokio::select! {
            () = tokio::time::sleep(tick(pace)) => {}
            changed = watched.changed() => if changed.is_err() { break; },
        }
    }
    for workspace in reported {
        ports.down(workspace);
    }
    jobs.send_if_modified(|was| {
        let changed = !was.is_empty();
        was.clear();
        changed
    });
}

/// What each holder of one agent runs besides its CLI. A holder shared by
/// several agents, as Codex's is, can't say whose a job is, so it has none.
fn jobs_of(scanner: &mut dyn Scanner, holders: &[WatchedHolder]) -> Jobs {
    holders
        .iter()
        .filter(|holder| holder.agent.is_some())
        .filter_map(|holder| {
            Some((
                holder.session,
                processes::running(&scanner.processes(holder))?,
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use maxhq_protocol::ids::{ClientId, ProjectId};
    use maxhq_protocol::model::{
        Activity, Agent, AgentLink, Control, PortProcess, ProjectSettings, Vendor, Workspace,
        WorkspaceLocation,
    };

    use super::*;
    use crate::tunnel::registry_tests::{forward, sink, world, World};

    const T0: Timestamp = Timestamp(1_000_000);

    fn at(seconds: u64) -> Timestamp {
        T0.after(Duration::from_secs(seconds))
    }

    fn holder(name: &str, pid: u32) -> WatchedHolder {
        WatchedHolder {
            session: SessionId::derive(name),
            agent: Some(AgentId::derive(name)),
            pid,
        }
    }

    fn port(number: u16, session: Option<&str>) -> GuestPort {
        GuestPort {
            port: number,
            bind: BindAddress::Loopback,
            session: session.map(SessionId::derive),
            process: Some(PortProcess {
                pid: 700 + u32::from(number % 100),
                name: "node".into(),
                command: "node server.js".into(),
            }),
        }
    }

    fn shown(world: &World, ports: &HostPorts, workspace: WorkspaceId) -> Vec<ListeningPort> {
        let books = ports.lock();
        let mut list: Vec<ListeningPort> = books
            .get(&workspace)
            .map(|book| book.ports.values().cloned().collect())
            .unwrap_or_default();
        list.sort_by_key(|port| port.port);
        for port in &list {
            assert_eq!(
                world.hub.get(&EntityKey::ListeningPort(port.id)),
                Some(Entity::ListeningPort(port.clone()))
            );
        }
        list
    }

    fn setup() -> (World, HostPorts) {
        let world = world();
        let ports = HostPorts::new(world.hub.clone(), world.tunnels.clone());
        ports.watch(world.container, &[holder("a", 50)]);
        (world, ports)
    }

    #[test]
    fn a_gone_port_stays_ten_seconds_and_closes_its_forwards() {
        let (world, ports) = setup();
        ports.report_at(world.container, vec![port(5173, Some("a"))], at(0));
        let listed = shown(&world, &ports, world.container);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].agent, Some(AgentId::derive("a")));
        assert_eq!((listed[0].first_seen, listed[0].gone_since), (at(0), None));

        let (client_sink, _client_rx) = sink();
        let client = world.tunnels.attach(ClientId::derive("mac"), client_sink);
        let opened = forward(&client, listed[0].id);

        ports.report_at(world.container, Vec::new(), at(4));
        assert_eq!(
            shown(&world, &ports, world.container)[0].gone_since,
            Some(at(4))
        );
        ports.sweep_at(at(13));
        assert_eq!(shown(&world, &ports, world.container).len(), 1);
        assert!(world.hub.get(&EntityKey::Forward(opened)).is_some());

        ports.sweep_at(at(14));
        assert!(shown(&world, &ports, world.container).is_empty());
        assert!(world
            .hub
            .get(&EntityKey::ListeningPort(listed[0].id))
            .is_none());
        assert!(world.hub.get(&EntityKey::Forward(opened)).is_none());
    }

    #[test]
    fn a_port_back_in_time_keeps_its_id() {
        let (world, ports) = setup();
        ports.report_at(world.container, vec![port(3000, Some("a"))], at(0));
        let first = shown(&world, &ports, world.container)[0].clone();
        ports.report_at(world.container, Vec::new(), at(2));
        ports.report_at(world.container, vec![port(3000, Some("a"))], at(5));
        ports.sweep_at(at(20));
        let again = shown(&world, &ports, world.container);
        assert_eq!(again, [first]);
    }

    #[test]
    fn a_dropped_link_marks_every_port_gone() {
        let (world, ports) = setup();
        ports.report(world.container, vec![port(3000, None), port(3001, None)]);
        ports.down(world.container);
        let listed = shown(&world, &ports, world.container);
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().all(|port| port.gone_since.is_some()));
        ports.forget(world.container);
        assert!(shown(&world, &ports, world.container).is_empty());
        assert!(world
            .hub
            .get(&EntityKey::ListeningPort(listed[0].id))
            .is_none());
    }

    #[test]
    fn a_projects_ignored_ports_never_show() {
        let (world, ports) = setup();
        let project = ProjectId::derive("p");
        let device = world.hub.device().id;
        world.hub.apply([
            Change::Upsert {
                value: Entity::Workspace(Workspace {
                    id: world.container,
                    project: Some(project),
                    device,
                    location: WorkspaceLocation::Container {
                        instance: "ws-a".into(),
                        path: "/work".into(),
                    },
                    managed: None,
                    clone: None,
                }),
            },
            Change::Upsert {
                value: Entity::ProjectSettings(ProjectSettings {
                    project,
                    device,
                    resume_on_restart: true,
                    ignored_ports: vec![9229],
                    git_author: None,
                    auto_settle_days: None,
                    settle_on_merge: true,
                    workflow_env: true,
                    memory_soft: None,
                    workflow_env_allowed: None,
                }),
            },
        ]);
        ports.report(
            world.container,
            vec![port(9229, Some("a")), port(3000, Some("a"))],
        );
        let numbers: Vec<u16> = shown(&world, &ports, world.container)
            .iter()
            .map(|port| port.port)
            .collect();
        assert_eq!(numbers, [3000]);
    }

    #[test]
    fn the_clis_own_listener_never_shows() {
        let (world, ports) = setup();
        let mut cli = port(41_000, Some("a"));
        cli.process.as_mut().unwrap().pid = 51;
        world.hub.apply([Change::Upsert {
            value: Entity::Agent(Agent {
                id: AgentId::derive("a"),
                workspace: world.container,
                branch: None,
                parent: None,
                vendor: Vendor::Claude,
                account: None,
                model: None,
                permission: None,
                session: Some(SessionId::derive("a")),
                vendor_session_id: None,
                control: Control::Held,
                pid: Some(51),
                link: AgentLink::Live,
                title: "a".into(),
                activity: Activity::Idle,
                context: None,
                started_at: T0,
                last_activity_at: T0,
                inbox: Default::default(),
                origin: None,
                pulls: Vec::new(),
                taking_over: false,
                asleep: false,
                effort: None,
                running: None,
                moved: None,
                local: None,
            }),
        }]);
        ports.report(world.container, vec![cli, port(3000, Some("a"))]);
        let listed = shown(&world, &ports, world.container);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].port, 3000);
    }

    #[test]
    fn what_a_guest_says_is_checked() {
        let (world, ports) = setup();
        let mut long = port(3000, Some("a"));
        long.process.as_mut().unwrap().name = "n".repeat(500);
        let mut bad_ip = port(3001, None);
        bad_ip.bind = BindAddress::Address {
            ip: "not an address".into(),
        };
        let mut loopback = port(3002, None);
        loopback.bind = BindAddress::Address { ip: "::1".into() };
        let mut ssh = port(22, None);
        ssh.bind = BindAddress::Any;
        let mut many = vec![
            long,
            port(3000, None),
            bad_ip,
            loopback,
            ssh,
            port(0, None),
            // A session the host doesn't watch here.
            port(3003, Some("b")),
        ];
        many.extend((10_000..10_000 + MAX_PORTS as u16).map(|number| port(number, None)));
        ports.report(world.container, many);
        let listed = shown(&world, &ports, world.container);
        // The list is cut to its limit first, then the four bad ones go.
        assert_eq!(listed.len(), MAX_PORTS - 4);
        assert_eq!(listed[0].process.as_ref().unwrap().name.len(), 64);
        assert_eq!(listed[0].agent, Some(AgentId::derive("a")));
        assert_eq!(listed[1].port, 3002);
        assert_eq!(listed[1].bind, BindAddress::Loopback);
        assert_eq!((listed[2].port, listed[2].agent), (3003, None));
    }

    #[test]
    fn a_port_shows_once_per_change() {
        let (world, ports) = setup();
        ports.report_at(world.container, vec![port(3000, Some("a"))], at(0));
        let seq = world.hub.seq();
        ports.report_at(world.container, vec![port(3000, Some("a"))], at(2));
        assert_eq!(world.hub.seq(), seq);
    }

    fn local(workspace: WorkspaceId, name: &str, pid: u32) -> LocalHolder {
        LocalHolder {
            workspace,
            holder: holder(name, pid),
        }
    }

    /// Gives the ports of the holders it's asked about, from a fixed list,
    /// and counts its polls.
    struct Fake {
        ports: Vec<GuestPort>,
        polls: Arc<AtomicUsize>,
    }

    impl Scanner for Fake {
        fn scan(&mut self, holders: &[WatchedHolder]) -> Vec<GuestPort> {
            let sessions: HashSet<SessionId> =
                holders.iter().map(|holder| holder.session).collect();
            self.polls.fetch_add(1, Ordering::SeqCst);
            self.ports
                .iter()
                .filter(|port| {
                    port.session
                        .is_some_and(|session| sessions.contains(&session))
                })
                .cloned()
                .collect()
        }
    }

    async fn settle(what: &str, check: impl Fn() -> bool) {
        for _ in 0..500 {
            if check() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for {what}");
    }

    #[tokio::test]
    async fn local_ports_show_after_two_polls_and_go_with_the_holder() {
        let world = world();
        let ports = HostPorts::new(world.hub.clone(), world.tunnels.clone());
        let polls = Arc::new(AtomicUsize::new(0));
        let fake = Fake {
            ports: vec![port(5173, Some("a")), port(8080, Some("z"))],
            polls: polls.clone(),
        };
        let holders = vec![local(world.folder, "a", 50)];
        let (tx, rx) = watch::channel((Pace::Fast, holders.clone()));
        // Polls only when told, so each send is one poll.
        let scanner: SharedScanner = Arc::new(Mutex::new(Box::new(fake)));
        let jobs = Arc::new(watch::Sender::new(Jobs::new()));
        let task = tokio::spawn(run_with(ports.clone(), rx, scanner, jobs, |_| {
            Duration::from_secs(3600)
        }));

        settle("the first poll", || polls.load(Ordering::SeqCst) == 1).await;
        assert!(shown(&world, &ports, world.folder).is_empty());
        tx.send((Pace::Fast, holders)).unwrap();
        settle("the port to show", || {
            shown(&world, &ports, world.folder).len() == 1
        })
        .await;
        let listed = shown(&world, &ports, world.folder);
        assert_eq!(listed[0].port, 5173);
        assert_eq!(listed[0].agent, Some(AgentId::derive("a")));

        tx.send((Pace::Fast, Vec::new())).unwrap();
        settle("the port to go", || {
            shown(&world, &ports, world.folder)[0].gone_since.is_some()
        })
        .await;
        assert_eq!(polls.load(Ordering::SeqCst), 2, "no holders, no poll");

        drop(tx);
        task.await.unwrap();
    }

    /// A live tree this test starts: a shell stands in for the holder, its
    /// first child for the CLI, and the CLI starts one short job. The guard
    /// ends the test's own group however the test ends.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn a_holders_jobs_follow_its_processes_and_change_only_when_they_do() {
        use std::os::unix::process::CommandExt;
        use std::process::{Child, Command, Stdio};

        use maxhq_protocol::processes::AgentRunning;

        /// Ends the group this test made. The holder isn't reaped yet, so
        /// its pid, the group's id, can't name another group.
        struct Group(Child);

        impl Drop for Group {
            #[allow(unsafe_code)]
            fn drop(&mut self) {
                if let Ok(pgid) = libc::pid_t::try_from(self.0.id()) {
                    // SAFETY: kill only reports an error, and the group is
                    // this test's own.
                    unsafe { libc::kill(-pgid, libc::SIGKILL) };
                }
                let _ = self.0.wait();
            }
        }

        let child = Command::new("/bin/sh")
            .arg("-c")
            .arg("sh -c 'sh -c \"sleep 3; true\" & exec sleep 60' & exec sleep 61")
            .stdin(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let session = SessionId::derive("live");
        let holder = WatchedHolder {
            session,
            agent: Some(AgentId::derive("live")),
            pid: child.id(),
        };
        let _group = Group(child);

        let world = world();
        let ports = HostPorts::new(world.hub.clone(), world.tunnels.clone());
        let held = vec![LocalHolder {
            workspace: world.folder,
            holder,
        }];
        let (tx, rx) = watch::channel((Pace::Fast, held));
        let jobs = Arc::new(watch::Sender::new(Jobs::new()));
        let mut seen = jobs.subscribe();
        let task = tokio::spawn(run_with(
            ports,
            rx,
            crate::ports::local_scanner(),
            jobs.clone(),
            |_| Duration::from_millis(50),
        ));

        let wait_for = |what: &'static str, check: fn(Option<&AgentRunning>) -> bool| {
            let mut seen = jobs.subscribe();
            async move {
                tokio::time::timeout(Duration::from_secs(10), async {
                    loop {
                        if check(seen.borrow_and_update().get(&session)) {
                            return;
                        }
                        seen.changed().await.unwrap();
                    }
                })
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
            }
        };
        wait_for("the job to show", |job| {
            job.is_some_and(|job| job.count == 1 && job.names == ["sleep 3"])
        })
        .await;
        // Many polls go by while the job runs, and none of them sends.
        seen.borrow_and_update();
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(!seen.has_changed().unwrap(), "{:?}", *seen.borrow());
        assert!(jobs.borrow().contains_key(&session), "the job ended early");

        wait_for("the job to end", |job| job.is_none()).await;
        drop(tx);
        task.await.unwrap();
    }
}
