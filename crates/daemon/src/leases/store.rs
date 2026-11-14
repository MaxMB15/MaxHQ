// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The test hosts this device offers and their leases, as stored and
//! published. Every change goes through [`TestHosts::edit`], which stores
//! and publishes what it touched and wakes whoever waits.
//!
//! A `maxd` that comes back can't vouch for what ran while it was gone, so
//! it ends every lease it finds started with [`LeaseEnd::HostFailed`] and
//! takes those hosts back to a clean state.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use futures::future::join_all;
use maxhq_protocol::ids::{DeviceId, LeaseId, TestHostId};
use maxhq_protocol::leases::{
    host_id, LeaseEnd, LeaseState, TestHost, TestHostBackend, TestHostSettings, TestHostState,
    TestKind, TestLease,
};
use maxhq_protocol::model::DeviceSettings;
use maxhq_protocol::state::{Change, Entity, EntityKey};
use tokio::sync::Notify;
use tracing::warn;

use super::backend::TestBackend;
use super::detect::{self, Detector, System};
use crate::db::Db;
use crate::hub::Hub;
use crate::session::clock::Clock;

/// The setup text for the Mac itself while you haven't allowed leasing it.
const MAC_HOST_OFF: &str =
    "Allow agents to lease this Mac's own Simulator in its test host settings.";

/// What detection found for one host: what this device lacks for it, or
/// none when it has all it needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// The host.
    pub host: TestHostId,
    /// The setup text the app shows, or none.
    pub setup: Option<String>,
}

/// How many ended leases stay published, so their holders read why they
/// ended. Older ones are forgotten.
const KEEP_ENDED: usize = 20;

/// This device's test hosts and their leases. Clones share them.
#[derive(Clone)]
pub struct TestHosts(pub(super) Arc<Inner>);

pub(super) struct Inner {
    pub hub: Hub,
    pub db: Db,
    pub clock: Arc<dyn Clock>,
    pub device: DeviceId,
    pub backends: HashMap<TestHostId, Arc<dyn TestBackend>>,
    pub state: Mutex<State>,
    /// Woken after every change, for waiting callers and the timers.
    pub changed: Notify,
}

#[derive(Default)]
pub(super) struct State {
    pub hosts: HashMap<TestHostId, TestHost>,
    pub leases: HashMap<LeaseId, Held>,
    /// The leases waiting for each kind, first come first served.
    pub queues: HashMap<TestKind, VecDeque<LeaseId>>,
    /// Hosts going back to a clean state as a whole, which no lease
    /// starts on meanwhile.
    pub cleaning: HashSet<TestHostId>,
    /// Hosts found in use at start, to clean once the store runs.
    pub unclean: Vec<TestHostId>,
}

/// One lease and what only this run knows about it.
pub(super) struct Held {
    pub lease: TestLease,
    /// How long it lasts once ready, in minutes.
    pub minutes: u16,
    /// When its time is up and when it goes idle, once ready.
    pub deadlines: Option<(Instant, Instant)>,
    /// Why it ends, once it's ending.
    pub ending: Option<LeaseEnd>,
    /// How many calls run on it now. It doesn't go idle meanwhile.
    pub calls: u32,
}

impl Held {
    /// Whether it holds a machine or is getting one.
    pub fn active(&self) -> bool {
        matches!(
            self.lease.state,
            LeaseState::Starting | LeaseState::Ready | LeaseState::Ending
        )
    }
}

/// What one edit changed, to store and publish.
#[derive(Default)]
pub(super) struct Touched {
    hosts: BTreeSet<TestHostId>,
    leases: BTreeSet<LeaseId>,
    gone: BTreeSet<LeaseId>,
}

impl Touched {
    pub fn host(&mut self, id: TestHostId) {
        self.hosts.insert(id);
    }

    pub fn lease(&mut self, id: LeaseId) {
        self.leases.insert(id);
    }

    /// The lease is forgotten, here and as stored.
    pub fn gone(&mut self, id: LeaseId) {
        self.gone.insert(id);
    }
}

impl State {
    /// How many active leases `host` has.
    pub fn active_on(&self, host: TestHostId) -> usize {
        self.leases
            .values()
            .filter(|held| held.active() && held.lease.host == host)
            .count()
    }

