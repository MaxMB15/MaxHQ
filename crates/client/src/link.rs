// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! One link's task: working out the route, dialing, the hello, keeping the
//! replica current and retrying with backoff.
//!
//! States follow decision N-2. An attempt shows as connecting only when
//! something predicts it will work: the first try, a reconnect after a
//! drop, a refresh, the peer coming online or Tailscale coming up. Timed
//! retries after a failure keep the link unreachable until one succeeds.
//!
//! Each connect sends again what the link still waits on: the timelines it
//! has open, from the last version each one saw, and every action with no
//! result yet, under its first key.
//!
//! A device that announced a wind-down for a restart, per A-11, is retried
//! on a short fixed delay once it drops, since it should be back soon.
//!
//! While the app is in the background a link holds no connection and runs
//! no timers. Coming back dials at once, past any backoff wait. With
//! several routes and a transport that staggers, as the phone's does, the
//! next route starts dialing when the one before hasn't answered in time,
//! and the first to say hello wins.
//!
//! The link's forwards go through a [`Tunnel`], which reaches a connection
//! only once its writer is up. A forward's open isn't sent again after a
//! reconnect. The forwards ask again themselves, on the same local port.
//!
//! A `maxd` whose hello carries a challenge wants the connection proven
//! before it does more than the few things open to anyone. When the client
//! has a [`Prover`] for the link's kind, the connection opens only once the
//! prover's verdict says it's proven: until then nothing from the book goes
//! out, and what `maxd` sends is kept for after. A device waiting for a yes
//! given somewhere else shows as such, for as long as it takes.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use futures::stream::{FuturesUnordered, StreamExt};
use maxhq_core::prove::{Challenge, Statement};
use maxhq_protocol::agents::ClientRole;
use maxhq_protocol::frame::{self, DecodeError, Frame, FrameDecoder};
use maxhq_protocol::ids::{
    BootId, ClientId, DeviceId, ForwardId, IdempotencyKey, InstanceId, PortId,
};
use maxhq_protocol::link::{ConnectStep, DeviceLink, LinkState, NetworkState, Peer, Route};
use maxhq_protocol::message::{
    ActionOutcome, Capabilities, ClientHello, ClientInfo, ErrorCode, ServerHello,
};
use maxhq_protocol::state::{Entity, EntityKey, Replica};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::{ClientMessage, ProtocolRange, ServerMessage};
use maxhq_wire::Queue;
use tokio::io::AsyncReadExt;
use tokio::sync::{mpsc, oneshot, watch, Notify};
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::{CancellationToken, DropGuard};
use tracing::{debug, info};

use crate::backoff::{Backoff, BackoffLimits};
use crate::forward::{ForwardError, ForwardLink, Forwards, Gone, Out};
use crate::proof::{Kind, Pipeline, Prover, Proving, Verdict};
use crate::requests::Book;
use crate::ssh_config::{split_hop, SshConfig};
use crate::tailnet::Tailnet;
use crate::transfer::{Places, Transfers};
use crate::transport::{Conn, DialError, Reader, Rest, Transport};
use crate::LinkEvent;

/// How many requests can wait for the writer.
const QUEUE: usize = 64;

/// How much is read from the stream at once.
const READ_CHUNK: usize = 64 * 1024;

/// The writer's queue for tunnel messages and stream bytes. They share
/// one, so a stream's close never passes its bytes.
const TUNNEL_QUEUE: Queue = Queue::Bulk;

/// Why a forward can't open on a device whose `maxd` has no tunnels.
const NO_TUNNELS: &str = "this device's maxd doesn't forward ports";

/// How many messages a connection keeps while it proves itself. `maxd`
/// sends little before then: the answer to a Sync that went behind a
/// proof, and the changes after it.
const EARLY: usize = 1024;

/// The timings a link works to. Tests shorten them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Timing {
    /// Retry delays.
    pub backoff: BackoffLimits,
    /// How long `maxd` gets to answer the hello.
    pub handshake: Duration,
    /// How often a quiet connection is pinged.
    pub ping_every: Duration,
    /// How long a connection can go without a word before it's dropped.
    pub silence: Duration,
    /// How long a refresh waits for an answer, per N-4.
    pub probe: Duration,
    /// How long a resync after a gap gets to close it.
    pub resync: Duration,
    /// How long after a drop a device that stops answering is taken to be
    /// restarting.
    pub reboot_window: Duration,
    /// How long a connection must last for its drop to count as a drop
    /// rather than a failed attempt.
    pub stable: Duration,
    /// How long an action waits for its result, across reconnects.
    pub reply: Duration,
    /// The delay between attempts after a device that announced a
    /// wind-down drops.
    pub wind_down_retry: Duration,
    /// How long after that drop the short delay lasts, before the backoff
    /// takes over again.
    pub wind_down_window: Duration,
}

impl Timing {
    /// The timings the app uses.
    pub const DEFAULT: Self = Self {
        backoff: BackoffLimits::DEVICE,
        handshake: Duration::from_secs(10),
        ping_every: Duration::from_secs(15),
        silence: Duration::from_secs(45),
        probe: Duration::from_secs(3),
        resync: Duration::from_secs(10),
        reboot_window: Duration::from_secs(10 * 60),
        stable: Duration::from_secs(5),
        reply: Duration::from_secs(30),
        wind_down_retry: Duration::from_secs(2),
        wind_down_window: Duration::from_secs(10 * 60),
    };

    /// The timings a peer link uses: an action, such as a long agent wait,
    /// waits up to an hour and two minutes for its answer.
    pub const PEER: Self = Self {
        reply: Duration::from_secs(3720),
        ..Self::DEFAULT
    };
}

/// What every link shares.
#[derive(Clone)]
pub(crate) struct Shared {
    /// How links reach devices.
    pub transport: Arc<dyn Transport>,
    /// The Tailscale watcher's view.
    pub tailnet: watch::Receiver<Arc<Tailnet>>,
    /// Sent in each hello.
    pub client: ClientInfo,
    /// Sent in each hello, the same across reconnects.
    pub client_id: ClientId,
    /// The timings.
    pub timing: Timing,
    /// Where downloads land and uploads are staged.
    pub places: Places,
    /// Names the network this machine is on, for a device with several
    /// routes.
    pub netkey: NetKey,
    /// Sent in each hello. A peer link syncs nothing.
    pub role: ClientRole,
    /// Whether the app is in the background. Each change, even to the same
    /// value, wakes the links.
    pub life: watch::Receiver<bool>,
    /// What proves each kind of connection, at most one for each kind.
    pub provers: Vec<Arc<dyn Prover>>,
    /// Changes when each open connection should prove itself again.
    pub reprove: watch::Receiver<()>,
}

impl Shared {
    /// The prover for this client's connections, if `maxd` will want one.
    /// A phone says hello as the app, and proves with its own key, or with
    /// the pairing's one-time key while it pairs.
    fn prover(&self) -> Option<&Arc<dyn Prover>> {
        let kinds: &[Kind] = match self.role {
            ClientRole::App => &[Kind::App, Kind::Phone, Kind::Pair],
            ClientRole::Peer => &[Kind::Peer],
            ClientRole::Mcp => return None,
        };
        self.provers
            .iter()
            .find(|prover| kinds.contains(&prover.kind()))
    }
}

/// Names the network this machine is on, or none when it can't tell.
pub(crate) type NetKey = Arc<dyn Fn() -> BoxFuture<'static, Option<String>> + Send + Sync>;

/// A device's routes: yours in order, and the one that last worked on each
/// network.
#[derive(Clone, Debug, Default)]
pub(crate) struct Routes {
    /// Every route, in your order. Empty or one long for a device with one.
    pub yours: Vec<Route>,
    /// The route that last worked, by network key.
    pub last_good: BTreeMap<String, Route>,
}

/// The order to try routes in: the one that last worked on this network
/// first, then the rest in your order.
pub(crate) fn try_order(yours: &[Route], last_good: Option<&Route>) -> Vec<Route> {
    let first = last_good.filter(|good| yours.contains(good));
    first
        .into_iter()
        .chain(yours.iter().filter(|route| Some(*route) != first))
        .cloned()
        .collect()
}

