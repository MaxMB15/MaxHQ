// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Links between devices' `maxd`, so an agent on one device can start,
//! message, read and wait on agents on another with the app closed.
//!
//! A link carries agent tool calls, wakes and their answers, and nothing
//! else. Each side checks a call against its own copy of the caller's
//! permission when it arrives. Links open for a call, over pinned SSH with
//! the device's own key, and close after [`IDLE`] with no call open. The
//! device at the other end serves a link only once it signs with that
//! key, so a program there can't pass for the device that dialed. A
//! device that takes no SSH logins only dials, and keeps a link up to the
//! home of each permission that names it, so calls reach it the other way
//! over that link. If both sides dial, the link the lower device id
//! dialed wins, and the other closes once nothing uses it.
//!
//! A link also carries model streams: a local agent here whose model is
//! on a Mac's managed server asks that Mac for a stream with
//! [`PeerRequest::ModelStream`], and the stream's bytes then travel in
//! tunnel frames over the same link, with credit, as a port forward's do.

mod key;
mod proof;
mod serve;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use maxhq_client::pinned::{PinnedSsh, Pins};
use maxhq_client::{Client, LinkError, LinkEvent, LinkHandle, LinkSpec};
use maxhq_core::authorized_keys;
use maxhq_protocol::agents::{
    AgentMoveStep, PeerBook, PeerCall, PeerEntry, PeerRequest, PeerWake, ToolAnswer,
};
use maxhq_protocol::frame;
use maxhq_protocol::ids::{AgentId, DeviceId, IdempotencyKey, LinkId, ModelServerId, StreamId};
use maxhq_protocol::link::{LinkState, Route};
use maxhq_protocol::local_models::LinkedModels;
use maxhq_protocol::message::{ActionOutcome, TunnelEnd};
use maxhq_protocol::{ClientMessage, ServerMessage};
use maxhq_wire::Queue;
use tokio::io::DuplexStream;
use tokio::sync::{mpsc, oneshot, watch, Semaphore};
use tracing::{debug, info, warn};

use crate::agents::{Answer, PeerCalls, Stepped, Unreachable, Woken};
use crate::db::Db;
use crate::secrets::files::Folder;
use crate::session::clock::Clock;
use crate::session::Sessions;
use crate::tunnel;

pub use serve::serve;

/// How long a link stays up with no call or wait open.
pub const IDLE: Duration = Duration::from_secs(10 * 60);

/// How often idle links are closed and held ones checked.
const SWEEP_EVERY: Duration = Duration::from_secs(60);

/// How long a call waits for a link to come up.
const CONNECT_WITHIN: Duration = Duration::from_secs(30);

/// How long a look at whether a device is reachable waits for its link,
/// short enough for a webhook's sender to get an answer.
const REACH_WITHIN: Duration = Duration::from_secs(5);

/// How long a request sent the other way waits for its answer, which
/// covers the longest agent wait.
const ANSWER_WITHIN: Duration = Duration::from_secs(3720);

/// How many answers are kept, so a request sent again after a dropped
/// link gets the same answer and doesn't run twice.
const ANSWERS_KEPT: usize = 256;

/// How many requests one connection can have running at once. Each is one
/// agent's blocking call, however long it waits, so a real link stays far
/// under this even with every agent on a device waiting on the same peer.
const MAX_REQUESTS: usize = 1024;

/// How many bytes of control messages may wait behind one link's full
/// queue. A link this far behind isn't reading, so it closes. A real link
/// waits on a handful of requests at most.
const MAX_WAITING: usize = 16 * 1024 * 1024;

/// The file under the SSH folder that holds the other devices' host keys.
const KNOWN_HOSTS: &str = "known_hosts";

/// How long opening a model stream waits for its link and the answer,
/// so a turn whose Mac is gone fails soon instead of hanging. Tests wait
/// less, so a dropped link's test doesn't take as long.
const STREAM_WITHIN: Duration = Duration::from_secs(if cfg!(test) { 5 } else { 15 });

/// How many bytes a model stream's pipe holds each way.
const PIPE: usize = 64 * 1024;

/// What a model stream is refused with on a device that shares no models.
const NO_MODELS: &str = "This device shares no models.";

/// What serves the model streams other devices open to this one.
pub trait Models: Send + Sync + 'static {
    /// Serves the model server `server` on `socket` for a local agent on
    /// `from`, or says why it won't.
    fn serve(
        &self,
        from: DeviceId,
        server: ModelServerId,
        socket: DuplexStream,
    ) -> Result<(), String>;
    /// Keeps `models` on `server` loaded for agents on `from` that still
    /// work between their requests.
    fn hold(&self, from: DeviceId, server: ModelServerId, models: &[String]) -> Result<(), String>;
}

/// A model server another device lets this one's local agents use, as
/// the book says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedModels {
    /// The device that runs it.
    pub device: DeviceId,
    /// That device's name.
    pub name: String,
    /// The server and its models.
    pub models: LinkedModels,
}

/// What runs a request another device sent.
pub trait Serve: Send + Sync + 'static {
    /// Runs an agent tool call for a caller on `from`.
    fn call(&self, from: DeviceId, call: PeerCall) -> BoxFuture<'_, ToolAnswer>;
    /// Tells a caller here that an agent it started on `from` finished a
    /// turn.
    fn wake(&self, from: DeviceId, wake: PeerWake) -> BoxFuture<'_, Result<(), String>>;
    /// Keeps what `from` said: the agents here its enabled tasks reach.
    fn task_targets(&self, from: DeviceId, agents: Vec<AgentId>) -> Result<(), String>;
    /// Runs a step of moving an agent that `from`, the device that runs
    /// the MaxHQ app, asked for.
    fn move_step(&self, from: DeviceId, step: AgentMoveStep) -> BoxFuture<'_, ActionOutcome>;
}

impl Serve for Sessions {
    fn call(&self, from: DeviceId, call: PeerCall) -> BoxFuture<'_, ToolAnswer> {
        Box::pin(self.peer_call(from, call))
    }

    fn wake(&self, from: DeviceId, wake: PeerWake) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(self.peer_wake(from, wake))
    }

    fn task_targets(&self, from: DeviceId, agents: Vec<AgentId>) -> Result<(), String> {
        Sessions::task_targets(self, from, agents)
    }

    fn move_step(&self, from: DeviceId, step: AgentMoveStep) -> BoxFuture<'_, ActionOutcome> {
        Box::pin(self.peer_move(from, step))
    }
}

