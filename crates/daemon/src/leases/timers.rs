// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What ends a lease without a call: its time limit, its idle limit, and
//! its agent going away.

use std::pin::pin;
use std::time::Instant;

use maxhq_protocol::agents::Actor;
use maxhq_protocol::ids::{AgentId, LeaseId};
use maxhq_protocol::leases::{LeaseEnd, LeaseState, TestKind};
use maxhq_protocol::state::{Change, EntityKey};
use tokio::sync::broadcast::error::RecvError;

use super::store::TestHosts;

impl TestHosts {
    /// Checks the hosts, cleans the ones the last run left in use, then
    /// ends leases as their limits pass or their agents go, for as long as
    /// `maxd` runs.
    pub async fn watch(self) {
        let mut deltas = self.0.hub.subscribe(None).deltas;
        let mut offered = self.offered();
        self.check().await;
        let unclean = std::mem::take(&mut self.lock().unclean);
        for host in unclean {
            let store = self.clone();
            tokio::spawn(async move { store.clean(host).await });
        }
        loop {
            let mut changed = pin!(self.0.changed.notified());
            changed.as_mut().enable();
            let next = self.expire();
            let sleep = async {
                match next {
                    Some(at) => self.0.clock.sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                () = changed => {}
                () = sleep => {}
                delta = deltas.recv() => match delta {
                    Ok(delta) => {
                        let gone: Vec<AgentId> = delta
                            .changes
                            .iter()
                            .filter_map(|change| match change {
                                Change::Remove { key: EntityKey::Agent(agent) } => Some(*agent),
                                _ => None,
                            })
                            .collect();
                        self.holders_gone(|agent| gone.contains(&agent));
                    }
                    Err(RecvError::Lagged(_)) => {
                        let hub = self.0.hub.clone();
                        self.holders_gone(|agent| hub.get(&EntityKey::Agent(agent)).is_none());
                    }
                    Err(RecvError::Closed) => return,
                },
            }
            // The stored settings arrive once the sessions revive, and you
            // change them later; either can change what to look for.
            let now = self.offered();
            if now != offered {
                offered = now;
                let store = self.clone();
                tokio::spawn(async move { store.check().await });
            }
        }
    }

    /// What the settings say this device offers, which a check follows.
    fn offered(&self) -> (Vec<TestKind>, bool) {
        let settings = self.settings();
        (settings.kinds, settings.mac_host)
    }

    /// Ends the leases whose limits passed, and answers when the next one
    /// passes.
    fn expire(&self) -> Option<Instant> {
        let now = self.0.clock.now();
        let mut next: Option<Instant> = None;
        let mut over: Vec<(LeaseId, LeaseEnd)> = Vec::new();
        for held in self.lock().leases.values() {
            let Some((ends, idle)) = held.deadlines else {
                continue;
            };
            if held.lease.state != LeaseState::Ready {
                continue;
            }
            // A call that runs keeps it from going idle.
            let idle = (held.calls == 0).then_some(idle);
            if now >= ends {
                over.push((held.lease.id, LeaseEnd::TimeUp));
            } else if idle.is_some_and(|idle| now >= idle) {
                over.push((held.lease.id, LeaseEnd::Idle));
            } else {
                let soonest = idle.map_or(ends, |idle| idle.min(ends));
                next = Some(next.map_or(soonest, |next| next.min(soonest)));
            }
        }
        self.end_all(over);
        next
    }

    /// Ends the leases held by agents that `gone` names.
    fn holders_gone(&self, gone: impl Fn(AgentId) -> bool) {
        let over: Vec<(LeaseId, LeaseEnd)> = self
            .lock()
            .leases
            .values()
            .filter(|held| held.lease.state != LeaseState::Ended)
            .filter(|held| matches!(held.lease.holder, Actor::Agent { agent } if gone(agent)))
            .map(|held| (held.lease.id, LeaseEnd::HolderGone))
            .collect();
        self.end_all(over);
    }

    /// Ends each lease for its reason, and takes their machines back each
    /// on its own.
    fn end_all(&self, over: Vec<(LeaseId, LeaseEnd)>) {
        if over.is_empty() {
            return;
        }
        let closing: Vec<_> = self.edit(|state, touched| {
            over.into_iter()
                .filter_map(|(id, reason)| state.begin_end(id, reason, touched))
                .collect()
        });
        for on in closing {
            let store = self.clone();
            tokio::spawn(async move { store.finish(on).await });
        }
    }
}
