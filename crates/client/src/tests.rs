// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Links against a fake `maxd`: the client's end of a socket pair, or of a
//! real Unix socket, with the test playing `maxd` frame by frame.

use std::collections::HashMap;
use std::io;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use futures::future::BoxFuture;
use maxhq_protocol::frame::{self, Frame, FrameDecoder};
use maxhq_protocol::ids::{AgentId, BootId, DeviceId, InstanceId, NetworkId};
use maxhq_protocol::link::{ConnectStep, NetworkKind, NetworkState};
use maxhq_protocol::message::{Capabilities, ServerHello};
use maxhq_protocol::state::{Delta, Entity, EntityKey, Seq, Snapshot};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::{ProtocolRange, ServerMessage};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(feature = "system-ssh")]
use tokio::net::UnixListener;
use tokio::net::UnixStream;

use super::*;
use crate::backoff::BackoffLimits;
use crate::ssh_config::SshConfig;
use crate::tailnet::tests::{peer, tailnet};
use crate::tailnet::TailnetPeer;
use crate::transport::{Conn, DialError, Transport};

mod device;
mod forwards;
mod life;
#[cfg(feature = "russh")]
mod phone;
mod proof;
mod requests;
mod routes;
mod transfers;
#[cfg(feature = "ws")]
mod ws;

/// Timings short enough for tests. Retries come quickly, and any drop
/// counts as a drop.
const FAST: Timing = Timing {
    backoff: BackoffLimits {
        base: Duration::from_millis(20),
        cap: Duration::from_millis(40),
        long_cap: Duration::from_millis(40),
        long_after: Duration::from_secs(3600),
    },
    handshake: Duration::from_secs(2),
    ping_every: Duration::from_secs(60),
    silence: Duration::from_secs(180),
    probe: Duration::from_millis(300),
    resync: Duration::from_secs(2),
    reboot_window: Duration::from_secs(600),
    stable: Duration::ZERO,
    reply: Duration::from_secs(4),
    wind_down_retry: Duration::from_millis(20),
    wind_down_window: Duration::from_secs(600),
};

/// Like [`FAST`], but timed retries never come within a test, so anything
/// that dials again was triggered.
const NO_RETRY: Timing = Timing {
    backoff: BackoffLimits {
        base: Duration::from_secs(3600),
        cap: Duration::from_secs(3600),
        long_cap: Duration::from_secs(3600),
        long_after: Duration::from_secs(3600),
    },
    ..FAST
};

/// How long a test waits for anything before it fails.
const PATIENCE: Duration = Duration::from_secs(5);

/// A transport whose dials hand the far end of a socket pair to the test.
struct Fake {
    /// What `ssh -G` says, by destination.
    configs: HashMap<String, SshConfig>,
    /// Where the far ends go.
    servers: mpsc::UnboundedSender<UnixStream>,
    /// Set to make dials fail.
    down: Mutex<Option<DialError>>,
    /// How many dials there were.
    dials: AtomicUsize,
    /// Hosts whose master was dropped.
    forgotten: Mutex<Vec<String>>,
    /// Hosts whose dials fail.
    down_hosts: Mutex<Vec<String>>,
    /// The route of each dial, in order.
    dialed: Mutex<Vec<Route>>,
}

impl Fake {
    /// Makes dials fail, or work again with none.
    fn set_down(&self, err: Option<DialError>) {
        *self.down.lock().unwrap() = err;
    }

    fn dials(&self) -> usize {
        self.dials.load(Ordering::SeqCst)
    }

    /// Makes dials to `host` fail.
    fn set_host_down(&self, host: &str) {
        self.down_hosts.lock().unwrap().push(host.to_owned());
    }

    /// The route of each dial so far.
    fn dialed(&self) -> Vec<Route> {
        self.dialed.lock().unwrap().clone()
    }
}