/// The handle's side of a link task.
pub(crate) struct Control {
    /// Where requests go while connected, and none otherwise. Closed once
    /// the task ends.
    pub outbox: watch::Receiver<Option<mpsc::Sender<ClientMessage>>>,
    /// The connection's writer while connected, for frames of any kind on
    /// any queue, and none otherwise. Closed once the task ends.
    pub wire: watch::Receiver<Option<maxhq_wire::Sender>>,
    /// Asks for a probe now.
    pub refresh: Arc<Notify>,
    /// What the link waits on, shared with the task.
    pub book: Arc<Book>,
    /// What the device said it does at the last hello.
    pub capabilities: watch::Receiver<Capabilities>,
    /// The protocol version the device's `maxd` said it speaks at the last
    /// hello.
    pub protocol: watch::Receiver<Option<u32>>,
    /// The uploads and downloads in flight, shared with the task.
    pub transfers: Arc<Transfers>,
    /// The owner's pings that wait for a pong, shared with the task.
    pub pings: Arc<Pings>,
    /// The link's port forwards, shared with the task.
    pub forwards: Arc<Forwards>,
    /// Stops the task when dropped.
    pub guard: DropGuard,
}

/// Pings the owner sent, each waiting for its pong.
#[derive(Default)]
pub(crate) struct Pings {
    waiting: std::sync::Mutex<HashMap<u32, oneshot::Sender<()>>>,
    next: AtomicU32,
}

impl Pings {
    /// A nonce for a new ping, and what its pong wakes. The task's own
    /// pings count up from 0, so these have the top bit set.
    pub fn start(&self) -> (u32, oneshot::Receiver<()>) {
        let nonce = self.next.fetch_add(1, Ordering::Relaxed) | 1 << 31;
        let (answered, pong) = oneshot::channel();
        self.lock().insert(nonce, answered);
        (nonce, pong)
    }

    /// Stops waiting for a ping's pong.
    pub fn forget(&self, nonce: u32) {
        self.lock().remove(&nonce);
    }

    /// Ends every ping in flight, since the connection that carried them
    /// is gone.
    fn dropped(&self) {
        self.lock().clear();
    }

    fn answered(&self, nonce: u32) {
        if let Some(answered) = self.lock().remove(&nonce) {
            let _ = answered.send(());
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u32, oneshot::Sender<()>>> {
        self.waiting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Starts a link's task. The first event, the link in its unknown state,
/// should already be in `events`.
pub(crate) fn spawn(
    shared: Arc<Shared>,
    link: DeviceLink,
    routes: Routes,
    events: mpsc::Sender<LinkEvent>,
) -> Control {
    let (outbox_tx, outbox) = watch::channel(None);
    let (capabilities_tx, capabilities) = watch::channel(Capabilities::default());
    let (protocol_tx, protocol) = watch::channel(None);
    let (wire_tx, wire) = watch::channel(None);
    let refresh = Arc::new(Notify::new());
    let book = Arc::new(Book::default());
    let transfers = Arc::new(Transfers::new(shared.places.clone()));
    let pings = Arc::new(Pings::default());
    let timing = shared.timing;
    let tunnel = Arc::new(Tunnel::new(timing.reply));
    let forwards = Arc::new(Forwards::new(Arc::clone(&tunnel) as Arc<dyn ForwardLink>));
    let cancel = CancellationToken::new();
    let mut tailnet = shared.tailnet.clone();
    let view = tailnet.borrow_and_update().clone();
    let life = shared.life.clone();
    let reprove = shared.reprove.clone();
    let task = Task {
        shared,
        link,
        events,
        outbox: outbox_tx,
        capabilities: capabilities_tx,
        protocol: protocol_tx,
        wire: wire_tx,
        refresh: Arc::clone(&refresh),
        book: Arc::clone(&book),
        transfers: Arc::clone(&transfers),
        pings: Arc::clone(&pings),
        tunnel,
        forwards: Arc::clone(&forwards),
        cancel: cancel.clone(),
        tailnet,
        tailnet_open: true,
        life,
        life_open: true,
        reprove,
        reprove_open: true,
        view,
        route: RouteInfo::default(),
        routes,
        replica: Replica::default(),
        instance: None,
        boot: None,
        backoff: Backoff::new(timing.backoff),
        attempt: 0,
        started: Instant::now(),
        last_connected: None,
        dropped_at: None,
        went_down: false,
        wind_down_drop: None,
        nonce: 0,
    };
    tokio::spawn(task.run());
    Control {
        outbox,
        wire,
        refresh,
        book,
        capabilities,
        protocol,
        transfers,
        pings,
        forwards,
        guard: cancel.drop_guard(),
    }
}

/// What the route resolved to.
#[derive(Debug, Default)]
struct RouteInfo {
    /// What `ssh -G` said, if it answered.
    config: Option<SshConfig>,
    /// The host name matched against tailnet peers.
    host: Option<String>,
    /// Addresses the host name looked up to, when it has no hops.
    resolved: Vec<IpAddr>,
    /// Each hop's host name, nearest first.
    hop_hosts: Vec<String>,
}

/// What kind of attempt is starting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Attempt {
    /// Something predicts it will work, so it shows as connecting.
    Foreground,
    /// The user asked, so it dials even past an offline peer.
    Refresh,
    /// A timed retry, which keeps the link unreachable until it works.
    Background,
}

/// Why an attempt failed.
#[derive(Debug)]
struct Failure {
    /// What the app shows.
    reason: String,
    /// Whether the device itself didn't answer.
    host_down: bool,
    /// When the device was last seen by another source, such as Tailscale.
    last_seen: Option<Timestamp>,
    /// The protocol versions `maxd` speaks, when its hello found none in
    /// common with ours.
    mismatch: Option<ProtocolRange>,
}

impl Failure {
    /// A device that answered and refused.
    fn refused(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            host_down: false,
            last_seen: None,
            mismatch: None,
        }
    }

    /// A `maxd` speaking `theirs`, with no protocol version in common.
    fn mismatched(reason: impl Into<String>, theirs: Option<ProtocolRange>) -> Self {
        Self {
            mismatch: theirs,
            ..Self::refused(reason)
        }
    }
}

/// How an attempt ended.
enum Step {
    /// The handle dropped.
    Cancelled,
    /// It never connected.
    Failed(Failure),
    /// It connected and later dropped.
    Dropped {
        /// Why, for the log and for a drop that came too soon.
        reason: String,
        /// How long the connection lasted.
        lasted: Duration,
    },
    /// The app went to the background.
    Suspended,
}

/// What woke a wait between attempts.
enum Wake {
    /// The backoff delay passed.
    Timer,
    /// Something predicts success: the peer came online, Tailscale came
    /// up or the app came back to the foreground.
    Trigger,
    /// The user asked.
    Refresh,
}

/// How a connected session ended.
enum End {
    /// The handle dropped.
    Cancelled,
    /// The connection closed or failed.
    Dropped {
        /// Why.
        reason: String,
        /// Whether it went quiet rather than closing, which means the SSH
        /// master may be stuck too.
        quiet: bool,
    },
    /// The app went to the background.
    Suspended,
    /// The device turned the connection's proof down, for the reason the
    /// app shows.
    Refused(String),
}

/// What proves one connection, and can again while it stays open.
struct Proof {
    prover: Arc<dyn Prover>,
    /// What each proof on this connection signs.
    statement: Statement,
    /// The link's name.
    name: String,
    /// The connection's control queue.
    queue: mpsc::Sender<ClientMessage>,
    book: Arc<Book>,
    /// How long each answer may take.
    wait: Duration,
    /// Woken when the device waits for a yes given somewhere else.
    asked: Arc<Notify>,
    /// The Sync that may go behind the first proof.
    pipeline: Arc<std::sync::Mutex<Pipeline>>,
}

impl Proof {
    /// Starts the prover on this connection.
    fn start(&self) -> BoxFuture<'static, Verdict> {
        let proving = Proving::new(
            self.statement,
            self.name.clone(),
            self.queue.clone(),
            Arc::clone(&self.book),
            self.wait,
            Arc::clone(&self.asked),
            Arc::clone(&self.pipeline),
        );
        self.prover.prove(proving)
    }

    /// The Sync if it never went out, and whether it went behind a proof
    /// the device took.
    fn sync(&self) -> (Option<ClientMessage>, bool) {
        let mut pipeline = self
            .pipeline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (pipeline.sync.take(), pipeline.proven)
    }
}

/// A connection that said hello and hasn't opened yet, since it's still
/// proving itself.
struct Unopened {
    hello: ServerHello,
    /// Whether it reached the same `maxd` run as the last connection.
    resume: bool,
    /// The Sync it sends once open, unless one went behind a proof the
    /// device took.
    sync: Option<ClientMessage>,
    /// What `maxd` sent meanwhile, for after the connection opens.
    early: Vec<ServerMessage>,
}

/// Why reading stopped.
enum ReadError {
    /// The far side closed.
    Eof,
    /// The stream failed.
    Io(io::Error),
    /// The bytes weren't frames.
    Bad(DecodeError),
}

/// One open stream: the reading half, and a writer that sends control
/// messages ahead of file data.
struct Wire {
    inbound: Inbound,
    /// Messages for the writer's control queue, in order.
    queue: mpsc::Sender<ClientMessage>,
    /// Tunnel messages and stream bytes for the writer, in order.
    tunnel: mpsc::UnboundedSender<Out>,
    /// The writer's queues, for frames of any kind.
    frames: maxhq_wire::Sender,
    writer: JoinHandle<io::Result<()>>,
    /// Moves queued messages onto the writer.
    encoder: JoinHandle<()>,
    /// Moves tunnel messages and bytes onto the writer.
    tunneler: JoinHandle<()>,
    rest: Rest,
}

impl Wire {
    /// Starts the writer.
    fn new(conn: Conn, transfers: Arc<Transfers>, forwards: Arc<Forwards>) -> Self {
        let (queue, pending) = mpsc::channel(QUEUE);
        let (tunnel, tunneled) = mpsc::unbounded_channel();
        let (frames, writer) = maxhq_wire::spawn(conn.writer);
        Self {
            inbound: Inbound {
                reader: conn.reader,
                decoder: FrameDecoder::new(),
                buf: vec![0; READ_CHUNK],
                transfers,
                forwards,
            },
            queue,
            tunnel,
            encoder: tokio::spawn(encode_loop(pending, frames.clone())),
            tunneler: tokio::spawn(tunnel_loop(tunneled, frames.clone())),
            frames,
            writer,
            rest: conn.rest,
        }
    }
}

/// The reading half and what it has buffered.
struct Inbound {
    reader: Reader,
    decoder: FrameDecoder,
    buf: Vec<u8>,
    /// Where file data goes.
    transfers: Arc<Transfers>,
    /// Where tunnel frames go.
    forwards: Arc<Forwards>,
}

impl Inbound {
    /// The next message from `maxd`. Frames this client has no use for,
    /// and messages it can't parse, are skipped. Safe to cancel.
    async fn next(&mut self) -> Result<ServerMessage, ReadError> {
        loop {
            match self.decoder.next_frame() {
                Ok(Some(Frame::Message(json))) => {
                    if let Some(message) = ServerMessage::read(json) {
                        return Ok(message);
                    }
                }
                Ok(Some(Frame::TransferChunk {
                    transfer,
                    offset,
                    data,
                })) => self.transfers.chunk(transfer, offset, data),
                Ok(Some(Frame::TunnelData { stream, data })) => self.forwards.data(stream, data),
                Ok(Some(_)) => {}
                Err(
                    DecodeError::UnknownKind(_)
                    | DecodeError::MissingSession
                    | DecodeError::ShortHeader(_),
                ) => {}
                Err(err) => return Err(ReadError::Bad(err)),
                Ok(None) => {
                    let n = self
                        .reader
                        .read(&mut self.buf)
                        .await
                        .map_err(ReadError::Io)?;
                    if n == 0 {
                        return Err(ReadError::Eof);
                    }
                    self.decoder.push(&self.buf[..n]);
                }
            }
        }
    }
}

/// Queues a message without waiting. A full queue drops it, which only
/// happens when `maxd` stopped reading.
fn post(queue: &mpsc::Sender<ClientMessage>, message: ClientMessage) {
    if queue.try_send(message).is_err() {
        debug!("the write queue is full");
    }
}

impl Drop for Wire {
    /// Stops the writer, for every clone of its sender too.
    fn drop(&mut self) {
        self.frames.close();
        self.encoder.abort();
        self.tunneler.abort();
        self.writer.abort();
    }
}

/// Puts queued messages on the writer's control queue until the queue
/// closes or the writer stops.
async fn encode_loop(mut pending: mpsc::Receiver<ClientMessage>, frames: maxhq_wire::Sender) {
    while let Some(message) = pending.recv().await {
        let mut out = Vec::new();
        if let Err(err) = frame::encode_message(&mut out, &message) {
            debug!(%err, "couldn't encode a message");
            continue;
        }
        if frames.send(Queue::Control, out).await.is_err() {
            return;
        }
    }
}

/// Puts tunnel messages and stream bytes on the writer, in the order the
/// forwards sent them, until the connection goes.
async fn tunnel_loop(mut tunneled: mpsc::UnboundedReceiver<Out>, frames: maxhq_wire::Sender) {
    while let Some(out) = tunneled.recv().await {
        let mut frame = Vec::new();
        if let Err(err) = out.encode(&mut frame) {
            debug!(%err, "couldn't encode a tunnel message");
            continue;
        }
        if frames.send(TUNNEL_QUEUE, frame).await.is_err() {
            return;
        }
    }
}

/// The link's side of its forwards: [`ForwardLink`] over whichever
/// connection is up. A connection counts once its writer exists and its
/// sync is queued, just before the link shows connected, and stops
/// counting as soon as it drops. Nothing the forwards send goes out
/// before then. An open waiting on a connection that drops fails, and the
/// forwards ask again on the next one.
pub(crate) struct Tunnel {
    state: Arc<std::sync::Mutex<TunnelState>>,
    /// How long an open waits for `maxd`'s answer.
    reply: Duration,
}

#[derive(Default)]
struct TunnelState {
    /// The connection, once its writer is up.
    conn: Option<TunnelConn>,
    /// Opens waiting for their result on that connection, by key.
    opening: HashMap<IdempotencyKey, oneshot::Sender<ActionOutcome>>,
}

/// One connection, as the forwards see it.
struct TunnelConn {
    /// The control queue, which `OpenForward` goes on like any action.
    queue: mpsc::Sender<ClientMessage>,
    /// Tunnel messages and stream bytes, in order.
    out: mpsc::UnboundedSender<Out>,
    /// Whether `maxd` said it forwards ports.
    tunnels: bool,
}

fn lock_tunnel(state: &std::sync::Mutex<TunnelState>) -> std::sync::MutexGuard<'_, TunnelState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Tunnel {
    fn new(reply: Duration) -> Self {
        Self {
            state: Arc::default(),
            reply,
        }
    }

