// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The open transfers on one side of a link. Each one is a task that reads
//! events from an inbox, so the connection that reads frames never waits
//! on a disk or another link.

use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use maxhq_protocol::ids::{TransferId, WorkspaceId};
use maxhq_wire::Window;
use tokio::sync::mpsc;
use tokio::time::Interval;
use tracing::debug;

/// The most chunk bytes one transfer may have waiting for its task. Credit
/// keeps an honest peer well under this, so a peer that passes it sent
/// more than it was allowed, or sent the same bytes over and over.
pub const QUEUE_LIMIT: u64 = 2 * Window::MAX;

/// Chunk bytes waiting for a transfer's task.
pub struct Chunk {
    /// Where `data` starts in the transfer's bytes.
    pub offset: u64,
    /// The bytes.
    pub data: Vec<u8>,
    _ticket: Ticket,
}

impl Debug for Chunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Chunk")
            .field("offset", &self.offset)
            .field("len", &self.data.len())
            .finish()
    }
}

/// Counts a chunk's bytes against [`QUEUE_LIMIT`] until the task drops it.
struct Ticket {
    queued: Arc<AtomicU64>,
    len: u64,
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.queued.fetch_sub(self.len, Ordering::Relaxed);
    }
}

/// Who may send frames that name a transfer on the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Peer {
    /// The client that started it.
    Client,
    /// The guest in this workspace, which the host gave the transfer to.
    Guest(WorkspaceId),
}

struct Slot<E, P> {
    events: mpsc::UnboundedSender<E>,
    queued: Arc<AtomicU64>,
    peers: Vec<P>,
    generation: u64,
}

struct Table<E, P> {
    slots: HashMap<TransferId, Slot<E, P>>,
    next: u64,
}

/// The open transfers, each with its inbox. `P` names who may reach one:
/// a [`Peer`] on the host, and the host alone in a guest.
pub struct Registry<E, P = Peer> {
    table: Arc<Mutex<Table<E, P>>>,
}

impl<E, P> Clone for Registry<E, P> {
    fn clone(&self) -> Self {
        Self {
            table: self.table.clone(),
        }
    }
}

impl<E, P> Default for Registry<E, P> {
    fn default() -> Self {
        Self {
            table: Arc::new(Mutex::new(Table {
                slots: HashMap::new(),
                next: 0,
            })),
        }
    }
}

/// What came while a transfer's task waited.
pub enum Next<E> {
    /// An event.
    Event(E),
    /// The tick fired.
    Tick,
    /// Nothing came for a while, and the transfer is closed.
    Idle,
}

/// A transfer's events. Dropping it closes the transfer.
pub struct Inbox<E, P = Peer> {
    events: mpsc::UnboundedReceiver<E>,
    transfer: TransferId,
    generation: u64,
    registry: Registry<E, P>,
}

fn lock<E, P>(table: &Mutex<Table<E, P>>) -> MutexGuard<'_, Table<E, P>> {
    table
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl<E, P: Copy + PartialEq + Debug> Registry<E, P> {
    /// Opens a transfer that frames from `peers` may name, or returns
    /// `None` when it's open already.
    pub fn open(&self, transfer: TransferId, peers: Vec<P>) -> Option<Inbox<E, P>> {
        let mut table = lock(&self.table);
        if table.slots.contains_key(&transfer) {
            return None;
        }
        let (tx, rx) = mpsc::unbounded_channel();
        let generation = table.next;
        table.next += 1;
        table.slots.insert(
            transfer,
            Slot {
                events: tx,
                queued: Arc::new(AtomicU64::new(0)),
                peers,
                generation,
            },
        );
        Some(Inbox {
            events: rx,
            transfer,
            generation,
            registry: self.clone(),
        })
    }

    /// Whether a transfer is open.
    pub fn is_open(&self, transfer: TransferId) -> bool {
        lock(&self.table).slots.contains_key(&transfer)
    }

    /// Hands an event to an open transfer that `from` may name. Gives the
    /// event back when there's none.
    pub fn send(&self, transfer: TransferId, from: P, event: E) -> Result<(), E> {
        let table = lock(&self.table);
        match table.slots.get(&transfer) {
            Some(slot) if slot.peers.contains(&from) => {
                slot.events.send(event).map_err(|err| err.0)
            }
            _ => Err(event),
        }
    }

    /// Hands a chunk to an open transfer that `from` may name, unless too
    /// many of its bytes wait already. Says whether it went.
    pub fn chunk(
        &self,
        transfer: TransferId,
        from: P,
        offset: u64,
        data: Vec<u8>,
        event: impl FnOnce(Chunk) -> E,
    ) -> bool {
        let table = lock(&self.table);
        let Some(slot) = table
            .slots
            .get(&transfer)
            .filter(|slot| slot.peers.contains(&from))
        else {
            debug!(%transfer, ?from, "dropped a chunk for a transfer that isn't open");
            return false;
        };
        let len = data.len() as u64;
        if slot.queued.load(Ordering::Relaxed) + len > QUEUE_LIMIT {
            debug!(%transfer, ?from, "dropped a chunk past what a transfer may queue");
            return false;
        }
        slot.queued.fetch_add(len, Ordering::Relaxed);
        let chunk = Chunk {
            offset,
            data,
            _ticket: Ticket {
                queued: slot.queued.clone(),
                len,
            },
        };
        slot.events.send(event(chunk)).is_ok()
    }

    /// Hands an event to every open transfer that `from` may name.
    pub fn each(&self, from: P, event: impl Fn() -> E) {
        let table = lock(&self.table);
        for slot in table.slots.values() {
            if slot.peers.contains(&from) {
                let _ = slot.events.send(event());
            }
        }
    }
}