impl Transport for Fake {
    fn resolve(&self, destination: &str) -> BoxFuture<'static, io::Result<SshConfig>> {
        let found = self.configs.get(destination).cloned();
        Box::pin(async move { found.ok_or_else(|| io::Error::other("no such host")) })
    }

    fn lookup(&self, _host: &str) -> BoxFuture<'static, Vec<IpAddr>> {
        Box::pin(async { Vec::new() })
    }

    fn dial<'a>(
        &'a self,
        route: &'a Route,
        _config: Option<&'a SshConfig>,
        _device: Option<DeviceId>,
    ) -> BoxFuture<'a, Result<Conn, DialError>> {
        Box::pin(async move {
            self.dials.fetch_add(1, Ordering::SeqCst);
            self.dialed.lock().unwrap().push(route.clone());
            let down = self.down.lock().unwrap().clone();
            if let Some(err) = down {
                return Err(err);
            }
            if let Route::Ssh { host } = route
                && self.down_hosts.lock().unwrap().contains(host)
            {
                return Err(DialError::down("no route to the host"));
            }
            let (ours, theirs) = UnixStream::pair().expect("a socket pair");
            let _ = self.servers.send(theirs);
            let (reader, writer) = ours.into_split();
            Ok(Conn::new(Box::new(reader), Box::new(writer)))
        })
    }

    fn forget<'a>(&'a self, host: &'a str) -> BoxFuture<'a, ()> {
        self.forgotten.lock().unwrap().push(host.to_owned());
        Box::pin(async {})
    }
}

/// A client over a [`Fake`], and the levers a test pulls.
struct Rig {
    client: Client,
    fake: Arc<Fake>,
    servers: mpsc::UnboundedReceiver<UnixStream>,
    tailnet: watch::Sender<Arc<Tailnet>>,
}

impl Rig {
    fn new(timing: Timing, view: Tailnet, configs: &[(&str, SshConfig)]) -> Self {
        let (servers_tx, servers) = mpsc::unbounded_channel();
        let fake = Arc::new(Fake {
            configs: configs
                .iter()
                .map(|(name, config)| ((*name).to_owned(), config.clone()))
                .collect(),
            servers: servers_tx,
            down: Mutex::new(None),
            dials: AtomicUsize::new(0),
            forgotten: Mutex::new(Vec::new()),
            down_hosts: Mutex::new(Vec::new()),
            dialed: Mutex::new(Vec::new()),
        });
        let (tailnet, tailnet_rx) = watch::channel(Arc::new(view));
        let (_, networks) = watch::channel(Vec::new());
        let transport: Arc<dyn Transport> = fake.clone();
        let client = Client::with_parts(transport, tailnet_rx, networks, timing);
        Self {
            client,
            fake,
            servers,
            tailnet,
        }
    }

    /// The `maxd` behind the next dial.
    async fn next_maxd(&mut self) -> FakeMaxd {
        let stream = tokio::time::timeout(PATIENCE, self.servers.recv())
            .await
            .expect("a dial in time")
            .expect("the transport is there");
        FakeMaxd::new(stream)
    }
}

/// The test's side of a connection: frames in and out.
struct FakeMaxd {
    stream: UnixStream,
    decoder: FrameDecoder,
}

impl FakeMaxd {
    fn new(stream: UnixStream) -> Self {
        Self {
            stream,
            decoder: FrameDecoder::new(),
        }
    }

    /// The client's next message, or none once it closes.
    async fn recv(&mut self) -> Option<ClientMessage> {
        let mut buf = [0; 4096];
        loop {
            if let Some(Frame::Message(json)) = self.decoder.next_frame().unwrap() {
                return Some(serde_json::from_slice(json).unwrap());
            }
            let read = tokio::time::timeout(PATIENCE, self.stream.read(&mut buf))
                .await
                .expect("a message in time");
            match read {
                Ok(0) | Err(_) => return None,
                Ok(n) => self.decoder.push(&buf[..n]),
            }
        }
    }

    async fn send(&mut self, message: &ServerMessage) {
        let mut out = Vec::new();
        frame::encode_message(&mut out, message).unwrap();
        self.stream.write_all(&out).await.unwrap();
    }

    /// Takes the client's hello, answers it, and returns the `since` of
    /// the sync that follows.
    async fn accept(&mut self, instance: InstanceId, boot: &str) -> Option<Seq> {
        self.accept_with(instance, boot, Capabilities::default())
            .await
    }

    /// Like [`FakeMaxd::accept`], for a device that does `capabilities`.
    async fn accept_with(
        &mut self,
        instance: InstanceId,
        boot: &str,
        capabilities: Capabilities,
    ) -> Option<Seq> {
        self.accept_as(device(), instance, boot, capabilities).await
    }

