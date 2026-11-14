// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Port forwards over a device link: a local port on this machine that
//! carries each connection to a port listening in a workspace.
//!
//! [`Forwards`] holds one link's forwards. Opening one binds 127.0.0.1 and
//! ::1 on the workspace's port number, or the next free one that browsers
//! load, and asks `maxd` with `OpenForward`. Each connection to the local
//! port opens a stream with `TunnelOpen`, and its bytes go in tunnel frames
//! with [`STREAM_CREDIT`] of credit each way. At most
//! [`MAX_STREAMS_PER_FORWARD`] streams are open through one forward, and
//! [`MAX_STREAMS_PER_CLIENT`] through the link.
//!
//! A forward closes when you close it, when its port or its agent leaves
//! the device's state, or when [`Forwards`] drops, as it does when the app
//! quits. `maxd` keeps a port listed for 10 s after it goes, so a server
//! that restarts keeps its forward. A forward stays through a reconnect and
//! asks `maxd` again once the link is back. Nothing restores forwards when
//! the app starts.
//!
//! The link's owner implements [`ForwardLink`] over its connection. It
//! hands [`Forwards`] the tunnel messages and frames `maxd` sends, the
//! link's events, and word of the connection going and coming back.

mod bind;
mod pump;

use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use futures::future::BoxFuture;
use maxhq_protocol::frame::{encode_message, encode_tunnel_data, EncodeError};
use maxhq_protocol::ids::{AgentId, ForwardId, PortId, StreamId};
use maxhq_protocol::message::TunnelEnd;
use maxhq_protocol::model::ListeningPort;
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::{ClientMessage, ServerMessage};
use thiserror::Error;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tracing::debug;

use crate::LinkEvent;

/// Credit each stream starts with, each way, in bytes.
pub const STREAM_CREDIT: u64 = 256 * 1024;
/// The most streams open through one forward.
pub const MAX_STREAMS_PER_FORWARD: usize = 256;
/// The most streams open through one link.
pub const MAX_STREAMS_PER_CLIENT: usize = 1024;

/// How long accepting waits after the system refused a connection, such
/// as when this process is out of file handles.
const ACCEPT_PAUSE: Duration = Duration::from_millis(100);

/// Something a forward sends on its link: a message, or bytes on a
/// stream.
#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    /// A message, sent as a kind 0 frame.
    Message(ClientMessage),
    /// Bytes on a stream, sent as kind 4 frames.
    Data {
        /// The stream.
        stream: StreamId,
        /// The bytes, at most one tunnel frame's worth.
        data: Vec<u8>,
    },
}

impl Out {
    /// Appends the frames for this to `out`.
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        match self {
            Self::Message(message) => encode_message(out, message),
            Self::Data { stream, data } => {
                encode_tunnel_data(out, *stream, data);
                Ok(())
            }
        }
    }
}

/// The link's connection is gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
#[error("the link is gone")]
pub struct Gone;

/// Why a forward didn't open.
#[derive(Debug, Error)]
pub enum ForwardError {
    /// No local port could be bound.
    #[error("couldn't listen on a local port: {0}")]
    Bind(#[source] io::Error),
    /// `maxd` refused it, such as for a port that went.
    #[error("{0}")]
    Refused(String),
    /// The link isn't connected now.
    #[error("not connected")]
    NotConnected,
}

/// The device link forwards go through. Its owner implements it over the
/// link's connection, and tests fake it.
pub trait ForwardLink: Send + Sync + 'static {
    /// Queues a message or a stream's bytes on the current connection,
    /// behind everything sent before. It never waits, and fails when there's
    /// no connection.
    fn send(&self, out: Out) -> Result<(), Gone>;

    /// Sends `OpenForward` for `port`, listened for on `local_port`, as an
    /// action under a fresh key, and gives the forward `maxd` opened.
    fn open(
        &self,
        port: PortId,
        local_port: u16,
    ) -> BoxFuture<'static, Result<ForwardId, ForwardError>>;
}

/// A forward as [`Forwards`] lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalForward {
    /// The port it reaches.
    pub port: PortId,
    /// The port it listens on here, on 127.0.0.1 and ::1.
    pub local_port: u16,
    /// `maxd`'s forward on the current connection, if it has answered.
    pub forward: Option<ForwardId>,
}