    /// Puts a host that leases use back to leased or warm, by whether a
    /// lease still holds it. A host being cleaned keeps its state.
    pub fn settle(&mut self, host: TestHostId, touched: &mut Touched) {
        let active = self.active_on(host);
        if self.cleaning.contains(&host) {
            return;
        }
        if let Some(host) = self.hosts.get_mut(&host)
            && matches!(host.state, TestHostState::Leased | TestHostState::Resetting)
        {
            host.state = if active > 0 {
                TestHostState::Leased
            } else {
                TestHostState::Warm
            };
            touched.host(host.id);
        }
    }

    /// Shows each host how many wait for its kind.
    fn count_queues(&mut self, touched: &mut Touched) {
        for host in self.hosts.values_mut() {
            let waiting = self.queues.get(&host.kind).map_or(0, VecDeque::len);
            let waiting = u32::try_from(waiting).unwrap_or(u32::MAX);
            if host.queue != waiting {
                host.queue = waiting;
                touched.host(host.id);
            }
        }
    }

    /// Forgets the oldest ended leases past [`KEEP_ENDED`].
    fn prune_ended(&mut self, touched: &mut Touched) {
        let mut ended: Vec<(_, LeaseId)> = self
            .leases
            .values()
            .filter(|held| held.lease.state == LeaseState::Ended)
            .map(|held| (held.lease.started_at, held.lease.id))
            .collect();
        if ended.len() <= KEEP_ENDED {
            return;
        }
        ended.sort();
        for (_, id) in &ended[..ended.len() - KEEP_ENDED] {
            self.leases.remove(id);
            touched.gone(*id);
        }
    }
}

impl TestHosts {
    /// The hosts `backends` run, with what the database kept from the last
    /// run. Leases it finds started end, and their hosts are cleaned once
    /// [`TestHosts::watch`] starts.
    pub fn open(
        hub: Hub,
        db: Db,
        clock: Arc<dyn Clock>,
        backends: Vec<Arc<dyn TestBackend>>,
    ) -> Self {
        let device = hub.device().id;
        let backends: HashMap<TestHostId, Arc<dyn TestBackend>> = backends
            .into_iter()
            .map(|backend| (host_id(device, backend.kind(), backend.backend()), backend))
            .collect();
        let mut state = State::default();
        for mut host in db.test_hosts().unwrap_or_else(|err| {
            warn!(%err, "couldn't read the test hosts");
            Vec::new()
        }) {
            if !backends.contains_key(&host.id) {
                if let Err(err) = db.remove_test_host(host.id) {
                    warn!(%err, host = %host.id, "couldn't forget a test host");
                }
                continue;
            }
            match host.state {
                TestHostState::Building => {
                    host.state = TestHostState::Failed;
                    host.error = Some("MaxHQ restarted while it was being built.".into());
                }
                TestHostState::Leased | TestHostState::Resetting => {
                    host.state = TestHostState::Resetting;
                    state.unclean.push(host.id);
                }
                _ => {}
            }
            host.queue = 0;
            state.hosts.insert(host.id, host);
        }
        for (&id, backend) in &backends {
            state.hosts.entry(id).or_insert_with(|| TestHost {
                id,
                device,
                kind: backend.kind(),
                backend: backend.backend(),
                state: TestHostState::Off,
                setup: None,
                error: None,
                agent_device: None,
                last_reset: None,
                queue: 0,
                vm: None,
            });
        }
        let mut dropped = Vec::new();
        for mut lease in db.leases().unwrap_or_else(|err| {
            warn!(%err, "couldn't read the leases");
            Vec::new()
        }) {
            match lease.state {
                // Its caller's wait ended with the last run.
                LeaseState::Queued => {
                    dropped.push(lease.id);
                    continue;
                }
                LeaseState::Starting | LeaseState::Ready | LeaseState::Ending => {
                    lease.state = LeaseState::Ended;
                    lease.ended = Some(LeaseEnd::HostFailed);
                    if state.hosts.contains_key(&lease.host) && !state.unclean.contains(&lease.host)
                    {
                        state.unclean.push(lease.host);
                    }
                }
                LeaseState::Ended => {}
            }
            state.leases.insert(
                lease.id,
                Held {
                    lease,
                    minutes: 0,
                    deadlines: None,
                    ending: None,
                    calls: 0,
                },
            );
        }
        for &host in &state.unclean {
            if let Some(host) = state.hosts.get_mut(&host) {
                host.state = TestHostState::Resetting;
            }
        }
        let store = Self(Arc::new(Inner {
            hub,
            db,
            clock,
            device,
            backends,
            state: Mutex::new(State::default()),
            changed: Notify::new(),
        }));
        store.edit(|now, touched| {
            for &id in state.hosts.keys() {
                touched.host(id);
            }
            for &id in state.leases.keys() {
                touched.lease(id);
            }
            for id in dropped {
                touched.gone(id);
            }
            *now = state;
        });
        store
    }

