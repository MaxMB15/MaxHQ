// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Every forward and stream this `maxd` holds, and where each stream's
//! bytes go.
//!
//! A client's connection holds a [`ClientTunnels`], and a container's
//! guest link holds a [`GuestRelay`]. A stream to a container relays
//! between the two and checks credit both ways, so neither queue grows past
//! what its reader granted. A stream to a host folder agent's port connects
//! here, through [`connect`]. An SSH session with a container's guest
//! relays the same way, with no forward behind it.
//!
//! The keys clients hand over for those sessions live here too, and every
//! guest link hears them.
//!
//! The lock on [`Tunnels`] is taken before the hub's, never after.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use maxhq_protocol::guest::{GuestToHost, HostToGuest, TunnelTarget, MAX_SSH_KEYS, MAX_TUNNELS};
use maxhq_protocol::ids::{ClientId, ForwardId, PortId, StreamId, WorkspaceId};
use maxhq_protocol::message::{ActionOutcome, ErrorCode, ServerMessage, TunnelEnd};
use maxhq_protocol::model::{Forward, ListeningPort, WorkspaceLocation};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use tracing::debug;

use super::pump::{connect, targets, End, ToClient};
use super::{
    Out, Sink, MAX_STREAMS_PER_CLIENT, MAX_STREAMS_PER_FORWARD, MOST_CREDIT, STREAM_CREDIT,
};
use crate::hub::Hub;
use crate::ports::clip;

/// The longest refusal from a guest passed on to a client, in bytes.
const MAX_REASON_LEN: usize = 512;

/// Every forward and stream, shared by the server and the guest links.
/// Clone it into each.
#[derive(Clone)]
pub struct Tunnels {
    state: Arc<Mutex<State>>,
    hub: Hub,
}

#[derive(Default)]
struct State {
    clients: HashMap<ClientId, ClientEntry>,
    forwards: HashMap<ForwardId, ForwardEntry>,
    streams: HashMap<StreamId, StreamEntry>,
    guests: HashMap<WorkspaceId, GuestEntry>,
    /// Each client's SSH public key, which guests let in.
    ssh_keys: HashMap<ClientId, String>,
    /// Numbers each client connection and guest link, so the goodbye of
    /// one that was replaced leaves the new one alone.
    next_link: u64,
    /// Numbers each stream, so a stream ID used again isn't ended by the
    /// last one's pump.
    next_serial: u64,
}

struct ClientEntry {
    conn: u64,
    sink: Arc<dyn Sink<ServerMessage>>,
    streams: HashSet<StreamId>,
}

struct ForwardEntry {
    forward: Forward,
    /// The connection that opened it, or opened it again last.
    conn: u64,
    streams: HashSet<StreamId>,
}

struct GuestEntry {
    link: u64,
    sink: Arc<dyn Sink<HostToGuest>>,
    streams: HashSet<StreamId>,
}

struct StreamEntry {
    client: ClientId,
    conn: u64,
    /// The forward it came through, or none for an SSH session.
    forward: Option<ForwardId>,
    serial: u64,
    way: Way,
}

/// Where a stream's bytes go past this `maxd`.
enum Way {
    /// Through a container's guest link.
    Guest(Relay),
    /// To a socket this process connected.
    Local(End),
}

struct Relay {
    workspace: WorkspaceId,
    /// Whether the guest connected.
    opened: bool,
    /// Bytes the guest still lets the client send.
    to_guest: u64,
    /// Bytes the client still lets the guest send.
    to_client: u64,
    client_done: bool,
    guest_done: bool,
}

impl Relay {
    /// A stream the guest hasn't answered yet, with the first credit
    /// each way.
    fn new(workspace: WorkspaceId) -> Self {
        Self {
            workspace,
            opened: false,
            to_guest: STREAM_CREDIT,
            to_client: STREAM_CREDIT,
            client_done: false,
            guest_done: false,
        }
    }
}

/// Which side ended a stream. The others hear a reset.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Client,
    Guest,
    Host,
}

impl State {
    fn link(&mut self) -> u64 {
        self.next_link += 1;
        self.next_link
    }

    /// Takes a stream out of every index. Dropping a local end aborts its
    /// socket.
    fn remove(&mut self, stream: StreamId) -> Option<StreamEntry> {
        let entry = self.streams.remove(&stream)?;
        if let Some(client) = self.clients.get_mut(&entry.client) {
            client.streams.remove(&stream);
        }
        if let Some(forward) = entry.forward.and_then(|id| self.forwards.get_mut(&id)) {
            forward.streams.remove(&stream);
        }
        if let Way::Guest(relay) = &entry.way
            && let Some(guest) = self.guests.get_mut(&relay.workspace)
        {
            guest.streams.remove(&stream);
        }
        Some(entry)
    }

    fn to_client(&self, client: ClientId, out: Out<ServerMessage>) -> bool {
        self.clients
            .get(&client)
            .is_some_and(|entry| entry.sink.send(out).is_ok())
    }

    fn to_guest(&self, workspace: WorkspaceId, out: Out<HostToGuest>) -> bool {
        self.guests
            .get(&workspace)
            .is_some_and(|entry| entry.sink.send(out).is_ok())
    }

    /// Ends a stream at once. Each side but `from` hears a reset.
    fn reset(&mut self, stream: StreamId, from: Side) {
        let Some(entry) = self.remove(stream) else {
            return;
        };
        let how = TunnelEnd::Reset;
        if from != Side::Client {
            self.to_client(
                entry.client,
                Out::Message(ServerMessage::TunnelClose { stream, how }),
            );
        }
        if let Way::Guest(relay) = &entry.way
            && from != Side::Guest
        {
            self.to_guest(
                relay.workspace,
                Out::Message(HostToGuest::TunnelClose { stream, how }),
            );
        }
    }

    /// The keys guests let in, each once, in a steady order.
    fn authorized(&self) -> HostToGuest {
        let mut keys: Vec<String> = self.ssh_keys.values().cloned().collect();
        keys.sort();
        keys.dedup();
        HostToGuest::SshAuthorized { keys }
    }

    /// Takes away a forward and its streams. Gives the forward, so its
    /// entity can go.
    fn drop_forward(&mut self, forward: ForwardId) -> Option<ForwardId> {
        let entry = self.forwards.get(&forward)?;
        let streams: Vec<StreamId> = entry.streams.iter().copied().collect();
        for stream in streams {
            self.reset(stream, Side::Host);
        }
        self.forwards.remove(&forward).map(|entry| entry.forward.id)
    }
}