    /// A connection is up, with its sync queued.
    fn connected(&self, wire: &Wire, capabilities: Capabilities) {
        lock_tunnel(&self.state).conn = Some(TunnelConn {
            queue: wire.queue.clone(),
            out: wire.tunnel.clone(),
            tunnels: capabilities.tunnels,
        });
    }

    /// The connection is gone. Sends fail until the next one, and opens
    /// waiting on this one fail now.
    fn dropped(&self) {
        let mut state = lock_tunnel(&self.state);
        state.conn = None;
        state.opening.clear();
    }

    /// Hands an open its result. A forward that opened after its open
    /// stopped waiting closes again. Gives any other outcome back when no
    /// open waits on `key`, since it's some other action's.
    fn settle(&self, key: IdempotencyKey, outcome: ActionOutcome) -> Option<ActionOutcome> {
        let state = &mut *lock_tunnel(&self.state);
        if let Some(reply) = state.opening.remove(&key) {
            // The open may have stopped waiting, which is fine.
            let _ = reply.send(outcome);
            return None;
        }
        let ActionOutcome::ForwardOpened { forward } = outcome else {
            return Some(outcome);
        };
        debug!(%forward, "closed a forward that opened too late");
        if let Some(conn) = &state.conn {
            let _ = conn
                .out
                .send(Out::Message(ClientMessage::CloseForward { forward }));
        }
        None
    }
}

impl ForwardLink for Tunnel {
    fn send(&self, out: Out) -> Result<(), Gone> {
        let state = lock_tunnel(&self.state);
        let conn = state.conn.as_ref().ok_or(Gone)?;
        conn.out.send(out).map_err(|_| Gone)
    }

    fn open(
        &self,
        port: PortId,
        local_port: u16,
    ) -> BoxFuture<'static, Result<ForwardId, ForwardError>> {
        let key = IdempotencyKey::generate();
        let (reply, result) = oneshot::channel();
        let queue = {
            let mut state = lock_tunnel(&self.state);
            let queue = match &state.conn {
                None => Err(ForwardError::NotConnected),
                Some(conn) if !conn.tunnels => Err(ForwardError::Refused(NO_TUNNELS.to_owned())),
                Some(conn) => Ok(conn.queue.clone()),
            };
            if queue.is_ok() {
                state.opening.insert(key, reply);
            }
            queue
        };
        let waiting = queue.map(|queue| {
            let opening = Opening {
                state: Arc::clone(&self.state),
                key,
            };
            (queue, opening)
        });
        let timeout = self.reply;
        Box::pin(async move {
            let (queue, _opening) = waiting?;
            let open = ClientMessage::OpenForward {
                key,
                port,
                local_port,
            };
            queue
                .send(open)
                .await
                .map_err(|_| ForwardError::NotConnected)?;
            match tokio::time::timeout(timeout, result).await {
                Ok(Ok(ActionOutcome::ForwardOpened { forward })) => Ok(forward),
                Ok(Ok(ActionOutcome::Refused { reason })) => Err(ForwardError::Refused(reason)),
                Ok(Ok(other)) => {
                    debug!(?other, "maxd answered a forward with something else");
                    Err(ForwardError::Refused(
                        "the device answered with something this app doesn't expect".to_owned(),
                    ))
                }
                // The connection dropped before the answer came.
                Ok(Err(_)) => Err(ForwardError::NotConnected),
                Err(_) => Err(ForwardError::Refused(
                    "the device didn't answer in time".to_owned(),
                )),
            }
        })
    }
}