    /// Makes a change, then stores and publishes what it touched and wakes
    /// whoever waits.
    pub(super) fn edit<T>(&self, change: impl FnOnce(&mut State, &mut Touched) -> T) -> T {
        let mut state = self.lock();
        let mut touched = Touched::default();
        let out = change(&mut state, &mut touched);
        state.count_queues(&mut touched);
        state.prune_ended(&mut touched);
        let db = &self.0.db;
        let mut changes = Vec::new();
        for id in &touched.hosts {
            if let Some(host) = state.hosts.get(id) {
                if let Err(err) = db.put_test_host(host) {
                    warn!(%err, host = %id, "couldn't store a test host");
                }
                changes.push(Change::Upsert {
                    value: Entity::TestHost(host.clone()),
                });
            }
        }
        for id in touched.leases.difference(&touched.gone) {
            if let Some(held) = state.leases.get(id) {
                if let Err(err) = db.put_lease(&held.lease) {
                    warn!(%err, lease = %id, "couldn't store a lease");
                }
                changes.push(Change::Upsert {
                    value: Entity::TestLease(held.lease.clone()),
                });
            }
        }
        for id in &touched.gone {
            if let Err(err) = db.remove_lease(*id) {
                warn!(%err, lease = %id, "couldn't forget a lease");
            }
            changes.push(Change::Remove {
                key: EntityKey::TestLease(*id),
            });
        }
        if !changes.is_empty() {
            self.0.hub.apply(changes);
        }
        drop(state);
        self.0.changed.notify_waiters();
        out
    }