impl<E, P> Inbox<E, P> {
    /// The next event, or `None` once nothing can send any.
    pub async fn recv(&mut self) -> Option<E> {
        self.events.recv().await
    }

    /// The next event, or a tick of `tick`, or [`Next::Idle`] once nothing
    /// has come for `idle`. An idle transfer is closed by then, so a
    /// resend that comes later opens a new one instead of waiting here.
    pub async fn next(&mut self, idle: Duration, mut tick: Option<&mut Interval>) -> Next<E> {
        loop {
            let recv = tokio::time::timeout(idle, self.events.recv());
            let event = match tick.as_mut() {
                Some(tick) => tokio::select! {
                    event = recv => event,
                    _ = tick.tick() => return Next::Tick,
                },
                None => recv.await,
            };
            match event {
                Ok(Some(event)) => return Next::Event(event),
                Ok(None) => return Next::Idle,
                Err(_) if self.close_if_empty() => return Next::Idle,
                Err(_) => {}
            }
        }
    }

    /// The next event if one waits.
    pub fn try_recv(&mut self) -> Option<E> {
        self.events.try_recv().ok()
    }

    /// Closes the transfer unless events wait, and says whether it did.
    /// Sends take the same lock, so an event either waits here already or
    /// finds the transfer closed and goes back to its sender.
    pub fn close_if_empty(&mut self) -> bool {
        let mut table = lock(&self.registry.table);
        if !self.events.is_empty() {
            return false;
        }
        if table
            .slots
            .get(&self.transfer)
            .is_some_and(|slot| slot.generation == self.generation)
        {
            table.slots.remove(&self.transfer);
        }
        true
    }
}

impl<E, P> Drop for Inbox<E, P> {
    fn drop(&mut self) {
        let mut table = lock(&self.registry.table);
        if table
            .slots
            .get(&self.transfer)
            .is_some_and(|slot| slot.generation == self.generation)
        {
            table.slots.remove(&self.transfer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    enum Event {
        Ping,
        Chunk(Chunk),
    }

    #[tokio::test]
    async fn only_the_named_peers_reach_a_transfer_and_closing_frees_it() {
        let registry = Registry::<Event>::default();
        let transfer = TransferId::generate();
        let guest = WorkspaceId::generate();
        let mut inbox = registry.open(transfer, vec![Peer::Guest(guest)]).unwrap();
        assert!(registry.open(transfer, vec![Peer::Client]).is_none());
        assert!(registry.send(transfer, Peer::Client, Event::Ping).is_err());
        assert!(registry
            .send(transfer, Peer::Guest(WorkspaceId::generate()), Event::Ping)
            .is_err());
        registry
            .send(transfer, Peer::Guest(guest), Event::Ping)
            .unwrap();
        assert!(matches!(inbox.recv().await, Some(Event::Ping)));
        drop(inbox);
        assert!(!registry.is_open(transfer));
        assert!(registry.open(transfer, vec![Peer::Client]).is_some());
    }

    #[test]
    fn chunks_past_the_queue_limit_are_dropped_until_the_task_takes_some() {
        let registry = Registry::<Event>::default();
        let transfer = TransferId::generate();
        let mut inbox = registry.open(transfer, vec![Peer::Client]).unwrap();
        let data = vec![0u8; 1024 * 1024];
        let fits = usize::try_from(QUEUE_LIMIT).unwrap() / data.len();
        for index in 0..fits {
            assert!(registry.chunk(
                transfer,
                Peer::Client,
                (index * data.len()) as u64,
                data.clone(),
                Event::Chunk
            ));
        }
        assert!(!registry.chunk(transfer, Peer::Client, 0, data.clone(), Event::Chunk));
        let Some(Event::Chunk(chunk)) = inbox.try_recv() else {
            panic!("a chunk waits");
        };
        assert_eq!(chunk.offset, 0);
        drop(chunk);
        assert!(registry.chunk(transfer, Peer::Client, 0, data, Event::Chunk));
    }

    #[test]
    fn a_waiting_event_keeps_a_transfer_from_closing() {
        let registry = Registry::<Event, ()>::default();
        let transfer = TransferId::generate();
        let mut inbox = registry.open(transfer, vec![()]).unwrap();
        registry.send(transfer, (), Event::Ping).unwrap();
        assert!(!inbox.close_if_empty());
        assert!(registry.is_open(transfer));
        assert!(inbox.try_recv().is_some());
        assert!(inbox.close_if_empty());
        assert!(!registry.is_open(transfer));
        assert!(registry.send(transfer, (), Event::Ping).is_err());
    }
}
