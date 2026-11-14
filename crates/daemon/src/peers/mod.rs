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

mod key;
mod proof;
mod serve;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use maxhq_client::pinned::{PinnedSsh, Pins};
use maxhq_client::{Client, LinkError, LinkEvent, LinkHandle, LinkSpec};
use maxhq_core::authorized_keys;
use maxhq_protocol::agents::{PeerBook, PeerCall, PeerEntry, PeerRequest, PeerWake, ToolAnswer};
use maxhq_protocol::frame;
use maxhq_protocol::ids::{DeviceId, IdempotencyKey, LinkId};
use maxhq_protocol::link::{LinkState, Route};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::{ClientMessage, ServerMessage};
use maxhq_wire::Queue;
use tokio::sync::{oneshot, watch, Semaphore};
use tracing::{debug, info, warn};

use crate::agents::{Answer, PeerCalls, Unreachable, Woken};
use crate::db::Db;
use crate::secrets::files::Folder;
use crate::session::clock::Clock;
use crate::session::Sessions;

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

/// The file under the SSH folder that holds the other devices' host keys.
const KNOWN_HOSTS: &str = "known_hosts";

/// What runs a request another device sent.
pub trait Serve: Send + Sync + 'static {
    /// Runs an agent tool call for a caller on `from`.
    fn call(&self, from: DeviceId, call: PeerCall) -> BoxFuture<'_, ToolAnswer>;
    /// Tells a caller here that an agent it started on `from` finished a
    /// turn.
    fn wake(&self, from: DeviceId, wake: PeerWake) -> BoxFuture<'_, Result<(), String>>;
}

impl Serve for Sessions {
    fn call(&self, from: DeviceId, call: PeerCall) -> BoxFuture<'_, ToolAnswer> {
        Box::pin(self.peer_call(from, call))
    }

    fn wake(&self, from: DeviceId, wake: PeerWake) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(self.peer_wake(from, wake))
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
    state: Mutex<State>,
    /// The devices a link is up to, either way.
    links: watch::Sender<BTreeSet<DeviceId>>,
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
    wire: maxhq_wire::Sender,
    serial: u64,
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
            state: Mutex::new(State::default()),
            links: watch::Sender::new(BTreeSet::new()),
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
            incoming.wire.close();
        }
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
        Ok(())
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
        let mut state = self.lock();
        let kept: BTreeSet<DeviceId> = book.peers.iter().map(|peer| peer.device).collect();
        // A changed route or host key takes a new link.
        state.out.clear();
        state.incoming.retain(|device, _| kept.contains(device));
        state.book = book;
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
                let _open = Open::new(self, device);
                handle
                    .act(ClientMessage::PeerRequest { key, request })
                    .await
                    .map_err(|err| self.unreachable(device, &err))
            }
            Way::Incoming => self.ask_back(device, key, request).await,
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
                let name = self.name(device);
                return Err(Unreachable(format!("Couldn't reach {name}.")));
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
                    let peers = self.clone();
                    let handle = handle.clone();
                    tokio::spawn(async move {
                        let outcome = peers.answer(device, key, request).await;
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
    ) -> Result<ActionOutcome, Unreachable> {
        let (done, answer) = oneshot::channel();
        {
            let mut state = self.lock();
            if let Some(incoming) = state.incoming.get(&device) {
                send(
                    &incoming.wire,
                    &ServerMessage::PeerRequest {
                        key,
                        request: request.clone(),
                    },
                );
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
        let answered = tokio::time::timeout(ANSWER_WITHIN, answer).await;
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
    pub async fn answer(
        &self,
        from: DeviceId,
        key: IdempotencyKey,
        request: PeerRequest,
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
        let outcome = self.serve(from, request).await;
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
    async fn serve(&self, from: DeviceId, request: PeerRequest) -> ActionOutcome {
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
            PeerRequest::NoteHashes { .. }
            | PeerRequest::NoteVersions { .. }
            | PeerRequest::PutNoteVersions { .. } => unreachable!("served above"),
            PeerRequest::Unknown => ActionOutcome::Refused {
                reason: "This maxd is too old for that; update it.".into(),
            },
        }
    }

    /// Records a link `device` opened, and sends it the requests that
    /// wait for it. The link counts until [`Peers::detach`] with the
    /// serial this returns.
    fn attach(&self, device: DeviceId, wire: maxhq_wire::Sender) -> u64 {
        let mut state = self.lock();
        state.serial += 1;
        let serial = state.serial;
        for (key, pending) in &state.pending {
            if pending.device == device {
                send(
                    &wire,
                    &ServerMessage::PeerRequest {
                        key: *key,
                        request: pending.request.clone(),
                    },
                );
            }
        }
        state.incoming.insert(device, Incoming { wire, serial });
        self.relink(&state);
        serial
    }

    /// Forgets a link `device` opened, unless a newer one took its place.
    fn detach(&self, device: DeviceId, serial: u64) {
        let mut state = self.lock();
        if state
            .incoming
            .get(&device)
            .is_some_and(|incoming| incoming.serial == serial)
        {
            state.incoming.remove(&device);
            self.relink(&state);
        }
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
    /// logins, the home of each permission that names it.
    fn held(&self) -> BTreeSet<DeviceId> {
        if self.lock().book.ssh {
            return BTreeSet::new();
        }
        let me = self.0.me;
        match self.0.db.permissions() {
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
        }
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

/// A call or wait open on a link, which keeps it from closing as idle.
struct Open<'a> {
    peers: &'a Peers,
    device: DeviceId,
}

impl<'a> Open<'a> {
    fn new(peers: &'a Peers, device: DeviceId) -> Self {
        if let Some(out) = peers.lock().out.get_mut(&device) {
            out.open += 1;
        }
        Self { peers, device }
    }
}

impl Drop for Open<'_> {
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

/// Queues one message on a link's control queue, in order with the ones
/// before it. A link that closed drops it.
fn send(wire: &maxhq_wire::Sender, message: &ServerMessage) {
    let mut out = Vec::new();
    if let Err(err) = frame::encode_message(&mut out, message) {
        warn!(%err, "couldn't encode a peer message");
        return;
    }
    if let Err(maxhq_wire::TrySendError::Full) = wire.try_send(Queue::Control, out.clone()) {
        let wire = wire.clone();
        tokio::spawn(async move {
            let _ = wire.send(Queue::Control, out).await;
        });
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