impl Tunnels {
    /// No forwards or streams yet. Forwards show in `hub`.
    pub fn new(hub: Hub) -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
            hub,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // Every change to the state is whole by the time a panic could
        // happen, so carry on with it.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts a client's connection, whose messages for it go to `sink`.
    /// Streams from the client's last connection are reset, since that one
    /// is gone. Its forwards stay, for the client to open again.
    pub fn attach(&self, client: ClientId, sink: Arc<dyn Sink<ServerMessage>>) -> ClientTunnels {
        let mut state = self.lock();
        let conn = state.link();
        let old = state.clients.insert(
            client,
            ClientEntry {
                conn,
                sink,
                streams: HashSet::new(),
            },
        );
        for stream in old.into_iter().flat_map(|entry| entry.streams) {
            state.reset(stream, Side::Client);
        }
        ClientTunnels {
            tunnels: self.clone(),
            client,
            conn,
        }
    }

    /// Starts a container's guest link, whose messages go to `sink`. A
    /// link the container had before is replaced, and its streams reset.
    pub fn guest(&self, workspace: WorkspaceId, sink: Arc<dyn Sink<HostToGuest>>) -> GuestRelay {
        let mut state = self.lock();
        let link = state.link();
        // A guest forgets the keys when its link drops, so a new link
        // hears them first.
        if !state.ssh_keys.is_empty() {
            let _ = sink.send(Out::Message(state.authorized()));
        }
        let old = state.guests.insert(
            workspace,
            GuestEntry {
                link,
                sink,
                streams: HashSet::new(),
            },
        );
        for stream in old.into_iter().flat_map(|entry| entry.streams) {
            state.reset(stream, Side::Guest);
        }
        GuestRelay {
            tunnels: self.clone(),
            workspace,
            link,
        }
    }

    fn set_ssh_key(&self, client: ClientId, key: String) -> ActionOutcome {
        let mut state = self.lock();
        if state.ssh_keys.get(&client) == Some(&key) {
            return ActionOutcome::Done;
        }
        if !state.ssh_keys.contains_key(&client) && state.ssh_keys.len() >= MAX_SSH_KEYS {
            return refused("this device holds as many SSH keys as it takes");
        }
        state.ssh_keys.insert(client, key);
        let authorized = state.authorized();
        for guest in state.guests.values() {
            let _ = guest.sink.send(Out::Message(authorized.clone()));
        }
        ActionOutcome::Done
    }

    /// The port is gone for good: its forwards close and their streams
    /// reset.
    pub fn port_removed(&self, port: PortId) {
        let mut state = self.lock();
        let forwards: Vec<ForwardId> = state
            .forwards
            .values()
            .filter(|entry| entry.forward.port == port)
            .map(|entry| entry.forward.id)
            .collect();
        let gone: Vec<ForwardId> = forwards
            .into_iter()
            .filter_map(|forward| state.drop_forward(forward))
            .collect();
        self.remove_entities(gone);
    }

    fn remove_entities(&self, forwards: Vec<ForwardId>) {
        if !forwards.is_empty() {
            self.hub
                .apply(forwards.into_iter().map(|forward| Change::Remove {
                    key: EntityKey::Forward(forward),
                }));
        }
    }

    /// The port this ID names, while it listens.
    fn listening(&self, port: PortId) -> Option<ListeningPort> {
        match self.hub.get(&EntityKey::ListeningPort(port)) {
            Some(Entity::ListeningPort(port)) if port.gone_since.is_none() => Some(port),
            _ => None,
        }
    }

    /// Whether the workspace is a folder on this device, so its ports are
    /// reached here.
    fn is_local(&self, workspace: WorkspaceId) -> bool {
        match self.hub.get(&EntityKey::Workspace(workspace)) {
            Some(Entity::Workspace(workspace)) => {
                workspace.device == self.hub.device().id
                    && matches!(workspace.location, WorkspaceLocation::Folder { .. })
            }
            _ => false,
        }
    }

    /// How many streams are open, for tests.
    #[cfg(test)]
    fn open_streams(&self) -> usize {
        self.lock().streams.len()
    }
}

/// One client connection's forwards and streams. The server holds one per
/// connection and passes it the client's tunnel messages and frames.
/// Dropping it resets the connection's streams and closes the forwards it
/// opened.
pub struct ClientTunnels {
    tunnels: Tunnels,
    client: ClientId,
    conn: u64,
}

impl ClientTunnels {
    /// Answers `OpenForward`. The client has bound `local_port` already.
    /// Opening a forward to the same port again gives the same forward.
    pub fn open_forward(&self, port: PortId, local_port: u16) -> ActionOutcome {
        let mut state = self.tunnels.lock();
        if !self.current(&state) {
            return refused("this connection was replaced");
        }
        if self.tunnels.listening(port).is_none() {
            return refused("that port isn't listening");
        }
        let existing = state
            .forwards
            .values_mut()
            .find(|entry| entry.forward.client == self.client && entry.forward.port == port);
        let forward = match existing {
            Some(entry) => {
                entry.conn = self.conn;
                if entry.forward.local_port == local_port {
                    return ActionOutcome::ForwardOpened {
                        forward: entry.forward.id,
                    };
                }
                entry.forward.local_port = local_port;
                entry.forward.clone()
            }
            None => {
                let forward = Forward {
                    id: ForwardId::generate(),
                    port,
                    client: self.client,
                    local_port,
                    opened_at: Timestamp::now(),
                };
                state.forwards.insert(
                    forward.id,
                    ForwardEntry {
                        forward: forward.clone(),
                        conn: self.conn,
                        streams: HashSet::new(),
                    },
                );
                forward
            }
        };
        let id = forward.id;
        self.tunnels.hub.apply([Change::Upsert {
            value: Entity::Forward(forward),
        }]);
        ActionOutcome::ForwardOpened { forward: id }
    }

    /// Handles `CloseForward`. Its streams reset.
    pub fn close_forward(&self, forward: ForwardId) {
        let mut state = self.tunnels.lock();
        let owned = state
            .forwards
            .get(&forward)
            .is_some_and(|entry| entry.forward.client == self.client);
        if !owned {
            debug!(%forward, "dropped a close for a forward the client doesn't have");
            return;
        }
        let gone = state.drop_forward(forward);
        self.tunnels.remove_entities(gone.into_iter().collect());
    }

