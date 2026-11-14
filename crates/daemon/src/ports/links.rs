// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Joins the guest links to the ports and tunnels. [`Links`] takes each
//! guest link's ports and stream messages, relays the streams through
//! [`Tunnels`] and keeps [`HostPorts`] current. It tells each guest which
//! holders to watch and how often, and does the same for host folder
//! agents through [`run_local`]. The session manager says what it holds
//! through a [`HolderWatch`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use maxhq_protocol::guest::{GuestPort, GuestToHost, HostToGuest, Pace, WatchedHolder};
use maxhq_protocol::ids::{StreamId, WorkspaceId};
use tokio::sync::watch;
use tracing::{debug, warn};

use super::{local_scanner, pace, run_local, HostPorts, Jobs, LocalHolder, SharedScanner};
use crate::guestlink::wire::SendError;
use crate::guestlink::{GuestSender, GuestTunnels};
use crate::hub::Hub;
use crate::later::Later;
use crate::tunnel::{self, Gone, GuestRelay, Out, Sink, Tunnels, Writer};

/// Why a stream the guest link has no room for is refused.
const NO_ROOM: &str = "the workspace has too many tunnels open";

/// A holder the session manager runs, as the port watch sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Holding {
    /// The workspace its agents run in.
    pub workspace: WorkspaceId,
    /// Whether it runs in that workspace's container, not on this device.
    pub guest: bool,
    /// The holder, with its process ID as the place it runs in sees it.
    pub holder: WatchedHolder,
}

/// What the session manager holds now, for the port watch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Holdings {
    /// Whether a client is connected.
    pub client: bool,
    /// Every holder that has started.
    pub holders: Vec<Holding>,
}

/// Where the session manager says what it holds, for the port watch, and
/// where it finds the scanner for this device's holders. Clones share it.
#[derive(Clone)]
pub struct HolderWatch {
    holdings: Arc<watch::Sender<Holdings>>,
    scanner: SharedScanner,
    jobs: Arc<watch::Sender<Jobs>>,
}

impl HolderWatch {
    /// Nothing held and no client yet.
    pub fn new() -> Self {
        Self {
            holdings: Arc::new(watch::Sender::new(Holdings::default())),
            scanner: local_scanner(),
            jobs: Arc::new(watch::Sender::new(Jobs::new())),
        }
    }

    /// Says what's held now. Only a change wakes the watch.
    pub fn set(&self, mut holdings: Holdings) {
        holdings
            .holders
            .sort_by_key(|holding| holding.holder.session);
        self.holdings.send_if_modified(|now| {
            let changed = *now != holdings;
            if changed {
                *now = holdings;
            }
            changed
        });
    }

    /// The scanner the port poll uses for this device's holders, for the
    /// Running list.
    pub fn scanner(&self) -> SharedScanner {
        self.scanner.clone()
    }

    /// What each holder on this device runs besides its CLI, as the port
    /// poll last found it. It changes only when that does.
    pub fn jobs(&self) -> watch::Receiver<Jobs> {
        self.jobs.subscribe()
    }

    /// Says what each holder runs, as the port poll would.
    #[cfg(test)]
    pub fn set_jobs(&self, jobs: Jobs) {
        self.jobs.send_replace(jobs);
    }

    fn now(&self) -> Holdings {
        self.holdings.borrow().clone()
    }
}

impl Default for HolderWatch {
    fn default() -> Self {
        Self::new()
    }
}

/// The hub between the guest links, the tunnels and the ports. Clones
/// share it. The guest links take it as their [`GuestTunnels`], and `maxd`
/// spawns [`Links::run`].
#[derive(Clone)]
pub struct Links {
    inner: Arc<Inner>,
}

struct Inner {
    tunnels: Tunnels,
    ports: HostPorts,
    holders: HolderWatch,
    /// Each guest link that's up.
    guests: Mutex<HashMap<WorkspaceId, Guest>>,
    /// What [`run_local`] watches.
    local: watch::Sender<(Pace, Vec<LocalHolder>)>,
}

/// One guest link that's up.
struct Guest {
    /// The tunnels' side of the link.
    relay: Arc<GuestRelay>,
    /// Sends to the guest in order: the tunnels' messages and stream bytes,
    /// and each watch.
    sink: Arc<tunnel::Queue<HostToGuest>>,
    /// The last watch the guest got.
    sent: (Pace, Vec<WatchedHolder>),
}