/// Where the peer links keep their files.
#[derive(Clone, Debug)]
pub struct Files {
    /// The 0700 folder with the device's key and the other devices' host
    /// keys, `~/.config/maxhq/ssh`.
    pub ssh: PathBuf,
    /// The home folder, whose `.ssh/authorized_keys` lets other devices in
    /// and whose `.local/bin/maxd` their key runs.
    pub home: PathBuf,
}

impl Files {
    /// The real places, or none without a home folder.
    pub fn system() -> Option<Self> {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        Some(Self {
            ssh: home.join(".config").join("maxhq").join("ssh"),
            home,
        })
    }

    fn authorized_keys(&self) -> PathBuf {
        self.home.join(".ssh").join("authorized_keys")
    }
}

/// The links to other devices. Clones share them.
#[derive(Clone)]
pub struct Peers(Arc<Inner>);

struct Inner {
    me: DeviceId,
    files: Files,
    db: Db,
    clock: Arc<dyn Clock>,
    client: Client,
    pins: Pins,
    serve: OnceLock<Arc<dyn Serve>>,
    /// The notes other devices' note requests read and write.
    notes: OnceLock<crate::notes::Notes>,
    /// What serves other devices' model streams.
    models: OnceLock<Arc<dyn Models>>,
    state: Mutex<State>,
    /// The devices a link is up to, either way.
    links: watch::Sender<BTreeSet<DeviceId>>,
    /// The model servers other devices share with this one.
    shared: watch::Sender<Vec<SharedModels>>,
}

#[derive(Default)]
struct State {
    book: PeerBook,
    /// The links this device dialed.
    out: HashMap<DeviceId, Out>,
    /// The links other devices dialed, by the device.
    incoming: HashMap<DeviceId, Incoming>,
    /// Requests sent the other way, waiting for their answers.
    pending: HashMap<IdempotencyKey, Pending>,
    /// Answers to requests that came in, by who sent each, newest last.
    answered: VecDeque<((DeviceId, IdempotencyKey), ActionOutcome)>,
    /// Requests that came in and are running, by who sent each, which one
    /// sent again waits for.
    running: HashMap<(DeviceId, IdempotencyKey), watch::Receiver<Option<ActionOutcome>>>,
    /// The route that last worked to each device.
    last_good: HashMap<DeviceId, Route>,
    /// Tells incoming links apart, so an old one's end doesn't drop a new
    /// one.
    serial: u64,
    /// The model streams on links other devices opened.
    streams: HashMap<StreamId, Carried>,
}

/// A model stream on a link another device opened, whichever side asked
/// for it.
struct Carried {
    device: DeviceId,
    /// The link's serial: the stream ends with it.
    serial: u64,
    end: tunnel::End,
}

struct Out {
    handle: Arc<LinkHandle>,
    up: watch::Receiver<bool>,
    /// Calls and waits open on it.
    open: usize,
    /// When the last one ended.
    used: Instant,
}

struct Incoming {
    control: Control,
    serial: u64,
    /// Where its model streams send, behind everything sent before.
    sink: Arc<dyn tunnel::Sink<ServerMessage>>,
}

struct Pending {
    device: DeviceId,
    request: PeerRequest,
    done: oneshot::Sender<ActionOutcome>,
}

/// How a request goes to a device.
enum Way {
    Out(Arc<LinkHandle>),
    Incoming,
}

/// The link a request came in on, which a model stream it opens goes back
/// over.
#[derive(Clone)]
pub(crate) enum Via {
    /// A link the sender opened, by its serial.
    Incoming(u64),
    /// A link this device opened.
    Out(Weak<LinkHandle>),
    /// Neither, as for a request a test makes.
    #[cfg(test)]
    Nowhere,
}

impl Peers {
    /// The links for device `me`, dialing through `client`, which
    /// [`Client::peers`] or [`Client::peers_over`] made over `pins`.
    pub fn new(
        me: DeviceId,
        files: Files,
        db: Db,
        clock: Arc<dyn Clock>,
        client: Client,
        pins: Pins,
    ) -> Self {
        let client = client.proving(Arc::new(proof::PeerProver {
            folder: files.ssh.clone(),
        }));
        let peers = Self(Arc::new(Inner {
            me,
            files,
            db,
            clock,
            client,
            pins,
            serve: OnceLock::new(),
            notes: OnceLock::new(),
            models: OnceLock::new(),
            state: Mutex::new(State::default()),
            links: watch::Sender::new(BTreeSet::new()),
            shared: watch::Sender::new(Vec::new()),
        }));
        match peers.0.db.peer_book() {
            Ok(Some(book)) => peers.take_book(book),
            Ok(None) => {}
            Err(err) => warn!(%err, "couldn't read the peer book"),
        }
        peers
    }

    /// Has requests from other devices run on `serve`, which starts after
    /// the links. Only the first call counts.
    pub fn serve_with(&self, serve: Arc<dyn Serve>) {
        let _ = self.0.serve.set(serve);
    }

    /// Has other devices' note requests read and write `notes`, and starts
    /// swapping them with those devices. Only the first call counts.
    pub fn notes_with(&self, notes: crate::notes::Notes) {
        if self.0.notes.set(notes.clone()).is_ok() {
            notes.sync_with(self.clone());
        }
    }

    /// Has other devices' model streams served by `models`. Only the
    /// first call counts.
    pub fn models_with(&self, models: Arc<dyn Models>) {
        let _ = self.0.models.set(models);
    }

    /// The model servers other devices share with this one, which change
    /// with the book.
    pub fn shared_models(&self) -> watch::Receiver<Vec<SharedModels>> {
        self.0.shared.subscribe()
    }

    /// The devices notes go to: those with routes to dial, and those with
    /// a link up now.
    pub(crate) fn note_targets(&self) -> BTreeSet<DeviceId> {
        let dials: Vec<DeviceId> = self
            .lock()
            .book
            .peers
            .iter()
            .filter(|peer| !peer.routes.is_empty())
            .map(|peer| peer.device)
            .collect();
        let mut targets: BTreeSet<DeviceId> = self.0.links.borrow().clone();
        targets.extend(dials);
        targets.remove(&self.0.me);
        targets
    }