    /// Like [`FakeMaxd::accept_with`], as the device `id`.
    async fn accept_as(
        &mut self,
        id: DeviceId,
        instance: InstanceId,
        boot: &str,
        capabilities: Capabilities,
    ) -> Option<Seq> {
        let hello = match self.recv().await {
            Some(ClientMessage::Hello(hello)) => hello,
            other => panic!("expected a hello, got {other:?}"),
        };
        assert_eq!(hello.protocol, ProtocolRange::OURS);
        assert_eq!(hello.client.name, "maxhq-client");
        self.send(&ServerMessage::Hello(ServerHello {
            protocol: ProtocolRange::OURS,
            maxd_version: "0.1.0".into(),
            device: id,
            boot_id: BootId(boot.into()),
            instance,
            capabilities,
            build: None,
            target: None,
            challenge: None,
        }))
        .await;
        match self.recv().await {
            Some(ClientMessage::Sync { since }) => since,
            other => panic!("expected a sync, got {other:?}"),
        }
    }
}

fn device() -> DeviceId {
    DeviceId::derive("test-device")
}

fn spec(route: Route) -> LinkSpec {
    LinkSpec {
        id: LinkId::generate(),
        name: "Test box".into(),
        route,
        fallbacks: Vec::new(),
        last_good: BTreeMap::new(),
    }
}

fn ssh(host: &str) -> Route {
    Route::Ssh { host: host.into() }
}

/// A small entity, told apart by `n`.
fn entity(n: u32) -> Entity {
    Entity::Network(Network {
        id: NetworkId::derive(&format!("test:{n}")),
        kind: NetworkKind::Tailscale {
            magic_dns_suffix: None,
            client_version: None,
        },
        name: format!("net {n}"),
        state: NetworkState::Up,
    })
}

fn upsert(n: u32) -> Change {
    Change::Upsert { value: entity(n) }
}

fn remove(n: u32) -> Change {
    Change::Remove {
        key: EntityKey::Network(NetworkId::derive(&format!("test:{n}"))),
    }
}

fn snapshot(seq: u64, entities: &[u32]) -> ServerMessage {
    ServerMessage::Snapshot(Snapshot {
        seq: Seq(seq),
        entities: entities.iter().copied().map(entity).collect(),
    })
}

fn delta(seq: u64, changes: Vec<Change>) -> ServerMessage {
    ServerMessage::Delta(Delta {
        seq: Seq(seq),
        changes,
    })
}

fn mark_seen() -> ClientMessage {
    ClientMessage::MarkSeen {
        agent: AgentId::derive("test-agent"),
    }
}

fn connecting(step: ConnectStep, attempt: u32) -> LinkState {
    LinkState::Connecting { step, attempt }
}

fn unreachable(reason: &str, last_seen: Option<Timestamp>) -> LinkState {
    LinkState::Unreachable {
        reason: reason.into(),
        last_seen,
    }
}

async fn next(events: &mut mpsc::Receiver<LinkEvent>) -> LinkEvent {
    tokio::time::timeout(PATIENCE, events.recv())
        .await
        .expect("an event in time")
        .expect("the link is open")
}

async fn next_link(events: &mut mpsc::Receiver<LinkEvent>) -> DeviceLink {
    match next(events).await {
        LinkEvent::Link(link) => *link,
        other => panic!("expected the link, got {other:?}"),
    }
}

async fn next_state(events: &mut mpsc::Receiver<LinkEvent>) -> LinkState {
    next_link(events).await.state
}

fn is_connected(state: &LinkState) -> bool {
    matches!(state, LinkState::Connected { .. })
}

/// The link's states until it's connected or unreachable.
#[cfg(any(feature = "russh", feature = "ws"))]
async fn settled(events: &mut mpsc::Receiver<LinkEvent>) -> LinkState {
    loop {
        let state = next_state(events).await;
        if matches!(
            state,
            LinkState::Connected { .. } | LinkState::Unreachable { .. }
        ) {
            return state;
        }
    }
}

/// The reason an unreachable link gives.
#[cfg(any(feature = "russh", feature = "ws"))]
async fn refused_with(events: &mut mpsc::Receiver<LinkEvent>) -> String {
    match settled(events).await {
        LinkState::Unreachable { reason, .. } => reason,
        other => panic!("expected the link unreachable, got {other:?}"),
    }
}