/// Stops an open waiting once its future ends, however it ends.
struct Opening {
    state: Arc<std::sync::Mutex<TunnelState>>,
    key: IdempotencyKey,
}

impl Drop for Opening {
    fn drop(&mut self) {
        lock_tunnel(&self.state).opening.remove(&self.key);
    }
}

/// The link's task.
struct Task {
    shared: Arc<Shared>,
    /// The link as last reported.
    link: DeviceLink,
    events: mpsc::Sender<LinkEvent>,
    outbox: watch::Sender<Option<mpsc::Sender<ClientMessage>>>,
    capabilities: watch::Sender<Capabilities>,
    protocol: watch::Sender<Option<u32>>,
    wire: watch::Sender<Option<maxhq_wire::Sender>>,
    refresh: Arc<Notify>,
    book: Arc<Book>,
    transfers: Arc<Transfers>,
    pings: Arc<Pings>,
    /// The forwards' way onto the connection.
    tunnel: Arc<Tunnel>,
    forwards: Arc<Forwards>,
    cancel: CancellationToken,
    tailnet: watch::Receiver<Arc<Tailnet>>,
    /// Whether the watcher is still there to send changes.
    tailnet_open: bool,
    /// Whether the app is in the background.
    life: watch::Receiver<bool>,
    /// Whether the client is still there to send changes.
    life_open: bool,
    /// Changes when the open connection should prove itself again.
    reprove: watch::Receiver<()>,
    /// Whether the client is still there to ask.
    reprove_open: bool,
    /// The tailnet as last seen.
    view: Arc<Tailnet>,
    route: RouteInfo,
    /// Every route the device has, and which worked where.
    routes: Routes,
    replica: Replica,
    /// The `maxd` run the replica came from.
    instance: Option<InstanceId>,
    /// The device's boot at the last hello.
    boot: Option<BootId>,
    backoff: Backoff,
    /// Attempts since the last connect.
    attempt: u32,
    /// When the task started, for how long a device that never connected
    /// has been gone.
    started: Instant,
    /// When the link was last connected.
    last_connected: Option<(Instant, Timestamp)>,
    /// When a connection last dropped, while it hasn't come back.
    dropped_at: Option<Instant>,
    /// Whether the device stopped answering since that drop.
    went_down: bool,
    /// When that drop came, if the device had announced a wind-down.
    wind_down_drop: Option<Instant>,
    nonce: u32,
}

impl Task {
    /// Runs until the handle or the event receiver drops.
    async fn run(mut self) {
        let unplaced = self.link.clone();
        self.resolve_route().await;
        self.place();
        if self.link != unplaced {
            self.emit_link().await;
        }
        let mut kind = Attempt::Foreground;
        loop {
            if self.cancel.is_cancelled() {
                return;
            }
            if *self.life.borrow_and_update() {
                if !self.until_active().await {
                    return;
                }
                kind = Attempt::Foreground;
            }
            self.attempt += 1;
            let failure = match self.attempt(kind).await {
                Step::Cancelled => return,
                Step::Suspended => {
                    self.attempt = 0;
                    self.set_state(LinkState::Unknown).await;
                    continue;
                }
                Step::Failed(failure) => failure,
                Step::Dropped { reason, lasted } => {
                    info!(link = %self.link.id, %reason, "connection dropped");
                    self.dropped_at = Some(Instant::now());
                    self.went_down = false;
                    self.wind_down_drop = self.winding_down().then(Instant::now);
                    if lasted >= self.shared.timing.stable {
                        kind = Attempt::Foreground;
                        continue;
                    }
                    // A connection that drops at once would otherwise
                    // reconnect in a tight loop.
                    Failure::refused(reason)
                }
            };
            self.fail(failure).await;
            let delay = self.retry_delay();
            kind = match self.wait(delay).await {
                None => return,
                Some(Wake::Timer) => Attempt::Background,
                Some(Wake::Trigger) => Attempt::Foreground,
                Some(Wake::Refresh) => {
                    // The SSH config may have changed since.
                    let before = self.link.clone();
                    self.resolve_route().await;
                    self.place();
                    if self.link != before {
                        self.emit_link().await;
                    }
                    Attempt::Refresh
                }
            };
        }
    }

    /// One attempt: dial, hello, and the session if it connects.
    async fn attempt(&mut self, kind: Attempt) -> Step {
        self.catch_up_tailnet().await;
        if kind != Attempt::Refresh
            && let Some(failure) = self.gate()
        {
            return Step::Failed(failure);
        }
        let shows = kind != Attempt::Background;
        let rebooting = self.rebooting();
        if rebooting {
            self.set_connecting(ConnectStep::WaitingForReboot).await;
        } else if shows {
            self.set_connecting(ConnectStep::Dialing).await;
        }

        let (mut wire, hello) = match self.reach(shows, rebooting).await {
            Ok(reached) => reached,
            Err(step) => return step,
        };

        // This connection proves with the keys there are now.
        self.reprove.borrow_and_update();
        let resume = self.instance == Some(hello.instance);
        let since = if resume {
            self.replica.seq()
        } else {
            self.replica = Replica::default();
            None
        };
        let sync = (self.shared.role != ClientRole::Peer).then_some(ClientMessage::Sync { since });
        let proof = self.proof(&hello, &wire, sync.clone());
        let mut opened = None;
        let unopened = if proof.is_some() {
            Some(Unopened {
                hello,
                resume,
                sync,
                early: Vec::new(),
            })
        } else {
            if let Some(sync) = sync {
                post(&wire.queue, sync);
            }
            self.open(&mut wire, hello, resume).await;
            opened = Some(Instant::now());
            None
        };

        let end = self.session(&mut wire, proof, unopened, &mut opened).await;
        // A proof still waiting names this connection, so it ends with it.
        self.book.lock().end_proofs();
        if opened.is_some() {
            self.outbox.send_replace(None);
            self.wire.send_replace(None);
            self.transfers.dropped();
            self.pings.dropped();
            self.tunnel.dropped();
            self.forwards.link_down();
        }
        drop(wire);
        match end {
            End::Cancelled => Step::Cancelled,
            End::Suspended => Step::Suspended,
            End::Refused(reason) => Step::Failed(Failure::refused(reason)),
            End::Dropped { reason, quiet } => {
                if quiet {
                    self.forget_master().await;
                }
                match opened {
                    Some(opened) => Step::Dropped {
                        reason,
                        lasted: opened.elapsed(),
                    },
                    // It never opened, so it counts as an attempt that
                    // failed.
                    None => Step::Failed(Failure::refused(reason)),
                }
            }
        }
    }

    /// The proof a connection gives before it opens, when its `maxd` asks
    /// for one and the client has a prover for the link's kind. An older
    /// `maxd` asks for none. One from before peer proofs serves a link
    /// from another device without a proof, and would turn one down.
    fn proof(
        &self,
        hello: &ServerHello,
        wire: &Wire,
        sync: Option<ClientMessage>,
    ) -> Option<Proof> {
        let challenge = hello.challenge.as_deref()?;
        if self.shared.role == ClientRole::Peer && !hello.capabilities.peer_proofs {
            return None;
        }
        let prover = self.shared.prover()?;
        let Some(challenge) = Challenge::parse(challenge) else {
            debug!(link = %self.link.id, "maxd sent a challenge that isn't one");
            return None;
        };
        let statement = Statement {
            kind: prover.kind(),
            device: hello.device,
            instance: hello.instance,
            client: self.shared.client_id,
            challenge,
        };
        Some(Proof {
            prover: Arc::clone(prover),
            statement,
            name: self.link.name.clone(),
            queue: wire.queue.clone(),
            book: Arc::clone(&self.book),
            wait: self.shared.timing.handshake,
            asked: Arc::new(Notify::new()),
            pipeline: Arc::new(std::sync::Mutex::new(Pipeline::new(sync))),
        })
    }