    pub(super) fn lock(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// This device's test host settings, as you last set them.
    pub(super) fn settings(&self) -> TestHostSettings {
        match self.0.hub.get(&EntityKey::DeviceSettings(self.0.device)) {
            Some(Entity::DeviceSettings(DeviceSettings { test_hosts, .. })) => test_hosts,
            _ => TestHostSettings::default(),
        }
    }

    /// The backend that runs `host`.
    pub(super) fn backend(&self, host: TestHostId) -> Result<Arc<dyn TestBackend>, String> {
        self.0
            .backends
            .get(&host)
            .cloned()
            .ok_or_else(|| "That test host isn't on this device.".to_owned())
    }

    /// Looks at what this device lacks for each host, and shows the
    /// answer through [`TestHosts::found`]. The installed detector asks
    /// each shared question once; without one, each backend answers.
    pub async fn check(&self) {
        let found = match detect::installed() {
            Some(detector) => self.detect_with(detector).await,
            None => {
                join_all(self.0.backends.iter().map(|(&host, backend)| async move {
                    Found {
                        host,
                        setup: backend.detect().await,
                    }
                }))
                .await
            }
        };
        self.found(found);
    }

    /// What `detector` finds for the kinds this device hosts.
    pub(super) async fn detect_with<S: System>(&self, detector: &Detector<S>) -> Vec<Found> {
        let findings = detector.check(&self.settings().kinds).await;
        findings
            .iter()
            .map(|finding| Found {
                host: finding.host(self.0.device),
                setup: finding.setup(),
            })
            .collect()
    }

    /// Shows what detection found. A host that has all it needs again
    /// waits to be built; one that lacks something and holds no lease
    /// becomes unavailable. The Mac itself stays unavailable until you
    /// allow leasing it.
    pub fn found(&self, found: Vec<Found>) {
        let mac_host = self.settings().mac_host;
        self.edit(|state, touched| {
            for Found { host: id, setup } in found {
                let busy = state.active_on(id) > 0 || state.cleaning.contains(&id);
                let Some(host) = state.hosts.get_mut(&id) else {
                    continue;
                };
                let setup = match setup {
                    None if host.backend == TestHostBackend::MacHost && !mac_host => {
                        Some(MAC_HOST_OFF.to_owned())
                    }
                    setup => setup,
                };
                match (&setup, host.state) {
                    (None, TestHostState::Unavailable) => host.state = TestHostState::Off,
                    (Some(_), now) if !busy && now != TestHostState::Building => {
                        host.state = TestHostState::Unavailable;
                    }
                    _ => {}
                }
                host.setup = setup;
                touched.host(id);
            }
        });
    }

    /// Starts building a host that's off or failed. Its state shows how
    /// the build goes.
    pub fn build(&self, id: TestHostId) -> Result<(), String> {
        let backend = self.backend(id)?;
        self.edit(|state, touched| {
            let host = state
                .hosts
                .get_mut(&id)
                .ok_or("That test host isn't on this device.")?;
            match host.state {
                TestHostState::Off | TestHostState::Failed => {}
                TestHostState::Unavailable => {
                    return Err(host
                        .setup
                        .clone()
                        .unwrap_or_else(|| "This device can't run it yet.".into()));
                }
                _ => return Err("It's built already.".into()),
            }
            host.state = TestHostState::Building;
            host.error = None;
            touched.host(id);
            Ok(())
        })?;
        let store = self.clone();
        tokio::spawn(async move {
            let built = backend.build().await;
            let wall = store.0.clock.wall();
            let starts = store.edit(|state, touched| {
                if let Some(host) = state.hosts.get_mut(&id) {
                    match built {
                        Ok(built) => {
                            host.state = TestHostState::Warm;
                            host.agent_device = built.agent_device;
                            host.vm = built.vm;
                            host.last_reset = Some(wall);
                        }
                        Err(error) => {
                            host.state = TestHostState::Failed;
                            host.error = Some(error);
                        }
                    }
                    touched.host(id);
                }
                store.pump(state, touched)
            });
            store.start_all(starts);
        });
        Ok(())
    }

    /// Ends every lease on a host, then takes it back to a clean state.
    pub async fn reset(&self, id: TestHostId) -> Result<(), String> {
        let closing = self.edit(|state, touched| {
            let host = state
                .hosts
                .get(&id)
                .ok_or("That test host isn't on this device.")?;
            if !matches!(
                host.state,
                TestHostState::Warm | TestHostState::Leased | TestHostState::Failed
            ) {
                return Err("It isn't built, so there's nothing to reset.".to_owned());
            }
            state.cleaning.insert(id);
            let leases: Vec<LeaseId> = state
                .leases
                .values()
                .filter(|held| held.active() && held.lease.host == id)
                .map(|held| held.lease.id)
                .collect();
            Ok(leases
                .into_iter()
                .filter_map(|lease| state.begin_end(lease, LeaseEnd::EndedByYou, touched))
                .collect::<Vec<_>>())
        })?;
        join_all(closing.into_iter().map(|on| self.finish(on))).await;
        self.clean(id).await
    }

    /// Takes a host back to a clean state as a whole.
    pub(super) async fn clean(&self, id: TestHostId) -> Result<(), String> {
        let backend = self.backend(id)?;
        self.edit(|state, touched| {
            state.cleaning.insert(id);
            if let Some(host) = state.hosts.get_mut(&id) {
                host.state = TestHostState::Resetting;
                host.error = None;
                touched.host(id);
            }
        });
        let reset = backend.reset().await;
        let wall = self.0.clock.wall();
        let starts = self.edit(|state, touched| {
            state.cleaning.remove(&id);
            if let Some(host) = state.hosts.get_mut(&id) {
                match &reset {
                    Ok(()) => {
                        host.state = TestHostState::Warm;
                        host.last_reset = Some(wall);
                    }
                    Err(error) => {
                        host.state = TestHostState::Failed;
                        host.error = Some(error.clone());
                    }
                }
                touched.host(id);
            }
            state.settle(id, touched);
            self.pump(state, touched)
        });
        self.start_all(starts);
        reset
    }
}