/// Whether two entity lists hold the same entities in any order.
fn same(a: &[Entity], b: &[Entity]) -> bool {
    a.len() == b.len() && a.iter().all(|entity| b.contains(entity))
}

/// Opens a local link and connects it to a fresh fake `maxd`.
async fn connected(
    rig: &mut Rig,
    instance: InstanceId,
) -> (LinkHandle, mpsc::Receiver<LinkEvent>, FakeMaxd) {
    let (handle, mut events) = rig.client.link(spec(Route::Local));
    assert_eq!(next_state(&mut events).await, LinkState::Unknown);
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Dialing, 1)
    );
    let mut maxd = rig.next_maxd().await;
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Handshake, 1)
    );
    assert_eq!(maxd.accept(instance, "boot-1").await, None);
    assert!(is_connected(&next_state(&mut events).await));
    (handle, events, maxd)
}

#[tokio::test]
async fn a_new_link_says_hello_then_syncs_and_streams_changes() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    let (handle, mut events) = rig.client.link(spec(Route::Local));
    assert_eq!(next_state(&mut events).await, LinkState::Unknown);
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Dialing, 1)
    );
    let mut maxd = rig.next_maxd().await;
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Handshake, 1)
    );
    assert_eq!(maxd.accept(InstanceId::generate(), "boot-1").await, None);
    let link = next_link(&mut events).await;
    assert!(is_connected(&link.state));
    assert_eq!(link.device, Some(device()));

    maxd.send(&snapshot(5, &[1, 2])).await;
    match next(&mut events).await {
        LinkEvent::Synced(entities) => assert!(same(&entities, &[entity(1), entity(2)])),
        other => panic!("expected a sync, got {other:?}"),
    }
    maxd.send(&delta(6, vec![upsert(3)])).await;
    assert_eq!(next(&mut events).await, LinkEvent::Changed(vec![upsert(3)]));
    // A delta the replica already holds says nothing.
    maxd.send(&delta(6, vec![upsert(3)])).await;
    maxd.send(&delta(7, vec![remove(1)])).await;
    assert_eq!(next(&mut events).await, LinkEvent::Changed(vec![remove(1)]));

    drop(handle);
    assert_eq!(maxd.recv().await, None, "the link closes with its handle");
}

#[tokio::test]
async fn a_gap_in_deltas_syncs_again_from_the_last_seq() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    let (handle, mut events, mut maxd) = connected(&mut rig, InstanceId::generate()).await;
    maxd.send(&snapshot(5, &[1])).await;
    assert!(matches!(next(&mut events).await, LinkEvent::Synced(_)));

    maxd.send(&delta(7, vec![upsert(3)])).await;
    assert_eq!(
        maxd.recv().await,
        Some(ClientMessage::Sync {
            since: Some(Seq(5))
        })
    );
    // Another gap while that sync is out doesn't ask again.
    maxd.send(&delta(8, vec![upsert(4)])).await;
    maxd.send(&delta(6, vec![upsert(2)])).await;
    maxd.send(&delta(7, vec![upsert(3)])).await;
    assert_eq!(next(&mut events).await, LinkEvent::Changed(vec![upsert(2)]));
    assert_eq!(next(&mut events).await, LinkEvent::Changed(vec![upsert(3)]));
    handle.send(mark_seen()).await.unwrap();
    assert_eq!(maxd.recv().await, Some(mark_seen()));
}

#[tokio::test]
async fn a_gap_that_a_resync_doesnt_close_drops_the_connection() {
    let timing = Timing {
        resync: Duration::from_millis(100),
        ..FAST
    };
    let mut rig = Rig::new(timing, Tailnet::default(), &[]);
    let (_handle, mut events, mut maxd) = connected(&mut rig, InstanceId::generate()).await;
    maxd.send(&snapshot(5, &[1])).await;
    assert!(matches!(next(&mut events).await, LinkEvent::Synced(_)));
    maxd.send(&delta(9, vec![upsert(3)])).await;
    assert_eq!(
        maxd.recv().await,
        Some(ClientMessage::Sync {
            since: Some(Seq(5))
        })
    );
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Dialing, 1)
    );
}