    /// Closes idle links and keeps held ones up, until `maxd` stops.
    pub async fn run(self) {
        loop {
            self.sweep();
            let at = self.0.clock.now() + SWEEP_EVERY;
            self.0.clock.sleep_until(at).await;
        }
    }

    /// The devices a link is up to, either way, which changes as links
    /// come and go.
    pub fn links(&self) -> watch::Receiver<BTreeSet<DeviceId>> {
        self.0.links.subscribe()
    }

    /// Tells [`Peers::links`] which links are up now.
    fn relink(&self, state: &State) {
        let up: BTreeSet<DeviceId> = state
            .incoming
            .keys()
            .copied()
            .chain(
                state
                    .out
                    .iter()
                    .filter(|(_, out)| *out.up.borrow())
                    .map(|(device, _)| *device),
            )
            .collect();
        self.0.links.send_if_modified(|links| {
            let changed = *links != up;
            *links = up;
            changed
        });
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.state.lock().expect("peers lock")
    }

    /// The public half of the device's own key, made the first time.
    pub fn public_key(&self) -> io::Result<String> {
        key::public_key(&Folder::open(self.0.files.ssh.clone())?)
    }

    /// Lets `device`'s key in to run only `maxd proxy --peer <device>`.
    pub fn authorize(&self, device: DeviceId, public_key: &str) -> Result<(), String> {
        let Some(public_key) = crate::sshkey::ed25519_line(public_key) else {
            return Err("That isn't an ed25519 public key.".into());
        };
        let home = self.0.files.home.display().to_string();
        let line = authorized_keys::line(&home, &public_key, Some(&device.to_string()))?;
        // One key per device: a new one replaces the old.
        let file = self.0.files.authorized_keys();
        let tag = authorized_keys::peer_tag(&device.to_string());
        let kept =
            authorized_keys::remove(&file, &tag).and_then(|_| authorized_keys::add(&file, &line));
        kept.map(|_| info!(%device, "let a device's key in"))
            .map_err(|err| format!("Couldn't write authorized_keys: {err}"))
    }

    /// Whether `key` is one `device` may log in with, as its lines in
    /// `authorized_keys` say.
    fn lets_in(&self, device: DeviceId, key: [u8; 32]) -> io::Result<bool> {
        let tag = authorized_keys::peer_tag(&device.to_string());
        let keys = authorized_keys::keys_tagged(&self.0.files.authorized_keys(), &tag)?;
        Ok(keys
            .iter()
            .any(|line| crate::sshkey::ed25519_key(line) == Some(key)))
    }

    /// Takes out `device`'s key and closes its links.
    pub fn revoke(&self, device: DeviceId) -> Result<(), String> {
        let tag = authorized_keys::peer_tag(&device.to_string());
        authorized_keys::remove(&self.0.files.authorized_keys(), &tag)
            .map_err(|err| format!("Couldn't write authorized_keys: {err}"))?;
        let mut state = self.lock();
        state.out.remove(&device);
        if let Some(incoming) = state.incoming.remove(&device) {
            incoming.control.wire.close();
        }
        state.streams.retain(|_, carried| carried.device != device);
        info!(%device, "took out a device's key");
        Ok(())
    }

    /// Replaces the devices this one may link to, with their host keys.
    pub fn set_book(&self, book: PeerBook) -> Result<(), String> {
        let usable = |peer: &PeerEntry| {
            let undialed = peer.routes.is_empty() && peer.host_key.is_empty();
            undialed || crate::sshkey::ed25519_line(&peer.host_key).is_some()
        };
        if let Some(bad) = book
            .peers
            .iter()
            .find(|peer| !usable(peer) || peer.device == self.0.me)
        {
            return Err(format!("{} has no usable host key.", bad.name));
        }
        self.0
            .db
            .set_peer_book(&book)
            .map_err(|err| format!("Couldn't store the peer book: {err}"))?;
        self.take_book(book);
        self.hold_for_models();
        Ok(())
    }

    /// The devices whose local agents use this one's models, which it
    /// keeps a link up to when they can't dial it.
    fn model_users(&self, state: &State) -> BTreeSet<DeviceId> {
        if state.book.ssh {
            return BTreeSet::new();
        }
        let me = self.0.me;
        state
            .book
            .peers
            .iter()
            .filter(|peer| peer.models_from.contains(&me) && !peer.routes.is_empty())
            .map(|peer| peer.device)
            .collect()
    }

    /// Dials the devices whose local agents use this one's models, so a
    /// new agent's first turn doesn't wait for the next sweep.
    fn hold_for_models(&self) {
        let mut state = self.lock();
        let users = self.model_users(&state);
        let mut dialed = false;
        for device in users {
            if !state.out.contains_key(&device) && !state.incoming.contains_key(&device) {
                let out = self.dial(&mut state, device);
                state.out.insert(device, out);
                dialed = true;
            }
        }
        if dialed {
            self.relink(&state);
        }
    }

    /// Uses `book`: its host keys go to `known_hosts`, its routes become
    /// pins, and links to devices that left it close.
    fn take_book(&self, book: PeerBook) {
        let ssh = &self.0.files.ssh;
        let known_hosts = ssh.join(KNOWN_HOSTS);
        let mut lines = String::new();
        let mut pins = HashMap::new();
        for peer in &book.peers {
            let alias = maxhq_client::setup::host_key_alias(&peer.device.to_string());
            if let Some(key) = crate::sshkey::ed25519_line(&peer.host_key) {
                lines.push_str(&format!("{alias} {key}\n"));
            }
            for (n, route) in peer.routes.iter().enumerate() {
                pins.insert(
                    route_host(peer.device, n),
                    PinnedSsh {
                        host: route.host.clone(),
                        port: route.port,
                        user: route.user.clone(),
                        key_file: ssh.join(key::KEY_FILE),
                        known_hosts: known_hosts.clone(),
                        alias: alias.clone(),
                    },
                );
            }
        }
        if let Err(err) =
            Folder::open(ssh.clone()).and_then(|folder| folder.write(KNOWN_HOSTS, lines.as_bytes()))
        {
            warn!(%err, "couldn't write the peers' host keys");
        }
        self.0.pins.set(pins);
        let shared: Vec<SharedModels> = book
            .peers
            .iter()
            .filter_map(|peer| {
                Some(SharedModels {
                    device: peer.device,
                    name: peer.name.clone(),
                    models: peer.model_server.clone()?,
                })
            })
            .collect();
        let mut state = self.lock();
        let kept: BTreeSet<DeviceId> = book.peers.iter().map(|peer| peer.device).collect();
        // A changed route or host key takes a new link.
        state.out.clear();
        state.incoming.retain(|device, _| kept.contains(device));
        state
            .streams
            .retain(|_, carried| kept.contains(&carried.device));
        state.book = book;
        drop(state);
        self.0.shared.send_if_modified(|now| {
            let changed = *now != shared;
            *now = shared;
            changed
        });
    }