    /// Opens a connection to requests once its Sync is queued: the
    /// forwards, the link shown connected, then everything the book still
    /// waits on.
    async fn open(&mut self, wire: &mut Wire, hello: ServerHello, resume: bool) {
        let capabilities = hello.capabilities;
        // The forwards can send from here on, so their opens go after the
        // sync, and an open made once the link shows connected finds the
        // connection.
        self.tunnel.connected(wire, capabilities);
        self.forwards.link_up();
        self.connected(hello).await;
        // The outbox opens before the book is read, so a request the handle
        // adds meanwhile goes out once or twice, never not at all. Pages of
        // older items wait their turn instead, so each goes out once and in
        // the order the book expects their answers.
        let pages = self.book.pages().await;
        self.book.lock().begin();
        self.outbox.send_replace(Some(wire.queue.clone()));
        let replay = self.book.lock().replay(resume);
        for message in replay {
            if wire.queue.send(message).await.is_err() {
                // The writer stopped, which the session sees at once.
                break;
            }
        }
        drop(pages);
        self.wire.send_replace(Some(wire.frames.clone()));
        self.transfers.connected(wire.frames.clone(), capabilities);
    }

    /// Opens a connection the device took as proven. Its Sync goes unless
    /// one went behind a proof the device took, and what `maxd` sent
    /// meanwhile is handled after.
    async fn open_proven(
        &mut self,
        wire: &mut Wire,
        unopened: Unopened,
        proof: &Proof,
        resync: &mut Option<Instant>,
    ) {
        let Unopened {
            hello,
            resume,
            sync,
            early,
        } = unopened;
        let (unsent, synced) = proof.sync();
        // A Sync that went behind a proof the device turned down got no
        // answer, so it goes again.
        if let Some(sync) = unsent.or(sync.filter(|_| !synced)) {
            post(&wire.queue, sync);
        }
        self.open(wire, hello, resume).await;
        for message in early {
            self.handle(message, &wire.queue, resync).await;
        }
    }

    /// Dials each route in turn until one reaches the device and it says
    /// hello. A route that reaches another device is refused, so a reused
    /// LAN address can't stand in for this one.
    async fn reach(&mut self, shows: bool, rebooting: bool) -> Result<(Wire, ServerHello), Step> {
        let several = self.routes.yours.len() > 1;
        let net = if several {
            (self.shared.netkey)().await
        } else {
            None
        };
        let good = net.as_ref().and_then(|net| self.routes.last_good.get(net));
        let mut order = try_order(&self.routes.yours, good);
        if order.is_empty() {
            order.push(self.link.route.clone());
        }
        self.mark_last_good(net.as_deref());
        if let Some(stagger) = self.shared.transport.stagger()
            && order.len() > 1
        {
            let (route, wire, hello) = self.race(order, stagger).await?;
            if route != self.link.route {
                self.link.route = route;
                self.resolve_route().await;
                self.place();
                self.emit_link().await;
            }
            self.worked(net).await;
            return Ok((wire, hello));
        }
        let mut failure = None;
        for route in order {
            if route != self.link.route {
                self.link.route = route;
                self.resolve_route().await;
                self.place();
                self.emit_link().await;
            }
            let (wire, hello) = match self.dial_once(shows, rebooting).await {
                Ok(reached) => reached,
                Err(Step::Failed(failed)) => {
                    failure = Some(failed);
                    continue;
                }
                Err(step) => return Err(step),
            };
            if let Err(different) = same_device(self.link.device, &self.link.route, &hello) {
                info!(link = %self.link.id, "a route reaches a different device");
                failure = Some(different);
                continue;
            }
            if several {
                self.worked(net).await;
            }
            return Ok((wire, hello));
        }
        Err(Step::Failed(failure.unwrap_or_else(no_route)))
    }

    /// Dials routes in `order` side by side: each starts once the one
    /// before has failed or `stagger` has passed, and the first to say
    /// hello as the right device wins. The others close.
    async fn race(
        &mut self,
        order: Vec<Route>,
        stagger: Duration,
    ) -> Result<(Route, Wire, ServerHello), Step> {
        let mut waiting = order.into_iter();
        let mut running = FuturesUnordered::new();
        let start = |route: Route| {
            let shared = Arc::clone(&self.shared);
            let transfers = Arc::clone(&self.transfers);
            let forwards = Arc::clone(&self.forwards);
            let known = self.link.device;
            async move {
                let reached = try_route(&shared, &route, transfers, forwards, known).await;
                (route, reached)
            }
        };
        running.extend(waiting.next().map(start));
        let next = tokio::time::sleep(stagger);
        tokio::pin!(next);
        let mut answered: Option<Failure> = None;
        let failure = loop {
            tokio::select! {
                () = self.cancel.cancelled() => return Err(Step::Cancelled),
                () = &mut next, if waiting.len() > 0 => {
                    running.extend(waiting.next().map(start));
                    next.as_mut().reset(Instant::now() + stagger);
                }
                Some((route, reached)) = running.next() => match reached {
                    Ok((wire, hello)) => return Ok((route, wire, hello)),
                    Err(failed) => {
                        debug!(link = %self.link.id, reason = %failed.reason, "a route failed");
                        // A route the device answered on says more than one
                        // it didn't.
                        let failed = match answered.take() {
                            Some(earlier) if failed.host_down => earlier,
                            _ => failed,
                        };
                        running.extend(waiting.next().map(start));
                        next.as_mut().reset(Instant::now() + stagger);
                        if running.is_empty() {
                            break failed;
                        }
                        answered = (!failed.host_down).then_some(failed);
                    }
                },
            }
        };
        Err(Step::Failed(failure))
    }

    /// Notes that the link's route worked on network `net`.
    async fn worked(&mut self, net: Option<String>) {
        let Some(net) = net else {
            return;
        };
        self.routes
            .last_good
            .insert(net.clone(), self.link.route.clone());
        self.mark_last_good(Some(&net));
        let route = self.link.route.clone();
        self.emit(LinkEvent::RouteWorked { net, route }).await;
    }

    /// Marks which of the link's routes last worked on network `net`.
    fn mark_last_good(&mut self, net: Option<&str>) {
        let good = net.and_then(|net| self.routes.last_good.get(net));
        for choice in &mut self.link.routes {
            choice.last_good_here = Some(&choice.route) == good;
        }
    }

    /// Dials the link's route once and waits for the hello.
    async fn dial_once(
        &mut self,
        shows: bool,
        rebooting: bool,
    ) -> Result<(Wire, ServerHello), Step> {
        let transport = Arc::clone(&self.shared.transport);
        let dialed = tokio::select! {
            () = self.cancel.cancelled() => return Err(Step::Cancelled),
            dialed = transport.dial(&self.link.route, self.route.config.as_ref()) => dialed,
        };
        let conn = dialed.map_err(|err| Step::Failed(err.into()))?;
        if shows && !rebooting {
            self.set_connecting(ConnectStep::Handshake).await;
        }

        let mut wire = Wire::new(
            conn,
            Arc::clone(&self.transfers),
            Arc::clone(&self.forwards),
        );
        let timing = self.shared.timing;
        let hello = tokio::select! {
            () = self.cancel.cancelled() => return Err(Step::Cancelled),
            hello = tokio::time::timeout(timing.handshake, handshake(&mut wire, &self.shared)) => hello,
        };
        let hello = match hello {
            Ok(Ok(hello)) => hello,
            Ok(Err(failure)) => return Err(Step::Failed(failure)),
            Err(_) => {
                self.forget_master().await;
                return Err(Step::Failed(no_hello(&self.link.route)));
            }
        };

        Ok((wire, hello))
    }

    /// Notes a hello and reports the link connected.
    async fn connected(&mut self, hello: ServerHello) {
        if self
            .boot
            .as_ref()
            .is_some_and(|boot| *boot != hello.boot_id)
        {
            info!(link = %self.link.id, "the device restarted since the last connection");
        }
        self.instance = Some(hello.instance);
        self.boot = Some(hello.boot_id);
        self.link.device = Some(hello.device);
        self.link.capabilities = Some(hello.capabilities);
        self.capabilities.send_replace(hello.capabilities);
        self.link.protocol = Some(hello.protocol.current);
        self.link.protocol_mismatch = None;
        self.protocol.send_replace(Some(hello.protocol.current));
        let now = Timestamp::now();
        self.last_connected = Some((Instant::now(), now));
        self.dropped_at = None;
        self.went_down = false;
        self.wind_down_drop = None;
        self.attempt = 0;
        self.backoff.reset();
        self.set_state(LinkState::Connected { since: now }).await;
    }