#[tokio::test]
async fn a_reconnect_to_the_same_maxd_catches_up_from_its_seq() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    let instance = InstanceId::generate();
    let (_handle, mut events, mut maxd) = connected(&mut rig, instance).await;
    maxd.send(&snapshot(5, &[1])).await;
    assert!(matches!(next(&mut events).await, LinkEvent::Synced(_)));

    drop(maxd);
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Dialing, 1)
    );
    let mut maxd = rig.next_maxd().await;
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Handshake, 1)
    );
    assert_eq!(maxd.accept(instance, "boot-1").await, Some(Seq(5)));
    assert!(is_connected(&next_state(&mut events).await));
    maxd.send(&delta(6, vec![upsert(2)])).await;
    assert_eq!(next(&mut events).await, LinkEvent::Changed(vec![upsert(2)]));
}

#[tokio::test]
async fn a_restarted_maxd_syncs_from_nothing() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    let (_handle, mut events, mut maxd) = connected(&mut rig, InstanceId::generate()).await;
    maxd.send(&snapshot(5, &[1])).await;
    assert!(matches!(next(&mut events).await, LinkEvent::Synced(_)));

    drop(maxd);
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Dialing, 1)
    );
    let mut maxd = rig.next_maxd().await;
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Handshake, 1)
    );
    assert_eq!(maxd.accept(InstanceId::generate(), "boot-1").await, None);
    assert!(is_connected(&next_state(&mut events).await));
    maxd.send(&snapshot(1, &[2])).await;
    assert_eq!(next(&mut events).await, LinkEvent::Synced(vec![entity(2)]));
}

#[tokio::test]
async fn requests_go_out_only_while_connected() {
    let mut rig = Rig::new(NO_RETRY, Tailnet::default(), &[]);
    rig.fake
        .set_down(Some(DialError::refused("maxd isn't running")));
    let (handle, mut events) = rig.client.link(spec(Route::Local));
    assert_eq!(next_state(&mut events).await, LinkState::Unknown);
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Dialing, 1)
    );
    assert_eq!(
        next_state(&mut events).await,
        unreachable("maxd isn't running", None)
    );
    assert_eq!(handle.send(mark_seen()).await, Err(LinkError::NotConnected));

    rig.fake.set_down(None);
    handle.refresh();
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Dialing, 2)
    );
    let mut maxd = rig.next_maxd().await;
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Handshake, 2)
    );
    assert_eq!(maxd.accept(InstanceId::generate(), "boot-1").await, None);
    assert!(is_connected(&next_state(&mut events).await));

    handle.send(mark_seen()).await.unwrap();
    assert_eq!(maxd.recv().await, Some(mark_seen()));
    // Syncs are the link's own business.
    assert_eq!(
        handle.send(ClientMessage::Sync { since: None }).await,
        Err(LinkError::NotConnected)
    );

    // With nobody reading events the link stops, and says so.
    drop(events);
    maxd.send(&snapshot(1, &[1])).await;
    let closed = async {
        while handle.send(mark_seen()).await != Err(LinkError::Closed) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::time::timeout(PATIENCE, closed)
        .await
        .expect("the link closes");
}

#[tokio::test]
async fn timed_retries_stay_unreachable_until_one_works() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    rig.fake
        .set_down(Some(DialError::down("no route to the host")));
    let (_handle, mut events) = rig.client.link(spec(Route::Local));
    assert_eq!(next_state(&mut events).await, LinkState::Unknown);
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Dialing, 1)
    );
    assert_eq!(
        next_state(&mut events).await,
        unreachable("no route to the host", None)
    );
    let retried = async {
        while rig.fake.dials() < 4 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    tokio::time::timeout(PATIENCE, retried)
        .await
        .expect("timed retries");
    assert!(events.try_recv().is_err(), "retries change nothing");

    rig.fake.set_down(None);
    let mut maxd = rig.next_maxd().await;
    assert_eq!(maxd.accept(InstanceId::generate(), "boot-1").await, None);
    assert!(is_connected(&next_state(&mut events).await));
}