/// One link's forwards. Create it inside a Tokio runtime, since each
/// forward accepts in a task. Dropping it closes them all.
pub struct Forwards {
    link: Arc<dyn ForwardLink>,
    shared: Arc<Mutex<Inner>>,
}

#[derive(Clone, Copy)]
struct Limits {
    per_forward: usize,
    per_client: usize,
}

struct Inner {
    forwards: HashMap<PortId, Entry>,
    streams: HashMap<StreamId, Stream>,
    /// Tells a forward or stream from a later one in the same place.
    next_serial: u64,
    limits: Limits,
}

struct Entry {
    serial: u64,
    local_port: u16,
    agent: Option<AgentId>,
    forward: Option<ForwardId>,
    streams: HashSet<StreamId>,
    _accepts: Accepts,
}

/// A forward's accept tasks, which hold its listeners. Dropping it stops
/// them, which closes the local port.
struct Accepts(Vec<JoinHandle<()>>);

impl Drop for Accepts {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

struct Stream {
    port: PortId,
    serial: u64,
    end: pump::End,
}

fn lock(shared: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Inner {
    fn serial(&mut self) -> u64 {
        let serial = self.next_serial;
        self.next_serial += 1;
        serial
    }

    fn remove_stream(&mut self, stream: StreamId) {
        if let Some(gone) = self.streams.remove(&stream)
            && let Some(entry) = self.forwards.get_mut(&gone.port)
        {
            entry.streams.remove(&stream);
        }
    }

    /// Closes a forward and its streams, and gives `maxd`'s forward to
    /// close.
    fn remove_forward(&mut self, port: PortId) -> Option<ForwardId> {
        let entry = self.forwards.remove(&port)?;
        for stream in &entry.streams {
            self.streams.remove(stream);
        }
        entry.forward
    }

    /// Opens a stream for a connection accepted on a forward's port, or
    /// drops the connection when the forward can't take it.
    fn connect(
        &mut self,
        shared: &Arc<Mutex<Inner>>,
        link: &Arc<dyn ForwardLink>,
        port: PortId,
        serial: u64,
        socket: TcpStream,
    ) {
        let open = self.streams.len();
        let limits = self.limits;
        let stream_serial = self.serial();
        let Some(entry) = self
            .forwards
            .get_mut(&port)
            .filter(|entry| entry.serial == serial)
        else {
            return;
        };
        let Some(forward) = entry.forward else {
            debug!(%port, "dropped a local connection while the device has no forward");
            return;
        };
        if entry.streams.len() >= limits.per_forward || open >= limits.per_client {
            debug!(%port, "dropped a local connection past the stream limit");
            return;
        }
        let stream = StreamId::generate();
        // The open goes before anything the pump sends.
        if link
            .send(Out::Message(ClientMessage::TunnelOpen { stream, forward }))
            .is_err()
        {
            return;
        }
        let weak = Arc::downgrade(shared);
        let ended = Box::new(move || {
            let Some(shared) = weak.upgrade() else {
                return;
            };
            let mut inner = lock(&shared);
            if inner
                .streams
                .get(&stream)
                .is_some_and(|entry| entry.serial == stream_serial)
            {
                inner.remove_stream(stream);
            }
        });
        let end = pump::start(stream, socket, Arc::clone(link), ended);
        entry.streams.insert(stream);
        self.streams.insert(
            stream,
            Stream {
                port,
                serial: stream_serial,
                end,
            },
        );
    }
}

impl Forwards {
    /// Forwards over `link`.
    pub fn new(link: Arc<dyn ForwardLink>) -> Self {
        Self::with_limits(link, MAX_STREAMS_PER_FORWARD, MAX_STREAMS_PER_CLIENT)
    }

    fn with_limits(link: Arc<dyn ForwardLink>, per_forward: usize, per_client: usize) -> Self {
        let inner = Inner {
            forwards: HashMap::new(),
            streams: HashMap::new(),
            next_serial: 0,
            limits: Limits {
                per_forward,
                per_client,
            },
        };
        Self {
            link,
            shared: Arc::new(Mutex::new(inner)),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        lock(&self.shared)
    }

    /// Opens a forward to `port` and gives the local port it listens on.
    /// One open already gives its local port.
    pub async fn open(&self, port: &ListeningPort) -> Result<u16, ForwardError> {
        let (serial, local_port) = {
            let mut inner = self.lock();
            if let Some(entry) = inner.forwards.get(&port.id) {
                return Ok(entry.local_port);
            }
            let bound = bind::bind(port.port).map_err(ForwardError::Bind)?;
            let serial = inner.serial();
            let accepts = bound
                .listeners
                .into_iter()
                .map(|listener| {
                    let weak = Arc::downgrade(&self.shared);
                    tokio::spawn(accept(
                        listener,
                        weak,
                        Arc::clone(&self.link),
                        port.id,
                        serial,
                    ))
                })
                .collect();
            let entry = Entry {
                serial,
                local_port: bound.port,
                agent: port.agent,
                forward: None,
                streams: HashSet::new(),
                _accepts: Accepts(accepts),
            };
            inner.forwards.insert(port.id, entry);
            (serial, bound.port)
        };
        let result = self.link.open(port.id, local_port).await;
        let mut inner = self.lock();
        let current = inner
            .forwards
            .get_mut(&port.id)
            .filter(|entry| entry.serial == serial);
        match (result, current) {
            (Ok(forward), Some(entry)) => {
                entry.forward = Some(forward);
                Ok(local_port)
            }
            (Ok(forward), None) => {
                let _ = self
                    .link
                    .send(Out::Message(ClientMessage::CloseForward { forward }));
                Err(ForwardError::Refused(
                    "the forward was closed while it opened".into(),
                ))
            }
            (Err(err), Some(_)) => {
                inner.remove_forward(port.id);
                Err(err)
            }
            (Err(err), None) => Err(err),
        }
    }

    /// Closes the forward to `port`, if one is open.
    pub fn close(&self, port: PortId) {
        let forward = self.lock().remove_forward(port);
        self.tell_closed(forward);
    }

    /// The local port a forward to `port` listens on.
    pub fn local_port(&self, port: PortId) -> Option<u16> {
        self.lock()
            .forwards
            .get(&port)
            .map(|entry| entry.local_port)
    }

    /// Every open forward, by local port.
    pub fn list(&self) -> Vec<LocalForward> {
        let inner = self.lock();
        let mut list: Vec<LocalForward> = inner
            .forwards
            .iter()
            .map(|(port, entry)| LocalForward {
                port: *port,
                local_port: entry.local_port,
                forward: entry.forward,
            })
            .collect();
        list.sort_by_key(|forward| forward.local_port);
        list
    }

    /// Takes the link's events. A forward closes when its port or agent
    /// leaves the device's state.
    pub fn apply(&self, event: &LinkEvent) {
        let gone: Vec<PortId> = {
            let inner = self.lock();
            match event {
                LinkEvent::Synced(entities) => {
                    let mut ports = HashSet::new();
                    let mut agents = HashSet::new();
                    for entity in entities {
                        match entity {
                            Entity::ListeningPort(port) => {
                                ports.insert(port.id);
                            }
                            Entity::Agent(agent) => {
                                agents.insert(agent.id);
                            }
                            _ => {}
                        }
                    }
                    inner
                        .forwards
                        .iter()
                        .filter(|(port, entry)| {
                            !ports.contains(*port)
                                || entry.agent.is_some_and(|agent| !agents.contains(&agent))
                        })
                        .map(|(port, _)| *port)
                        .collect()
                }
                LinkEvent::Changed(changes) => {
                    let mut gone = Vec::new();
                    for change in changes {
                        match change {
                            Change::Remove {
                                key: EntityKey::ListeningPort(port),
                            } => gone.push(*port),
                            Change::Remove {
                                key: EntityKey::Agent(agent),
                            } => gone.extend(
                                inner
                                    .forwards
                                    .iter()
                                    .filter(|(_, entry)| entry.agent == Some(*agent))
                                    .map(|(port, _)| *port),
                            ),
                            _ => {}
                        }
                    }
                    gone
                }
                LinkEvent::Link(_)
                | LinkEvent::Timeline { .. }
                | LinkEvent::Workflow { .. }
                | LinkEvent::RouteWorked { .. }
                | LinkEvent::PeerRequest { .. } => Vec::new(),
            }
        };
        for port in gone {
            self.close(port);
        }
    }

    /// Takes a tunnel message from `maxd`. False for any other message.
    pub fn handle(&self, message: &ServerMessage) -> bool {
        let mut inner = self.lock();
        match message {
            ServerMessage::TunnelOpened { stream } => {
                if let Some(entry) = inner.streams.get(stream) {
                    entry.end.opened();
                }
            }
            ServerMessage::TunnelRefused {
                stream,
                code,
                message,
            } => {
                debug!(%stream, ?code, %message, "maxd refused a stream");
                inner.remove_stream(*stream);
            }
            ServerMessage::TunnelCredit { stream, granted } => {
                if let Some(entry) = inner.streams.get(stream) {
                    entry.end.credit(*granted);
                }
            }
            ServerMessage::TunnelClose { stream, how } => match how {
                TunnelEnd::Reset => inner.remove_stream(*stream),
                TunnelEnd::Write => {
                    if let Some(entry) = inner.streams.get(stream) {
                        entry.end.close(TunnelEnd::Write);
                    }
                }
            },
            _ => return false,
        }
        true
    }

    /// Takes a tunnel frame from `maxd`. Bytes past the credit this side
    /// granted reset the stream.
    pub fn data(&self, stream: StreamId, data: &[u8]) {
        let mut inner = self.lock();
        let Some(entry) = inner.streams.get(&stream) else {
            debug!(%stream, len = data.len(), "dropped bytes for a stream that isn't open");
            return;
        };
        if !entry.end.data(data) {
            debug!(%stream, "reset a stream maxd sent too much on");
            inner.remove_stream(stream);
            let _ = self.link.send(Out::Message(ClientMessage::TunnelClose {
                stream,
                how: TunnelEnd::Reset,
            }));
        }
    }

    /// The connection dropped. Every stream ends, and the forwards wait for
    /// [`Forwards::link_up`], dropping connections meanwhile.
    pub fn link_down(&self) {
        let mut inner = self.lock();
        inner.streams.clear();
        for entry in inner.forwards.values_mut() {
            entry.forward = None;
            entry.streams.clear();
        }
    }

    /// A new connection is up. Every forward asks `maxd` again, on the
    /// same local port, and one `maxd` refuses closes.
    pub fn link_up(&self) {
        let inner = self.lock();
        for (port, entry) in &inner.forwards {
            let port = *port;
            let (serial, local_port) = (entry.serial, entry.local_port);
            let link = Arc::clone(&self.link);
            let weak = Arc::downgrade(&self.shared);
            tokio::spawn(async move {
                let result = link.open(port, local_port).await;
                let shared = weak.upgrade();
                let mut inner = shared.as_deref().map(lock);
                let current = inner
                    .as_mut()
                    .and_then(|inner| inner.forwards.get_mut(&port))
                    .filter(|entry| entry.serial == serial);
                match (result, current) {
                    (Ok(forward), Some(entry)) => entry.forward = Some(forward),
                    (Ok(forward), None) => {
                        let _ = link.send(Out::Message(ClientMessage::CloseForward { forward }));
                    }
                    (Err(ForwardError::Refused(why)), Some(_)) => {
                        debug!(%port, %why, "maxd refused a forward again, so it closed");
                        if let Some(inner) = inner.as_mut() {
                            inner.remove_forward(port);
                        }
                    }
                    // The next connection asks again.
                    (Err(_), _) => {}
                }
            });
        }
    }

    fn tell_closed(&self, forward: Option<ForwardId>) {
        if let Some(forward) = forward {
            let _ = self
                .link
                .send(Out::Message(ClientMessage::CloseForward { forward }));
        }
    }

    /// How many streams are open, for tests.
    #[cfg(test)]
    fn open_streams(&self) -> usize {
        self.lock().streams.len()
    }
}

impl Drop for Forwards {
    fn drop(&mut self) {
        let ports: Vec<PortId> = self.lock().forwards.keys().copied().collect();
        for port in ports {
            self.close(port);
        }
    }
}

/// Accepts connections on one of a forward's listeners until the forward
/// closes.
async fn accept(
    listener: TcpListener,
    shared: Weak<Mutex<Inner>>,
    link: Arc<dyn ForwardLink>,
    port: PortId,
    serial: u64,
) {
    loop {
        let socket = match listener.accept().await {
            Ok((socket, _)) => socket,
            Err(err) => {
                debug!(%port, %err, "couldn't accept a local connection");
                tokio::time::sleep(ACCEPT_PAUSE).await;
                continue;
            }
        };
        let Some(shared) = shared.upgrade() else {
            return;
        };
        let mut inner = lock(&shared);
        if inner
            .forwards
            .get(&port)
            .is_none_or(|entry| entry.serial != serial)
        {
            return;
        }
        inner.connect(&shared, &link, port, serial, socket);
    }
}

#[cfg(test)]
mod tests;