impl Links {
    /// Ports go in `hub`, and streams through `tunnels`. `holders` is what
    /// the session manager sets.
    pub fn new(hub: Hub, tunnels: Tunnels, holders: HolderWatch) -> Self {
        Self {
            inner: Arc::new(Inner {
                ports: HostPorts::new(hub, tunnels.clone()),
                tunnels,
                holders,
                guests: Mutex::new(HashMap::new()),
                local: watch::Sender::new((Pace::Slow, Vec::new())),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<WorkspaceId, Guest>> {
        self.inner
            .guests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Removes gone ports, polls for host folder agents' ports, and keeps
    /// every watch current, for as long as it runs.
    pub async fn run(self) {
        let sweep = self.inner.ports.clone().sweep_forever();
        let local = run_local(
            self.inner.ports.clone(),
            self.inner.local.subscribe(),
            self.inner.holders.scanner(),
            self.inner.holders.jobs.clone(),
        );
        tokio::join!(sweep, local, self.follow());
    }

    /// Sends each change in what's held on to the watches.
    async fn follow(&self) {
        let mut held = self.inner.holders.holdings.subscribe();
        loop {
            let now = held.borrow_and_update().clone();
            self.apply(&now);
            if held.changed().await.is_err() {
                return;
            }
        }
    }

    /// Brings every watch up to date with what's held.
    fn apply(&self, now: &Holdings) {
        let local: Vec<LocalHolder> = now
            .holders
            .iter()
            .filter(|holding| !holding.guest)
            .map(|holding| LocalHolder {
                workspace: holding.workspace,
                holder: holding.holder.clone(),
            })
            .collect();
        let local = (pace(now.client, !local.is_empty()), local);
        self.inner.local.send_if_modified(|was| {
            let changed = *was != local;
            if changed {
                *was = local;
            }
            changed
        });

        let mut guests = self.lock();
        for (workspace, guest) in guests.iter_mut() {
            let watch = watch_in(now, *workspace);
            if guest.sent != watch {
                self.watch(*workspace, &guest.sink, &watch);
                guest.sent = watch;
            }
        }
    }

    /// Tells the ports and the guest which holders to watch.
    fn watch(
        &self,
        workspace: WorkspaceId,
        sink: &tunnel::Queue<HostToGuest>,
        (pace, holders): &(Pace, Vec<WatchedHolder>),
    ) {
        self.inner.ports.watch(workspace, holders);
        let watch = HostToGuest::WatchPorts {
            pace: *pace,
            holders: holders.clone(),
        };
        if sink.send(Out::Message(watch)).is_err() {
            debug!(%workspace, "the guest link went before its watch");
        }
    }

    /// The tunnels' side of a guest link, while it's up.
    fn relay(&self, workspace: WorkspaceId) -> Option<Arc<GuestRelay>> {
        self.lock().get(&workspace).map(|guest| guest.relay.clone())
    }

    /// Drops a guest link's entry. Its relay goes outside the lock, since
    /// dropping it resets the streams through it.
    fn drop_guest(&self, workspace: WorkspaceId) {
        let gone = self.lock().remove(&workspace);
        drop(gone);
    }
}

/// The watch a container's guest gets: its holders, fast while a client is
/// connected and one of them runs, even between turns, so a server an
/// agent left running is seen to go.
fn watch_in(now: &Holdings, workspace: WorkspaceId) -> (Pace, Vec<WatchedHolder>) {
    let mine: Vec<WatchedHolder> = now
        .holders
        .iter()
        .filter(|holding| holding.guest && holding.workspace == workspace)
        .map(|holding| holding.holder.clone())
        .collect();
    (pace(now.client, !mine.is_empty()), mine)
}

impl GuestTunnels for Links {
    fn link_up(&self, workspace: WorkspaceId, sender: GuestSender) {
        let relay_of = Later::new();
        let sink = Arc::new(tunnel::Queue::spawn(ToGuest {
            sender: sender.clone(),
            relay: relay_of.clone(),
        }));
        let watch = watch_in(&self.inner.holders.now(), workspace);
        let old = {
            let mut guests = self.lock();
            // The link may have closed already, and then its link_down has
            // come or comes next, after this lock.
            if !sender.is_up() {
                return;
            }
            let relay = Arc::new(self.inner.tunnels.guest(workspace, sink.clone()));
            relay_of.set(Arc::downgrade(&relay));
            // The guest polls nothing until its first watch.
            self.watch(workspace, &sink, &watch);
            guests.insert(
                workspace,
                Guest {
                    relay,
                    sink,
                    sent: watch,
                },
            )
        };
        drop(old);
    }

    fn ports(&self, workspace: WorkspaceId, listening: Vec<GuestPort>) {
        self.inner.ports.report(workspace, listening);
    }

    fn message(&self, workspace: WorkspaceId, message: GuestToHost) {
        let handled = self
            .relay(workspace)
            .is_some_and(|relay| relay.handle(&message));
        if !handled {
            debug!(%workspace, "dropped a guest's tunnel message with no stream for it");
        }
    }

    fn data(&self, workspace: WorkspaceId, stream: StreamId, data: Vec<u8>) {
        match self.relay(workspace) {
            Some(relay) => relay.data(stream, &data),
            None => debug!(%workspace, %stream, "dropped a guest's tunnel bytes"),
        }
    }

    fn link_down(&self, workspace: WorkspaceId) {
        self.drop_guest(workspace);
        self.inner.ports.down(workspace);
    }

    fn forget(&self, workspace: WorkspaceId) {
        self.drop_guest(workspace);
        self.inner.ports.forget(workspace);
    }
}

/// Writes what the tunnels and the watch send to a guest link.
struct ToGuest {
    sender: GuestSender,
    /// The link's relay, which hears of a stream the link had no room for.
    relay: Later<Weak<GuestRelay>>,
}

impl Writer<HostToGuest> for ToGuest {
    async fn write(&mut self, out: Out<HostToGuest>) -> Result<(), Gone> {
        let sent = match &out {
            Out::Message(message) => self.sender.send(message).await,
            Out::Data { stream, data } => self.sender.tunnel_data(*stream, data).await,
        };
        match sent {
            Ok(()) => Ok(()),
            Err(SendError::Down) => Err(Gone),
            Err(SendError::TooManyTunnels) => {
                if let Out::Message(HostToGuest::TunnelOpen { stream, .. }) = out
                    && let Some(relay) = self.relay.get().and_then(Weak::upgrade)
                {
                    relay.handle(&GuestToHost::TunnelRefused {
                        stream,
                        reason: NO_ROOM.to_owned(),
                    });
                }
                Ok(())
            }
            Err(SendError::Encode(err)) => {
                warn!(%err, "a tunnel message for the guest didn't encode");
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::ids::{AgentId, SessionId};

    use super::*;

    fn holding(name: &str, workspace: WorkspaceId, guest: bool) -> Holding {
        Holding {
            workspace,
            guest,
            holder: WatchedHolder {
                session: SessionId::derive(name),
                agent: Some(AgentId::derive(name)),
                pid: 100,
            },
        }
    }

    #[test]
    fn a_guest_watches_its_own_holders_fast_only_with_a_client() {
        let here = WorkspaceId::derive("here");
        let there = WorkspaceId::derive("there");
        let mut now = Holdings {
            client: true,
            holders: vec![
                holding("a", here, true),
                holding("b", there, true),
                holding("c", there, false),
            ],
        };
        // An agent between turns can still have a server running.
        let (pace, holders) = watch_in(&now, here);
        assert_eq!(pace, Pace::Fast);
        assert_eq!(holders, [now.holders[0].holder.clone()]);
        assert_eq!(watch_in(&now, there).1, [now.holders[1].holder.clone()]);
        assert_eq!(
            watch_in(&now, WorkspaceId::derive("none")),
            (Pace::Slow, vec![])
        );
        now.client = false;
        assert_eq!(watch_in(&now, here).0, Pace::Slow);
    }

    #[tokio::test]
    async fn a_set_wakes_the_watch_only_when_it_changes() {
        let watch = HolderWatch::new();
        let mut rx = watch.holdings.subscribe();
        rx.borrow_and_update();
        let here = WorkspaceId::derive("here");
        let holdings = Holdings {
            client: true,
            holders: vec![holding("b", here, true), holding("a", here, true)],
        };
        watch.set(holdings.clone());
        assert!(rx.has_changed().unwrap());
        let sorted = rx.borrow_and_update().clone();
        let mut sessions: Vec<SessionId> =
            sorted.holders.iter().map(|h| h.holder.session).collect();
        let before = sessions.clone();
        sessions.sort();
        assert_eq!(sessions, before);
        watch.set(holdings);
        assert!(!rx.has_changed().unwrap());
    }
}