#[tokio::test]
async fn an_offline_peer_isnt_dialed_until_it_comes_online() {
    let seen = Timestamp(1_790_000_000_000);
    let offline = |online: bool| -> TailnetPeer {
        let mut box_peer = peer("box", "box.tail1234.ts.net", "100.101.1.9", online);
        box_peer.peer.last_seen = Some(seen);
        box_peer
    };
    let config = SshConfig {
        host_name: "box".into(),
        ..SshConfig::default()
    };
    let mut rig = Rig::new(NO_RETRY, tailnet(vec![offline(false)]), &[("box", config)]);
    let network = rig
        .tailnet
        .borrow()
        .network
        .as_ref()
        .map(|network| network.id);
    let (_handle, mut events) = rig.client.link(spec(ssh("box")));
    assert_eq!(next_state(&mut events).await, LinkState::Unknown);
    let link = next_link(&mut events).await;
    assert_eq!(link.network, network);
    assert_eq!(link.peer.as_ref().map(|peer| peer.online), Some(false));
    assert!(link.via.is_empty());
    assert_eq!(
        next_state(&mut events).await,
        unreachable("offline", Some(seen))
    );
    assert_eq!(rig.fake.dials(), 0);

    rig.tailnet
        .send_replace(Arc::new(tailnet(vec![offline(true)])));
    let link = next_link(&mut events).await;
    assert_eq!(link.peer.as_ref().map(|peer| peer.online), Some(true));
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Dialing, 2)
    );
    let mut maxd = rig.next_maxd().await;
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Handshake, 2)
    );
    assert_eq!(maxd.accept(InstanceId::generate(), "boot-1").await, None);
    assert!(is_connected(&next_state(&mut events).await));
}

#[tokio::test]
async fn tailscale_being_off_is_the_reason() {
    let mut off = tailnet(Vec::new());
    if let Some(network) = off.network.as_mut() {
        network.state = NetworkState::Off;
    }
    let mut rig = Rig::new(NO_RETRY, off, &[]);
    let (_handle, mut events) = rig.client.link(spec(ssh("box.tail1234.ts.net")));
    assert_eq!(next_state(&mut events).await, LinkState::Unknown);
    let link = next_link(&mut events).await;
    assert!(link.network.is_some(), "the MagicDNS name places it");
    assert_eq!(link.peer, None);
    assert_eq!(
        next_state(&mut events).await,
        unreachable("Tailscale is off", None)
    );
    assert_eq!(rig.fake.dials(), 0);

    let on = tailnet(vec![peer(
        "box",
        "box.tail1234.ts.net",
        "100.101.1.9",
        true,
    )]);
    rig.tailnet.send_replace(Arc::new(on));
    let link = next_link(&mut events).await;
    assert_eq!(
        link.peer.as_ref().map(|peer| peer.host_name.as_str()),
        Some("box")
    );
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Dialing, 2)
    );
    let mut maxd = rig.next_maxd().await;
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Handshake, 2)
    );
    assert_eq!(maxd.accept(InstanceId::generate(), "boot-1").await, None);
    assert!(is_connected(&next_state(&mut events).await));
}

#[tokio::test]
async fn a_plain_name_is_dialed_once_tailscale_is_off() {
    let config = SshConfig {
        host_name: "devbox".into(),
        ..SshConfig::default()
    };
    let devbox = peer("devbox", "devbox.tail1234.ts.net", "100.101.1.2", false);
    let mut rig = Rig::new(NO_RETRY, tailnet(vec![devbox]), &[("devbox", config)]);
    let (_handle, mut events) = rig.client.link(spec(ssh("devbox")));
    assert_eq!(next_state(&mut events).await, LinkState::Unknown);
    let link = next_link(&mut events).await;
    assert!(link.network.is_some(), "the name matches a peer");
    assert_eq!(next_state(&mut events).await, unreachable("offline", None));
    assert_eq!(rig.fake.dials(), 0);

    // The home network may still reach it by that name.
    let mut off = tailnet(Vec::new());
    if let Some(network) = off.network.as_mut() {
        network.state = NetworkState::Off;
    }
    rig.tailnet.send_replace(Arc::new(off));
    let link = next_link(&mut events).await;
    assert!(link.network.is_some(), "it keeps the network it had");
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Dialing, 2)
    );
    let mut maxd = rig.next_maxd().await;
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Handshake, 2)
    );
    assert_eq!(maxd.accept(InstanceId::generate(), "boot-1").await, None);
    assert!(is_connected(&next_state(&mut events).await));
}

