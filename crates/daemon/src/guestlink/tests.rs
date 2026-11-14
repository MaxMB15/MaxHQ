// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The host's guest links against a fake guest that sends what a test
//! tells it to, broken frames among them. The fixtures here serve the
//! guest's own tests too.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::stream::{BoxStream, StreamExt};
use maxhq_protocol::find::{PathKind, PathMatch};
use maxhq_protocol::frame::{encode_message, MAX_FRAME_LEN};
use maxhq_protocol::guest::{
    ChildState, GuestPort, GuestToHost, HolderEvent, HolderHello, HolderLine, HostToGuest,
    TokenReply, TokenWant, TunnelTarget, WatchedHolder, GUEST_PROTOCOL_VERSION, MAX_HISTORIES,
    MAX_HOLDER_CONNECTIONS, MAX_MESSAGE_LEN, MAX_PORTS, MAX_TOKEN_REQUESTS_PER_MINUTE, MAX_TUNNELS,
};
use maxhq_protocol::ids::{
    AccountId, AgentId, BootId, DeviceId, SessionId, StreamId, TransferId, WorkspaceId,
};
use maxhq_protocol::message::ErrorCode;
use maxhq_protocol::model::{BindAddress, Vendor};
use maxhq_protocol::processes::{AgentProcess, ProcessList, ProcessSignal};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{ItemBody, ItemId, ItemStatus, TimelineItem};
use maxhq_protocol::Secret;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};

use super::link::{UNREAD_KIB, VERSION};
use super::wire::{Read, ReadError, Reader, SendError};
use super::{
    github_repo, socket_path, CodexTokens, Config, GithubTokens, GuestFiles, GuestSender,
    GuestTunnels, Guests, Upgrader,
};
use crate::incus::InstanceName;
use crate::session::clock::BoxFuture;
use crate::session::fake::FakeHolders;
use crate::session::holders::{
    ConnectError, Connection, HolderPlaces, Holders, PlaceChange, SessionOwner, SessionOwners,
};
use crate::testing::TempDir;
use crate::workspace::GuestLinks;

/// The token every fake broker hands out.
pub(crate) const TOKEN: &str = "test-not-a-real-token";

/// What the fake brokers answer.
pub(crate) fn token() -> TokenReply {
    TokenReply::Token {
        value: Secret::new(TOKEN.to_owned()),
        expires_at: Timestamp(1_800_000_000_000),
    }
}

/// Waits for `future`, failing the test after ten seconds.
pub(crate) async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("the test waited more than ten seconds")
}