    /// Sends `request` to `device` and waits for what came of it.
    pub(crate) async fn request(
        &self,
        device: DeviceId,
        request: PeerRequest,
    ) -> Result<ActionOutcome, Unreachable> {
        let key = IdempotencyKey::generate();
        match self.way(device, CONNECT_WITHIN).await? {
            Way::Out(handle) => {
                let _open = Open::new(self.clone(), device);
                handle
                    .act(ClientMessage::PeerRequest { key, request })
                    .await
                    .map_err(|err| self.unreachable(device, &err))
            }
            Way::Incoming => self.ask_back(device, key, request, ANSWER_WITHIN).await,
        }
    }

    /// Opens a stream to `server`, a model server on `device`, for a local
    /// agent here. It gives the near end of a pipe whose far end goes over
    /// the link, and what keeps the link up while the stream is in use.
    /// It fails within [`STREAM_WITHIN`] when the device can't be reached,
    /// and with the device's reason when it refuses.
    pub(crate) async fn model_stream(
        &self,
        device: DeviceId,
        server: ModelServerId,
    ) -> Result<(DuplexStream, Open), String> {
        let stream = StreamId::generate();
        let key = IdempotencyKey::generate();
        let request = PeerRequest::ModelStream { stream, server };
        let (near, far) = tokio::io::duplex(PIPE);
        let cant = || cant_reach(&self.name(device));
        // A device that takes no SSH logins dials this one once it hears
        // that agents here use its models, so wait a little for that.
        let waits = {
            let state = self.lock();
            !state.incoming.contains_key(&device)
                && state
                    .book
                    .peers
                    .iter()
                    .any(|peer| peer.device == device && peer.routes.is_empty())
        };
        if waits {
            let mut links = self.links();
            let came =
                tokio::time::timeout(STREAM_WITHIN, links.wait_for(|up| up.contains(&device)))
                    .await;
            if !matches!(came, Ok(Ok(_))) {
                return Err(cant());
            }
        }
        match self.way(device, STREAM_WITHIN).await.map_err(|err| err.0)? {
            Way::Out(handle) => {
                let up = self.lock().out.get(&device).map(|out| out.up.clone());
                let Some(mut up) = up else {
                    return Err(cant());
                };
                handle.attach_stream(stream, far)?;
                let open = Open::new(self.clone(), device);
                let asked = tokio::select! {
                    asked = handle.act(ClientMessage::PeerRequest { key, request }) => {
                        asked.map_err(|err| self.unreachable(device, &err).0)
                    }
                    () = tokio::time::sleep(STREAM_WITHIN) => Err(cant()),
                    _ = up.wait_for(|up| !*up) => Err(cant()),
                };
                match asked.and_then(|outcome| self.opened(device, outcome)) {
                    Ok(()) => Ok((near, open)),
                    Err(reason) => {
                        handle.forget_stream(stream);
                        Err(reason)
                    }
                }
            }
            Way::Incoming => {
                let serial = self
                    .lock()
                    .incoming
                    .get(&device)
                    .map(|incoming| incoming.serial);
                let Some(serial) = serial else {
                    return Err(cant());
                };
                let (open, opened) = oneshot::channel::<DuplexStream>();
                self.carry(device, serial, stream, async move {
                    opened
                        .await
                        .map_err(|_| "the stream wasn't opened".to_owned())
                })?;
                let mut links = self.links();
                let asked = tokio::select! {
                    asked = self.ask_back(device, key, request, STREAM_WITHIN) => {
                        asked.map_err(|_| cant())
                    }
                    _ = links.wait_for(|up| !up.contains(&device)) => Err(cant()),
                };
                match asked.and_then(|outcome| self.opened(device, outcome)) {
                    Ok(()) => {
                        let _ = open.send(far);
                        Ok((near, Open::new(self.clone(), device)))
                    }
                    Err(reason) => {
                        // Gone before the pipe is, so the stream ends
                        // quietly instead of refusing.
                        self.lock().streams.remove(&stream);
                        drop(open);
                        Err(reason)
                    }
                }
            }
        }
    }

    /// Whether a model stream's answer opened it, or why not.
    fn opened(&self, device: DeviceId, outcome: ActionOutcome) -> Result<(), String> {
        match outcome {
            ActionOutcome::Done => Ok(()),
            ActionOutcome::Refused { reason } => Err(reason),
            other => Err(format!("{} answered oddly: {other:?}", self.name(device))),
        }
    }

    /// Asks `device` to keep `models` on `server` loaded, for local agents
    /// here that still work between their requests.
    pub(crate) async fn hold_models(
        &self,
        device: DeviceId,
        server: ModelServerId,
        models: Vec<String>,
    ) -> Result<(), String> {
        let request = PeerRequest::ModelHold { server, models };
        match self.request(device, request).await.map_err(|err| err.0)? {
            ActionOutcome::Refused { reason } => Err(reason),
            _ => Ok(()),
        }
    }

    /// Pumps a model stream on the link `device` opened with `serial`,
    /// once `socket` gives the pipe.
    fn carry(
        &self,
        device: DeviceId,
        serial: u64,
        stream: StreamId,
        socket: impl std::future::Future<Output = Result<DuplexStream, String>> + Send + 'static,
    ) -> Result<(), String> {
        let mut state = self.lock();
        let sink = state
            .incoming
            .get(&device)
            .filter(|incoming| incoming.serial == serial)
            .map(|incoming| Arc::clone(&incoming.sink))
            .ok_or_else(|| format!("The link to {} closed.", self.name_in(&state, device)))?;
        if state.streams.contains_key(&stream) {
            return Err("That stream is already open.".into());
        }
        let open = state
            .streams
            .values()
            .filter(|carried| carried.device == device)
            .count();
        if open >= tunnel::MAX_STREAMS_PER_CLIENT {
            return Err("Too many streams are open on this link.".into());
        }
        let inner = Arc::downgrade(&self.0);
        let ended = Box::new(move || {
            let Some(inner) = inner.upgrade() else {
                return;
            };
            let mut state = inner.state.lock().expect("peers lock");
            if state
                .streams
                .get(&stream)
                .is_some_and(|carried| carried.device == device && carried.serial == serial)
            {
                state.streams.remove(&stream);
            }
        });
        let end = tunnel::open::<tunnel::ToClient, DuplexStream>(stream, socket, sink, ended);
        state.streams.insert(
            stream,
            Carried {
                device,
                serial,
                end,
            },
        );
        Ok(())
    }