    /// Handles `TunnelOpen`. The answer comes later, as `TunnelOpened` or
    /// `TunnelRefused`.
    pub fn open(&self, stream: StreamId, forward: ForwardId) {
        let mut state = self.tunnels.lock();
        if !self.current(&state) {
            debug!(%stream, "dropped a stream from a connection that was replaced");
            return;
        }
        if let Err((code, message)) = self.try_open(&mut state, stream, forward) {
            self.refuse(&state, stream, code, message);
        }
    }

    /// Answers `SetGuestSshKey`: guests let `key` in for this client from
    /// now on, in place of the key it gave before, even once this
    /// connection ends. `key` is an ed25519 line already checked.
    pub fn set_ssh_key(&self, key: String) -> ActionOutcome {
        self.tunnels.set_ssh_key(self.client, key)
    }

    /// Handles `GuestSshOpen`, once the server has checked the workspace
    /// is a container that runs. The answer comes later, as for
    /// [`open`](Self::open).
    pub fn open_ssh(&self, stream: StreamId, workspace: WorkspaceId) {
        let mut state = self.tunnels.lock();
        if !self.current(&state) {
            debug!(%stream, "dropped a stream from a connection that was replaced");
            return;
        }
        if let Err((code, message)) = self.try_open_ssh(&mut state, stream, workspace) {
            self.refuse(&state, stream, code, message);
        }
    }

    fn try_open_ssh(
        &self,
        state: &mut State,
        stream: StreamId,
        workspace: WorkspaceId,
    ) -> Result<(), (ErrorCode, &'static str)> {
        if state.streams.contains_key(&stream) {
            return Err((ErrorCode::NotAllowed, "that stream is open already"));
        }
        if state.clients[&self.client].streams.len() >= MAX_STREAMS_PER_CLIENT {
            return Err((ErrorCode::TooLarge, "the client has too many streams open"));
        }
        let guest = state
            .guests
            .get_mut(&workspace)
            .ok_or((ErrorCode::Unreachable, "the container's link is down"))?;
        if guest.streams.len() >= MAX_TUNNELS {
            return Err((
                ErrorCode::TooLarge,
                "the container has too many streams open",
            ));
        }
        let open = HostToGuest::TunnelOpen {
            stream,
            target: TunnelTarget::Ssh,
        };
        if guest.sink.send(Out::Message(open)).is_err() {
            return Err((ErrorCode::Unreachable, "the container's link is down"));
        }
        guest.streams.insert(stream);
        let serial = state.next_serial;
        state.next_serial += 1;
        if let Some(client) = state.clients.get_mut(&self.client) {
            client.streams.insert(stream);
        }
        state.streams.insert(
            stream,
            StreamEntry {
                client: self.client,
                conn: self.conn,
                forward: None,
                serial,
                way: Way::Guest(Relay::new(workspace)),
            },
        );
        Ok(())
    }

    fn refuse(&self, state: &State, stream: StreamId, code: ErrorCode, message: &str) {
        let refusal = ServerMessage::TunnelRefused {
            stream,
            code,
            message: message.to_owned(),
        };
        if let Some(client) = state.clients.get(&self.client) {
            let _ = client.sink.send(Out::Message(refusal));
        }
    }

    fn try_open(
        &self,
        state: &mut State,
        stream: StreamId,
        forward: ForwardId,
    ) -> Result<(), (ErrorCode, &'static str)> {
        if state.streams.contains_key(&stream) {
            return Err((ErrorCode::NotAllowed, "that stream is open already"));
        }
        let entry = state
            .forwards
            .get(&forward)
            .filter(|entry| entry.forward.client == self.client)
            .ok_or((ErrorCode::NoSuchPort, "no such forward"))?;
        if entry.streams.len() >= MAX_STREAMS_PER_FORWARD {
            return Err((ErrorCode::TooLarge, "the forward has too many streams open"));
        }
        let client = &state.clients[&self.client];
        if client.streams.len() >= MAX_STREAMS_PER_CLIENT {
            return Err((ErrorCode::TooLarge, "the client has too many streams open"));
        }
        let port = self
            .tunnels
            .listening(entry.forward.port)
            .ok_or((ErrorCode::PortGone, "the port isn't listening any more"))?;

        let way = if let Some(guest) = state.guests.get(&port.workspace) {
            if guest.streams.len() >= MAX_TUNNELS {
                return Err((
                    ErrorCode::TooLarge,
                    "the container has too many streams open",
                ));
            }
            let open = HostToGuest::TunnelOpen {
                stream,
                target: TunnelTarget::Port(port.port),
            };
            if guest.sink.send(Out::Message(open)).is_err() {
                return Err((ErrorCode::Unreachable, "the container's link is down"));
            }
            Way::Guest(Relay::new(port.workspace))
        } else if self.tunnels.is_local(port.workspace) {
            let serial = state.next_serial;
            let state_ref = Arc::downgrade(&self.tunnels.state);
            let ended = Box::new(move || {
                let Some(state) = state_ref.upgrade() else {
                    return;
                };
                let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
                if state
                    .streams
                    .get(&stream)
                    .is_some_and(|entry| entry.serial == serial)
                {
                    state.remove(stream);
                }
            });
            Way::Local(connect::<ToClient>(
                stream,
                targets(&port.bind, port.port),
                client.sink.clone(),
                ended,
            ))
        } else {
            return Err((ErrorCode::Unreachable, "the workspace can't be reached now"));
        };

        let serial = state.next_serial;
        state.next_serial += 1;
        if let Way::Guest(_) = way
            && let Some(guest) = state.guests.get_mut(&port.workspace)
        {
            guest.streams.insert(stream);
        }
        if let Some(client) = state.clients.get_mut(&self.client) {
            client.streams.insert(stream);
        }
        if let Some(entry) = state.forwards.get_mut(&forward) {
            entry.streams.insert(stream);
        }
        state.streams.insert(
            stream,
            StreamEntry {
                client: self.client,
                conn: self.conn,
                forward: Some(forward),
                serial,
                way,
            },
        );
        Ok(())
    }

    /// Handles a client's `TunnelCredit`.
    pub fn credit(&self, stream: StreamId, granted: u64) {
        let mut state = self.tunnels.lock();
        let Some(entry) = self.stream(&mut state, stream) else {
            return;
        };
        match &mut entry.way {
            Way::Local(end) => end.credit(granted),
            Way::Guest(relay) => {
                let granted = granted.min(MOST_CREDIT.saturating_sub(relay.to_client));
                relay.to_client += granted;
                let workspace = relay.workspace;
                if granted > 0 {
                    state.to_guest(
                        workspace,
                        Out::Message(HostToGuest::TunnelCredit { stream, granted }),
                    );
                }
            }
        }
    }