/// Waits until `check` holds, failing the test after ten seconds.
pub(crate) async fn eventually(mut check: impl FnMut() -> bool) {
    within(async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
}

/// An instance name for a test.
pub(crate) fn instance(name: &str) -> InstanceName {
    InstanceName::new(name).unwrap()
}

/// The session manager's word on whose each session is.
#[derive(Default)]
pub(crate) struct Owners(Mutex<HashMap<SessionId, SessionOwner>>);

impl Owners {
    /// `session` runs `vendor` in `workspace`. Returns its account.
    pub(crate) fn set(
        &self,
        session: SessionId,
        workspace: WorkspaceId,
        vendor: Vendor,
    ) -> AccountId {
        let account = AccountId::generate();
        self.0.lock().unwrap().insert(
            session,
            SessionOwner {
                workspace,
                account,
                vendor,
            },
        );
        account
    }
}

impl SessionOwners for Owners {
    fn owner(&self, session: SessionId) -> Option<SessionOwner> {
        self.0.lock().unwrap().get(&session).copied()
    }
}

/// Brokers that hand out [`TOKEN`] and note each ask.
#[derive(Default)]
pub(crate) struct Brokers(Mutex<Vec<String>>);

impl Brokers {
    /// Each ask, as "codex <account>" or "github <repo>".
    pub(crate) fn asked(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

impl CodexTokens for Brokers {
    fn token(&self, account: AccountId) -> BoxFuture<TokenReply> {
        self.0.lock().unwrap().push(format!("codex {account}"));
        Box::pin(async { token() })
    }
}

impl GithubTokens for Brokers {
    fn token(&self, _workspace: WorkspaceId, repo: String) -> BoxFuture<TokenReply> {
        self.0.lock().unwrap().push(format!("github {repo}"));
        Box::pin(async { token() })
    }
}

/// An install that only notes what it was asked to install.
#[derive(Default)]
pub(crate) struct Upgrades(Mutex<Vec<(String, PathBuf)>>);

impl Upgrades {
    /// Each install, as the instance and the binary.
    pub(crate) fn taken(&self) -> Vec<(String, PathBuf)> {
        self.0.lock().unwrap().clone()
    }
}

impl Upgrader for Upgrades {
    fn upgrade(&self, instance: InstanceName, binary: PathBuf) -> BoxFuture<Result<(), String>> {
        self.0
            .lock()
            .unwrap()
            .push((instance.as_str().to_owned(), binary));
        Box::pin(async { Ok(()) })
    }
}

/// Drops transfer and tunnel traffic, for tests of the link alone.
pub(crate) struct Unrouted;

impl GuestFiles for Unrouted {
    fn message(&self, _workspace: WorkspaceId, _message: GuestToHost) {}

    fn chunk(&self, _workspace: WorkspaceId, _transfer: TransferId, _offset: u64, _data: Vec<u8>) {}

    fn link_down(&self, _workspace: WorkspaceId) {}
}

impl GuestTunnels for Unrouted {
    fn link_up(&self, _workspace: WorkspaceId, _sender: GuestSender) {}

    fn ports(&self, _workspace: WorkspaceId, _listening: Vec<GuestPort>) {}

    fn message(&self, _workspace: WorkspaceId, _message: GuestToHost) {}

    fn data(&self, _workspace: WorkspaceId, _stream: StreamId, _data: Vec<u8>) {}

    fn link_down(&self, _workspace: WorkspaceId) {}
}

/// A host with its guest links and fakes for the rest of `maxd`.
pub(crate) struct Host {
    pub(crate) temp: TempDir,
    pub(crate) guests: Guests,
    pub(crate) owners: Arc<Owners>,
    pub(crate) brokers: Arc<Brokers>,
    pub(crate) upgrades: Arc<Upgrades>,
    changes: BoxStream<'static, PlaceChange>,
}

impl Host {
    /// A host that pings each guest every `keepalive`.
    pub(crate) fn new(keepalive: Duration) -> Self {
        Self::installing(keepalive, |_| PathBuf::from("/opt/maxd-guest"))
    }

    /// A host whose guest binary is the path `binary` gives inside its
    /// temporary folder.
    pub(crate) fn installing(keepalive: Duration, binary: impl FnOnce(&Path) -> PathBuf) -> Self {
        let temp = TempDir::new();
        let guest_binary = binary(temp.path());
        std::fs::create_dir(temp.path().join(super::GUESTS_DIR)).unwrap();
        let owners = Arc::new(Owners::default());
        let brokers = Arc::new(Brokers::default());
        let upgrades = Arc::new(Upgrades::default());
        let config = Config {
            runtime: temp.path().to_owned(),
            device: DeviceId::generate(),
            owners: owners.clone(),
            codex: brokers.clone(),
            github: brokers.clone(),
            upgrader: upgrades.clone(),
            guest_binary: Some(guest_binary),
            files: Arc::new(Unrouted),
            tunnels: Arc::new(Unrouted),
            keepalive,
        };
        let guests = Guests::new(Arc::new(FakeHolders::new()), config);
        let changes = guests.changes();
        Self {
            temp,
            guests,
            owners,
            brokers,
            upgrades,
            changes,
        }
    }

    /// Where `instance`'s proxy socket goes.
    pub(crate) fn socket(&self, instance: &InstanceName) -> PathBuf {
        socket_path(self.temp.path(), instance)
    }

    /// The next link coming up or going down.
    pub(crate) async fn change(&mut self) -> PlaceChange {
        within(self.changes.next())
            .await
            .expect("the changes ended")
    }

    /// Waits for `workspace`'s link to come up, and gives its hello.
    pub(crate) async fn up(&mut self, workspace: WorkspaceId) -> (BootId, Vec<SessionId>) {
        match self.change().await {
            PlaceChange::Up {
                workspace: up,
                boot_id,
                holders,
            } if up == workspace => (boot_id, holders),
            other => panic!("expected {workspace} up, got {other:?}"),
        }
    }

    /// Waits for `workspace`'s link to drop, and gives why.
    pub(crate) async fn down(&mut self, workspace: WorkspaceId) -> String {
        match self.change().await {
            PlaceChange::Down {
                workspace: down,
                reason,
            } if down == workspace => reason,
            other => panic!("expected {workspace} down, got {other:?}"),
        }
    }
}

/// One end of a link, speaking frames a test writes by hand.
pub(crate) struct Peer {
    reader: Reader<OwnedReadHalf>,
    write: OwnedWriteHalf,
}

impl Peer {
    /// The end of `stream`.
    pub(crate) fn new(stream: UnixStream) -> Self {
        let (read, write) = stream.into_split();
        Self {
            reader: Reader::new(read),
            write,
        }
    }

    /// Connects to `path`, retrying while nothing listens there yet.
    pub(crate) async fn connect(path: &Path) -> Self {
        within(async {
            loop {
                match UnixStream::connect(path).await {
                    Ok(stream) => return Self::new(stream),
                    Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
                }
            }
        })
        .await
    }

    /// Sends one message frame.
    pub(crate) async fn send(&mut self, message: &impl Serialize) {
        let mut frame = Vec::new();
        encode_message(&mut frame, message).unwrap();
        self.write.write_all(&frame).await.unwrap();
    }

    /// Sends `message` `count` times, or until the other end closes.
    pub(crate) async fn flood(&mut self, message: &impl Serialize, count: usize) {
        let mut frame = Vec::new();
        encode_message(&mut frame, message).unwrap();
        for _ in 0..count {
            if self.write.write_all(&frame).await.is_err() {
                return;
            }
        }
    }

    /// Sends a frame of `kind` with `body`, whatever they hold.
    pub(crate) async fn raw(&mut self, kind: u8, body: &[u8]) {
        let len = u32::try_from(body.len() + 1).unwrap();
        let mut frame = len.to_be_bytes().to_vec();
        frame.push(kind);
        frame.extend_from_slice(body);
        self.write.write_all(&frame).await.unwrap();
    }

    /// Sends bytes as they are.
    pub(crate) async fn bytes(&mut self, bytes: &[u8]) {
        self.write.write_all(bytes).await.unwrap();
    }

    /// The next frame.
    pub(crate) async fn next(&mut self) -> Result<Read, ReadError> {
        within(self.reader.next()).await
    }

    /// The next message.
    pub(crate) async fn recv<T: DeserializeOwned>(&mut self) -> T {
        match self.next().await {
            Ok(Read::Message(body)) => serde_json::from_slice(&body).unwrap(),
            Ok(other) => panic!("expected a message, got {other:?}"),
            Err(err) => panic!("expected a message, got {err}"),
        }
    }

    /// Reads until the other end closes the link. A close with bytes left
    /// unread can reach this end as a reset.
    pub(crate) async fn closed(&mut self) {
        within(async { while self.reader.next().await.is_ok() {} }).await;
    }

    /// Reads and drops everything from now on.
    pub(crate) fn drain(self) {
        let mut reader = self.reader;
        let write = self.write;
        tokio::spawn(async move {
            let _write = write;
            while reader.next().await.is_ok() {}
        });
    }
}

/// A guest that listens where an instance's proxy socket goes.
pub(crate) struct FakeGuest {
    listener: UnixListener,
    /// The build it says it runs.
    pub(crate) build: Option<String>,
}

impl FakeGuest {
    /// Listens at `path`.
    pub(crate) fn bind(path: &Path) -> Self {
        Self {
            listener: UnixListener::bind(path).unwrap(),
            build: None,
        }
    }

    /// Takes the host's next link and says hello as `version`. Gives the
    /// link and the host's hello.
    pub(crate) async fn accept(
        &self,
        version: &str,
        holders: Vec<SessionId>,
    ) -> (Peer, HostToGuest) {
        let (stream, _) = within(self.listener.accept()).await.unwrap();
        let mut peer = Peer::new(stream);
        let hello: HostToGuest = peer.recv().await;
        peer.send(&GuestToHost::Hello {
            protocol: GUEST_PROTOCOL_VERSION,
            maxd_version: version.to_owned(),
            instance: "mhq-fake".to_owned(),
            boot_id: BootId("boot-1".to_owned()),
            holders,
            build: self.build.clone(),
        })
        .await;
        (peer, hello)
    }
}

/// A host that hardly ever pings.
pub(crate) const QUIET: Duration = Duration::from_secs(3600);

#[test]
fn each_instance_has_its_own_socket() {
    let instance = InstanceName::new("mhq-maxhq").unwrap();
    assert_eq!(
        socket_path(Path::new("/run/user/1000/maxhq"), &instance),
        Path::new("/run/user/1000/maxhq/guests/mhq-maxhq.sock")
    );
}

#[test]
fn github_repos_come_from_their_urls() {
    for (url, repo) in [
        (
            "https://github.com/Example-User/MaxHQ.git",
            Some("example-user/maxhq"),
        ),
        (
            "git@github.com:example-user/maxhq.git",
            Some("example-user/maxhq"),
        ),
        (
            "ssh://git@github.com/example-user/maxhq",
            Some("example-user/maxhq"),
        ),
        (
            "https://github.com/example-user/maxhq/",
            Some("example-user/maxhq"),
        ),
        ("example-user/maxhq", Some("example-user/maxhq")),
        ("https://gitlab.com/example-user/maxhq", None),
        ("https://github.com/example-user", None),
        ("https://github.com/../maxhq", None),
        ("file:///home/agent/work/maxhq", None),
        ("/home/agent/work/maxhq", None),
        ("", None),
    ] {
        assert_eq!(github_repo(url).as_deref(), repo, "{url:?}");
    }
}

/// Something a guest may not do.
#[derive(Debug)]
enum Breach {
    FrameTooLong,
    MessageTooLong,
    UnknownKind,
    TerminalFrame,
    BadJson,
    UnknownMessage,
    SecondHello,
    TooManyPorts,
    ForeignToken(SessionId),
}

impl Breach {
    async fn commit(&self, peer: &mut Peer) {
        match self {
            Self::FrameTooLong => {
                let len = u32::try_from(MAX_FRAME_LEN + 1).unwrap();
                peer.bytes(&len.to_be_bytes()).await;
            }
            Self::MessageTooLong => {
                peer.send(&GuestToHost::TransferFailed {
                    transfer: TransferId::generate(),
                    code: ErrorCode::Internal,
                    entry: None,
                    message: "x".repeat(MAX_MESSAGE_LEN + 1),
                })
                .await;
            }
            Self::UnknownKind => peer.raw(9, b"").await,
            Self::TerminalFrame => peer.raw(1, &[0; 16]).await,
            Self::BadJson => peer.raw(0, b"{not json").await,
            Self::UnknownMessage => peer.raw(0, br#"{"type":"nope"}"#).await,
            Self::SecondHello => {
                peer.send(&GuestToHost::Hello {
                    protocol: GUEST_PROTOCOL_VERSION,
                    maxd_version: VERSION.to_owned(),
                    instance: "mhq-fake".to_owned(),
                    boot_id: BootId("boot-2".to_owned()),
                    holders: Vec::new(),
                    build: None,
                })
                .await;
            }
            Self::TooManyPorts => {
                let port = GuestPort {
                    port: 3000,
                    bind: BindAddress::Loopback,
                    session: None,
                    process: None,
                };
                peer.send(&GuestToHost::Ports {
                    listening: vec![port; MAX_PORTS + 1],
                })
                .await;
            }
            Self::ForeignToken(session) => {
                peer.send(&GuestToHost::TokenRequest {
                    req: 1,
                    session: *session,
                    want: TokenWant::Codex,
                })
                .await;
            }
        }
    }
}

#[tokio::test]
async fn a_misbehaving_guest_loses_its_link_and_the_others_keep_theirs() {
    let mut host = Host::new(QUIET);
    let (good, bad) = (WorkspaceId::generate(), WorkspaceId::generate());
    let (good_name, bad_name) = (instance("mhq-good"), instance("mhq-bad"));
    let good_guest = FakeGuest::bind(&host.socket(&good_name));
    let bad_guest = FakeGuest::bind(&host.socket(&bad_name));
    host.guests.add(good, good_name, None);
    let (mut good_peer, _) = good_guest.accept(VERSION, Vec::new()).await;
    host.up(good).await;
    host.guests.add(bad, bad_name, None);

    // The good workspace's session, which the bad guest asks a token for.
    let theirs = SessionId::generate();
    host.owners.set(theirs, good, Vendor::Codex);
    for breach in [
        Breach::FrameTooLong,
        Breach::MessageTooLong,
        Breach::UnknownKind,
        Breach::TerminalFrame,
        Breach::BadJson,
        Breach::UnknownMessage,
        Breach::SecondHello,
        Breach::TooManyPorts,
        Breach::ForeignToken(theirs),
    ] {
        let (mut peer, _) = bad_guest.accept(VERSION, Vec::new()).await;
        host.up(bad).await;
        assert!(host.guests.guest(bad).is_some());
        breach.commit(&mut peer).await;
        peer.closed().await;
        let reason = host.down(bad).await;
        assert!(!reason.is_empty(), "{breach:?}");
        assert!(host.guests.guest(bad).is_none(), "{breach:?}");
    }
    assert!(host.brokers.asked().is_empty());

    // The good link served all along.
    let holders = host.guests.guest(good).unwrap();
    let list = tokio::spawn(async move { holders.list().await });
    let HostToGuest::ListHolders { req } = good_peer.recv().await else {
        panic!("expected a list");
    };
    good_peer
        .send(&GuestToHost::Holders {
            req,
            sessions: vec![theirs],
        })
        .await;
    assert_eq!(within(list).await.unwrap().unwrap(), vec![theirs]);
}

#[tokio::test]
async fn token_requests_get_answers_only_for_their_own_sessions() {
    let mut host = Host::new(QUIET);
    let workspace = WorkspaceId::generate();
    let name = instance("mhq-tokens");
    let guest = FakeGuest::bind(&host.socket(&name));
    let repo = "https://github.com/Example-User/MaxHQ.git";
    host.guests.add(workspace, name, Some(repo.to_owned()));
    let (mut peer, hello) = guest.accept(VERSION, Vec::new()).await;
    match hello {
        HostToGuest::Hello {
            protocol,
            maxd_version,
            workspace: theirs,
            repo: Some(sent),
            ..
        } => {
            assert_eq!(protocol, GUEST_PROTOCOL_VERSION);
            assert_eq!(maxd_version, VERSION);
            assert_eq!(theirs, workspace);
            assert_eq!(sent, repo);
        }
        other => panic!("expected a hello with the repo, got {other:?}"),
    }
    host.up(workspace).await;

    let codex = SessionId::generate();
    let account = host.owners.set(codex, workspace, Vendor::Codex);
    let claude = SessionId::generate();
    host.owners.set(claude, workspace, Vendor::Claude);
    let stranger = SessionId::generate();
    let github = |repo: &str| TokenWant::Github {
        repo: repo.to_owned(),
    };
    let cases = [
        (codex, TokenWant::Codex, None),
        (
            claude,
            TokenWant::Codex,
            Some("that session doesn't run Codex"),
        ),
        (claude, github("example-user/maxhq"), None),
        (codex, github("git@github.com:Example-User/MaxHQ.git"), None),
        (
            claude,
            github("example-user/other"),
            Some("only the workspace's own repository gets a token"),
        ),
        (
            stranger,
            TokenWant::Codex,
            Some("that session isn't running"),
        ),
    ];
    for (req, (session, want, refusal)) in (1..).zip(cases) {
        peer.send(&GuestToHost::TokenRequest { req, session, want })
            .await;
        let HostToGuest::TokenReply {
            req: answered,
            reply,
        } = peer.recv().await
        else {
            panic!("expected a token reply");
        };
        assert_eq!(answered, req);
        match refusal {
            None => assert_eq!(reply, token(), "request {req}"),
            Some(why) => assert_eq!(
                reply,
                TokenReply::Refused {
                    reason: why.to_owned()
                },
                "request {req}"
            ),
        }
    }
    assert_eq!(
        host.brokers.asked(),
        vec![
            format!("codex {account}"),
            format!("github {repo}"),
            format!("github {repo}"),
        ]
    );

    // A session that asks too often drops the link.
    let busy = SessionId::generate();
    host.owners.set(busy, workspace, Vendor::Codex);
    for req in 100..100 + u64::from(MAX_TOKEN_REQUESTS_PER_MINUTE) {
        peer.send(&GuestToHost::TokenRequest {
            req,
            session: busy,
            want: TokenWant::Codex,
        })
        .await;
    }
    let mut answered = Vec::new();
    for _ in 0..MAX_TOKEN_REQUESTS_PER_MINUTE {
        let HostToGuest::TokenReply { req, reply } = peer.recv().await else {
            panic!("expected a token reply");
        };
        assert_eq!(reply, token());
        answered.push(req);
    }
    answered.sort_unstable();
    assert_eq!(
        answered,
        (100..100 + u64::from(MAX_TOKEN_REQUESTS_PER_MINUTE)).collect::<Vec<_>>()
    );
    peer.send(&GuestToHost::TokenRequest {
        req: 200,
        session: busy,
        want: TokenWant::Codex,
    })
    .await;
    peer.closed().await;
    let reason = host.down(workspace).await;
    assert!(reason.contains("tokens in a minute"), "{reason}");
}

#[tokio::test]
async fn a_workspace_without_a_repo_gets_no_github_token() {
    let mut host = Host::new(QUIET);
    let workspace = WorkspaceId::generate();
    let name = instance("mhq-norepo");
    let guest = FakeGuest::bind(&host.socket(&name));
    host.guests.add(workspace, name, None);
    let (mut peer, _) = guest.accept(VERSION, Vec::new()).await;
    host.up(workspace).await;
    let session = SessionId::generate();
    host.owners.set(session, workspace, Vendor::Claude);
    peer.send(&GuestToHost::TokenRequest {
        req: 1,
        session,
        want: TokenWant::Github {
            repo: "example-user/maxhq".to_owned(),
        },
    })
    .await;
    let HostToGuest::TokenReply { reply, .. } = peer.recv().await else {
        panic!("expected a token reply");
    };
    assert_eq!(
        reply,
        TokenReply::Refused {
            reason: "this workspace has no GitHub repository".to_owned()
        }
    );
    assert!(host.brokers.asked().is_empty());
}

#[tokio::test]
async fn a_guest_on_another_version_gets_this_build() {
    let mut host = Host::new(QUIET);
    let workspace = WorkspaceId::generate();
    let name = instance("mhq-old");
    let guest = FakeGuest::bind(&host.socket(&name));
    host.guests.add(workspace, name, None);

    let (mut peer, _) = guest.accept("0.0.1-old", Vec::new()).await;
    peer.closed().await;
    // Back again too soon, still old, so it isn't installed twice.
    let (mut peer, _) = guest.accept("0.0.1-old", Vec::new()).await;
    peer.closed().await;
    let (_peer, _) = guest.accept(VERSION, Vec::new()).await;
    host.up(workspace).await;
    assert_eq!(
        host.upgrades.taken(),
        vec![("mhq-old".to_owned(), PathBuf::from("/opt/maxd-guest"))]
    );
}

/// A guest binary on this host that reports `build`, for the build
/// compare. Gives its path and its build as `maxd --build` would read it.
fn guest_binary_in(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("maxd-guest");
    std::fs::write(&path, "#!/bin/sh\necho 00000000000000aa\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[tokio::test]
async fn a_guest_on_another_build_gets_this_hosts_guest_binary() {
    let mut host = Host::installing(QUIET, guest_binary_in);
    let binary = host.temp.path().join("maxd-guest");
    let wanted = crate::identity::build_of(&binary).unwrap();
    let workspace = WorkspaceId::generate();
    let name = instance("mhq-stale");
    let mut guest = FakeGuest::bind(&host.socket(&name));
    host.guests.add(workspace, name, None);

    guest.build = Some("ffffffffffffffff".to_owned());
    let (mut peer, _) = guest.accept(VERSION, Vec::new()).await;
    peer.closed().await;
    assert_eq!(
        host.upgrades.taken(),
        vec![("mhq-stale".to_owned(), binary.clone())]
    );

    // Once it runs the same build, it links.
    guest.build = Some(wanted);
    let (_peer, _) = guest.accept(VERSION, Vec::new()).await;
    host.up(workspace).await;
    assert_eq!(host.upgrades.taken().len(), 1);
}

#[tokio::test]
async fn a_new_guest_binary_drops_each_link_to_compare_again() {
    let mut host = Host::installing(QUIET, guest_binary_in);
    let wanted = crate::identity::build_of(&host.temp.path().join("maxd-guest")).unwrap();
    let workspace = WorkspaceId::generate();
    let name = instance("mhq-linked");
    let mut guest = FakeGuest::bind(&host.socket(&name));
    host.guests.add(workspace, name, None);
    guest.build = Some(wanted);
    let (mut peer, _) = guest.accept(VERSION, Vec::new()).await;
    host.up(workspace).await;

    host.guests.refresh();
    peer.closed().await;
    // It comes back, and with the same build nothing is installed.
    let (_peer, _) = guest.accept(VERSION, Vec::new()).await;
    assert!(host.upgrades.taken().is_empty());
}

#[test]
fn a_guest_that_keeps_coming_back_wrong_waits_longer_each_time() {
    use super::link::{upgrade_wait, LastUpgrade};
    let mut now = std::time::Instant::now();
    let mut last = None;
    let mut waits = Vec::new();
    for _ in 0..7 {
        let wait = upgrade_wait(last.as_ref(), now);
        waits.push(wait.as_secs());
        last = Some(LastUpgrade::new(now, wait));
        // It's back as soon as it may be.
        now += wait;
    }
    assert_eq!(waits, [60, 120, 240, 480, 900, 900, 900]);
    // After a quiet spell, an upgrade is an ordinary one again.
    now += Duration::from_secs(3600);
    assert_eq!(upgrade_wait(last.as_ref(), now).as_secs(), 60);
}

#[test]
fn only_a_known_build_that_differs_counts() {
    use super::link::differs;
    use maxhq_protocol::guest::GUEST_PROTOCOL_VERSION as OURS;
    assert!(differs(OURS, VERSION, Some("a"), Some("a")).is_none());
    assert!(differs(OURS, VERSION, Some("a"), Some("b")).is_some());
    // A guest from before builds, or a host that can't read its binary,
    // goes by the version alone.
    assert!(differs(OURS, VERSION, None, Some("b")).is_none());
    assert!(differs(OURS, VERSION, Some("a"), None).is_none());
    assert!(differs(OURS, "0.0.1-old", Some("a"), Some("a")).is_some());
}

#[tokio::test]
async fn a_guest_that_stops_answering_pings_loses_its_link() {
    let mut host = Host::new(Duration::from_millis(100));
    let workspace = WorkspaceId::generate();
    let name = instance("mhq-quiet");
    let guest = FakeGuest::bind(&host.socket(&name));
    host.guests.add(workspace, name, None);
    let (mut peer, _) = guest.accept(VERSION, Vec::new()).await;
    host.up(workspace).await;
    for _ in 0..3 {
        let HostToGuest::Ping { nonce } = peer.recv().await else {
            panic!("expected a ping");
        };
        peer.send(&GuestToHost::Pong { nonce }).await;
    }
    peer.closed().await;
    assert_eq!(
        host.down(workspace).await,
        "the guest stopped answering pings"
    );
}

#[tokio::test]
async fn the_host_keeps_to_the_link_limits() {
    let mut host = Host::new(QUIET);
    let workspace = WorkspaceId::generate();
    let name = instance("mhq-limits");
    let guest = FakeGuest::bind(&host.socket(&name));
    host.guests.add(workspace, name, None);
    let (mut peer, _) = guest.accept(VERSION, Vec::new()).await;
    host.up(workspace).await;

    // Holder connections.
    let holders = host.guests.guest(workspace).unwrap();
    let session = SessionId::generate();
    let hello = HolderHello {
        holder_pid: 4242,
        child: ChildState::Running { pid: 4243 },
        first_seq: 1,
        last_seq: 0,
        pending: Vec::new(),
    };
    let mut open = Vec::new();
    for _ in 0..MAX_HOLDER_CONNECTIONS {
        let connect = tokio::spawn({
            let holders = holders.clone();
            async move { holders.connect(session, None).await }
        });
        let HostToGuest::Connect { conn, .. } = peer.recv().await else {
            panic!("expected a connect");
        };
        let event = HolderEvent::Hello(hello.clone());
        peer.send(&GuestToHost::Holder { conn, event }).await;
        let connection = within(connect).await.unwrap().unwrap();
        assert_eq!(connection.hello.holder_pid, 4242);
        open.push(connection);
    }
    let Err(ConnectError::Io(err)) = holders.connect(session, None).await else {
        panic!("expected the connect refused");
    };
    assert!(err.to_string().contains("holder connections"), "{err}");
    drop(open.pop());
    let HostToGuest::Disconnect { .. } = peer.recv().await else {
        panic!("expected a disconnect");
    };

    // History readers.
    let agent = AgentId::generate();
    let open_history = || {
        host.guests.history(
            workspace,
            Vendor::Codex,
            "/c".into(),
            "s".into(),
            agent,
            None,
        )
    };
    let mut histories = Vec::new();
    for _ in 0..MAX_HISTORIES {
        histories.push(open_history().expect("a history"));
        let HostToGuest::OpenHistory { .. } = peer.recv().await else {
            panic!("expected an open history");
        };
    }
    assert!(open_history().is_none());
    drop(histories.pop());
    let HostToGuest::CloseHistory { .. } = peer.recv().await else {
        panic!("expected a close history");
    };
    assert!(open_history().is_some());

    // Tunnels.
    peer.drain();
    let sender = host.guests.sender(workspace).unwrap();
    for _ in 0..MAX_TUNNELS {
        let open = HostToGuest::TunnelOpen {
            stream: StreamId::generate(),
            target: TunnelTarget::Port(3000),
        };
        sender.send(&open).await.unwrap();
    }
    let open = HostToGuest::TunnelOpen {
        stream: StreamId::generate(),
        target: TunnelTarget::Port(3000),
    };
    assert!(matches!(
        sender.send(&open).await,
        Err(SendError::TooManyTunnels)
    ));
}

/// An item in a history.
fn timeline_item(agent: AgentId) -> TimelineItem {
    TimelineItem {
        id: ItemId("item".into()),
        agent,
        turn: None,
        parent: None,
        ordinal: 0,
        status: ItemStatus::Completed,
        started_at: None,
        completed_at: None,
        body: ItemBody::AssistantMessage {
            text: "text".into(),
            streaming: false,
        },
    }
}

/// A line that takes 1 MiB of a link's unread room.
fn big_line() -> HolderLine {
    HolderLine {
        seq: 1,
        at: Timestamp::now(),
        text: "x".repeat(1024 * 1024),
        replayed: false,
    }
}

/// A holder's hello with nothing left to come.
fn plain_hello() -> HolderHello {
    HolderHello {
        holder_pid: 4242,
        child: ChildState::Running { pid: 4243 },
        first_seq: 1,
        last_seq: 0,
        pending: Vec::new(),
    }
}

/// A holder connection on `holders` that has had its hello, and its number.
async fn connected(holders: &Arc<dyn Holders>, peer: &mut Peer) -> (Connection, u64) {
    let connect = tokio::spawn({
        let holders = holders.clone();
        async move { holders.connect(SessionId::generate(), None).await }
    });
    let HostToGuest::Connect { conn, .. } = peer.recv().await else {
        panic!("expected a connect");
    };
    let event = HolderEvent::Hello(plain_hello());
    peer.send(&GuestToHost::Holder { conn, event }).await;
    (within(connect).await.unwrap().unwrap(), conn)
}

#[tokio::test]
async fn a_guest_that_outruns_what_the_host_reads_loses_its_link() {
    let mut host = Host::new(QUIET);
    let workspace = WorkspaceId::generate();
    let name = instance("mhq-flood");
    let guest = FakeGuest::bind(&host.socket(&name));
    host.guests.add(workspace, name, None);
    let agent = AgentId::generate();
    let open_history = |host: &Host| {
        host.guests
            .history(
                workspace,
                Vendor::Codex,
                "/c".into(),
                "s".into(),
                agent,
                None,
            )
            .expect("a history")
    };
    let megabytes = UNREAD_KIB / 1024 + 1;
    let unread = |reason: &str| assert!(reason.contains("hasn't read"), "{reason}");

    // Lines on a connection nobody reads.
    let (mut peer, _) = guest.accept(VERSION, Vec::new()).await;
    host.up(workspace).await;
    let holders = host.guests.guest(workspace).unwrap();
    let (_connection, conn) = connected(&holders, &mut peer).await;
    let event = HolderEvent::Line(big_line());
    let message = GuestToHost::Holder { conn, event };
    peer.flood(&message, megabytes).await;
    peer.closed().await;
    unread(&host.down(workspace).await);

    // A hello whose parts never end, which its connect holds as they come.
    let (mut peer, _) = guest.accept(VERSION, Vec::new()).await;
    host.up(workspace).await;
    let holders = host.guests.guest(workspace).unwrap();
    let connect = tokio::spawn(async move { holders.connect(SessionId::generate(), None).await });
    let HostToGuest::Connect { conn, .. } = peer.recv().await else {
        panic!("expected a connect");
    };
    let mut part = plain_hello();
    part.pending.push(big_line());
    let event = HolderEvent::Hello(part);
    let message = GuestToHost::Holder { conn, event };
    peer.flood(&message, megabytes).await;
    peer.closed().await;
    unread(&host.down(workspace).await);
    assert!(within(connect).await.unwrap().is_err());

    // Empty batches for a history nobody reads, which count as well.
    let (mut peer, _) = guest.accept(VERSION, Vec::new()).await;
    host.up(workspace).await;
    let _history = open_history(&host);
    let HostToGuest::OpenHistory { req, .. } = peer.recv().await else {
        panic!("expected an open history");
    };
    let message = GuestToHost::HistoryItems {
        req,
        items: Vec::new(),
    };
    peer.flood(&message, UNREAD_KIB + 1).await;
    peer.closed().await;
    unread(&host.down(workspace).await);

    // A first read of a history that never ends, which holds what came so far.
    let (mut peer, _) = guest.accept(VERSION, Vec::new()).await;
    host.up(workspace).await;
    let mut history = open_history(&host);
    let HostToGuest::OpenHistory { req, .. } = peer.recv().await else {
        panic!("expected an open history");
    };
    let read = tokio::task::spawn_blocking(move || history.read());
    let message = GuestToHost::HistoryItems {
        req,
        items: vec![timeline_item(agent)],
    };
    peer.flood(&message, UNREAD_KIB + 1).await;
    peer.closed().await;
    unread(&host.down(workspace).await);
    assert!(within(read).await.unwrap().is_err());
}

#[tokio::test]
async fn a_reader_that_keeps_up_keeps_its_link() {
    let mut host = Host::new(QUIET);
    let workspace = WorkspaceId::generate();
    let name = instance("mhq-steady");
    let guest = FakeGuest::bind(&host.socket(&name));
    host.guests.add(workspace, name, None);
    let (mut peer, _) = guest.accept(VERSION, Vec::new()).await;
    host.up(workspace).await;
    let holders = host.guests.guest(workspace).unwrap();
    let (mut connection, conn) = connected(&holders, &mut peer).await;
    for _ in 0..2 * (UNREAD_KIB / 1024) {
        let event = HolderEvent::Line(big_line());
        peer.send(&GuestToHost::Holder { conn, event }).await;
        assert!(within(connection.events.next()).await.is_some());
    }
    assert!(host.guests.guest(workspace).is_some());
}

#[tokio::test]
async fn waiting_for_a_link_ends_with_the_guests_hello() {
    let host = Host::new(QUIET);
    let workspace = WorkspaceId::generate();
    let name = instance("mhq-wait");
    let guest = FakeGuest::bind(&host.socket(&name));
    host.guests.open(workspace, &name, None);
    let wait = host.guests.wait_up(workspace, Duration::from_secs(10));
    let (up, _link) = within(async { tokio::join!(wait, guest.accept(VERSION, Vec::new())) }).await;
    assert_eq!(up, Ok(Some(VERSION.to_owned())));

    // A link that's up already counts at once.
    let again = within(host.guests.wait_up(workspace, Duration::from_secs(10))).await;
    assert_eq!(again, Ok(Some(VERSION.to_owned())));
}

#[tokio::test]
async fn waiting_for_a_link_gives_up_when_no_hello_comes() {
    let host = Host::new(QUIET);
    let workspace = WorkspaceId::generate();
    let name = instance("mhq-silent");
    // The guest's socket takes the connection, but nothing ever answers.
    let _guest = FakeGuest::bind(&host.socket(&name));
    host.guests.open(workspace, &name, None);
    let wait = host.guests.wait_up(workspace, Duration::from_secs(1));
    let reason = within(wait).await.unwrap_err();
    assert!(
        reason.starts_with("the guest didn't link within 1 s"),
        "{reason}"
    );

    // A link that's closed is down, so waiting fails again.
    host.guests.close(workspace);
    let wait = host.guests.wait_up(workspace, Duration::from_secs(1));
    assert!(within(wait).await.is_err());
}

#[tokio::test]
async fn a_deleted_workspace_is_gone_for_the_session_manager() {
    let mut host = Host::new(QUIET);
    let workspace = WorkspaceId::generate();
    let name = instance("mhq-deleted");
    let guest = FakeGuest::bind(&host.socket(&name));
    host.guests.open(workspace, &name, None);
    let (_peer, _) = guest.accept(VERSION, Vec::new()).await;
    host.up(workspace).await;

    // Its link drops first, then the agents that ran there are forgotten.
    host.guests.forget(workspace);
    assert_eq!(host.down(workspace).await, "the workspace was removed");
    assert_eq!(host.change().await, PlaceChange::Gone { workspace });

    // A workspace whose link was already closed is gone all the same.
    let closed = WorkspaceId::generate();
    host.guests.forget(closed);
    assert_eq!(host.change().await, PlaceChange::Gone { workspace: closed });
}

#[tokio::test]
async fn a_spawn_waits_for_the_guest_to_answer_a_forget() {
    let mut host = Host::new(QUIET);
    let workspace = WorkspaceId::generate();
    let name = instance("mhq-forget");
    let guest = FakeGuest::bind(&host.socket(&name));
    host.guests.add(workspace, name, None);
    let (mut peer, _) = guest.accept(VERSION, Vec::new()).await;
    host.up(workspace).await;

    let holders = host.guests.guest(workspace).unwrap();
    let ended = SessionId::generate();
    let connect = tokio::spawn({
        let holders = holders.clone();
        async move { holders.connect(ended, None).await }
    });
    let HostToGuest::Connect { conn, .. } = peer.recv().await else {
        panic!("expected a connect");
    };
    let hello = HolderHello {
        holder_pid: 4242,
        child: ChildState::Running { pid: 4243 },
        first_seq: 1,
        last_seq: 0,
        pending: Vec::new(),
    };
    let event = HolderEvent::Hello(hello);
    peer.send(&GuestToHost::Holder { conn, event }).await;
    let connection = within(connect).await.unwrap().unwrap();

    // The session manager asks for the forget, then spawns the agent's
    // next session at once.
    let forget = tokio::spawn(connection.writer.forget());
    let next = SessionId::generate();
    let spawn = tokio::spawn(holders.spawn(codex_launch(next)));
    let HostToGuest::Forget { req, session } = peer.recv().await else {
        panic!("expected the forget");
    };
    assert_eq!(session, ended);

    // Nothing else goes out until the guest answers it.
    let quiet = tokio::time::timeout(Duration::from_millis(300), peer.reader.next()).await;
    assert!(
        quiet.is_err(),
        "the spawn went out before the forget's answer"
    );
    assert!(!spawn.is_finished());

    peer.send(&GuestToHost::Done {
        req,
        result: Ok(()),
    })
    .await;
    assert!(within(forget).await.unwrap().is_ok());
    let HostToGuest::Spawn { req, launch } = peer.recv().await else {
        panic!("expected the spawn");
    };
    assert_eq!(launch.session, next);
    peer.send(&GuestToHost::SpawnResult {
        req,
        result: Ok(()),
    })
    .await;
    assert!(within(spawn).await.unwrap().is_ok());
}

#[tokio::test]
async fn a_spawn_the_guest_refused_reads_as_the_guest_said_it() {
    let mut host = Host::new(QUIET);
    let workspace = WorkspaceId::generate();
    let name = instance("mhq-refused");
    let guest = FakeGuest::bind(&host.socket(&name));
    host.guests.add(workspace, name, None);
    let (mut peer, _) = guest.accept(VERSION, Vec::new()).await;
    host.up(workspace).await;

    let holders = host.guests.guest(workspace).unwrap();
    let spawn = tokio::spawn(holders.spawn(codex_launch(SessionId::generate())));
    let HostToGuest::Spawn { req, .. } = peer.recv().await else {
        panic!("expected the spawn");
    };
    // The host adds nothing to the guest's reason.
    let gone = "the folder /home/agent/work/maxhq is gone";
    peer.send(&GuestToHost::SpawnResult {
        req,
        result: Err(gone.to_owned()),
    })
    .await;
    let err = within(spawn).await.unwrap().unwrap_err();
    assert_eq!(err.to_string(), gone);
}

/// A launch of Codex's app-server in a workspace.
fn codex_launch(session: SessionId) -> maxhq_hold::Launch {
    maxhq_hold::Launch {
        session,
        program: PathBuf::from("/usr/local/bin/codex"),
        args: Vec::new(),
        env: Vec::new(),
        env_remove: Vec::new(),
        secret_env: Vec::new(),
        run_as: None,
        cwd: PathBuf::from("/home/agent/work/maxhq"),
        framing: maxhq_hold::Framing::JsonRpc,
    }
}

#[tokio::test]
async fn a_picture_goes_to_the_guest_of_its_workspace() {
    let mut host = Host::new(QUIET);
    let workspace = WorkspaceId::generate();
    let name = instance("mhq-picture");
    let guest = FakeGuest::bind(&host.socket(&name));
    host.guests.add(workspace, name, None);
    let (mut peer, _) = guest.accept(VERSION, Vec::new()).await;
    host.up(workspace).await;

    let picture = |workspace| crate::drive::pictures::ForGuest {
        workspace,
        agent: AgentId::derive("lead"),
        media_type: "image/png".into(),
        data: "iVBORw0KGgo=".into(),
    };
    // One for a workspace with no link goes nowhere.
    host.guests.keep_picture(picture(WorkspaceId::generate()));
    host.guests.keep_picture(picture(workspace));
    let HostToGuest::KeepPicture {
        agent,
        media_type,
        data,
    } = peer.recv().await
    else {
        panic!("expected the picture");
    };
    assert_eq!(agent, AgentId::derive("lead"));
    assert_eq!(media_type, "image/png");
    assert_eq!(data, "iVBORw0KGgo=");
}

#[tokio::test]
async fn a_path_search_runs_in_the_guest_and_its_answer_is_checked() {
    let mut host = Host::new(QUIET);
    let workspace = WorkspaceId::generate();
    let name = instance("mhq-paths");
    let guest = FakeGuest::bind(&host.socket(&name));
    host.guests.add(workspace, name, None);
    let (mut peer, _) = guest.accept(VERSION, Vec::new()).await;
    host.up(workspace).await;

    let one = |path: &str| PathMatch {
        path: path.into(),
        kind: PathKind::File,
    };
    let search = host
        .guests
        .find_paths(workspace, "/path/to/work".into(), "main".into());
    let answer = async {
        let HostToGuest::FindPaths { req, cwd, query } = peer.recv().await else {
            panic!("expected the search");
        };
        assert_eq!((cwd.as_str(), query.as_str()), ("/path/to/work", "main"));
        peer.send(&GuestToHost::FoundPaths {
            req,
            result: Ok(vec![one("src/main.rs")]),
        })
        .await;
    };
    let (found, ()) = within(async { tokio::join!(search, answer) }).await;
    assert_eq!(found.unwrap(), [one("src/main.rs")]);

    // A path that climbs out of the folder fails the whole answer.
    let search = host
        .guests
        .find_paths(workspace, "/path/to/work".into(), "up".into());
    let answer = async {
        let HostToGuest::FindPaths { req, .. } = peer.recv().await else {
            panic!("expected the search");
        };
        peer.send(&GuestToHost::FoundPaths {
            req,
            result: Ok(vec![one("../secret.txt")]),
        })
        .await;
    };
    let (found, ()) = within(async { tokio::join!(search, answer) }).await;
    assert!(found.is_err());

    // A workspace with no link says so.
    let found = host
        .guests
        .find_paths(WorkspaceId::generate(), "/path/to".into(), "x".into())
        .await;
    assert!(found.is_err());
}

fn running(pid: u32, start: u64) -> AgentProcess {
    AgentProcess {
        pid,
        parent: 50,
        start,
        run_ms: 1000,
        cpu_ms: 10,
        memory: 4096,
        name: "node".into(),
        command: "node vite".into(),
        cli: false,
        ports: vec![5173],
    }
}

#[tokio::test]
async fn a_process_list_goes_only_to_a_guest_new_enough_and_its_answer_is_checked() {
    let mut host = Host::new(QUIET);
    let workspace = WorkspaceId::generate();
    let name = instance("mhq-running");
    let guest = FakeGuest::bind(&host.socket(&name));
    host.guests.add(workspace, name, None);
    let (mut peer, _) = guest.accept(VERSION, Vec::new()).await;
    host.up(workspace).await;
    let holder = WatchedHolder {
        session: SessionId::derive("s"),
        agent: None,
        pid: 50,
    };
    let ask = HostToGuest::ListProcesses {
        req: 0,
        holder: holder.clone(),
    };

    if ask.since() > GUEST_PROTOCOL_VERSION {
        // A guest on an older protocol is never asked, and its link stays.
        let listed = within(host.guests.list_processes(workspace, holder.clone())).await;
        assert_eq!(listed, Ok(ProcessList::TooOld));
        let stop = host
            .guests
            .signal_process(workspace, holder, 61, 210, ProcessSignal::Term);
        assert!(within(stop).await.unwrap_err().contains("too old"));
        let search = host
            .guests
            .find_paths(workspace, "/path/to/work".into(), "main".into());
        let answer = async {
            let HostToGuest::FindPaths { req, .. } = peer.recv().await else {
                panic!("the guest was sent something it can't read");
            };
            peer.send(&GuestToHost::FoundPaths {
                req,
                result: Ok(Vec::new()),
            })
            .await;
        };
        let (found, ()) = within(async { tokio::join!(search, answer) }).await;
        assert_eq!(found.unwrap(), []);
        return;
    }

    let list = host.guests.list_processes(workspace, holder.clone());
    let answer = async {
        let HostToGuest::ListProcesses { req, holder: asked } = peer.recv().await else {
            panic!("expected the list");
        };
        assert_eq!(asked.pid, 50);
        let mut long = running(61, 210);
        long.command = "x".repeat(100_000);
        peer.send(&GuestToHost::Processes {
            req,
            result: Ok(vec![long, running(51, 100)]),
        })
        .await;
    };
    let (listed, ()) = within(async { tokio::join!(list, answer) }).await;
    let Ok(ProcessList::Listed { processes }) = listed else {
        panic!("expected a list, got {listed:?}");
    };
    let pids: Vec<u32> = processes.iter().map(|process| process.pid).collect();
    assert_eq!(pids, [51, 61], "oldest first");
    assert!(
        processes[1].command.len() < 100_000,
        "the command wasn't cut short"
    );

    let stop = host
        .guests
        .signal_process(workspace, holder, 61, 210, ProcessSignal::Kill);
    let answer = async {
        let HostToGuest::SignalProcess {
            req,
            pid,
            start,
            signal,
            ..
        } = peer.recv().await
        else {
            panic!("expected the signal");
        };
        assert_eq!((pid, start, signal), (61, 210, ProcessSignal::Kill));
        peer.send(&GuestToHost::Signalled {
            req,
            result: Err("that process has already exited".into()),
        })
        .await;
    };
    let (stopped, ()) = within(async { tokio::join!(stop, answer) }).await;
    assert_eq!(stopped, Err("that process has already exited".to_owned()));
}