    /// Takes a model stream's bytes from the link `device` opened. Bytes
    /// past the credit reset the stream.
    fn tunnel_data(&self, device: DeviceId, serial: u64, stream: StreamId, data: &[u8]) {
        let mut state = self.lock();
        let Some(carried) = state
            .streams
            .get(&stream)
            .filter(|carried| carried.device == device && carried.serial == serial)
        else {
            debug!(%stream, "dropped bytes for a model stream that isn't open");
            return;
        };
        if carried.end.data(data) {
            return;
        }
        debug!(%stream, %device, "reset a model stream a peer sent too much on");
        state.streams.remove(&stream);
        if let Some(incoming) = state.incoming.get(&device) {
            let _ = incoming
                .sink
                .send(tunnel::Out::Message(ServerMessage::TunnelClose {
                    stream,
                    how: TunnelEnd::Reset,
                }));
        }
    }

    /// The far side of a model stream may send more.
    fn tunnel_credit(&self, device: DeviceId, serial: u64, stream: StreamId, granted: u64) {
        let state = self.lock();
        if let Some(carried) = state
            .streams
            .get(&stream)
            .filter(|carried| carried.device == device && carried.serial == serial)
        {
            carried.end.credit(granted);
        }
    }

    /// The far side of a model stream closed its half, or reset it.
    fn tunnel_close(&self, device: DeviceId, serial: u64, stream: StreamId, how: TunnelEnd) {
        let mut state = self.lock();
        let ours = state
            .streams
            .get(&stream)
            .is_some_and(|carried| carried.device == device && carried.serial == serial);
        if !ours {
            return;
        }
        match how {
            TunnelEnd::Reset => {
                state.streams.remove(&stream);
            }
            TunnelEnd::Write => state.streams[&stream].end.close(TunnelEnd::Write),
        }
    }

    /// The link a request to `device` takes, dialing one if it has to and
    /// waiting `within` for it to come up.
    async fn way(&self, device: DeviceId, within: Duration) -> Result<Way, Unreachable> {
        let mut up = {
            let mut state = self.lock();
            let Some(peer) = state.book.peers.iter().find(|peer| peer.device == device) else {
                return Err(Unreachable(
                    "That device isn't one this device links to. Open MaxHQ to update its devices."
                        .into(),
                ));
            };
            let name = peer.name.clone();
            let dials = !peer.routes.is_empty();
            let incoming = state.incoming.contains_key(&device);
            let ours = self.0.me < device;
            if let Some(out) = state.out.get(&device)
                && *out.up.borrow()
                && (ours || !incoming)
            {
                return Ok(Way::Out(Arc::clone(&out.handle)));
            }
            if incoming {
                return Ok(Way::Incoming);
            }
            if !dials {
                return Err(Unreachable(format!(
                    "{name} takes no SSH logins and has no link open to this device. Open MaxHQ on {name}."
                )));
            }
            if !state.out.contains_key(&device) {
                let out = self.dial(&mut state, device);
                state.out.insert(device, out);
            }
            state.out[&device].up.clone()
        };
        match tokio::time::timeout(within, up.wait_for(|up| *up)).await {
            Ok(Ok(_)) => {}
            _ => {
                return Err(Unreachable(cant_reach(&self.name(device))));
            }
        }
        let state = self.lock();
        match state.out.get(&device) {
            Some(out) => Ok(Way::Out(Arc::clone(&out.handle))),
            None => Err(Unreachable(format!(
                "The link to {} closed.",
                self.name_in(&state, device)
            ))),
        }
    }

    /// Opens a link to `device` over its routes, and starts reading what
    /// it reports.
    fn dial(&self, state: &mut State, device: DeviceId) -> Out {
        let routes = state
            .book
            .peers
            .iter()
            .find(|peer| peer.device == device)
            .map_or(0, |peer| peer.routes.len());
        let mut hosts = (0..routes).map(|n| Route::Ssh {
            host: route_host(device, n),
        });
        let route = hosts.next().unwrap_or(Route::Local);
        let spec = LinkSpec {
            id: LinkId::derive(&format!("peer-{device}")),
            name: self.name_in(state, device),
            route,
            fallbacks: hosts.collect(),
            last_good: state
                .last_good
                .get(&device)
                .map(|route| (String::new(), route.clone()))
                .into_iter()
                .collect::<BTreeMap<_, _>>(),
        };
        let (handle, events) = self.0.client.link_to(spec, device);
        let handle = Arc::new(handle);
        let (up_tx, up) = watch::channel(false);
        tokio::spawn(
            self.clone()
                .pump(device, Arc::downgrade(&handle), events, up_tx),
        );
        debug!(%device, "dialing a peer");
        Out {
            handle,
            up,
            open: 0,
            used: self.0.clock.now(),
        }
    }