    /// Serves a session until it ends. One still `unopened` proves itself
    /// first and opens once `proof` says it's proven, which sets `opened`.
    async fn session(
        &mut self,
        wire: &mut Wire,
        proof: Option<Proof>,
        mut unopened: Option<Unopened>,
        opened: &mut Option<Instant>,
    ) -> End {
        let timing = self.shared.timing;
        let mut ping =
            tokio::time::interval_at(Instant::now() + timing.ping_every, timing.ping_every);
        ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut last_heard = Instant::now();
        let mut probe: Option<Instant> = None;
        let mut resync: Option<Instant> = None;
        let asked = proof.as_ref().map(|proof| Arc::clone(&proof.asked));
        let mut running = proof.as_ref().map(Proof::start);
        // A proof asked for while one runs, which starts once it ends.
        let mut again = false;
        loop {
            let deadline = probe.into_iter().chain(resync).min();
            tokio::select! {
                () = self.cancel.cancelled() => return End::Cancelled,
                message = wire.inbound.next() => {
                    let message = match message {
                        Ok(message) => message,
                        Err(ReadError::Eof) => return dropped("maxd closed the connection"),
                        Err(ReadError::Io(err)) => {
                            return dropped(format!("the connection failed: {err}"));
                        }
                        Err(ReadError::Bad(err)) => {
                            return dropped(format!("maxd sent a bad frame: {err}"));
                        }
                    };
                    last_heard = Instant::now();
                    probe = None;
                    match unopened.as_mut() {
                        Some(waiting) if !answers_proof(&message) => {
                            if waiting.early.len() >= EARLY {
                                return dropped("maxd sent too much before the proof");
                            }
                            waiting.early.push(message);
                        }
                        _ => self.handle(message, &wire.queue, &mut resync).await,
                    }
                }
                verdict = verdict(&mut running) => {
                    running = None;
                    match (verdict, unopened.take()) {
                        (Verdict::Proven, Some(waiting)) => {
                            if let Some(proof) = &proof {
                                self.open_proven(wire, waiting, proof, &mut resync).await;
                            }
                            *opened = Some(Instant::now());
                        }
                        (Verdict::Refused(reason), Some(_)) => return End::Refused(reason),
                        (Verdict::Lost, Some(_)) => return quiet("maxd didn't answer the proof"),
                        // A proof again on an open connection, which stays
                        // open whatever the device said: it was proven.
                        (verdict, None) => {
                            debug!(link = %self.link.id, ?verdict, "proved the connection again");
                        }
                    }
                    if again && let Some(proof) = &proof {
                        again = false;
                        running = Some(proof.start());
                    }
                }
                () = notified(asked.as_deref()), if unopened.is_some() => {
                    self.set_connecting(ConnectStep::WaitingForApproval).await;
                }
                changed = self.reprove.changed(), if self.reprove_open && unopened.is_none() => {
                    if changed.is_err() {
                        self.reprove_open = false;
                        continue;
                    }
                    self.reprove.borrow_and_update();
                    match &proof {
                        Some(_) if running.is_some() => again = true,
                        Some(proof) => running = Some(proof.start()),
                        // An older `maxd`, or a link with no prover.
                        None => {}
                    }
                }
                _ = ping.tick() => {
                    if last_heard.elapsed() >= timing.silence {
                        return quiet("maxd stopped answering");
                    }
                    self.nonce = self.nonce.wrapping_add(1);
                    post(&wire.queue, ClientMessage::Ping { nonce: self.nonce });
                }
                () = self.refresh.notified() => self.probe(&wire.queue, &mut probe),
                changed = self.life.changed(), if self.life_open => {
                    if changed.is_err() {
                        self.life_open = false;
                        continue;
                    }
                    if *self.life.borrow_and_update() {
                        return End::Suspended;
                    }
                    // Back in the foreground, maybe on another network.
                    self.probe(&wire.queue, &mut probe);
                }
                changed = self.tailnet.changed(), if self.tailnet_open => {
                    if changed.is_err() {
                        self.tailnet_open = false;
                        continue;
                    }
                    self.view = self.tailnet.borrow_and_update().clone();
                    if self.place() {
                        self.emit_link().await;
                    }
                }
                written = &mut wire.writer => {
                    let reason = match written {
                        Ok(Err(err)) => format!("the connection failed: {err}"),
                        _ => "the connection closed".to_owned(),
                    };
                    return dropped(reason);
                }
                () = sleep_until(deadline) => {
                    if probe.is_some_and(|probe| probe <= Instant::now()) {
                        return quiet("maxd didn't answer the refresh");
                    }
                    // A resync that didn't close the gap.
                    return dropped("lost track of maxd's changes");
                }
            }
        }
    }

    /// Pings a connected `maxd` now and drops the connection if it doesn't
    /// answer soon, unless such a ping is out already.
    fn probe(&mut self, queue: &mpsc::Sender<ClientMessage>, probe: &mut Option<Instant>) {
        self.backoff.reset();
        if probe.is_none() {
            *probe = Some(Instant::now() + self.shared.timing.probe);
            self.nonce = self.nonce.wrapping_add(1);
            post(queue, ClientMessage::Ping { nonce: self.nonce });
        }
    }

    /// Waits while the app is in the background. False means the task
    /// should end.
    async fn until_active(&mut self) -> bool {
        while self.life_open && *self.life.borrow_and_update() {
            tokio::select! {
                () = self.cancel.cancelled() => return false,
                changed = self.life.changed() => self.life_open = changed.is_ok(),
            }
        }
        true
    }

    /// Acts on one message from a connected `maxd`.
    async fn handle(
        &mut self,
        message: ServerMessage,
        queue: &mpsc::Sender<ClientMessage>,
        resync: &mut Option<Instant>,
    ) {
        let Some(message) = self.transfers.take(message) else {
            return;
        };
        if self.forwards.handle(&message) {
            return;
        }
        match message {
            ServerMessage::Snapshot(snapshot) => {
                self.replica.apply_snapshot(snapshot);
                *resync = None;
                let entities = self.replica.entities().cloned().collect();
                self.emit(LinkEvent::Synced(entities)).await;
            }
            ServerMessage::Delta(delta) => {
                let before = self.replica.seq();
                let changes = delta.changes.clone();
                match self.replica.apply_delta(delta) {
                    Ok(()) if self.replica.seq() != before => {
                        *resync = None;
                        if !changes.is_empty() {
                            self.emit(LinkEvent::Changed(changes)).await;
                        }
                    }
                    // A repeat of a delta already held.
                    Ok(()) => {}
                    Err(gap) => {
                        if resync.is_none() {
                            debug!(link = %self.link.id, ?gap, "missed a delta, syncing again");
                            post(queue, ClientMessage::Sync { since: gap.have });
                            *resync = Some(Instant::now() + self.shared.timing.resync);
                        }
                    }
                }
            }
            ServerMessage::Pong { nonce } => self.pings.answered(nonce),
            ServerMessage::AnswerResult {
                idempotency_key,
                outcome,
            } => {
                if !self.book.lock().settle_answer(idempotency_key, outcome) {
                    debug!(link = %self.link.id, %idempotency_key, "a repeat answer result");
                }
            }
            ServerMessage::Error { code, message } => {
                debug!(link = %self.link.id, ?code, %message, "maxd refused a message");
            }
            ServerMessage::Hello(_) => {
                debug!(link = %self.link.id, "ignored a second hello");
            }
            ServerMessage::ActionResult { key, outcome } => {
                let Some(outcome) = self.tunnel.settle(key, outcome) else {
                    return;
                };
                if !self.book.lock().settle_action(key, outcome) {
                    debug!(link = %self.link.id, %key, "a repeat action result");
                }
            }
            ServerMessage::Timeline { agent, update } => {
                let seen = self.book.lock().saw(agent, &update);
                if let Some(live) = seen {
                    self.emit(LinkEvent::Timeline {
                        agent,
                        update,
                        live,
                    })
                    .await;
                } else {
                    debug!(link = %self.link.id, %agent, "an update for a closed timeline");
                }
            }
            ServerMessage::Workflow { run, update } => {
                if self.book.lock().saw_run(&run) {
                    self.emit(LinkEvent::Workflow { run, update }).await;
                } else {
                    debug!(link = %self.link.id, workflow = %run.workflow, "an update for a closed run");
                }
            }
            // A transfer that ended already. The forwards took every tunnel
            // message above.
            ServerMessage::UploadDone { .. }
            | ServerMessage::FetchManifest { .. }
            | ServerMessage::TransferCredit { .. }
            | ServerMessage::TransferFailed { .. }
            | ServerMessage::TunnelOpened { .. }
            | ServerMessage::TunnelRefused { .. }
            | ServerMessage::TunnelCredit { .. }
            | ServerMessage::TunnelClose { .. } => {
                debug!(link = %self.link.id, "ignored a transfer or tunnel message");
            }
            ServerMessage::PeerRequest { key, request } => {
                if self.shared.role == ClientRole::Peer {
                    self.emit(LinkEvent::PeerRequest { key, request }).await;
                } else {
                    debug!(link = %self.link.id, "ignored a peer request on an app link");
                }
            }
        }
    }