    /// Handles a client's `TunnelClose`.
    pub fn close(&self, stream: StreamId, how: TunnelEnd) {
        let mut state = self.tunnels.lock();
        let Some(entry) = self.stream(&mut state, stream) else {
            return;
        };
        if how == TunnelEnd::Reset {
            state.reset(stream, Side::Client);
            return;
        }
        match &mut entry.way {
            // The pump ends the stream once both halves close.
            Way::Local(end) => end.close(TunnelEnd::Write),
            Way::Guest(relay) => {
                relay.client_done = true;
                let workspace = relay.workspace;
                let both = relay.guest_done;
                state.to_guest(
                    workspace,
                    Out::Message(HostToGuest::TunnelClose { stream, how }),
                );
                if both {
                    state.remove(stream);
                }
            }
        }
    }

    /// Handles a tunnel frame from the client. Bytes before the stream
    /// opens, after the client closed its half, or past the credit reset
    /// the stream.
    pub fn data(&self, stream: StreamId, data: &[u8]) {
        let mut state = self.tunnels.lock();
        let Some(entry) = self.stream(&mut state, stream) else {
            return;
        };
        let fits = match &mut entry.way {
            Way::Local(end) => end.data(data),
            Way::Guest(relay) => {
                let len = data.len() as u64;
                if !relay.opened || relay.client_done || len > relay.to_guest {
                    false
                } else {
                    relay.to_guest -= len;
                    let workspace = relay.workspace;
                    state.to_guest(
                        workspace,
                        Out::Data {
                            stream,
                            data: data.to_vec(),
                        },
                    )
                }
            }
        };
        if !fits {
            debug!(%stream, "reset a stream the client sent too much on");
            state.reset(stream, Side::Host);
        }
    }

    /// The client's own stream on this connection.
    fn stream<'a>(&self, state: &'a mut State, stream: StreamId) -> Option<&'a mut StreamEntry> {
        let entry = state
            .streams
            .get_mut(&stream)
            .filter(|entry| entry.client == self.client && entry.conn == self.conn);
        if entry.is_none() {
            debug!(%stream, "dropped a message for a stream the client doesn't have");
        }
        entry
    }

    fn current(&self, state: &State) -> bool {
        state
            .clients
            .get(&self.client)
            .is_some_and(|entry| entry.conn == self.conn)
    }
}

impl Drop for ClientTunnels {
    fn drop(&mut self) {
        let mut state = self.tunnels.lock();
        if self.current(&state)
            && let Some(entry) = state.clients.remove(&self.client)
        {
            for stream in entry.streams {
                state.reset(stream, Side::Client);
            }
        }
        let forwards: Vec<ForwardId> = state
            .forwards
            .values()
            .filter(|entry| entry.conn == self.conn)
            .map(|entry| entry.forward.id)
            .collect();
        let gone = forwards
            .into_iter()
            .filter_map(|forward| state.drop_forward(forward))
            .collect();
        self.tunnels.remove_entities(gone);
    }
}

/// One guest link's side of the relay. The guest link holds one and
/// passes it the guest's tunnel messages and frames. Dropping it resets
/// every stream through it.
pub struct GuestRelay {
    tunnels: Tunnels,
    workspace: WorkspaceId,
    link: u64,
}

impl GuestRelay {
    /// Handles a message from the guest if it's about a stream. False when
    /// it isn't, for the link to handle.
    pub fn handle(&self, message: &GuestToHost) -> bool {
        match message {
            GuestToHost::TunnelOpened { stream } => self.opened(*stream),
            GuestToHost::TunnelRefused { stream, reason } => self.refused(*stream, reason),
            GuestToHost::TunnelCredit { stream, granted } => self.credit(*stream, *granted),
            GuestToHost::TunnelClose { stream, how } => self.close(*stream, *how),
            _ => return false,
        }
        true
    }

    fn opened(&self, stream: StreamId) {
        let mut state = self.tunnels.lock();
        let Some((client, relay)) = self.relay(&mut state, stream) else {
            return;
        };
        if relay.opened {
            return;
        }
        relay.opened = true;
        state.to_client(client, Out::Message(ServerMessage::TunnelOpened { stream }));
    }

    fn refused(&self, stream: StreamId, reason: &str) {
        let mut state = self.tunnels.lock();
        let Some((client, relay)) = self.relay(&mut state, stream) else {
            return;
        };
        if relay.opened {
            state.reset(stream, Side::Guest);
            return;
        }
        state.remove(stream);
        state.to_client(
            client,
            Out::Message(ServerMessage::TunnelRefused {
                stream,
                code: ErrorCode::Unreachable,
                message: clip(reason, MAX_REASON_LEN),
            }),
        );
    }

    fn credit(&self, stream: StreamId, granted: u64) {
        let mut state = self.tunnels.lock();
        let Some((client, relay)) = self.relay(&mut state, stream) else {
            return;
        };
        let granted = granted.min(MOST_CREDIT.saturating_sub(relay.to_guest));
        relay.to_guest += granted;
        if granted > 0 {
            state.to_client(
                client,
                Out::Message(ServerMessage::TunnelCredit { stream, granted }),
            );
        }
    }

    fn close(&self, stream: StreamId, how: TunnelEnd) {
        let mut state = self.tunnels.lock();
        let Some((client, relay)) = self.relay(&mut state, stream) else {
            return;
        };
        if how == TunnelEnd::Reset {
            state.reset(stream, Side::Guest);
            return;
        }
        relay.guest_done = true;
        let both = relay.client_done;
        state.to_client(
            client,
            Out::Message(ServerMessage::TunnelClose { stream, how }),
        );
        if both {
            state.remove(stream);
        }
    }

    /// Handles a tunnel frame from the guest. Bytes before the stream
    /// opens, after the guest closed its half, or past the client's credit
    /// reset the stream.
    pub fn data(&self, stream: StreamId, data: &[u8]) {
        let mut state = self.tunnels.lock();
        let Some((client, relay)) = self.relay(&mut state, stream) else {
            return;
        };
        let len = data.len() as u64;
        let fits = relay.opened && !relay.guest_done && len <= relay.to_client;
        if fits {
            relay.to_client -= len;
        }
        if !fits
            || !state.to_client(
                client,
                Out::Data {
                    stream,
                    data: data.to_vec(),
                },
            )
        {
            debug!(%stream, "reset a stream the guest sent too much on");
            state.reset(stream, Side::Host);
        }
    }

