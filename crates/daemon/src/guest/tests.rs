// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The guest's end of the link, driven by the host's real end over a
//! socket, with fake holders and history readers behind it. Transfers and
//! the port watch run for real, on folders and a fake `/proc` of this
//! device.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{self, Permissions};
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::stream::{BoxStream, StreamExt};
use maxhq_core::paths::ROLE_ENV;
use maxhq_hold::RunAs;
use maxhq_protocol::frame::{encode_transfer_chunks, encode_tunnel_data};
use maxhq_protocol::guest::{
    Framing, GuestLaunch, GuestToHost, HostToGuest, Pace, TunnelTarget, GUEST_PROTOCOL_VERSION,
};
use maxhq_protocol::ids::{
    AgentId, BootId, DeviceId, SessionId, StreamId, TransferId, WorkspaceId,
};
use maxhq_protocol::message::PlaceFailure;
use maxhq_protocol::model::Vendor;
use maxhq_protocol::skills::PackState;
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{ItemBody, ItemId, ItemStatus, TimelineItem};
use maxhq_protocol::Secret;
use maxhq_transfer::{walk, Room, DEVICE_QUOTA};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UnixListener};
use tokio::task::JoinHandle;

use super::accounts::Folders;
use super::agent::{self, session_of, AgentWant};
use super::files::{Config, Files};
use super::helper;
use super::lease;
use super::link::{self, Guest, HolderPids, Opener};
use super::ports::Ports;
use crate::guestlink::tests::{eventually, instance, within, Host, Peer, QUIET, TOKEN};
use crate::guestlink::wire::{Abort, Read};
use crate::ports::guest::PortWatch;
use crate::ports::{interval, Procfs, FAST};
use crate::session::clock::BoxFuture;
use crate::session::fake::{FakeHolders, Say};
use crate::session::holders::{
    ConnectError, Ended, Event, HolderPlaces, Holders, Launch, WriteError, WriteOutcome,
};
use crate::session::vendors::History;
use crate::testing::TempDir;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const REPO: &str = "https://github.com/Example-User/MaxHQ.git";

/// The guest's word on which processes are holders.
#[derive(Default)]
struct Pids(Mutex<HashMap<u32, SessionId>>);

impl Pids {
    fn set(&self, pid: u32, session: SessionId) {
        self.0.lock().unwrap().insert(pid, session);
    }
}

impl HolderPids for Pids {
    fn holder_pids(&self) -> BoxFuture<HashMap<u32, SessionId>> {
        let pids = self.0.lock().unwrap().clone();
        Box::pin(async move { pids })
    }
}

/// History readers that never find a file.
fn no_history() -> Opener {
    Arc::new(|_, _, _, _| None)
}

/// A fast pace short enough for a test, and a slow one that never comes.
fn quick(pace: Pace) -> Duration {
    match pace {
        Pace::Fast => Duration::from_millis(30),
        Pace::Slow => Duration::from_secs(3600),
    }
}

/// A guest over `fake` that runs CLIs as the test's own user.
fn guest(fake: &FakeHolders, pids: Arc<Pids>, opener: Opener, work: PathBuf) -> Arc<Guest> {
    guest_ticking(fake, pids, opener, work, quick)
}

/// A guest whose port watch waits `tick` of a pace between polls. Its
/// attachments, the agent home and the `/proc` it reads are beside
/// `work`.
fn guest_ticking(
    fake: &FakeHolders,
    pids: Arc<Pids>,
    opener: Opener,
    work: PathBuf,
    tick: fn(Pace) -> Duration,
) -> Arc<Guest> {
    let beside = work.parent().expect("the work folder has a parent");
    let files = Files::new(Config {
        attachments: beside.join("attachments"),
        work: work.clone(),
        room: Room {
            keep_bytes: 0,
            keep_percent: 0,
        },
        quota: DEVICE_QUOTA,
    });
    let proc_root = beside.join("guest-proc");
    let ports = Ports::with(|sink| PortWatch::with(sink, Procfs::new(proc_root, true), tick));
    let mut guest = Guest::new(
        Arc::new(fake.clone()),
        pids,
        opener,
        Arc::new(files),
        Arc::new(ports),
        "mhq-test".to_owned(),
        BootId("boot-1".to_owned()),
    );
    guest.mount = beside.join("mount");
    guest.data = beside.join("data");
    guest.folders = Folders {
        home: beside.join("home"),
        chown: Arc::new(|_, _, _, _| Ok(())),
    };
    guest.work = work;
    guest.users = |_| Ok(None);
    guest.history_poll = Duration::from_millis(20);
    // Each test here hears the guest's frames in order, so no unsaved
    // reports come between them.
    guest.unsaved_every = None;
    Arc::new(guest)
}

/// The guest's host socket, which a test can cut.
struct GuestEnd {
    accept: JoinHandle<()>,
    serving: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl GuestEnd {
    /// Serves host links at `path`, each new one in place of the last.
    fn listen(guest: Arc<Guest>, path: &Path) -> Self {
        let listener = UnixListener::bind(path).unwrap();
        let serving = Arc::new(Mutex::new(None::<JoinHandle<()>>));
        let accept = tokio::spawn({
            let serving = serving.clone();
            async move {
                while let Ok((stream, _)) = listener.accept().await {
                    let task = tokio::spawn(link::serve(guest.clone(), stream));
                    if let Some(old) = serving.lock().unwrap().replace(task) {
                        old.abort();
                    }
                }
            }
        });
        Self { accept, serving }
    }

    /// Cuts the link being served. The host links again.
    fn drop_link(&self) {
        if let Some(task) = self.serving.lock().unwrap().take() {
            task.abort();
        }
    }
}

impl Drop for GuestEnd {
    fn drop(&mut self) {
        self.accept.abort();
        self.drop_link();
    }
}

/// A host linked to one guest.
struct Rig {
    host: Host,
    fake: FakeHolders,
    pids: Arc<Pids>,
    guest: Arc<Guest>,
    workspace: WorkspaceId,
    /// The boot ID and holders of the first hello.
    up: (BootId, Vec<SessionId>),
    end: GuestEnd,
}

impl Rig {
    async fn new(opener: Opener, repo: Option<&str>) -> Self {
        let mut host = Host::new(QUIET);
        let fake = FakeHolders::new();
        let pids = Arc::new(Pids::default());
        let guest = guest(&fake, pids.clone(), opener, host.temp.path().join("work"));
        let name = instance("mhq-test");
        let end = GuestEnd::listen(guest.clone(), &host.socket(&name));
        let workspace = WorkspaceId::generate();
        host.guests.add(workspace, name, repo.map(str::to_owned));
        let up = host.up(workspace).await;
        Self {
            host,
            fake,
            pids,
            guest,
            workspace,
            up,
            end,
        }
    }