    /// Waits out a delay, or until something worth an attempt now happens.
    /// None means the task should end.
    async fn wait(&mut self, delay: Duration) -> Option<Wake> {
        let sleep = tokio::time::sleep(delay);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                () = self.cancel.cancelled() => return None,
                () = &mut sleep => return Some(Wake::Timer),
                () = self.refresh.notified() => {
                    self.backoff.reset();
                    return Some(Wake::Refresh);
                }
                changed = self.life.changed(), if self.life_open => {
                    if changed.is_err() {
                        self.life_open = false;
                        continue;
                    }
                    if !self.until_active().await {
                        return None;
                    }
                    self.backoff.reset();
                    return Some(Wake::Trigger);
                }
                changed = self.tailnet.changed(), if self.tailnet_open => {
                    if changed.is_err() {
                        self.tailnet_open = false;
                        continue;
                    }
                    let before = Arc::clone(&self.view);
                    self.view = self.tailnet.borrow_and_update().clone();
                    if self.place() {
                        self.emit_link().await;
                    }
                    let came_up = !before.is_up() && self.view.is_up();
                    let came_online = self.gate_peers(&before).iter().any(|(_, online)| !online)
                        && self.gate_peers(&self.view).iter().all(|(_, online)| *online);
                    // A name that isn't the tailnet's may work without it.
                    let went_down = before.is_up() && !self.view.is_up() && !self.needs_tailnet();
                    if self.uses_tailnet() && (came_up || came_online || went_down) {
                        self.backoff.reset();
                        return Some(Wake::Trigger);
                    }
                    // Keep the reason current, such as Tailscale turning
                    // off while the link waits.
                    if matches!(self.link.state, LinkState::Unreachable { .. })
                        && let Some(failure) = self.gate()
                    {
                        self.fail(failure).await;
                    }
                }
            }
        }
    }

    /// Takes a tailnet change that came in while the task was busy, so an
    /// attempt is gated on the latest view.
    async fn catch_up_tailnet(&mut self) {
        if !self.tailnet.has_changed().unwrap_or(false) {
            return;
        }
        self.view = self.tailnet.borrow_and_update().clone();
        if self.place() {
            self.emit_link().await;
        }
    }

    /// A failure without dialing, when Tailscale says the attempt can't
    /// work: it's off or logged out, or a peer the route needs is offline.
    fn gate(&self) -> Option<Failure> {
        if !self.uses_tailnet() {
            return None;
        }
        let network = self.view.network.as_ref()?;
        if let Some(reason) = network_reason(network.state) {
            // Any other name may still lead somewhere without Tailscale,
            // such as the home network, so it's dialed.
            return self.needs_tailnet().then(|| Failure::refused(reason));
        }
        if network.state != NetworkState::Up {
            return None;
        }
        let peers = self.gate_peers(&self.view);
        let (index, _) = peers.iter().find(|(_, online)| !online)?;
        Some(match index {
            None => Failure {
                reason: "offline".to_owned(),
                host_down: true,
                last_seen: self.link.peer.as_ref().and_then(|peer| peer.last_seen),
                mismatch: None,
            },
            // The device may be fine. It's the way there that's gone, so
            // this says nothing about a restart.
            Some(hop) => Failure::refused(format!("{} is offline", self.link.via[*hop])),
        })
    }

    /// The peers the route needs, as the device's own peer (none) or a
    /// hop's index, each with whether it's online.
    fn gate_peers(&self, tailnet: &Tailnet) -> Vec<(Option<usize>, bool)> {
        let mut peers = Vec::new();
        if let Some(host) = &self.route.host
            && let Some(peer) = tailnet.find(host, &self.route.resolved)
        {
            peers.push((None, peer.peer.online));
        }
        for (index, host) in self.route.hop_hosts.iter().enumerate() {
            if let Some(peer) = tailnet.find(host, &[]) {
                peers.push((Some(index), peer.peer.online));
            }
        }
        peers
    }

    /// Whether the route can only work over the tailnet: the host or a hop
    /// is a tailnet address or a MagicDNS name.
    fn needs_tailnet(&self) -> bool {
        let owned = |host: &String| self.view.owns(host);
        self.route.host.as_ref().is_some_and(owned) || self.route.hop_hosts.iter().any(owned)
    }

    /// Whether the route goes over the tailnet.
    fn uses_tailnet(&self) -> bool {
        let network = self.view.network.as_ref().map(|network| network.id);
        self.link.network.is_some() && self.link.network == network
    }

    /// Reports a failed attempt.
    async fn fail(&mut self, failure: Failure) {
        let mut reason = failure.reason;
        if self.uses_tailnet()
            && let Some(network) = &self.view.network
        {
            match network.state {
                NetworkState::Up | NetworkState::Unknown => {}
                NetworkState::Starting => reason = "Tailscale is starting".to_owned(),
                state => {
                    if let Some(network_reason) = network_reason(state) {
                        reason = network_reason.to_owned();
                    }
                }
            }
        }
        if failure.host_down && self.dropped_at.is_some() {
            self.went_down = true;
        }
        // Kept through failures that never reach a hello, so the phone can
        // still say which side to update.
        let mismatch_changed =
            failure.mismatch.is_some() && failure.mismatch != self.link.protocol_mismatch;
        if mismatch_changed {
            self.link.protocol_mismatch = failure.mismatch;
        }
        if self.rebooting() {
            self.set_connecting(ConnectStep::WaitingForReboot).await;
            return;
        }
        let last_seen = self
            .last_connected
            .map(|(_, at)| at)
            .into_iter()
            .chain(failure.last_seen)
            .max();
        let state = LinkState::Unreachable { reason, last_seen };
        if mismatch_changed && self.link.state == state {
            self.emit_link().await;
        } else {
            self.set_state(state).await;
        }
    }

    /// Whether the device dropped and then stopped answering recently,
    /// which is what a restart looks like from here, or dropped after it
    /// said it would restart.
    fn rebooting(&self) -> bool {
        let went_down = matches!(self.link.route, Route::Ssh { .. })
            && self.went_down
            && self
                .dropped_at
                .is_some_and(|at| at.elapsed() < self.shared.timing.reboot_window);
        went_down || self.after_wind_down()
    }

    /// Whether the device's state, as last synced, says it's winding down
    /// for a restart.
    fn winding_down(&self) -> bool {
        self.link.device.is_some_and(|device| {
            matches!(
                self.replica.get(&EntityKey::Device(device)),
                Some(Entity::Device(found)) if found.winding_down_until.is_some()
            )
        })
    }

    /// Whether the device dropped after announcing a wind-down, recently
    /// enough that it should be back from its restart soon.
    fn after_wind_down(&self) -> bool {
        self.wind_down_drop
            .is_some_and(|at| at.elapsed() < self.shared.timing.wind_down_window)
    }

    /// The delay before the next timed attempt. For a while after a device
    /// that announced a wind-down drops, it's short and fixed, so the link
    /// is back soon after the restart. Otherwise it's the backoff's.
    fn retry_delay(&mut self) -> Duration {
        if self.after_wind_down() {
            return self.shared.timing.wind_down_retry;
        }
        self.backoff.next(self.gone_for())
    }

    /// How long since the device was last connected, or since the link
    /// started if it never was.
    fn gone_for(&self) -> Duration {
        self.last_connected
            .map_or(self.started, |(at, _)| at)
            .elapsed()
    }

    /// Works out the route: `ssh -G` for the host and each hop, and a name
    /// lookup when nothing else ties the host to a peer.
    async fn resolve_route(&mut self) {
        let Route::Ssh { host } = &self.link.route else {
            return;
        };
        let host = host.clone();
        let transport = Arc::clone(&self.shared.transport);
        let config = match transport.resolve(&host).await {
            Ok(config) => Some(config),
            Err(err) => {
                debug!(link = %self.link.id, %err, "ssh -G failed");
                None
            }
        };
        let hops = config.as_ref().map(SshConfig::hops).unwrap_or_default();
        let mut hop_hosts = Vec::with_capacity(hops.len());
        for hop in &hops {
            let name = match transport.resolve(hop).await {
                Ok(config) => config.host_name,
                Err(_) => bare_host(&split_hop(hop).0),
            };
            hop_hosts.push(name);
        }
        let host_name = config
            .as_ref()
            .map_or_else(|| bare_host(&host), |config| config.host_name.clone());
        let resolved = if hops.is_empty()
            && host_name.parse::<IpAddr>().is_err()
            && !self.view.peers.is_empty()
            && self.view.find(&host_name, &[]).is_none()
        {
            transport.lookup(&host_name).await
        } else {
            Vec::new()
        };
        self.link.via = hops;
        self.route = RouteInfo {
            config,
            host: Some(host_name),
            resolved,
            hop_hosts,
        };
    }

    /// Sets the link's network and peer from the tailnet, and says whether
    /// they changed.
    fn place(&mut self) -> bool {
        let (network, peer) = match &self.link.route {
            Route::Local => (None, None),
            Route::Ssh { .. } => self.placement(),
        };
        let changed = self.link.network != network || self.link.peer != peer;
        self.link.network = network;
        self.link.peer = peer;
        changed
    }

    /// The network and peer for an SSH route.
    fn placement(&self) -> (Option<maxhq_protocol::ids::NetworkId>, Option<Peer>) {
        let tailnet = &self.view;
        let own = self
            .route
            .host
            .as_deref()
            .and_then(|host| tailnet.find(host, &self.route.resolved));
        let on = own.is_some()
            || self
                .route
                .host
                .as_deref()
                .is_some_and(|host| tailnet.owns(host))
            || self
                .route
                .hop_hosts
                .iter()
                .any(|hop| tailnet.find(hop, &[]).is_some() || tailnet.owns(hop));
        let id = tailnet.network.as_ref().map(|network| network.id);
        let network = if on {
            id
        } else if !tailnet.is_up() {
            // Peers may be missing while Tailscale is down, so a link keeps
            // the network it had.
            self.link.network
        } else {
            None
        };
        // Online flags only mean something while Tailscale is up.
        let peer = own
            .filter(|_| tailnet.is_up())
            .map(|peer| peer.peer.clone());
        (network, peer)
    }

    /// Drops the SSH master after a connection went quiet.
    async fn forget_master(&self) {
        if let Route::Ssh { host } = &self.link.route {
            self.shared.transport.forget(host).await;
        }
    }

    /// Shows an attempt in flight.
    async fn set_connecting(&mut self, step: ConnectStep) {
        let attempt = self.attempt.max(1);
        self.set_state(LinkState::Connecting { step, attempt })
            .await;
    }

    /// Changes the state and reports it.
    async fn set_state(&mut self, state: LinkState) {
        if self.link.state != state {
            self.link.state = state;
            self.emit_link().await;
        }
    }

    /// Reports the link.
    async fn emit_link(&mut self) {
        let link = Box::new(self.link.clone());
        self.emit(LinkEvent::Link(link)).await;
    }

    /// Sends an event. With nobody listening the link has no reason to run,
    /// so it stops. A forward closes first when its port or agent went.
    async fn emit(&mut self, event: LinkEvent) {
        self.forwards.apply(&event);
        if self.events.send(event).await.is_err() {
            self.cancel.cancel();
        }
    }
}