#[tokio::test]
async fn a_hop_that_goes_offline_is_the_reason() {
    let workspace = SshConfig {
        user: Some("agent".into()),
        host_name: "ws-bravo".into(),
        proxy_command: Some("ssh dev@devbox ws-proxy %h".into()),
        ..SshConfig::default()
    };
    let node = SshConfig {
        user: Some("dev".into()),
        host_name: "devbox".into(),
        ..SshConfig::default()
    };
    let devbox = |online| peer("devbox", "devbox.tail1234.ts.net", "100.101.1.2", online);
    let mut rig = Rig::new(
        NO_RETRY,
        tailnet(vec![devbox(true)]),
        &[("ws-bravo", workspace), ("dev@devbox", node)],
    );
    let (_handle, mut events) = rig.client.link(spec(ssh("ws-bravo")));
    assert_eq!(next_state(&mut events).await, LinkState::Unknown);
    let link = next_link(&mut events).await;
    assert_eq!(link.via, ["dev@devbox"]);
    assert!(link.network.is_some(), "the hop is on the tailnet");
    assert_eq!(link.peer, None, "the workspace itself isn't a peer");
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Dialing, 1)
    );
    let mut maxd = rig.next_maxd().await;
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Handshake, 1)
    );
    maxd.accept(InstanceId::generate(), "boot-1").await;
    assert!(is_connected(&next_state(&mut events).await));

    rig.tailnet
        .send_replace(Arc::new(tailnet(vec![devbox(false)])));
    drop(maxd);
    match next_state(&mut events).await {
        LinkState::Unreachable { reason, last_seen } => {
            assert_eq!(reason, "dev@devbox is offline");
            assert!(last_seen.is_some(), "it was connected a moment ago");
        }
        other => panic!("expected unreachable, got {other:?}"),
    }
    assert_eq!(rig.fake.dials(), 1);

    rig.tailnet
        .send_replace(Arc::new(tailnet(vec![devbox(true)])));
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Dialing, 2)
    );
}

#[tokio::test]
async fn a_device_that_drops_and_stops_answering_is_waiting_for_reboot() {
    let config = SshConfig {
        host_name: "box".into(),
        ..SshConfig::default()
    };
    let mut rig = Rig::new(FAST, Tailnet::default(), &[("box", config)]);
    let (_handle, mut events) = rig.client.link(spec(ssh("box")));
    assert_eq!(next_state(&mut events).await, LinkState::Unknown);
    // The route resolved, but nothing about the link changed.
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Dialing, 1)
    );
    let mut maxd = rig.next_maxd().await;
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Handshake, 1)
    );
    maxd.accept(InstanceId::generate(), "boot-1").await;
    assert!(is_connected(&next_state(&mut events).await));

    rig.fake.set_down(Some(DialError::down("SSH timed out")));
    drop(maxd);
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Dialing, 1)
    );
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::WaitingForReboot, 1)
    );
    let retried = async {
        while rig.fake.dials() < 4 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    tokio::time::timeout(PATIENCE, retried)
        .await
        .expect("timed retries");

    rig.fake.set_down(None);
    let mut maxd = rig.next_maxd().await;
    assert_eq!(maxd.accept(InstanceId::generate(), "boot-2").await, None);
    loop {
        match next_state(&mut events).await {
            LinkState::Connecting {
                step: ConnectStep::WaitingForReboot,
                ..
            } => {}
            state => {
                assert!(is_connected(&state), "expected connected, got {state:?}");
                break;
            }
        }
    }
}

#[tokio::test]
async fn a_maxd_with_no_protocol_in_common_is_refused() {
    let mut rig = Rig::new(NO_RETRY, Tailnet::default(), &[]);
    let (_handle, mut events) = rig.client.link(spec(Route::Local));
    assert_eq!(next_state(&mut events).await, LinkState::Unknown);
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Dialing, 1)
    );
    let mut maxd = rig.next_maxd().await;
    assert!(matches!(maxd.recv().await, Some(ClientMessage::Hello(_))));
    let ahead = ProtocolRange::OURS.current + 10;
    maxd.send(&ServerMessage::Hello(ServerHello {
        protocol: ProtocolRange {
            current: ahead,
            oldest: ahead,
        },
        maxd_version: "9.0.0".into(),
        device: device(),
        boot_id: BootId("boot-1".into()),
        instance: InstanceId::generate(),
        capabilities: Capabilities::default(),
        build: None,
        target: None,
        challenge: None,
    }))
    .await;
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Handshake, 1)
    );
    assert_eq!(
        next_state(&mut events).await,
        unreachable("maxd is newer than this app, so update the app", None)
    );
}