    fn holders(&self) -> Arc<dyn Holders> {
        self.host
            .guests
            .guest(self.workspace)
            .expect("the link is up")
    }
}

/// A Codex launch for `session`.
fn launch(session: SessionId) -> Launch {
    Launch {
        session,
        program: PathBuf::from("/usr/local/bin/codex"),
        args: vec![OsString::from("app-server")],
        env: vec![(OsString::from("TERM"), OsString::from("dumb"))],
        env_remove: vec![OsString::from("GONE")],
        secret_env: vec![(OsString::from("CODEX_TOKEN"), Secret::new(TOKEN.to_owned()))],
        run_as: None,
        cwd: PathBuf::from("/home/agent/work/maxhq"),
        framing: maxhq_hold::Framing::JsonRpc,
    }
}

async fn next(events: &mut BoxStream<'static, Event>) -> Event {
    within(events.next()).await.expect("the events ended")
}

/// The text of the next line, and whether it was replayed.
async fn line(events: &mut BoxStream<'static, Event>) -> (String, bool) {
    match next(events).await {
        Event::Line(line) => (line.text, line.replayed),
        other => panic!("expected a line, got {other:?}"),
    }
}

#[tokio::test]
async fn a_holder_runs_through_the_link() {
    let rig = Rig::new(no_history(), Some(REPO)).await;
    assert_eq!(rig.up, (BootId("boot-1".to_owned()), Vec::new()));
    let holders = rig.holders();

    let session = SessionId::generate();
    within(holders.spawn(launch(session))).await.unwrap();
    let held = rig.fake.launch(session);
    assert_eq!(held.program, PathBuf::from("/usr/local/bin/codex"));
    assert_eq!(held.args, vec![OsString::from("app-server")]);
    assert_eq!(
        held.env,
        vec![(OsString::from("TERM"), OsString::from("dumb"))]
    );
    for gone in [ROLE_ENV, "INVOCATION_ID", "GONE"] {
        assert!(held.env_remove.contains(&OsString::from(gone)), "{gone}");
    }
    assert_eq!(held.secret_env.len(), 1);
    assert_eq!(held.secret_env[0].0, OsString::from("CODEX_TOKEN"));
    assert_eq!(held.secret_env[0].1.expose(), TOKEN);
    assert!(held.run_as.is_none());
    assert_eq!(held.cwd, PathBuf::from("/home/agent/work/maxhq"));
    assert!(matches!(held.framing, maxhq_hold::Framing::JsonRpc));
    assert!(within(holders.spawn(launch(session))).await.is_err());

    let mut as_root = launch(SessionId::generate());
    as_root.run_as = Some(RunAs {
        user: "root".to_owned(),
        uid: 0,
        gid: 0,
        home: PathBuf::from("/root"),
    });
    let err = within(holders.spawn(as_root)).await.unwrap_err();
    assert!(err.to_string().contains("CLIs run as agent"), "{err}");

    let stranger = SessionId::generate();
    assert!(matches!(
        within(holders.connect(stranger, None)).await,
        Err(ConnectError::NotFound { session }) if session == stranger
    ));

    let mut connection = within(holders.connect(session, None)).await.unwrap();
    rig.fake.print(session, "one");
    assert_eq!(
        line(&mut connection.events).await,
        ("one".to_owned(), false)
    );
    let written = within(connection.writer.write("hi".to_owned(), None)).await;
    assert_eq!(written.unwrap(), WriteOutcome::Written);
    assert_eq!(rig.fake.writes(session), vec![("hi".to_owned(), None)]);
    assert_eq!(within(holders.list()).await.unwrap(), vec![session]);

    within(connection.writer.stop(Duration::from_secs(3)))
        .await
        .unwrap();
    assert_eq!(rig.fake.stops(session), vec![Duration::from_secs(3)]);
    match next(&mut connection.events).await {
        Event::Exited(exit) => assert_eq!(exit.signal, Some(15)),
        other => panic!("expected the exit, got {other:?}"),
    }
    assert!(within(connection.events.next()).await.is_none());
    within(connection.writer.forget()).await.unwrap();
    assert!(rig.fake.forgotten(session));

    // Ending runs the guest's own end, which signals only what it can
    // confirm, and says what may still run.
    let other = SessionId::generate();
    within(holders.spawn(launch(other))).await.unwrap();
    assert_eq!(within(holders.end(other, Some(41))).await, Ended::Gone);
    assert_eq!(rig.fake.ends(other), vec![Some(41)]);
    let stuck = SessionId::generate();
    within(holders.spawn(launch(stuck))).await.unwrap();
    rig.fake.unconfirmable(stuck);
    match within(holders.end(stuck, None)).await {
        Ended::Unconfirmed { pid, reason } => {
            assert!(pid.is_some_and(|pid| pid > 10_000), "{pid:?}");
            assert!(reason.contains("doesn't look like its holder"), "{reason}");
        }
        Ended::Gone => panic!("expected the end to stay unconfirmed"),
    }

    // The test's work folder holds no repo.
    let worktree = rig.host.guests.worktree(
        rig.workspace,
        AgentId::generate(),
        "feature".to_owned(),
        None,
    );
    assert!(within(worktree).await.is_err());
}

#[tokio::test]
async fn replay_after_a_reconnect_has_no_gap() {
    let mut rig = Rig::new(no_history(), None).await;
    let holders = rig.holders();
    let session = SessionId::generate();
    within(holders.spawn(launch(session))).await.unwrap();
    let connection = within(holders.connect(session, None)).await.unwrap();
    let (mut events, old_writer) = (connection.events, connection.writer);
    for text in ["1", "2", "3"] {
        rig.fake.print(session, text);
    }
    for text in ["1", "2", "3"] {
        assert_eq!(line(&mut events).await, (text.to_owned(), false));
    }

    rig.end.drop_link();
    assert!(within(events.next()).await.is_none());
    rig.host.down(rig.workspace).await;
    rig.fake.print(session, "4");
    rig.fake.print(session, "5");
    let (_, holders_up) = rig.host.up(rig.workspace).await;
    assert_eq!(holders_up, vec![session]);

    let holders = rig.holders();
    let connection = within(holders.connect(session, Some(3))).await.unwrap();
    let mut events = connection.events;
    assert_eq!(line(&mut events).await, ("4".to_owned(), true));
    assert_eq!(line(&mut events).await, ("5".to_owned(), true));
    rig.fake.print(session, "6");
    assert_eq!(line(&mut events).await, ("6".to_owned(), false));
    assert_eq!(rig.fake.connects(session).last(), Some(&Some(3)));

    let late = within(old_writer.write("late".to_owned(), None)).await;
    assert!(matches!(late, Err(WriteError::Disconnected)), "{late:?}");
}

#[tokio::test]
async fn a_pending_answer_goes_through_the_relay() {
    let rig = Rig::new(no_history(), None).await;
    let holders = rig.holders();
    let session = SessionId::generate();
    within(holders.spawn(launch(session))).await.unwrap();
    // Five prompts of a mebibyte each don't fit one hello, and the sixth
    // takes more than any other message may.
    let big = Say {
        writes: vec!["x".repeat(1 << 20)],
        ..Default::default()
    };
    for n in 1..=5 {
        rig.fake.ask(session, &format!("req-{n}"), &big);
    }
    let huge = Say {
        writes: vec!["x".repeat(6 << 20)],
        ..Default::default()
    };
    rig.fake.ask(session, "req-6", &huge);

    let connection = within(holders.connect(session, None)).await.unwrap();
    let seqs: Vec<u64> = connection
        .hello
        .pending
        .iter()
        .map(|line| line.seq)
        .collect();
    assert_eq!(seqs, vec![1, 2, 3, 4, 5, 6]);
    let answer = |request: &str| {
        connection
            .writer
            .write("yes".to_owned(), Some(request.to_owned()))
    };
    assert_eq!(
        within(answer("req-1")).await.unwrap(),
        WriteOutcome::Written
    );
    assert_eq!(
        within(answer("req-1")).await.unwrap(),
        WriteOutcome::AlreadyAnswered
    );
    rig.fake.answer_elsewhere(session, "req-2");
    assert_eq!(
        within(answer("req-2")).await.unwrap(),
        WriteOutcome::AlreadyAnswered
    );
}

/// A history reader with items for its first read.
struct Items {
    items: Vec<TimelineItem>,
    dropped: Arc<AtomicBool>,
    /// Each limit the guest set on it.
    limits: Limits,
}

type Limits = Arc<Mutex<Vec<Option<u64>>>>;

impl History for Items {
    fn read(&mut self) -> io::Result<Vec<TimelineItem>> {
        Ok(std::mem::take(&mut self.items))
    }