impl Drop for Task {
    /// Closes the book, the transfers and the tunnel, so whatever still
    /// waits on this link stops.
    fn drop(&mut self) {
        self.book.lock().shut();
        self.transfers.shut();
        self.tunnel.dropped();
    }
}

/// Dials `route` and waits for the hello, for a route raced against
/// others. A hello from a device other than `known` is refused.
async fn try_route(
    shared: &Shared,
    route: &Route,
    transfers: Arc<Transfers>,
    forwards: Arc<Forwards>,
    known: Option<DeviceId>,
) -> Result<(Wire, ServerHello), Failure> {
    let conn = shared.transport.dial(route, None).await?;
    let mut wire = Wire::new(conn, transfers, forwards);
    let hello = tokio::time::timeout(shared.timing.handshake, handshake(&mut wire, shared))
        .await
        .map_err(|_| no_hello(route))??;
    same_device(known, route, &hello)?;
    Ok((wire, hello))
}

/// Refuses a hello from a device other than `known`, so a reused LAN
/// address can't stand in for it, and neither can a device that says
/// another's ID.
fn same_device(known: Option<DeviceId>, route: &Route, hello: &ServerHello) -> Result<(), Failure> {
    match known {
        Some(known) if known != hello.device => Err(Failure::refused(format!(
            "{} reports a different device ID. If this device was reinstalled, \
             remove it and add it again to trust the new ID.",
            route_name(route)
        ))),
        _ => Ok(()),
    }
}

/// Why `maxd` didn't say hello in time.
fn no_hello(route: &Route) -> Failure {
    Failure {
        reason: "maxd didn't answer".to_owned(),
        host_down: matches!(route, Route::Ssh { .. }),
        last_seen: None,
        mismatch: None,
    }
}

/// Why a link with no route to try failed.
fn no_route() -> Failure {
    Failure::refused("the device has no route")
}

impl From<DialError> for Failure {
    fn from(err: DialError) -> Self {
        Self {
            reason: err.reason,
            host_down: err.host_down,
            last_seen: None,
            mismatch: None,
        }
    }
}

/// A route as you'd name it.
fn route_name(route: &Route) -> &str {
    match route {
        Route::Local => "this computer",
        Route::Ssh { host } => host,
    }
}

/// Says hello and waits for `maxd`'s.
async fn handshake(wire: &mut Wire, shared: &Shared) -> Result<ServerHello, Failure> {
    post(
        &wire.queue,
        ClientMessage::Hello(ClientHello {
            protocol: ProtocolRange::OURS,
            client: shared.client.clone(),
            client_id: shared.client_id,
            role: shared.role,
        }),
    );
    loop {
        match wire.inbound.next().await {
            Ok(ServerMessage::Hello(hello)) => {
                if ProtocolRange::OURS.agree(hello.protocol).is_none() {
                    return Err(Failure::mismatched(
                        incompatible(hello.protocol),
                        Some(hello.protocol),
                    ));
                }
                return Ok(hello);
            }
            Ok(ServerMessage::Error {
                code: ErrorCode::UnsupportedProtocol,
                message,
            }) => {
                debug!(%message, "maxd refused the protocol");
                return Err(Failure::mismatched(
                    "maxd speaks a different protocol version",
                    ProtocolRange::from_refusal(&message),
                ));
            }
            Ok(_) => {}
            Err(ReadError::Eof | ReadError::Io(_)) => {
                return Err(Failure::refused(wire.rest.closed_reason().await));
            }
            Err(ReadError::Bad(err)) => {
                return Err(Failure::refused(format!("maxd sent a bad frame: {err}")));
            }
        }
    }
}

/// The reason for a protocol range with nothing in common.
fn incompatible(theirs: ProtocolRange) -> String {
    if theirs.oldest > ProtocolRange::OURS.current {
        "maxd is newer than this app, so update the app".to_owned()
    } else {
        "maxd is older than this app, so update maxd".to_owned()
    }
}

/// The reason a network state blocks an attempt, if it does.
fn network_reason(state: NetworkState) -> Option<&'static str> {
    match state {
        NetworkState::Off => Some("Tailscale is off"),
        NetworkState::NeedsLogin => Some("Tailscale needs you to log in"),
        NetworkState::NeedsApproval => Some("Tailscale is waiting for approval"),
        NetworkState::NotRunning => Some("Tailscale isn't running"),
        NetworkState::Up | NetworkState::Starting | NetworkState::Unknown => None,
    }
}

/// A host without its `user@`.
fn bare_host(destination: &str) -> String {
    destination
        .rsplit_once('@')
        .map_or(destination, |(_, host)| host)
        .to_owned()
}

/// A session end for a closed or failed connection.
fn dropped(reason: impl Into<String>) -> End {
    End::Dropped {
        reason: reason.into(),
        quiet: false,
    }
}

/// A session end for a connection that went quiet.
fn quiet(reason: impl Into<String>) -> End {
    End::Dropped {
        reason: reason.into(),
        quiet: true,
    }
}

/// Sleeps until a deadline, or forever without one.
async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// Whether a connection still proving itself acts on `message` now:
/// results, pongs, refusals and stray hellos. Anything else waits for it to
/// open.
fn answers_proof(message: &ServerMessage) -> bool {
    matches!(
        message,
        ServerMessage::ActionResult { .. }
            | ServerMessage::Pong { .. }
            | ServerMessage::Error { .. }
            | ServerMessage::Hello(_)
    )
}

/// The verdict of the proof in flight, or never without one.
async fn verdict(running: &mut Option<BoxFuture<'static, Verdict>>) -> Verdict {
    match running {
        Some(running) => running.await,
        None => std::future::pending().await,
    }
}

/// Waits for `notify`, or forever without one.
async fn notified(notify: Option<&Notify>) {
    match notify {
        Some(notify) => notify.notified().await,
        None => std::future::pending().await,
    }
}