    /// A stream relayed through this link, and its client.
    fn relay<'a>(
        &self,
        state: &'a mut State,
        stream: StreamId,
    ) -> Option<(ClientId, &'a mut Relay)> {
        let current = state
            .guests
            .get(&self.workspace)
            .is_some_and(|guest| guest.link == self.link);
        if !current {
            return None;
        }
        let found = state
            .streams
            .get_mut(&stream)
            .and_then(|entry| match &mut entry.way {
                Way::Guest(relay) if relay.workspace == self.workspace => {
                    Some((entry.client, relay))
                }
                _ => None,
            });
        if found.is_none() {
            // The host may have reset it while the guest's message was on
            // the way.
            debug!(%stream, "dropped a guest message for a stream that isn't open");
        }
        found
    }
}

impl Drop for GuestRelay {
    fn drop(&mut self) {
        let mut state = self.tunnels.lock();
        let current = state
            .guests
            .get(&self.workspace)
            .is_some_and(|guest| guest.link == self.link);
        if !current {
            return;
        }
        if let Some(guest) = state.guests.remove(&self.workspace) {
            for stream in guest.streams {
                state.reset(stream, Side::Guest);
            }
        }
    }
}

fn refused(reason: &str) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use maxhq_protocol::frame::MAX_TUNNEL_CHUNK;
    use maxhq_protocol::ids::{BootId, DeviceId};
    use maxhq_protocol::model::{BindAddress, Device, Workspace};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc::UnboundedReceiver;

    use super::*;
    use crate::tunnel::Queue;

    pub(crate) type Rx<M> = UnboundedReceiver<Out<M>>;

    /// A hub with a container workspace and a folder one on this device.
    pub(crate) struct World {
        pub(crate) hub: Hub,
        pub(crate) tunnels: Tunnels,
        pub(crate) container: WorkspaceId,
        pub(crate) folder: WorkspaceId,
    }

    pub(crate) fn world() -> World {
        let device = DeviceId::derive("test");
        let hub = Hub::new(
            Device {
                id: device,
                name: "box".into(),
                os: "linux".into(),
                boot_id: BootId("b".into()),
                maxd_version: "0.1.0".into(),
                winding_down_until: None,
                workspace_host: None,
                build: None,
                target: None,
                wsl: None,
            },
            HashMap::new(),
        );
        let container = WorkspaceId::derive("container");
        let folder = WorkspaceId::derive("folder");
        hub.apply([
            Change::Upsert {
                value: Entity::Workspace(Workspace {
                    id: container,
                    project: None,
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
                value: Entity::Workspace(Workspace {
                    id: folder,
                    project: None,
                    device,
                    location: WorkspaceLocation::Folder {
                        path: "/srv/app".into(),
                    },
                    managed: None,
                    clone: None,
                }),
            },
        ]);
        World {
            tunnels: Tunnels::new(hub.clone()),
            hub,
            container,
            folder,
        }
    }

    /// Shows a port listening on loopback in `workspace`.
    pub(crate) fn listen(hub: &Hub, workspace: WorkspaceId, port: u16) -> PortId {
        let id = PortId::generate();
        hub.apply([Change::Upsert {
            value: Entity::ListeningPort(ListeningPort {
                id,
                workspace,
                agent: None,
                port,
                bind: BindAddress::Loopback,
                process: None,
                url_hint: None,
                first_seen: Timestamp(1),
                gone_since: None,
            }),
        }]);
        id
    }

    pub(crate) fn sink<M: Send + 'static>() -> (Arc<dyn Sink<M>>, Rx<M>) {
        let (queue, rx) = Queue::new();
        (Arc::new(queue), rx)
    }

    /// Everything queued so far.
    pub(crate) fn sent<M>(rx: &mut Rx<M>) -> Vec<Out<M>> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    pub(crate) async fn next<M>(rx: &mut Rx<M>) -> Out<M> {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out")
            .expect("queue closed")
    }

    pub(crate) fn forward(client: &ClientTunnels, port: PortId) -> ForwardId {
        match client.open_forward(port, 40_000) {
            ActionOutcome::ForwardOpened { forward } => forward,
            other => panic!("not opened: {other:?}"),
        }
    }

    fn client_reset(stream: StreamId) -> Out<ServerMessage> {
        Out::Message(ServerMessage::TunnelClose {
            stream,
            how: TunnelEnd::Reset,
        })
    }

    fn guest_reset(stream: StreamId) -> Out<HostToGuest> {
        Out::Message(HostToGuest::TunnelClose {
            stream,
            how: TunnelEnd::Reset,
        })
    }

    fn is_refusal(sent: &[Out<ServerMessage>], want: ErrorCode) -> bool {
        matches!(
            sent,
            [Out::Message(ServerMessage::TunnelRefused { code, .. })] if *code == want
        )
    }

    /// A client with a forward to a container port, and the container's
    /// guest link.
    struct Relayed {
        world: World,
        port: PortId,
        client: ClientTunnels,
        to_client: Rx<ServerMessage>,
        relay: GuestRelay,
        to_guest: Rx<HostToGuest>,
        forward: ForwardId,
    }

    fn relayed() -> Relayed {
        let world = world();
        let port = listen(&world.hub, world.container, 5173);
        let (client_sink, to_client) = sink();
        let client = world.tunnels.attach(ClientId::derive("mac"), client_sink);
        let (guest_sink, to_guest) = sink();
        let relay = world.tunnels.guest(world.container, guest_sink);
        let forward = forward(&client, port);
        Relayed {
            world,
            port,
            client,
            to_client,
            relay,
            to_guest,
            forward,
        }
    }

    impl Relayed {
        /// Opens a stream the guest connects.
        fn open(&mut self) -> StreamId {
            let stream = StreamId::generate();
            self.client.open(stream, self.forward);
            assert_eq!(
                sent(&mut self.to_guest),
                [Out::Message(HostToGuest::TunnelOpen {
                    stream,
                    target: TunnelTarget::Port(5173),
                })]
            );
            assert!(self.relay.handle(&GuestToHost::TunnelOpened { stream }));
            assert_eq!(
                sent(&mut self.to_client),
                [Out::Message(ServerMessage::TunnelOpened { stream })]
            );
            stream
        }

        /// Asks for a stream, and gives what the client heard.
        fn try_open(&mut self, forward: ForwardId) -> Vec<Out<ServerMessage>> {
            self.client.open(StreamId::generate(), forward);
            sent(&mut self.to_guest);
            sent(&mut self.to_client)
        }
    }

    #[test]
    fn a_forward_needs_a_listening_port() {
        let world = world();
        let (client_sink, _rx) = sink();
        let client = world.tunnels.attach(ClientId::derive("mac"), client_sink);
        assert!(matches!(
            client.open_forward(PortId::generate(), 8080),
            ActionOutcome::Refused { .. }
        ));

        let port = listen(&world.hub, world.folder, 8080);
        let Some(Entity::ListeningPort(mut gone)) = world.hub.get(&EntityKey::ListeningPort(port))
        else {
            panic!("no port");
        };
        gone.gone_since = Some(Timestamp(2));
        world.hub.apply([Change::Upsert {
            value: Entity::ListeningPort(gone),
        }]);
        assert!(matches!(
            client.open_forward(port, 8080),
            ActionOutcome::Refused { .. }
        ));
    }

    #[test]
    fn opening_a_forward_again_gives_the_same_one() {
        let world = world();
        let port = listen(&world.hub, world.container, 5173);
        let (client_sink, _rx) = sink();
        let client = world.tunnels.attach(ClientId::derive("mac"), client_sink);
        let first = forward(&client, port);
        let again = client.open_forward(port, 40_001);
        assert_eq!(again, ActionOutcome::ForwardOpened { forward: first });
        let Some(Entity::Forward(shown)) = world.hub.get(&EntityKey::Forward(first)) else {
            panic!("the forward doesn't show");
        };
        assert_eq!((shown.port, shown.local_port), (port, 40_001));
        assert_eq!(shown.client, ClientId::derive("mac"));
    }

    #[test]
    fn a_huge_grant_is_relayed_only_as_far_as_the_most_credit() {
        let mut r = relayed();
        let stream = r.open();
        let room = MOST_CREDIT - STREAM_CREDIT;

        r.client.credit(stream, u64::MAX);
        assert_eq!(
            sent(&mut r.to_guest),
            [Out::Message(HostToGuest::TunnelCredit {
                stream,
                granted: room
            })]
        );
        r.client.credit(stream, 1);
        assert!(sent(&mut r.to_guest).is_empty());
        // What the guest then sends makes room again, and nothing it was
        // told it could send is past what the relay lets through.
        let chunk = vec![0; MAX_TUNNEL_CHUNK];
        for _ in 0..(MOST_CREDIT as usize / MAX_TUNNEL_CHUNK) {
            r.relay.data(stream, &chunk);
        }
        assert_eq!(r.world.tunnels.open_streams(), 1);
        r.client.credit(stream, 1000);
        assert_eq!(
            sent(&mut r.to_guest),
            [Out::Message(HostToGuest::TunnelCredit {
                stream,
                granted: 1000
            })]
        );

        assert!(r.relay.handle(&GuestToHost::TunnelCredit {
            stream,
            granted: u64::MAX
        }));
        sent(&mut r.to_client);
        assert!(r.relay.handle(&GuestToHost::TunnelCredit {
            stream,
            granted: u64::MAX
        }));
        assert!(sent(&mut r.to_client).is_empty());
    }

    #[test]
    fn a_stream_relays_through_the_guest_and_half_closes() {
        let mut r = relayed();
        let stream = r.open();

        r.client.data(stream, b"ping");
        r.relay.data(stream, b"pong");
        r.client.credit(stream, 10);
        assert!(r.relay.handle(&GuestToHost::TunnelCredit {
            stream,
            granted: 20
        }));
        assert_eq!(
            sent(&mut r.to_guest),
            [
                Out::Data {
                    stream,
                    data: b"ping".to_vec()
                },
                Out::Message(HostToGuest::TunnelCredit {
                    stream,
                    granted: 10
                }),
            ]
        );
        assert_eq!(
            sent(&mut r.to_client),
            [
                Out::Data {
                    stream,
                    data: b"pong".to_vec()
                },
                Out::Message(ServerMessage::TunnelCredit {
                    stream,
                    granted: 20
                }),
            ]
        );

        r.client.close(stream, TunnelEnd::Write);
        assert_eq!(
            sent(&mut r.to_guest),
            [Out::Message(HostToGuest::TunnelClose {
                stream,
                how: TunnelEnd::Write
            })]
        );
        // The guest's half still runs.
        r.relay.data(stream, b"last");
        assert_eq!(r.world.tunnels.open_streams(), 1);
        assert!(r.relay.handle(&GuestToHost::TunnelClose {
            stream,
            how: TunnelEnd::Write
        }));
        assert_eq!(
            sent(&mut r.to_client),
            [
                Out::Data {
                    stream,
                    data: b"last".to_vec()
                },
                Out::Message(ServerMessage::TunnelClose {
                    stream,
                    how: TunnelEnd::Write
                }),
            ]
        );
        assert_eq!(r.world.tunnels.open_streams(), 0);
    }

    #[test]
    fn a_reset_from_either_side_reaches_the_other() {
        let mut r = relayed();
        let stream = r.open();
        r.client.close(stream, TunnelEnd::Reset);
        assert_eq!(sent(&mut r.to_guest), [guest_reset(stream)]);
        assert!(sent(&mut r.to_client).is_empty());

        let stream = r.open();
        r.relay.handle(&GuestToHost::TunnelClose {
            stream,
            how: TunnelEnd::Reset,
        });
        assert_eq!(sent(&mut r.to_client), [client_reset(stream)]);
        assert!(sent(&mut r.to_guest).is_empty());
        assert_eq!(r.world.tunnels.open_streams(), 0);
    }

    #[test]
    fn bytes_past_the_credit_reset_both_sides() {
        let mut r = relayed();
        let stream = r.open();
        r.client.data(stream, &vec![0; STREAM_CREDIT as usize]);
        sent(&mut r.to_guest);
        r.client.data(stream, b"x");
        assert_eq!(sent(&mut r.to_guest), [guest_reset(stream)]);
        assert_eq!(sent(&mut r.to_client), [client_reset(stream)]);

        let stream = r.open();
        r.relay.data(stream, &vec![0; STREAM_CREDIT as usize + 1]);
        assert_eq!(sent(&mut r.to_guest), [guest_reset(stream)]);
        assert_eq!(sent(&mut r.to_client), [client_reset(stream)]);
        assert_eq!(r.world.tunnels.open_streams(), 0);
    }

    #[test]
    fn bytes_before_the_guest_connects_reset_the_stream() {
        let mut r = relayed();
        let stream = StreamId::generate();
        r.client.open(stream, r.forward);
        r.client.data(stream, b"early");
        assert_eq!(
            sent(&mut r.to_guest),
            [
                Out::Message(HostToGuest::TunnelOpen {
                    stream,
                    target: TunnelTarget::Port(5173),
                }),
                guest_reset(stream)
            ]
        );
        assert_eq!(sent(&mut r.to_client), [client_reset(stream)]);
    }

    #[test]
    fn a_guest_refusal_reaches_the_client_cut_short() {
        let mut r = relayed();
        let stream = StreamId::generate();
        r.client.open(stream, r.forward);
        r.relay.handle(&GuestToHost::TunnelRefused {
            stream,
            reason: "é".repeat(1000),
        });
        let heard = sent(&mut r.to_client);
        let [Out::Message(ServerMessage::TunnelRefused {
            code: ErrorCode::Unreachable,
            message,
            ..
        })] = &heard[..]
        else {
            panic!("no refusal: {heard:?}");
        };
        assert_eq!(message.len(), MAX_REASON_LEN);
        assert_eq!(r.world.tunnels.open_streams(), 0);
    }

    #[test]
    fn messages_for_streams_a_guest_doesnt_have_are_dropped() {
        let mut r = relayed();
        let stream = r.open();
        let (other_sink, mut other_rx) = sink();
        let other = r.world.tunnels.guest(r.world.folder, other_sink);
        other.handle(&GuestToHost::TunnelClose {
            stream,
            how: TunnelEnd::Reset,
        });
        other.data(stream, b"x");
        assert!(sent(&mut r.to_client).is_empty());
        assert!(sent(&mut other_rx).is_empty());
        assert_eq!(r.world.tunnels.open_streams(), 1);
        assert!(!r.relay.handle(&GuestToHost::Ports { listening: vec![] }));
    }

    #[test]
    fn streams_are_limited_per_forward_and_per_client() {
        let mut r = relayed();
        for _ in 0..MAX_STREAMS_PER_FORWARD {
            assert!(r.try_open(r.forward).is_empty());
        }
        assert!(is_refusal(&r.try_open(r.forward), ErrorCode::TooLarge));

        let forwards: Vec<ForwardId> = (0..4)
            .map(|n| {
                let port = listen(&r.world.hub, r.world.container, 6000 + n);
                forward(&r.client, port)
            })
            .collect();
        for forward in &forwards[..3] {
            for _ in 0..MAX_STREAMS_PER_FORWARD {
                assert!(r.try_open(*forward).is_empty());
            }
        }
        assert_eq!(r.world.tunnels.open_streams(), MAX_STREAMS_PER_CLIENT);
        assert!(is_refusal(&r.try_open(forwards[3]), ErrorCode::TooLarge));
    }

    #[test]
    fn the_guest_link_dropping_resets_its_streams() {
        let mut r = relayed();
        let stream = r.open();
        drop(r.relay);
        assert_eq!(sent(&mut r.to_client), [client_reset(stream)]);
        assert_eq!(r.world.tunnels.open_streams(), 0);

        // Without its link, the container can't be reached.
        r.client.open(StreamId::generate(), r.forward);
        assert!(is_refusal(&sent(&mut r.to_client), ErrorCode::Unreachable));
    }

    #[test]
    fn a_new_connection_resets_the_old_streams_and_keeps_the_forwards() {
        let mut r = relayed();
        let stream = r.open();
        let (new_sink, mut new_rx) = sink();
        let new = r.world.tunnels.attach(ClientId::derive("mac"), new_sink);
        assert_eq!(sent(&mut r.to_guest), [guest_reset(stream)]);

        // The old connection is ignored from now on.
        r.client.open(StreamId::generate(), r.forward);
        assert!(sent(&mut r.to_guest).is_empty());
        assert!(sent(&mut r.to_client).is_empty());

        assert_eq!(
            new.open_forward(r.port, 40_000),
            ActionOutcome::ForwardOpened { forward: r.forward }
        );
        drop(r.client);
        assert!(r.world.hub.get(&EntityKey::Forward(r.forward)).is_some());
        let stream = StreamId::generate();
        new.open(stream, r.forward);
        assert_eq!(
            sent(&mut r.to_guest),
            [Out::Message(HostToGuest::TunnelOpen {
                stream,
                target: TunnelTarget::Port(5173),
            })]
        );
        assert!(sent(&mut new_rx).is_empty());

        drop(new);
        assert_eq!(sent(&mut r.to_guest), [guest_reset(stream)]);
        assert!(r.world.hub.get(&EntityKey::Forward(r.forward)).is_none());
    }

    #[test]
    fn closing_a_forward_or_losing_its_port_resets_its_streams() {
        let mut r = relayed();
        let stream = r.open();
        r.client.close_forward(r.forward);
        assert_eq!(sent(&mut r.to_guest), [guest_reset(stream)]);
        assert_eq!(sent(&mut r.to_client), [client_reset(stream)]);
        assert!(r.world.hub.get(&EntityKey::Forward(r.forward)).is_none());

        r.forward = forward(&r.client, r.port);
        let stream = r.open();
        r.world.tunnels.port_removed(r.port);
        assert_eq!(sent(&mut r.to_guest), [guest_reset(stream)]);
        assert_eq!(sent(&mut r.to_client), [client_reset(stream)]);
        assert!(r.world.hub.get(&EntityKey::Forward(r.forward)).is_none());
    }

    #[test]
    fn another_clients_forward_is_no_such_port() {
        let mut r = relayed();
        let (phone_sink, mut phone_rx) = sink();
        let phone = r
            .world
            .tunnels
            .attach(ClientId::derive("phone"), phone_sink);
        phone.open(StreamId::generate(), r.forward);
        assert!(is_refusal(&sent(&mut phone_rx), ErrorCode::NoSuchPort));
        phone.close_forward(r.forward);
        assert!(r.world.hub.get(&EntityKey::Forward(r.forward)).is_some());
        assert!(sent(&mut r.to_guest).is_empty());
    }

    #[tokio::test]
    async fn a_folder_port_on_this_device_connects_here() {
        let world = world();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listen(
            &world.hub,
            world.folder,
            listener.local_addr().unwrap().port(),
        );
        let (client_sink, mut rx) = sink();
        let client = world.tunnels.attach(ClientId::derive("mac"), client_sink);
        let forward = forward(&client, port);

        let stream = StreamId::generate();
        client.open(stream, forward);
        let (mut server, _) = listener.accept().await.unwrap();
        assert_eq!(
            next(&mut rx).await,
            Out::Message(ServerMessage::TunnelOpened { stream })
        );
        client.data(stream, b"GET /");
        client.close(stream, TunnelEnd::Write);
        let mut got = Vec::new();
        server.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, b"GET /");
        server.write_all(b"200").await.unwrap();
        drop(server);
        assert_eq!(
            next(&mut rx).await,
            Out::Data {
                stream,
                data: b"200".to_vec()
            }
        );
        assert_eq!(
            next(&mut rx).await,
            Out::Message(ServerMessage::TunnelClose {
                stream,
                how: TunnelEnd::Write
            })
        );
        for _ in 0..100 {
            if world.tunnels.open_streams() == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the ended stream is still open");
    }

    /// A made-up key line, for the registry, which never reads it.
    fn key(name: &str) -> String {
        format!("ssh-ed25519 {name}")
    }

    #[test]
    fn an_ssh_session_relays_like_a_forward_with_none_behind_it() {
        let mut r = relayed();
        let stream = StreamId::generate();
        r.client.open_ssh(stream, r.world.container);
        assert_eq!(
            sent(&mut r.to_guest),
            [Out::Message(HostToGuest::TunnelOpen {
                stream,
                target: TunnelTarget::Ssh,
            })]
        );
        assert!(r.relay.handle(&GuestToHost::TunnelOpened { stream }));
        assert_eq!(
            sent(&mut r.to_client),
            [Out::Message(ServerMessage::TunnelOpened { stream })]
        );

        r.client.data(stream, b"SSH-2.0-client\r\n");
        r.relay.data(stream, b"SSH-2.0-guest\r\n");
        assert_eq!(
            sent(&mut r.to_guest),
            [Out::Data {
                stream,
                data: b"SSH-2.0-client\r\n".to_vec()
            }]
        );
        assert_eq!(
            sent(&mut r.to_client),
            [Out::Data {
                stream,
                data: b"SSH-2.0-guest\r\n".to_vec()
            }]
        );

        // Closing the forwards leaves it alone, since it has none.
        r.client.close_forward(r.forward);
        assert_eq!(r.world.tunnels.open_streams(), 1);
        r.client.close(stream, TunnelEnd::Write);
        r.relay.handle(&GuestToHost::TunnelClose {
            stream,
            how: TunnelEnd::Write,
        });
        assert_eq!(r.world.tunnels.open_streams(), 0);
        let _ = r.port;
    }

    #[test]
    fn an_ssh_session_needs_the_containers_link() {
        let world = world();
        let (client_sink, mut to_client) = sink();
        let client = world.tunnels.attach(ClientId::derive("mac"), client_sink);
        client.open_ssh(StreamId::generate(), world.container);
        assert!(is_refusal(&sent(&mut to_client), ErrorCode::Unreachable));

        let (guest_sink, mut to_guest) = sink();
        let relay = world.tunnels.guest(world.container, guest_sink);
        let stream = StreamId::generate();
        client.open_ssh(stream, world.container);
        assert_eq!(sent(&mut to_guest).len(), 1);
        client.open_ssh(stream, world.container);
        assert!(is_refusal(&sent(&mut to_client), ErrorCode::NotAllowed));

        // The link dropping resets it.
        drop(relay);
        assert_eq!(sent(&mut to_client), [client_reset(stream)]);
        assert_eq!(world.tunnels.open_streams(), 0);
    }

    #[test]
    fn guests_hear_every_clients_ssh_key() {
        let world = world();
        let (guest_sink, mut to_guest) = sink();
        let _relay = world.tunnels.guest(world.container, guest_sink);
        // No keys yet, so a new link hears nothing.
        assert_eq!(sent(&mut to_guest), []);

        let (mac_sink, _mac_rx) = sink();
        let mac = world.tunnels.attach(ClientId::derive("mac"), mac_sink);
        let (phone_sink, _phone_rx) = sink();
        let phone = world.tunnels.attach(ClientId::derive("phone"), phone_sink);
        assert_eq!(mac.set_ssh_key(key("b")), ActionOutcome::Done);
        assert_eq!(phone.set_ssh_key(key("a")), ActionOutcome::Done);
        // The same key again says nothing new.
        assert_eq!(phone.set_ssh_key(key("a")), ActionOutcome::Done);
        assert_eq!(
            sent(&mut to_guest),
            [
                Out::Message(HostToGuest::SshAuthorized {
                    keys: vec![key("b")]
                }),
                Out::Message(HostToGuest::SshAuthorized {
                    keys: vec![key("a"), key("b")]
                }),
            ]
        );

        // A client's new key takes the place of its old one, and outlives
        // its connection.
        mac.set_ssh_key(key("c"));
        drop(mac);
        assert_eq!(
            sent(&mut to_guest),
            [Out::Message(HostToGuest::SshAuthorized {
                keys: vec![key("a"), key("c")]
            })]
        );

        // A new link hears them all first.
        let (guest_sink, mut to_guest) = sink();
        let _relay = world.tunnels.guest(world.container, guest_sink);
        assert_eq!(
            sent(&mut to_guest),
            [Out::Message(HostToGuest::SshAuthorized {
                keys: vec![key("a"), key("c")]
            })]
        );
        drop(phone);
    }

    #[test]
    fn the_ssh_keys_are_bounded() {
        let world = world();
        let mut clients = Vec::new();
        for n in 0..MAX_SSH_KEYS {
            let (client_sink, _rx) = sink();
            let client = world
                .tunnels
                .attach(ClientId::derive(&format!("client {n}")), client_sink);
            assert_eq!(client.set_ssh_key(key(&n.to_string())), ActionOutcome::Done);
            clients.push(client);
        }
        let (client_sink, _rx) = sink();
        let one_more = world
            .tunnels
            .attach(ClientId::derive("one more"), client_sink);
        assert!(matches!(
            one_more.set_ssh_key(key("more")),
            ActionOutcome::Refused { .. }
        ));
        // One that has a key may still change it.
        assert_eq!(clients[0].set_ssh_key(key("new")), ActionOutcome::Done);
    }
}