    /// Reads a link's reports until it closes: its state, and requests the
    /// far side sends back over it.
    async fn pump(
        self,
        device: DeviceId,
        handle: Weak<LinkHandle>,
        mut events: tokio::sync::mpsc::Receiver<LinkEvent>,
        up: watch::Sender<bool>,
    ) {
        let running = Arc::new(Semaphore::new(MAX_REQUESTS));
        while let Some(event) = events.recv().await {
            match event {
                LinkEvent::Link(link) => {
                    let _ = up.send(matches!(link.state, LinkState::Connected { .. }));
                    self.relink(&self.lock());
                }
                LinkEvent::RouteWorked { route, .. } => {
                    self.lock().last_good.insert(device, route);
                }
                LinkEvent::PeerRequest { key, request } => {
                    let Ok(place) = Arc::clone(&running).try_acquire_owned() else {
                        warn!(%device, "a peer asked too much at once, so its link closes");
                        self.drop_link(device, &handle);
                        break;
                    };
                    // A request back over the link counts as using it.
                    let now = self.0.clock.now();
                    if let Some(out) = self.lock().out.get_mut(&device) {
                        out.used = now;
                    }
                    let peers = self.clone();
                    let handle = handle.clone();
                    tokio::spawn(async move {
                        let via = Via::Out(handle.clone());
                        let outcome = peers.answer_on(device, key, request, via).await;
                        if let Some(handle) = handle.upgrade() {
                            // A lost answer is asked for again on the next
                            // connect, and comes from the kept answers.
                            let _ = handle
                                .send(ClientMessage::PeerAnswer { key, outcome })
                                .await;
                        }
                        drop(place);
                    });
                }
                LinkEvent::Synced(_)
                | LinkEvent::Changed(_)
                | LinkEvent::Timeline { .. }
                | LinkEvent::Workflow { .. } => {}
            }
        }
        let _ = up.send(false);
        self.relink(&self.lock());
    }

    /// Drops the link to `device`, if `handle` is still the one in use.
    fn drop_link(&self, device: DeviceId, handle: &Weak<LinkHandle>) {
        let mut state = self.lock();
        let current = state
            .out
            .get(&device)
            .is_some_and(|out| Arc::as_ptr(&out.handle) == handle.as_ptr());
        if current {
            state.out.remove(&device);
        }
    }

    /// Sends `request` back over the link `device` opened, and waits for
    /// its answer through any reconnects.
    async fn ask_back(
        &self,
        device: DeviceId,
        key: IdempotencyKey,
        request: PeerRequest,
        within: Duration,
    ) -> Result<ActionOutcome, Unreachable> {
        let (done, answer) = oneshot::channel();
        {
            let mut state = self.lock();
            if let Some(incoming) = state.incoming.get(&device) {
                incoming.control.send(&ServerMessage::PeerRequest {
                    key,
                    request: request.clone(),
                });
            }
            state.pending.insert(
                key,
                Pending {
                    device,
                    request,
                    done,
                },
            );
        }
        let answered = tokio::time::timeout(within, answer).await;
        self.lock().pending.remove(&key);
        match answered {
            Ok(Ok(outcome)) => Ok(outcome),
            _ => Err(Unreachable(format!("{} didn't answer.", self.name(device)))),
        }
    }

    /// Takes the answer to a request sent back over an incoming link.
    pub fn answered(&self, device: DeviceId, key: IdempotencyKey, outcome: ActionOutcome) {
        let mut state = self.lock();
        if state
            .pending
            .get(&key)
            .is_some_and(|pending| pending.device == device)
            && let Some(pending) = state.pending.remove(&key)
        {
            let _ = pending.done.send(outcome);
        }
    }

    /// Runs a request `from` sent, once per key. A key only finds what the
    /// same device sent, so no device reads another's answer by naming its
    /// key.
    #[cfg(test)]
    pub async fn answer(
        &self,
        from: DeviceId,
        key: IdempotencyKey,
        request: PeerRequest,
    ) -> ActionOutcome {
        self.answer_on(from, key, request, Via::Nowhere).await
    }

    /// [`Peers::answer`], for a request that came in `via` a link, which a
    /// model stream it opens goes back over.
    pub(crate) async fn answer_on(
        &self,
        from: DeviceId,
        key: IdempotencyKey,
        request: PeerRequest,
        via: Via,
    ) -> ActionOutcome {
        let key = (from, key);
        let done = {
            let mut state = self.lock();
            if let Some((_, outcome)) = state.answered.iter().find(|(seen, _)| *seen == key) {
                return outcome.clone();
            }
            if let Some(running) = state.running.get(&key) {
                Err(running.clone())
            } else {
                let (done, running) = watch::channel(None);
                state.running.insert(key, running);
                Ok(done)
            }
        };
        let done = match done {
            Ok(done) => done,
            Err(mut running) => {
                return match running.wait_for(Option::is_some).await {
                    Ok(outcome) => outcome.clone().expect("an outcome"),
                    Err(_) => ActionOutcome::Refused {
                        reason: "maxd stopped before it answered.".into(),
                    },
                };
            }
        };
        let outcome = self.serve(from, request, via).await;
        let mut state = self.lock();
        state.running.remove(&key);
        if state.answered.len() == ANSWERS_KEPT {
            state.answered.pop_front();
        }
        state.answered.push_back((key, outcome.clone()));
        let _ = done.send(Some(outcome.clone()));
        outcome
    }

    /// Runs a request `from` sent.
    async fn serve(&self, from: DeviceId, request: PeerRequest, via: Via) -> ActionOutcome {
        match request {
            PeerRequest::ModelStream { stream, server } => {
                return self.serve_stream(from, stream, server, via);
            }
            PeerRequest::ModelHold { server, models } => {
                let held = match self.0.models.get() {
                    Some(serves) => serves.hold(from, server, &models),
                    None => Err(NO_MODELS.into()),
                };
                return done(held);
            }
            _ => {}
        }
        if let PeerRequest::NoteHashes { .. }
        | PeerRequest::NoteVersions { .. }
        | PeerRequest::PutNoteVersions { .. } = request
        {
            return match self.0.notes.get() {
                Some(notes) => notes.serve(from, request).await,
                None => ActionOutcome::Refused {
                    reason: crate::notes::NOT_KEPT.into(),
                },
            };
        }
        let Some(serve) = self.0.serve.get() else {
            return ActionOutcome::Refused {
                reason: "maxd is still starting.".into(),
            };
        };
        match request {
            PeerRequest::Call { call } => ActionOutcome::Tool {
                answer: serve.call(from, call).await,
            },
            PeerRequest::Wake { wake } => match serve.wake(from, wake).await {
                Ok(()) => ActionOutcome::Done,
                Err(reason) => ActionOutcome::Refused { reason },
            },
            PeerRequest::Move { step } if self.is_home(from) => serve.move_step(from, step).await,
            PeerRequest::Move { .. } => ActionOutcome::Refused {
                reason: "Only the device that runs the MaxHQ app moves agents here.".into(),
            },
            PeerRequest::NoteHashes { .. }
            | PeerRequest::NoteVersions { .. }
            | PeerRequest::PutNoteVersions { .. }
            | PeerRequest::ModelStream { .. }
            | PeerRequest::ModelHold { .. } => unreachable!("served above"),
            PeerRequest::TaskTargets { agents } => match serve.task_targets(from, agents) {
                Ok(()) => ActionOutcome::Done,
                Err(reason) => ActionOutcome::Refused { reason },
            },
            PeerRequest::Unknown => ActionOutcome::Refused {
                reason: "This maxd is too old for that; update it.".into(),
            },
        }
    }