#[tokio::test]
async fn quiet_connections_are_pinged_then_dropped() {
    let timing = Timing {
        ping_every: Duration::from_millis(40),
        silence: Duration::from_millis(200),
        ..NO_RETRY
    };
    let config = SshConfig {
        host_name: "box".into(),
        ..SshConfig::default()
    };
    let mut rig = Rig::new(timing, Tailnet::default(), &[("box", config)]);
    let (_handle, mut events) = rig.client.link(spec(ssh("box")));
    assert_eq!(next_state(&mut events).await, LinkState::Unknown);
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Dialing, 1)
    );
    let mut maxd = rig.next_maxd().await;
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Handshake, 1)
    );
    maxd.accept(InstanceId::generate(), "boot-1").await;
    assert!(is_connected(&next_state(&mut events).await));

    for _ in 0..3 {
        let Some(ClientMessage::Ping { nonce }) = maxd.recv().await else {
            panic!("expected a ping");
        };
        maxd.send(&ServerMessage::Pong { nonce }).await;
    }
    // Pongs kept it up, and silence now drops it. The master may be stuck
    // too, so it goes.
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Dialing, 1)
    );
    assert_eq!(*rig.fake.forgotten.lock().unwrap(), ["box"]);
}

#[tokio::test]
async fn a_refresh_while_connected_probes_the_connection() {
    let mut rig = Rig::new(NO_RETRY, Tailnet::default(), &[]);
    let (handle, mut events, mut maxd) = connected(&mut rig, InstanceId::generate()).await;

    handle.refresh();
    let Some(ClientMessage::Ping { nonce }) = maxd.recv().await else {
        panic!("expected a ping");
    };
    maxd.send(&ServerMessage::Pong { nonce }).await;
    tokio::time::sleep(NO_RETRY.probe * 2).await;
    assert!(
        events.try_recv().is_err(),
        "an answered probe changes nothing"
    );

    handle.refresh();
    assert!(matches!(
        maxd.recv().await,
        Some(ClientMessage::Ping { .. })
    ));
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Dialing, 1)
    );
}

#[cfg(feature = "system-ssh")]
#[tokio::test]
async fn the_local_route_reaches_maxd_over_its_socket() {
    // A socket path has to fit in 104 bytes on macOS.
    let mut dir = std::env::temp_dir();
    if dir.as_os_str().len() > 60 {
        dir = PathBuf::from("/tmp");
    }
    let path = dir.join(format!("maxhq-client-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).unwrap();
    let (_tailnet, tailnet_rx) = watch::channel(Arc::new(Tailnet::default()));
    let (_, networks) = watch::channel(Vec::new());
    let transport = Arc::new(System::new(Some(path.clone()), None));
    let client = Client::with_parts(transport, tailnet_rx, networks, NO_RETRY);

    let (_handle, mut events) = client.link(spec(Route::Local));
    assert_eq!(next_state(&mut events).await, LinkState::Unknown);
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Dialing, 1)
    );
    let (stream, _) = listener.accept().await.unwrap();
    let mut maxd = FakeMaxd::new(stream);
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Handshake, 1)
    );
    assert_eq!(maxd.accept(InstanceId::generate(), "boot-1").await, None);
    assert!(is_connected(&next_state(&mut events).await));
    maxd.send(&snapshot(1, &[1])).await;
    assert_eq!(next(&mut events).await, LinkEvent::Synced(vec![entity(1)]));

    drop(maxd);
    drop(listener);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Dialing, 1)
    );
    match next_state(&mut events).await {
        LinkState::Unreachable { reason, last_seen } => {
            assert_eq!(reason, "maxd isn't running");
            assert!(last_seen.is_some(), "it was connected a moment ago");
        }
        other => panic!("expected unreachable, got {other:?}"),
    }
}