    fn limit(&mut self, bytes: Option<u64>) {
        self.limits.lock().unwrap().push(bytes);
    }
}

impl Drop for Items {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

fn item(agent: AgentId, n: u64) -> TimelineItem {
    TimelineItem {
        id: ItemId(format!("item-{n}")),
        agent,
        turn: None,
        parent: None,
        ordinal: n,
        status: ItemStatus::Completed,
        started_at: None,
        completed_at: None,
        body: ItemBody::AssistantMessage {
            text: "y".repeat(1 << 20),
            streaming: false,
        },
    }
}

type Opened = Arc<Mutex<Vec<(Vendor, PathBuf, String, AgentId)>>>;

#[tokio::test]
async fn history_items_come_in_parts() {
    let opened: Opened = Arc::default();
    let dropped = Arc::new(AtomicBool::new(false));
    let limits: Limits = Arc::default();
    let opener: Opener = Arc::new({
        let (opened, dropped, limits) = (opened.clone(), dropped.clone(), limits.clone());
        move |vendor: Vendor, dir: &Path, session: &str, agent: AgentId| {
            let mut opened = opened.lock().unwrap();
            opened.push((vendor, dir.to_owned(), session.to_owned(), agent));
            // The CLI hasn't written its files the first time.
            (opened.len() == 2).then(|| {
                let items = (0..6).map(|n| item(agent, n)).collect();
                let (dropped, limits) = (dropped.clone(), limits.clone());
                Box::new(Items {
                    items,
                    dropped,
                    limits,
                }) as Box<dyn History>
            })
        }
    });
    let rig = Rig::new(opener, None).await;
    let agent = AgentId::generate();
    let mut history = rig
        .host
        .guests
        .history(
            rig.workspace,
            Vendor::Codex,
            "/home/agent/.codex".to_owned(),
            "thread-1".to_owned(),
            agent,
            Some(64 << 20),
        )
        .expect("a history");
    // Reads block, so they run on a thread as the session manager's do.
    let read = tokio::task::spawn_blocking(move || {
        let mut items = Vec::new();
        while items.len() < 6 {
            items.extend(history.read().unwrap());
            std::thread::sleep(Duration::from_millis(10));
        }
        (history, items)
    });
    let (history, items) = within(read).await.unwrap();
    let ids: Vec<String> = items.iter().map(|item| item.id.0.clone()).collect();
    let want: Vec<String> = (0..6).map(|n| format!("item-{n}")).collect();
    assert_eq!(ids, want);
    assert!(items.iter().all(|item| item.agent == agent));
    assert_eq!(
        opened.lock().unwrap()[0],
        (
            Vendor::Codex,
            PathBuf::from("/home/agent/.codex"),
            "thread-1".to_owned(),
            agent
        )
    );
    // The reader takes as much as the host's history limit says, once.
    assert_eq!(*limits.lock().unwrap(), [Some(64 << 20)]);

    drop(history);
    eventually(|| dropped.load(Ordering::SeqCst)).await;
}

#[tokio::test]
async fn a_historys_first_read_holds_all_the_guest_read_first() {
    let limits: Limits = Arc::default();
    let opener: Opener = Arc::new({
        let limits = limits.clone();
        move |_, _, _, agent| {
            let items = (0..6).map(|n| item(agent, n)).collect();
            let (dropped, limits) = (Arc::default(), limits.clone());
            Some(Box::new(Items {
                items,
                dropped,
                limits,
            }) as Box<dyn History>)
        }
    });
    let rig = Rig::new(opener, None).await;
    let open = |rig: &Rig| {
        rig.host
            .guests
            .history(
                rig.workspace,
                Vendor::Claude,
                "/home/agent/.claude-1".to_owned(),
                "session-1".to_owned(),
                AgentId::generate(),
                None,
            )
            .expect("a history")
    };
    // The items come in several parts, and one read takes them all.
    let mut history = open(&rig);
    let read = tokio::task::spawn_blocking(move || history.read());
    let items = within(read).await.unwrap().unwrap();
    assert_eq!(items.len(), 6);
    // A host with no history limit has the reader take it all.
    assert_eq!(*limits.lock().unwrap(), [None]);

    // A history the CLI hasn't written yet reads as empty, without a wait.
    let rig = Rig::new(no_history(), None).await;
    let mut history = open(&rig);
    let read = tokio::task::spawn_blocking(move || history.read());
    assert!(within(read).await.unwrap().unwrap().is_empty());
}

#[tokio::test]
async fn helpers_get_tokens_for_their_own_session_only() {
    let rig = Rig::new(no_history(), Some(REPO)).await;
    let socket = rig.host.temp.path().join("a.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let proc_root = rig.host.temp.path().join("proc");
    let _agent = Abort(tokio::spawn(agent::accept(
        rig.guest.clone(),
        listener,
        proc_root,
    )));
    let ask = |want: AgentWant| {
        let socket = socket.clone();
        async move {
            let asked = tokio::task::spawn_blocking(move || helper::ask_at(&socket, &want));
            within(asked).await.unwrap()
        }
    };
    let github = |repo: Option<&str>| AgentWant::Github {
        repo: repo.map(str::to_owned),
    };
    let refusal = |asked: Result<(Secret, Timestamp), String>| match asked {
        Ok(_) => panic!("expected a refusal"),
        Err(reason) => reason,
    };

    // The test itself runs no holder.
    let reason = refusal(ask(AgentWant::Codex).await);
    assert!(reason.contains("only a process an agent runs"), "{reason}");

    let session = SessionId::generate();
    let account = rig.host.owners.set(session, rig.workspace, Vendor::Codex);
    rig.pids.set(std::process::id(), session);
    let (value, _) = ask(AgentWant::Codex).await.unwrap();
    assert_eq!(value.expose(), TOKEN);
    let (value, _) = ask(github(None)).await.unwrap();
    assert_eq!(value.expose(), TOKEN);
    let (value, _) = ask(github(Some("https://github.com/example-user/MAXHQ")))
        .await
        .unwrap();
    assert_eq!(value.expose(), TOKEN);
    let reason = refusal(ask(github(Some("example-user/other"))).await);
    assert!(
        reason.contains("only the workspace's own repository"),
        "{reason}"
    );

    // A holder whose session the host doesn't know.
    rig.pids.set(std::process::id(), SessionId::generate());
    let reason = refusal(ask(AgentWant::Codex).await);
    assert!(reason.contains("that session isn't running"), "{reason}");

    assert_eq!(
        rig.host.brokers.asked(),
        vec![
            format!("codex {account}"),
            format!("github {REPO}"),
            format!("github {REPO}"),
        ]
    );
    assert!(rig.host.guests.guest(rig.workspace).is_some());
}

#[test]
fn the_parent_walk_finds_the_holder() {
    let temp = TempDir::new();
    let proc_root = temp.path();
    // Above Linux's largest process ID, so no real process matches.
    let status = |pid: u32, ppid: u32| {
        let dir = proc_root.join(pid.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let text = format!("Name:\tsh\nState:\tS (sleeping)\nPid:\t{pid}\nPPid:\t{ppid}\n");
        std::fs::write(dir.join("status"), text).unwrap();
    };
    let session = SessionId::generate();
    let holders = HashMap::from([(5_000_001, session)]);
    status(5_000_002, 5_000_001);
    status(5_000_003, 5_000_002);
    assert_eq!(session_of(proc_root, 5_000_003, &holders), Some(session));
    assert_eq!(session_of(proc_root, 5_000_001, &holders), Some(session));

    status(5_000_012, 1);
    status(5_000_013, 5_000_012);
    assert_eq!(session_of(proc_root, 5_000_013, &holders), None);

    status(5_000_021, 5_000_022);
    status(5_000_022, 5_000_021);
    assert_eq!(session_of(proc_root, 5_000_021, &holders), None);

    assert_eq!(session_of(proc_root, 5_000_099, &holders), None);
}

/// Says hello to the guest as a host on `version`, and gives its answer.
async fn hello(peer: &mut Peer, version: &str) -> GuestToHost {
    peer.send(&HostToGuest::Hello {
        protocol: GUEST_PROTOCOL_VERSION,
        maxd_version: version.to_owned(),
        device: DeviceId::generate(),
        workspace: WorkspaceId::generate(),
        repo: None,
    })
    .await;
    peer.recv().await
}

async fn ping(peer: &mut Peer, nonce: u64) {
    peer.send(&HostToGuest::Ping { nonce }).await;
    match peer.recv().await {
        GuestToHost::Pong { nonce: back } => assert_eq!(back, nonce),
        other => panic!("expected a pong, got {other:?}"),
    }
}

#[tokio::test]
async fn a_new_host_link_replaces_the_old() {
    let temp = TempDir::new();
    let fake = FakeHolders::new();
    let session = SessionId::generate();
    fake.spawn(launch(session)).await.unwrap();
    let guest = guest(
        &fake,
        Arc::default(),
        no_history(),
        temp.path().join("work"),
    );
    let path = temp.path().join("h.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let _accept = Abort(tokio::spawn(link::accept(guest, listener)));

    let mut first = Peer::connect(&path).await;
    match hello(&mut first, VERSION).await {
        GuestToHost::Hello {
            protocol,
            maxd_version,
            instance,
            boot_id,
            holders,
            build,
        } => {
            assert_eq!(build.as_deref(), crate::identity::build());
            assert_eq!(protocol, GUEST_PROTOCOL_VERSION);
            assert_eq!(maxd_version, VERSION);
            assert_eq!(instance, "mhq-test");
            assert_eq!(boot_id, BootId("boot-1".to_owned()));
            assert_eq!(holders, vec![session]);
        }
        other => panic!("expected a hello, got {other:?}"),
    }
    ping(&mut first, 1).await;

    let mut second = Peer::connect(&path).await;
    let GuestToHost::Hello { .. } = hello(&mut second, VERSION).await else {
        panic!("expected a hello");
    };
    first.closed().await;
    ping(&mut second, 2).await;

    // A host on another build gets a hello and nothing more, since it
    // installs its own build next.
    let mut third = Peer::connect(&path).await;
    let GuestToHost::Hello { .. } = hello(&mut third, "0.0.1-old").await else {
        panic!("expected a hello");
    };
    second.closed().await;
    third.send(&HostToGuest::Ping { nonce: 3 }).await;
    let quiet = tokio::time::timeout(Duration::from_millis(300), third.next()).await;
    assert!(quiet.is_err(), "{quiet:?}");
}

/// A guest on a folder of this device that takes host links, with a
/// fake `/proc` for its port watch.
struct Container {
    temp: TempDir,
    fake: FakeHolders,
    socket: PathBuf,
    _accept: Abort<()>,
}

impl Container {
    fn new(tick: fn(Pace) -> Duration) -> Self {
        Self::hosting(tick, None)
    }

    /// A container that's a test host when `test_host` is set.
    fn hosting(tick: fn(Pace) -> Duration, test_host: Option<Arc<dyn lease::TestHost>>) -> Self {
        let temp = TempDir::new();
        let fake = FakeHolders::new();
        let work = temp.path().join("work");
        fs::create_dir_all(&work).unwrap();
        fs::create_dir_all(temp.path().join("guest-proc/net")).unwrap();
        let mut guest = guest_ticking(&fake, Arc::default(), no_history(), work, tick);
        Arc::get_mut(&mut guest).unwrap().test_host = test_host;
        let socket = temp.path().join("h.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let accept = Abort(tokio::spawn(link::accept(guest, listener)));
        Self {
            temp,
            fake,
            socket,
            _accept: accept,
        }
    }

    fn attachments(&self) -> PathBuf {
        self.temp.path().join("attachments")
    }

    fn work(&self) -> PathBuf {
        self.temp.path().join("work")
    }

    /// Lists `ports` as loopback listeners in the fake `/proc/net/tcp`,
    /// in place of what was there.
    fn listen(&self, ports: &[u16]) {
        let mut text = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n".to_owned();
        for (n, port) in ports.iter().enumerate() {
            let inode = 7000 + n;
            text.push_str(&format!(
                "{n:4}: 0100007F:{port:04X} 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 {inode} 1 0000000000000000 100 0 0 10 0\n"
            ));
        }
        // A scan never reads half a table.
        let net = self.temp.path().join("guest-proc/net");
        fs::write(net.join("tcp.new"), text).unwrap();
        fs::rename(net.join("tcp.new"), net.join("tcp")).unwrap();
    }

    /// Links a host, past the hellos.
    async fn link(&self) -> Peer {
        let mut peer = Peer::connect(&self.socket).await;
        let GuestToHost::Hello { .. } = hello(&mut peer, VERSION).await else {
            panic!("expected a hello");
        };
        peer
    }
}

/// The first message `pick` takes, skipping the frames before it.
async fn until<T>(peer: &mut Peer, mut pick: impl FnMut(GuestToHost) -> Option<T>) -> T {
    loop {
        match peer.next().await {
            Ok(Read::Message(body)) => {
                if let Some(found) = pick(serde_json::from_slice(&body).unwrap()) {
                    return found;
                }
            }
            Ok(_) => {}
            Err(err) => panic!("the link failed: {err}"),
        }
    }
}

/// The ports in the next `Ports`.
async fn listed(peer: &mut Peer) -> Vec<u16> {
    until(peer, |message| match message {
        GuestToHost::Ports { listening } => Some(listening.iter().map(|port| port.port).collect()),
        _ => None,
    })
    .await
}

/// Asks for ports at the fast pace, and waits until `ports` show.
async fn watch(peer: &mut Peer, ports: &[u16]) {
    peer.send(&HostToGuest::WatchPorts {
        pace: Pace::Fast,
        holders: Vec::new(),
    })
    .await;
    while listed(peer).await != ports {}
}

#[tokio::test]
async fn an_upload_lands_with_its_modes() {
    let container = Container::new(quick);
    let mut host = container.link().await;
    // What the user picked on their device.
    let picked = container.temp.path().join("picked");
    fs::create_dir(&picked).unwrap();
    fs::write(picked.join("run.sh"), "#!/bin/sh\necho hi\n").unwrap();
    fs::set_permissions(picked.join("run.sh"), Permissions::from_mode(0o700)).unwrap();
    fs::write(picked.join("notes.txt"), "plain\n").unwrap();
    fs::set_permissions(picked.join("notes.txt"), Permissions::from_mode(0o600)).unwrap();
    let walked = walk(&picked).unwrap();
    let bytes: Vec<u8> = walked
        .sources
        .iter()
        .flatten()
        .flat_map(|source| fs::read(source).unwrap())
        .collect();

    let (transfer, agent) = (TransferId::generate(), AgentId::generate());
    host.send(&HostToGuest::UploadBegin {
        transfer,
        agent: Some(agent),
        entries: walked.entries,
    })
    .await;
    let window_end = until(&mut host, |message| match message {
        GuestToHost::TransferCredit {
            transfer: at,
            window_end,
            ..
        } if at == transfer => Some(window_end),
        GuestToHost::TransferFailed { message, .. } => panic!("the upload failed: {message}"),
        _ => None,
    })
    .await;
    assert!(window_end >= bytes.len() as u64, "{window_end}");
    let mut frames = Vec::new();
    encode_transfer_chunks(&mut frames, transfer, 0, &bytes);
    host.bytes(&frames).await;
    let paths = until(&mut host, |message| match message {
        GuestToHost::UploadStored {
            transfer: at,
            paths,
            ..
        } if at == transfer => Some(paths),
        GuestToHost::TransferFailed { message, .. } => panic!("the upload failed: {message}"),
        _ => None,
    })
    .await;

    let landed = container
        .attachments()
        .join(agent.to_string())
        .join(transfer.to_string())
        .join("picked");
    assert_eq!(paths, vec![landed.to_str().unwrap().to_owned()]);
    let mode = |name: &str| {
        fs::metadata(landed.join(name))
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    };
    assert_eq!(mode("run.sh"), 0o755);
    assert_eq!(mode("notes.txt"), 0o644);
    assert_eq!(
        fs::read_to_string(landed.join("run.sh")).unwrap(),
        "#!/bin/sh\necho hi\n"
    );
}

#[tokio::test]
async fn a_fetch_serves_what_the_agent_wrote() {
    let container = Container::new(quick);
    let mut host = container.link().await;
    let wrote = container.work().join("maxhq").join("out.txt");
    fs::create_dir_all(wrote.parent().unwrap()).unwrap();
    let text = b"the agent's output\n";
    fs::write(&wrote, text).unwrap();

    let transfer = TransferId::generate();
    host.send(&HostToGuest::FetchBegin {
        transfer,
        path: wrote.to_str().unwrap().to_owned(),
    })
    .await;
    let entries = until(&mut host, |message| match message {
        GuestToHost::FetchManifest {
            transfer: at,
            entries,
        } if at == transfer => Some(entries),
        GuestToHost::TransferFailed { message, .. } => panic!("the fetch failed: {message}"),
        _ => None,
    })
    .await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, "out.txt");
    assert_eq!(entries[0].size, text.len() as u64);

    // The first credit says where the download starts.
    host.send(&HostToGuest::TransferCredit {
        transfer,
        committed: 0,
        window_end: text.len() as u64,
    })
    .await;
    let mut got = Vec::new();
    while got.len() < text.len() {
        match host.next().await {
            Ok(Read::Chunk {
                transfer: at,
                offset,
                data,
            }) if at == transfer => {
                assert_eq!(offset, got.len() as u64);
                got.extend(data);
            }
            Ok(Read::Message(body)) => {
                let message: GuestToHost = serde_json::from_slice(&body).unwrap();
                assert!(
                    !matches!(message, GuestToHost::TransferFailed { .. }),
                    "{message:?}"
                );
            }
            other => panic!("expected the file's bytes, got {other:?}"),
        }
    }
    assert_eq!(got, text);
}

#[tokio::test]
async fn a_picture_the_host_hands_over_lands_where_the_host_named_it() {
    use base64::Engine as _;
    let container = Container::new(quick);
    let mut host = container.link().await;
    let agent = AgentId::generate();
    let bytes = b"a screenshot's bytes".to_vec();
    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
    host.send(&HostToGuest::KeepPicture {
        agent,
        media_type: "image/png".into(),
        data: data.clone(),
    })
    .await;
    let sha = maxhq_transfer::receive::sha256_hex(&bytes);
    let landed = container
        .attachments()
        .join(agent.to_string())
        .join("tool-output")
        .join(format!("{sha}.png"));
    eventually(|| landed.exists()).await;
    assert_eq!(fs::read(&landed).unwrap(), bytes);

    // What MaxHQ doesn't keep is left out, and the link stays up.
    host.send(&HostToGuest::KeepPicture {
        agent,
        media_type: "image/svg+xml".into(),
        data,
    })
    .await;
    host.send(&HostToGuest::Ping { nonce: 7 }).await;
    until(&mut host, |message| match message {
        GuestToHost::Pong { nonce: 7 } => Some(()),
        _ => None,
    })
    .await;
    let folder = landed.parent().unwrap();
    assert_eq!(fs::read_dir(folder).unwrap().count(), 1);
}

/// The real paces, twenty times quicker.
fn brisk(pace: Pace) -> Duration {
    interval(pace) / 20
}

#[tokio::test]
async fn a_found_port_shows_then_goes_when_it_closes() {
    let container = Container::new(brisk);
    container.listen(&[5173]);
    let mut host = container.link().await;
    host.send(&HostToGuest::WatchPorts {
        pace: Pace::Fast,
        holders: Vec::new(),
    })
    .await;
    // One poll isn't enough to show a port, but the host hears at once.
    assert!(listed(&mut host).await.is_empty());
    assert_eq!(listed(&mut host).await, [5173]);

    // The host keeps a gone port for a while. The guest just stops
    // listing it, at the next poll.
    container.listen(&[]);
    let closed = Instant::now();
    assert!(listed(&mut host).await.is_empty());
    assert!(closed.elapsed() < FAST, "{:?}", closed.elapsed());
}

#[tokio::test]
async fn a_tunnel_reaches_a_found_port_only() {
    let container = Container::new(quick);
    let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hidden = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = server.local_addr().unwrap().port();
    container.listen(&[port]);
    let mut host = container.link().await;
    watch(&mut host, &[port]).await;

    let stream = StreamId::generate();
    host.send(&HostToGuest::TunnelOpen {
        stream,
        target: TunnelTarget::Port(port),
    })
    .await;
    let (mut socket, _) = within(server.accept()).await.unwrap();
    until(&mut host, |message| match message {
        GuestToHost::TunnelOpened { stream: at } if at == stream => Some(()),
        GuestToHost::TunnelRefused { reason, .. } => panic!("the tunnel was refused: {reason}"),
        _ => None,
    })
    .await;
    let mut frame = Vec::new();
    encode_tunnel_data(&mut frame, stream, b"ping");
    host.bytes(&frame).await;
    let mut heard = [0; 4];
    within(socket.read_exact(&mut heard)).await.unwrap();
    assert_eq!(&heard, b"ping");
    socket.write_all(b"pong").await.unwrap();
    let mut back = Vec::new();
    while back.len() < 4 {
        match host.next().await {
            Ok(Read::Tunnel { stream: at, data }) if at == stream => back.extend(data),
            Ok(Read::Message(_)) => {}
            other => panic!("expected the stream's bytes, got {other:?}"),
        }
    }
    assert_eq!(back, b"pong");

    let refused = StreamId::generate();
    let unlisted = hidden.local_addr().unwrap().port();
    host.send(&HostToGuest::TunnelOpen {
        stream: refused,
        target: TunnelTarget::Port(unlisted),
    })
    .await;
    let reason = until(&mut host, |message| match message {
        GuestToHost::TunnelRefused { stream: at, reason } if at == refused => Some(reason),
        GuestToHost::TunnelOpened { stream: at } if at == refused => {
            panic!("the guest opened a port it never found")
        }
        _ => None,
    })
    .await;
    assert!(reason.contains("nothing listens"), "{reason}");
    let knock = tokio::time::timeout(Duration::from_millis(200), hidden.accept()).await;
    assert!(
        knock.is_err(),
        "the guest connected to a port it never found"
    );
}

#[tokio::test]
async fn a_new_host_link_hears_the_ports_at_once() {
    let container = Container::new(quick);
    container.listen(&[8080]);
    let mut first = container.link().await;
    watch(&mut first, &[8080]).await;

    // The slow pace a dropped link leaves never comes in this test, so
    // only the new link's start can send these.
    let mut second = container.link().await;
    first.closed().await;
    match second.recv().await {
        GuestToHost::Ports { listening } => {
            let ports: Vec<u16> = listening.iter().map(|port| port.port).collect();
            assert_eq!(ports, [8080]);
        }
        other => panic!("expected the ports, got {other:?}"),
    }
}

#[tokio::test]
async fn forgetting_a_session_removes_the_attachments_only_it_held() {
    let container = Container::new(quick);
    let mut host = container.link().await;
    let folder = |agent: AgentId| container.attachments().join(agent.to_string());
    let (own, shared) = (AgentId::generate(), AgentId::generate());
    for agent in [own, shared] {
        fs::create_dir_all(folder(agent).join("upload")).unwrap();
        fs::write(folder(agent).join("upload/a.txt"), "a").unwrap();
    }
    let spawn = |req: u64, session: SessionId, agents: &[AgentId]| HostToGuest::Spawn {
        req,
        launch: GuestLaunch {
            session,
            program: "/usr/local/bin/claude".to_owned(),
            args: agents
                .iter()
                .flat_map(|agent| {
                    let dir = folder(*agent).to_str().unwrap().to_owned();
                    ["--add-dir".to_owned(), dir]
                })
                .collect(),
            env: Vec::new(),
            env_remove: Vec::new(),
            secret_env: Vec::new(),
            cwd: "/home/agent/work/maxhq".to_owned(),
            framing: Framing::ClaudeStreamJson,
            user: "agent".to_owned(),
        },
    };
    let done = |want: u64| {
        move |message: GuestToHost| match message {
            GuestToHost::SpawnResult { req, result } | GuestToHost::Done { req, result }
                if req == want =>
            {
                Some(result)
            }
            _ => None,
        }
    };
    let (one, two) = (SessionId::generate(), SessionId::generate());
    host.send(&spawn(1, one, &[own, shared])).await;
    assert_eq!(until(&mut host, done(1)).await, Ok(()));
    host.send(&spawn(2, two, &[shared])).await;
    assert_eq!(until(&mut host, done(2)).await, Ok(()));

    // A holder the CLI still runs in can't be forgotten, so nothing goes.
    host.send(&HostToGuest::Forget {
        req: 3,
        session: one,
    })
    .await;
    assert!(until(&mut host, done(3)).await.is_err());
    assert!(folder(own).exists());

    container.fake.exit(one, Some(0), None);
    host.send(&HostToGuest::Forget {
        req: 4,
        session: one,
    })
    .await;
    assert_eq!(until(&mut host, done(4)).await, Ok(()));
    assert!(!folder(own).exists());
    assert!(folder(shared).join("upload/a.txt").exists());

    container.fake.exit(two, Some(0), None);
    host.send(&HostToGuest::Forget {
        req: 5,
        session: two,
    })
    .await;
    assert_eq!(until(&mut host, done(5)).await, Ok(()));
    assert!(!folder(shared).exists());
}

#[tokio::test]
async fn a_spawn_gets_its_account_folder_ready_first() {
    let container = Container::new(quick);
    let mut host = container.link().await;
    let home = container.temp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let spawn = |req: u64, folder: &str| HostToGuest::Spawn {
        req,
        launch: GuestLaunch {
            session: SessionId::generate(),
            program: "/usr/local/bin/codex".to_owned(),
            args: vec!["app-server".to_owned()],
            env: vec![("CODEX_HOME".to_owned(), folder.to_owned())],
            env_remove: Vec::new(),
            secret_env: Vec::new(),
            cwd: folder.to_owned(),
            framing: Framing::JsonRpc,
            user: "agent".to_owned(),
        },
    };
    let done = |want: u64| {
        move |message: GuestToHost| match message {
            GuestToHost::SpawnResult { req, result } if req == want => Some(result),
            _ => None,
        }
    };
    host.send(&spawn(1, "/home/agent/.codex-acct")).await;
    assert_eq!(until(&mut host, done(1)).await, Ok(()));
    let config = fs::read_to_string(home.join(".codex-acct/config.toml")).unwrap();
    assert!(config.contains("trust_level = \"trusted\""), "{config}");

    // A folder that isn't the agent's never reaches a holder, and the
    // reason goes out as it is, for the host to pass on.
    host.send(&spawn(2, "/etc")).await;
    let err = until(&mut host, done(2)).await.unwrap_err();
    assert!(err.starts_with("couldn't get /etc ready: "), "{err}");
    assert_eq!(container.fake.sessions().len(), 1);
}

/// A guest's transfers in `root`, with a checkout at `work/name`.
fn placing(root: &Path) -> (Files, PathBuf) {
    let checkout = root.join("work/name");
    fs::create_dir_all(&checkout).unwrap();
    let files = Files::new(Config {
        attachments: root.join("attachments"),
        work: root.join("work"),
        room: Room {
            keep_bytes: 0,
            keep_percent: 0,
        },
        quota: DEVICE_QUOTA,
    });
    (files, checkout)
}

/// Stages an upload whose top folder holds `files`, as a copy's lands.
fn stage(root: &Path, files: &[(&str, &str)]) -> TransferId {
    let staged = TransferId::generate();
    let top = root.join(format!("attachments/staged/{staged}/name"));
    for (path, text) in files {
        let path = top.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    staged
}

fn failure(path: &str, reason: &str) -> PlaceFailure {
    PlaceFailure {
        path: path.to_owned(),
        reason: reason.to_owned(),
    }
}

#[tokio::test]
async fn placing_a_copy_keeps_what_is_there_unless_told_to_overwrite() {
    let dir = TempDir::new();
    let (files, checkout) = placing(dir.path());
    fs::write(checkout.join(".env"), "old").unwrap();
    let copied = [(".env", "new"), ("config/local.json", "{}")];

    let staged = stage(dir.path(), &copied);
    let cwd = checkout.to_str().unwrap().to_owned();
    let placed = files.place(staged, cwd.clone(), false).await.unwrap();
    assert_eq!(placed.written, 1);
    assert_eq!(placed.failed, [failure(".env", "a file is already there")]);
    assert_eq!(fs::read_to_string(checkout.join(".env")).unwrap(), "old");
    assert_eq!(
        fs::read_to_string(checkout.join("config/local.json")).unwrap(),
        "{}"
    );
    // The staged upload goes once it's placed.
    assert!(!dir
        .path()
        .join(format!("attachments/staged/{staged}"))
        .exists());
    assert_eq!(
        files.place(staged, cwd.clone(), false).await,
        Err("the copied files aren't staged here any more".to_owned())
    );

    let staged = stage(dir.path(), &copied);
    let placed = files.place(staged, cwd, true).await.unwrap();
    assert_eq!((placed.written, placed.failed), (2, Vec::new()));
    assert_eq!(fs::read_to_string(checkout.join(".env")).unwrap(), "new");
}

#[tokio::test]
async fn placing_never_follows_a_link_planted_in_the_checkout() {
    let dir = TempDir::new();
    let (files, checkout) = placing(dir.path());
    let outside = dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, checkout.join("config")).unwrap();
    std::os::unix::fs::symlink(outside.join("env"), checkout.join(".env")).unwrap();
    let staged = stage(
        dir.path(),
        &[
            (".env", "new"),
            ("config/local.json", "{}"),
            (".git/config", "x"),
        ],
    );
    let top = dir.path().join(format!("attachments/staged/{staged}/name"));
    std::os::unix::fs::symlink("/etc/hosts", top.join("hosts")).unwrap();

    let cwd = checkout.to_str().unwrap().to_owned();
    let mut placed = files.place(staged, cwd, true).await.unwrap();
    placed.failed.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(placed.written, 0);
    assert_eq!(
        placed.failed,
        [
            failure(".env", "something that isn't a file is there"),
            failure(".git", "MaxHQ doesn't copy a file by that name"),
            failure("config", "something that isn't a folder is in the way"),
            failure("hosts", "it's a link or a special file, not a file"),
        ]
    );
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
}

#[tokio::test]
async fn placing_outside_the_work_folder_is_refused() {
    let dir = TempDir::new();
    let (files, _) = placing(dir.path());
    let staged = stage(dir.path(), &[(".env", "new")]);
    let elsewhere = dir.path().to_str().unwrap().to_owned();
    assert!(files.place(staged, elsewhere, false).await.is_err());
}

#[test]
fn placed_files_and_folders_go_to_the_owner() {
    use std::os::unix::fs::MetadataExt;

    let dir = TempDir::new();
    let top = dir.path().join("staged/name");
    fs::create_dir_all(top.join("config")).unwrap();
    fs::write(top.join("config/local.json"), "{}").unwrap();
    let checkout = dir.path().join("checkout");
    fs::create_dir(&checkout).unwrap();
    // Without root, only a user's own IDs can be given, which still runs
    // the chown on each file and folder.
    let mine = fs::metadata(&checkout).unwrap();
    let owner = RunAs {
        user: "dev".to_owned(),
        uid: mine.uid(),
        gid: mine.gid(),
        home: dir.path().to_path_buf(),
    };
    let placed = crate::transfer::deliver::place(&top, &checkout, false, Some(&owner)).unwrap();
    assert_eq!(placed.written, 1);
    for path in ["config", "config/local.json"] {
        let meta = fs::symlink_metadata(checkout.join(path)).unwrap();
        assert_eq!((meta.uid(), meta.gid()), (owner.uid, owner.gid), "{path}");
    }
}

#[tokio::test]
async fn a_guest_that_isnt_a_test_host_refuses_leases() {
    let container = Container::new(brisk);
    let mut host = container.link().await;
    let lease = maxhq_protocol::ids::LeaseId::derive("lease");
    host.send(&HostToGuest::LeaseRun {
        req: 1,
        lease,
        command: vec!["snapshot".into()],
        timeout_seconds: 5,
    })
    .await;
    host.send(&HostToGuest::LeaseScreenshot { req: 2, lease })
        .await;
    host.send(&HostToGuest::LeaseClose { req: 3, lease }).await;
    let ran = until(&mut host, |message| match message {
        GuestToHost::LeaseRan { req: 1, result } => Some(result),
        _ => None,
    })
    .await;
    assert_eq!(ran, Err(lease::NOT_A_TEST_HOST.to_owned()));
    let screen = until(&mut host, |message| match message {
        GuestToHost::LeaseScreen { req: 2, result } => Some(result),
        _ => None,
    })
    .await;
    assert_eq!(screen, Err(lease::NOT_A_TEST_HOST.to_owned()));
    let closed = until(&mut host, |message| match message {
        GuestToHost::LeaseDone { req: 3, result } => Some(result),
        _ => None,
    })
    .await;
    assert_eq!(closed, Err(lease::NOT_A_TEST_HOST.to_owned()));
}

#[tokio::test]
async fn a_test_host_answers_leases() {
    let home = TempDir::new();
    let linux = lease::Linux::here(home.path().to_owned());
    let container = Container::hosting(brisk, Some(Arc::new(linux)));
    let mut host = container.link().await;
    let lease = maxhq_protocol::ids::LeaseId::derive("lease");
    host.send(&HostToGuest::LeaseRun {
        req: 1,
        lease,
        command: vec!["/bin/sh".into(), "-c".into(), "echo hello".into()],
        timeout_seconds: 5,
    })
    .await;
    let ran = until(&mut host, |message| match message {
        GuestToHost::LeaseRan { req: 1, result } => Some(result),
        _ => None,
    })
    .await
    .unwrap();
    assert_eq!((ran.stdout.as_str(), ran.exit_code), ("hello\n", Some(0)));
    host.send(&HostToGuest::LeaseClose { req: 2, lease }).await;
    let closed = until(&mut host, |message| match message {
        GuestToHost::LeaseDone { req: 2, result } => Some(result),
        _ => None,
    })
    .await;
    assert_eq!(closed, Ok(()));
}

#[tokio::test]
async fn a_guest_links_a_checkout_and_the_always_set_to_the_mount() {
    let container = Container::new(brisk);
    let checkout = container.work().join(".worktrees/feature");
    fs::create_dir_all(checkout.join(".git/info")).unwrap();
    let home = container.temp.path().join("home");
    let claude = home.join(".claude-acct/skills");
    let mut host = container.link().await;
    host.send(&HostToGuest::SkillLinks {
        req: 1,
        checkout: checkout.display().to_string(),
        names: vec!["git-flow".into()],
        always: vec!["unslop".into()],
        homes: vec![claude.display().to_string()],
        pack: None,
        retry: true,
    })
    .await;
    let linked = until(&mut host, |message| match message {
        GuestToHost::SkillLinksDone { req: 1, result, .. } => Some(result),
        _ => None,
    })
    .await;
    assert_eq!(linked, Ok(()));
    let read = |path: PathBuf| fs::read_link(path).unwrap();
    assert_eq!(
        read(checkout.join(".maxhq/skills")),
        Path::new("/opt/maxhq/skills/current")
    );
    assert_eq!(
        read(checkout.join(".claude/skills/git-flow")),
        Path::new("../../.maxhq/skills/git-flow")
    );
    assert_eq!(
        read(checkout.join(".agents/skills/git-flow")),
        Path::new("../../.maxhq/skills/git-flow")
    );
    assert!(checkout.join(".maxhq/proposals").is_dir());
    assert!(fs::read_to_string(checkout.join(".git/info/exclude"))
        .unwrap()
        .contains("/.claude/skills/git-flow"));
    assert_eq!(
        read(claude.join("unslop")),
        Path::new("/opt/maxhq/skills/current/unslop")
    );

    // A home outside the agent's is refused.
    host.send(&HostToGuest::SkillLinks {
        req: 2,
        checkout: checkout.display().to_string(),
        names: Vec::new(),
        always: vec!["unslop".into()],
        homes: vec!["/etc/skills".into()],
        pack: None,
        retry: true,
    })
    .await;
    let refused = until(&mut host, |message| match message {
        GuestToHost::SkillLinksDone { req: 2, result, .. } => Some(result),
        _ => None,
    })
    .await;
    assert!(refused.unwrap_err().contains("isn't in the agent's home"));
}

#[tokio::test]
async fn a_guest_wont_link_skills_through_a_link_out_of_the_checkout() {
    let container = Container::new(brisk);
    let outside = container.temp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    let mut host = container.link().await;
    for (req, name, link) in [
        (1, "a", ".maxhq"),
        (2, "b", ".maxhq/proposals"),
        (3, "c", ".claude"),
        (4, "d", ".agents/skills"),
    ] {
        let checkout = container.work().join(name);
        fs::create_dir_all(checkout.join(".git/info")).unwrap();
        fs::create_dir_all(checkout.join(link).parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&outside, checkout.join(link)).unwrap();
        host.send(&HostToGuest::SkillLinks {
            req,
            checkout: checkout.display().to_string(),
            names: vec!["git-flow".into()],
            always: Vec::new(),
            homes: Vec::new(),
            pack: None,
            retry: true,
        })
        .await;
        let refused = until(&mut host, |message| match message {
            GuestToHost::SkillLinksDone {
                req: answered,
                result,
                ..
            } if answered == req => Some(result),
            _ => None,
        })
        .await;
        let why = refused.unwrap_err();
        assert!(why.contains("leads out of the checkout"), "{link}: {why}");
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0, "{link}");
        assert!(!checkout.join(".maxhq/skills").exists(), "{link}");
    }
}

#[tokio::test]
async fn a_guest_reads_a_checkouts_proposals_and_removes_one_when_asked() {
    let container = Container::new(brisk);
    let checkout = container.work().join("repo");
    let proposals = checkout.join(".maxhq/proposals");
    fs::create_dir_all(proposals.join("alpha/scripts")).unwrap();
    fs::write(proposals.join("alpha/SKILL.md"), "---\nname: alpha\n---\n").unwrap();
    fs::write(proposals.join("alpha/scripts/go.sh"), "echo go\n").unwrap();
    std::os::unix::fs::symlink("/etc", proposals.join("linked")).unwrap();
    fs::create_dir_all(proposals.join("beta")).unwrap();
    std::os::unix::fs::symlink("/etc/hosts", proposals.join("beta/hosts")).unwrap();
    let mut host = container.link().await;
    let ask = |req, remove: Option<&str>| HostToGuest::SkillProposals {
        req,
        checkout: checkout.display().to_string(),
        remove: remove.map(str::to_owned),
    };
    host.send(&ask(1, None)).await;
    let read = until(&mut host, |message| match message {
        GuestToHost::SkillProposalsRead { req: 1, result } => Some(result),
        _ => None,
    })
    .await
    .unwrap();
    // A link, or a folder with one in it, is left out.
    let [alpha] = read.as_slice() else {
        panic!("expected one proposal, got {read:?}");
    };
    assert_eq!(alpha.folder, "alpha");
    let paths: Vec<&str> = alpha.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["SKILL.md", "scripts/go.sh"]);

    host.send(&ask(2, Some("alpha"))).await;
    let read = until(&mut host, |message| match message {
        GuestToHost::SkillProposalsRead { req: 2, result } => Some(result),
        _ => None,
    })
    .await;
    assert_eq!(read, Ok(Vec::new()));
    assert!(!proposals.join("alpha").exists());

    // Nothing outside the work folder is read.
    host.send(&HostToGuest::SkillProposals {
        req: 3,
        checkout: "/etc".into(),
        remove: None,
    })
    .await;
    let refused = until(&mut host, |message| match message {
        GuestToHost::SkillProposalsRead { req: 3, result } => Some(result),
        _ => None,
    })
    .await;
    assert!(refused
        .unwrap_err()
        .contains("isn't a checkout in the work folder"));
    host.send(&ask(4, Some("../x"))).await;
    let bad = until(&mut host, |message| match message {
        GuestToHost::SkillProposalsRead { req: 4, result } => Some(result),
        _ => None,
    })
    .await;
    assert!(bad.is_err());
}

#[tokio::test]
async fn a_guest_reads_what_can_run_in_a_folder_and_nothing_a_link_leads_to() {
    let container = Container::new(brisk);
    let checkout = container.work().join("repo");
    fs::create_dir_all(checkout.join(".claude")).unwrap();
    fs::write(checkout.join(".claude/settings.json"), "{\"hooks\":{}}").unwrap();
    let outside = container.work().parent().unwrap().join("secret");
    fs::write(&outside, "hidden").unwrap();
    let mut host = container.link().await;
    let ask = |req| HostToGuest::FolderTrustScan {
        req,
        cwd: checkout.display().to_string(),
        locked: Vec::new(),
    };
    host.send(&ask(1)).await;
    let read = until(&mut host, |message| match message {
        GuestToHost::FolderTrustRead { req: 1, result } => Some(result),
        _ => None,
    })
    .await
    .unwrap();
    assert!(!read.failed);
    assert_eq!(read.files[0].name, ".claude/settings.json");
    assert_eq!(read.shown[0].text, "{\"hooks\":{}}");

    std::os::unix::fs::symlink(&outside, checkout.join(".mcp.json")).unwrap();
    host.send(&ask(2)).await;
    let read = until(&mut host, |message| match message {
        GuestToHost::FolderTrustRead { req: 2, result } => Some(result),
        _ => None,
    })
    .await
    .unwrap();
    assert!(read.failed);
    assert!(read.shown.iter().all(|file| !file.text.contains("hidden")));
}

#[tokio::test]
async fn a_guest_copies_the_pack_from_the_mount_and_installs_it_once() {
    use std::os::unix::fs::PermissionsExt;

    let container = Container::new(brisk);
    let checkout = container.work().join("repo");
    fs::create_dir_all(checkout.join(".git/info")).unwrap();
    let from = container.temp.path().join("mount/.devflow-vendor");
    fs::create_dir_all(&from).unwrap();
    let script = from.join("install.sh");
    fs::write(&script, "#!/usr/bin/env bash\necho ran >> installed.txt\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let tree = maxhq_core::skills::tree::of(&maxhq_core::skills::disk::DiskFs, &from).unwrap();
    let mut host = container.link().await;
    for req in [1, 2] {
        host.send(&HostToGuest::SkillLinks {
            req,
            checkout: checkout.display().to_string(),
            names: Vec::new(),
            always: Vec::new(),
            homes: Vec::new(),
            pack: Some(tree.clone()),
            retry: false,
        })
        .await;
        let done = until(&mut host, |message| match message {
            GuestToHost::SkillLinksDone {
                req: got,
                result,
                pack,
            } if got == req => Some((result, pack)),
            _ => None,
        })
        .await;
        assert_eq!(
            done,
            (Ok(()), Some(PackState::Installed { tree: tree.clone() }))
        );
    }
    let runs = fs::read_to_string(checkout.join("installed.txt")).unwrap();
    assert_eq!(runs, "ran\n");
    assert!(container.temp.path().join("data/pack-logs").is_dir());
    assert!(fs::read_to_string(checkout.join(".git/info/exclude"))
        .unwrap()
        .contains("/.devflow-vendor/"));
}

/// Moves the folder at `folder` away and leaves a link to `real` there.
fn swap_for_link(folder: &Path, real: &Path) {
    fs::remove_dir_all(folder).unwrap();
    std::os::unix::fs::symlink(real, folder).unwrap();
}

#[tokio::test]
async fn a_fetch_refuses_a_work_folder_thats_a_link() {
    let container = Container::new(quick);
    let real = container.temp.path().join("elsewhere");
    fs::create_dir_all(real.join("repo")).unwrap();
    fs::write(real.join("secret.txt"), "root's").unwrap();
    swap_for_link(&container.work(), &real);
    let canonical = real.canonicalize().unwrap();
    let mut host = container.link().await;
    let asks = [
        "secret.txt".to_owned(),
        container.work().join("secret.txt").display().to_string(),
        canonical.join("secret.txt").display().to_string(),
        canonical.display().to_string(),
    ];
    for path in asks {
        let transfer = TransferId::generate();
        host.send(&HostToGuest::FetchBegin {
            transfer,
            path: path.clone(),
        })
        .await;
        let message = until(&mut host, |message| match message {
            GuestToHost::FetchManifest { transfer: at, .. } if at == transfer => {
                panic!("the fetch of {path} went ahead")
            }
            GuestToHost::TransferFailed {
                transfer: at,
                message,
                ..
            } if at == transfer => Some(message),
            _ => None,
        })
        .await;
        assert!(message.contains("link"), "{path}: {message}");
    }
    let transfer = TransferId::generate();
    host.send(&HostToGuest::FetchFiles {
        transfer,
        cwd: "repo".to_owned(),
        paths: vec!["file.txt".to_owned()],
    })
    .await;
    until(&mut host, |message| match message {
        GuestToHost::FetchManifest { transfer: at, .. } if at == transfer => {
            panic!("the fetch of chosen files went ahead")
        }
        GuestToHost::TransferFailed { transfer: at, .. } if at == transfer => Some(()),
        _ => None,
    })
    .await;
}

#[tokio::test]
async fn a_fetch_stops_at_a_folder_swapped_for_a_link_after_the_manifest() {
    let container = Container::new(quick);
    let mut host = container.link().await;
    let pick = container.work().join("out");
    fs::create_dir_all(pick.join("sub")).unwrap();
    fs::write(pick.join("sub/file.txt"), "mine").unwrap();
    let secret = container.temp.path().join("secret");
    fs::create_dir_all(&secret).unwrap();
    fs::write(secret.join("file.txt"), "root").unwrap();

    let transfer = TransferId::generate();
    host.send(&HostToGuest::FetchBegin {
        transfer,
        path: pick.to_str().unwrap().to_owned(),
    })
    .await;
    until(&mut host, |message| match message {
        GuestToHost::FetchManifest { transfer: at, .. } if at == transfer => Some(()),
        GuestToHost::TransferFailed { message, .. } => panic!("the fetch failed: {message}"),
        _ => None,
    })
    .await;
    fs::rename(pick.join("sub"), pick.join("old")).unwrap();
    std::os::unix::fs::symlink(&secret, pick.join("sub")).unwrap();
    host.send(&HostToGuest::TransferCredit {
        transfer,
        committed: 0,
        window_end: 4,
    })
    .await;
    loop {
        match host.next().await {
            Ok(Read::Chunk { transfer: at, .. }) if at == transfer => {
                panic!("the fetch sent what the link points at")
            }
            Ok(Read::Message(body)) => {
                let message: GuestToHost = serde_json::from_slice(&body).unwrap();
                if matches!(message, GuestToHost::TransferFailed { transfer: at, .. } if at == transfer)
                {
                    break;
                }
            }
            Ok(_) => {}
            Err(err) => panic!("the link failed: {err}"),
        }
    }
}

#[tokio::test]
async fn placing_refuses_a_work_folder_thats_a_link() {
    let dir = TempDir::new();
    let (files, _) = placing(dir.path());
    let staged = stage(dir.path(), &[(".env", "new")]);
    let real = dir.path().join("elsewhere");
    fs::create_dir_all(real.join("name")).unwrap();
    let work = dir.path().join("work");
    swap_for_link(&work, &real);
    let canonical = real.canonicalize().unwrap();
    for cwd in [
        "name".to_owned(),
        work.join("name").display().to_string(),
        canonical.join("name").display().to_string(),
    ] {
        let placed = files.place(staged, cwd.clone(), false).await;
        assert!(placed.is_err(), "{cwd}");
    }
    assert!(!real.join("name/.env").exists());
}

#[test]
fn only_a_host_follows_a_folder_thats_a_link() {
    let dir = TempDir::new();
    let real = dir.path().join("real");
    fs::create_dir_all(real.join("repo")).unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let roots = [link.clone()];
    assert!(crate::transfer::disk::resolve(&roots, "repo").is_ok());
    assert!(crate::transfer::disk::inside(&roots, "repo").is_err());

    let found = crate::transfer::disk::inside(std::slice::from_ref(&real), "repo").unwrap();
    assert_eq!(found.parts, ["repo"]);
    assert_eq!(found.name, "repo");
    assert_eq!(found.path, real.join("repo"));
    let whole = crate::transfer::disk::inside(std::slice::from_ref(&real), ".").unwrap();
    assert!(whole.parts.is_empty());
    assert_eq!(whole.name, "real");
    assert_eq!(whole.path, real);
}