    /// Opens a model stream `from` asked for on `server`, whose bytes go
    /// back over the link the request came `via`.
    fn serve_stream(
        &self,
        from: DeviceId,
        stream: StreamId,
        server: ModelServerId,
        via: Via,
    ) -> ActionOutcome {
        let Some(serves) = self.0.models.get() else {
            return done(Err(NO_MODELS.into()));
        };
        #[cfg(test)]
        if matches!(via, Via::Nowhere) {
            return done(Err("A model stream needs a link.".into()));
        }
        let (near, far) = tokio::io::duplex(PIPE);
        if let Err(reason) = serves.serve(from, server, near) {
            return done(Err(reason));
        }
        // When the stream can't be carried, `far` drops and the server's
        // end closes.
        done(match via {
            Via::Incoming(serial) => self.carry(from, serial, stream, async { Ok(far) }),
            Via::Out(handle) => match handle.upgrade() {
                Some(handle) => handle.attach_stream(stream, far),
                None => Err(format!("The link to {} closed.", self.name(from))),
            },
            #[cfg(test)]
            Via::Nowhere => unreachable!("refused above"),
        })
    }

    /// Records a link `device` opened, and sends it the requests that
    /// wait for it. The link counts until [`Peers::detach`] with the
    /// serial this returns.
    fn attach(&self, device: DeviceId, wire: maxhq_wire::Sender) -> u64 {
        let mut state = self.lock();
        state.serial += 1;
        let serial = state.serial;
        let control = Control::new(wire.clone());
        for (key, pending) in &state.pending {
            if pending.device == device {
                control.send(&ServerMessage::PeerRequest {
                    key: *key,
                    request: pending.request.clone(),
                });
            }
        }
        let sink: Arc<dyn tunnel::Sink<ServerMessage>> =
            Arc::new(tunnel::Queue::spawn(crate::server::ToClient(wire)));
        state.streams.retain(|_, carried| carried.device != device);
        state.incoming.insert(
            device,
            Incoming {
                control,
                serial,
                sink,
            },
        );
        self.relink(&state);
        serial
    }

    /// Forgets a link `device` opened, unless a newer one took its place,
    /// and ends the model streams on it.
    fn detach(&self, device: DeviceId, serial: u64) {
        let mut state = self.lock();
        state
            .streams
            .retain(|_, carried| carried.device != device || carried.serial != serial);
        if state
            .incoming
            .get(&device)
            .is_some_and(|incoming| incoming.serial == serial)
        {
            state.incoming.remove(&device);
            self.relink(&state);
        }
    }

    /// Whether `device` runs the MaxHQ app, as the book says.
    fn is_home(&self, device: DeviceId) -> bool {
        self.lock()
            .book
            .peers
            .iter()
            .any(|peer| peer.device == device && peer.home)
    }

    /// Whether `device` is in the book.
    fn knows(&self, device: DeviceId) -> bool {
        self.lock()
            .book
            .peers
            .iter()
            .any(|peer| peer.device == device)
    }

    /// Closes links nothing used for [`IDLE`], and ones that lost to a
    /// link the other side dialed, and opens the ones this device holds.
    pub fn sweep(&self) {
        let held = self.held();
        let now = self.0.clock.now();
        let me = self.0.me;
        let mut state = self.lock();
        let State { out, incoming, .. } = &mut *state;
        out.retain(|device, link| {
            let lost = *device < me && incoming.contains_key(device);
            let idle = now.saturating_duration_since(link.used) >= IDLE;
            link.open > 0 || (held.contains(device) && !lost) || !(idle || lost)
        });
        for device in held {
            let known = state
                .book
                .peers
                .iter()
                .any(|peer| peer.device == device && !peer.routes.is_empty());
            if known && !state.out.contains_key(&device) && !state.incoming.contains_key(&device) {
                let link = self.dial(&mut state, device);
                state.out.insert(device, link);
            }
        }
        self.relink(&state);
    }

    /// The devices this one keeps a link up to: when it takes no SSH
    /// logins, the home of each permission that names it, and each device
    /// whose local agents use its models.
    fn held(&self) -> BTreeSet<DeviceId> {
        let users = {
            let state = self.lock();
            if state.book.ssh {
                return BTreeSet::new();
            }
            self.model_users(&state)
        };
        let me = self.0.me;
        let mut held = match self.0.db.permissions() {
            Ok(permissions) => permissions
                .into_iter()
                .map(|(permission, _)| permission)
                .filter(|permission| permission.home != me && permission.devices.contains(&me))
                .map(|permission| permission.home)
                .collect(),
            Err(err) => {
                warn!(%err, "couldn't read the permissions");
                BTreeSet::new()
            }
        };
        held.extend(users);
        held
    }

    fn name(&self, device: DeviceId) -> String {
        self.name_in(&self.lock(), device)
    }

    fn name_in(&self, state: &State, device: DeviceId) -> String {
        state
            .book
            .peers
            .iter()
            .find(|peer| peer.device == device)
            .map_or_else(|| device.to_string(), |peer| peer.name.clone())
    }

    fn unreachable(&self, device: DeviceId, err: &LinkError) -> Unreachable {
        Unreachable(format!("{} didn't answer: {err}.", self.name(device)))
    }
}

/// What a request to a device that can't be reached fails with.
fn cant_reach(name: &str) -> String {
    format!("Can't reach {name}.")
}

/// A call, wait or model stream open on a link, which keeps it from
/// closing as idle.
pub(crate) struct Open {
    peers: Peers,
    device: DeviceId,
}

impl Open {
    fn new(peers: Peers, device: DeviceId) -> Self {
        if let Some(out) = peers.lock().out.get_mut(&device) {
            out.open += 1;
        }
        Self { peers, device }
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        let now = self.peers.0.clock.now();
        if let Some(out) = self.peers.lock().out.get_mut(&self.device) {
            out.open = out.open.saturating_sub(1);
            out.used = now;
        }
    }
}

impl PeerCalls for Peers {
    fn call(&self, device: DeviceId, call: PeerCall) -> Answer<'_> {
        Box::pin(async move {
            match self.request(device, PeerRequest::Call { call }).await? {
                ActionOutcome::Tool { answer } => Ok(answer),
                ActionOutcome::Refused { reason } => Ok(ToolAnswer {
                    text: reason,
                    failed: true,
                    images: Vec::new(),
                }),
                ActionOutcome::Unknown => Ok(ToolAnswer {
                    text: format!(
                        "maxd on {} is newer than this one and answered in a way it can't read; update this one.",
                        self.name(device)
                    ),
                    failed: true,
                    images: Vec::new(),
                }),
                other => Err(Unreachable(format!(
                    "{} answered oddly: {other:?}",
                    self.name(device)
                ))),
            }
        })
    }

    fn reach(&self, device: DeviceId) -> Woken<'_> {
        Box::pin(async move { self.way(device, REACH_WITHIN).await.map(drop) })
    }

    fn move_step(&self, device: DeviceId, step: AgentMoveStep) -> Stepped<'_> {
        Box::pin(self.request(device, PeerRequest::Move { step }))
    }

    fn home(&self) -> Option<DeviceId> {
        self.lock()
            .book
            .peers
            .iter()
            .find(|peer| peer.home)
            .map(|peer| peer.device)
    }

    fn wake(&self, device: DeviceId, wake: PeerWake) -> Woken<'_> {
        Box::pin(async move {
            match self.request(device, PeerRequest::Wake { wake }).await? {
                // A newer maxd's outcome still means the wake went through.
                ActionOutcome::Done | ActionOutcome::Unknown => Ok(()),
                ActionOutcome::Refused { reason } => Err(Unreachable(reason)),
                other => Err(Unreachable(format!(
                    "{} answered oddly: {other:?}",
                    self.name(device)
                ))),
            }
        })
    }
}

/// The route host that names `device`'s `n`th route in the pins.
fn route_host(device: DeviceId, n: usize) -> String {
    format!("maxhq-peer-{device}-{n}")
}

/// The control messages going over one link another device opened. What
/// the link's queue has no room for yet waits, in order, for one task that
/// queues it as room frees, up to [`MAX_WAITING`] bytes.
#[derive(Clone)]
struct Control {
    wire: maxhq_wire::Sender,
    /// Bytes handed to the task that aren't in the link's queue yet.
    waiting: Arc<AtomicUsize>,
    later: mpsc::UnboundedSender<Vec<u8>>,
}

impl Control {
    fn new(wire: maxhq_wire::Sender) -> Self {
        let (later, mut frames) = mpsc::unbounded_channel::<Vec<u8>>();
        let waiting = Arc::new(AtomicUsize::new(0));
        let (to, left) = (wire.clone(), Arc::clone(&waiting));
        tokio::spawn(async move {
            while let Some(frame) = frames.recv().await {
                let len = frame.len();
                let sent = to.send(Queue::Control, frame).await;
                left.fetch_sub(len, Ordering::AcqRel);
                if sent.is_err() {
                    break;
                }
            }
        });
        Control {
            wire,
            waiting,
            later,
        }
    }

    /// Queues one message, after the ones sent before it. A link that
    /// closed drops it, and one too far behind closes. Messages from one
    /// sender at a time keep their order, as [`Peers`] sends under its
    /// lock.
    fn send(&self, message: &ServerMessage) {
        if self.wire.is_closed() {
            return;
        }
        let mut out = Vec::new();
        if let Err(err) = frame::encode_message(&mut out, message) {
            warn!(%err, "couldn't encode a peer message");
            return;
        }
        // The task's count only drops once its frame is queued, so none
        // waiting means nothing sent before is still ahead of this one.
        if self.waiting.load(Ordering::Acquire) == 0 {
            match self.wire.try_send(Queue::Control, out.clone()) {
                Err(maxhq_wire::TrySendError::Full) => {}
                Ok(()) | Err(maxhq_wire::TrySendError::Closed) => return,
            }
        }
        let len = out.len();
        if self.waiting.load(Ordering::Acquire) + len > MAX_WAITING {
            warn!("a peer stopped reading what this device sends, so its link closes");
            self.wire.close();
            return;
        }
        self.waiting.fetch_add(len, Ordering::AcqRel);
        if self.later.send(out).is_err() {
            self.waiting.fetch_sub(len, Ordering::AcqRel);
        }
    }
}

/// The links `maxd` runs, once it has made them.
static PEERS: OnceLock<Peers> = OnceLock::new();

/// Makes `peers` the links the server and the session manager reach, and
/// starts their sweep. Only the first call counts.
pub fn install(peers: Peers) {
    if PEERS.set(peers.clone()).is_ok() {
        tokio::spawn(peers.run());
    }
}

/// The installed links.
fn installed() -> Result<&'static Peers, String> {
    PEERS
        .get()
        .ok_or_else(|| "This maxd doesn't link to other devices.".into())
}

/// Answers `PeerKey`.
pub fn peer_key() -> ActionOutcome {
    let key = installed().and_then(|peers| {
        peers
            .public_key()
            .map_err(|err| format!("Couldn't make the device's SSH key: {err}"))
    });
    match key {
        Ok(public_key) => ActionOutcome::PeerKey { public_key },
        Err(reason) => ActionOutcome::Refused { reason },
    }
}

/// Answers `AuthorizePeer`.
pub fn authorize(device: DeviceId, public_key: &str) -> ActionOutcome {
    done(installed().and_then(|peers| peers.authorize(device, public_key)))
}

/// Answers `RevokePeer`.
pub fn revoke(device: DeviceId) -> ActionOutcome {
    done(installed().and_then(|peers| peers.revoke(device)))
}

/// Answers `SetPeerBook`.
pub fn set_book(book: PeerBook) -> ActionOutcome {
    done(installed().and_then(|peers| peers.set_book(book)))
}

fn done(result: Result<(), String>) -> ActionOutcome {
    match result {
        Ok(()) => ActionOutcome::Done,
        Err(reason) => ActionOutcome::Refused { reason },
    }
}

/// The installed links, for a connection `maxd proxy --peer` opened.
pub fn for_link() -> Option<Peers> {
    PEERS.get().cloned()
}
