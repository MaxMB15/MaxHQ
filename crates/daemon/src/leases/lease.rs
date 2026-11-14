// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! One lease from asking to ending: the queue for each kind, granting a
//! free host, the calls a holder makes, and taking the machine back.

use std::pin::pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use maxhq_core::leases::{LeaseCall, MAX_RUN_SECONDS, MAX_SCREEN_EDGE, MAX_WAIT_SECONDS};
use maxhq_protocol::agents::ToolImage;
use maxhq_protocol::ids::LeaseId;
use maxhq_protocol::leases::{
    LeaseCommand, LeaseEnd, LeaseState, TestHostBackend, TestHostState, TestKind, TestLease,
    MAX_VMS, RECENT_COMMANDS,
};
use maxhq_protocol::message::ActionOutcome;

use super::backend::{LeaseTarget, TestBackend};
use super::store::{Held, State, TestHosts, Touched};
use super::text::{host_line, kind_label, not_ready, quick_start, run_output};
use super::{LeaseAnswer, LeaseCaller, Leases};
use crate::drive::ToolResult;
use crate::thumbnail;

/// A lease that starts on a backend once the edit that granted it ends.
pub(super) type Start = (LeaseId, Arc<dyn TestBackend>);

impl State {
    /// Starts ending a lease. A waiting one ends at once, a starting one
    /// ends once it started, and a ready one answers its machine for
    /// [`TestHosts::finish`] to take back.
    pub fn begin_end(
        &mut self,
        id: LeaseId,
        reason: LeaseEnd,
        touched: &mut Touched,
    ) -> Option<LeaseTarget> {
        let held = self.leases.get_mut(&id)?;
        let state = held.lease.state;
        match state {
            LeaseState::Queued => {
                held.lease.state = LeaseState::Ended;
                held.lease.ended = Some(reason);
                if let Some(queue) = self.queues.get_mut(&held.lease.kind) {
                    queue.retain(|queued| *queued != id);
                }
            }
            LeaseState::Starting | LeaseState::Ready => {
                held.lease.state = LeaseState::Ending;
                held.ending = Some(reason);
                held.deadlines = None;
            }
            LeaseState::Ending | LeaseState::Ended => return None,
        }
        touched.lease(id);
        match (state, &held.lease.target) {
            (LeaseState::Ready, Some(target)) => Some(LeaseTarget {
                lease: id,
                target: target.clone(),
            }),
            _ => None,
        }
    }

    /// The holder's lease, if it's theirs.
    fn held_by(&mut self, caller: &LeaseCaller, id: LeaseId) -> Result<&mut Held, String> {
        match self.leases.get_mut(&id) {
            Some(held) if held.lease.holder == caller.actor => Ok(held),
            Some(_) => Err(format!("Lease {id} isn't yours.")),
            None => Err(format!("There's no lease {id}. Call lease_open for one.")),
        }
    }
}

/// How many leases one host takes at once.
fn room(backend: TestHostBackend, max_vms: u8) -> usize {
    match backend {
        TestHostBackend::MacVm => usize::from(max_vms.clamp(1, MAX_VMS)),
        _ => 1,
    }
}

impl TestHosts {
    /// Grants waiting leases the hosts that are free, first come first
    /// served for each kind, and answers what to start.
    pub(super) fn pump(&self, state: &mut State, touched: &mut Touched) -> Vec<Start> {
        let settings = self.settings();
        let mut starts = Vec::new();
        for kind in TestKind::ALL {
            while let Some(&head) = state.queues.get(&kind).and_then(|queue| queue.front()) {
                let vms = state
                    .leases
                    .values()
                    .filter(|held| held.active())
                    .filter(|held| {
                        state.hosts.get(&held.lease.host).map(|host| host.backend)
                            == Some(TestHostBackend::MacVm)
                    })
                    .count();
                let free = kind.backends().iter().find_map(|&backend| {
                    let host = state
                        .hosts
                        .values()
                        .find(|host| host.kind == kind && host.backend == backend)?;
                    let usable = matches!(host.state, TestHostState::Warm | TestHostState::Leased)
                        && !state.cleaning.contains(&host.id)
                        && (backend != TestHostBackend::MacHost || settings.mac_host)
                        && (backend != TestHostBackend::MacVm
                            || vms < room(backend, settings.max_vms))
                        && state.active_on(host.id) < room(backend, settings.max_vms);
                    usable.then_some(host.id)
                });
                let Some(host) = free else {
                    break;
                };
                let Ok(backend) = self.backend(host) else {
                    break;
                };
                if let Some(queue) = state.queues.get_mut(&kind) {
                    queue.pop_front();
                }
                if let Some(held) = state.leases.get_mut(&head) {
                    held.lease.host = host;
                    held.lease.state = LeaseState::Starting;
                    touched.lease(head);
                    starts.push((head, backend));
                }
                if let Some(host) = state.hosts.get_mut(&host) {
                    host.state = TestHostState::Leased;
                    touched.host(host.id);
                }
            }
        }
        starts
    }

    /// Gets the granted leases' machines ready, each on its own.
    pub(super) fn start_all(&self, starts: Vec<Start>) {
        for (id, backend) in starts {
            let store = self.clone();
            tokio::spawn(async move { store.start(id, backend).await });
        }
    }

    async fn start(&self, id: LeaseId, backend: Arc<dyn TestBackend>) {
        let started = backend.start(id).await;
        let idle = Duration::from_secs(u64::from(self.settings().idle_minutes) * 60);
        let now = self.0.clock.now();
        let wall = self.0.clock.wall();
        let ending = self.edit(|state, touched| {
            let held = state.leases.get_mut(&id)?;
            touched.lease(id);
            let host = held.lease.host;
            match started {
                Ok(target) => {
                    held.lease.target = Some(target.clone());
                    if held.lease.state == LeaseState::Ending {
                        return Some(LeaseTarget { lease: id, target });
                    }
                    let length = Duration::from_secs(u64::from(held.minutes) * 60);
                    held.lease.state = LeaseState::Ready;
                    held.lease.ends_at = wall.after(length);
                    held.lease.idle_ends_at = wall.after(idle);
                    held.deadlines = Some((now + length, now + idle));
                }
                Err(error) => {
                    held.lease.state = LeaseState::Ended;
                    held.lease.ended = Some(held.ending.unwrap_or(LeaseEnd::HostFailed));
                    if let Some(host) = state.hosts.get_mut(&host) {
                        host.state = TestHostState::Failed;
                        host.error = Some(error);
                        touched.host(host.id);
                    }
                }
            }
            None
        });
        if let Some(on) = ending {
            self.finish(on).await;
        }
    }

    /// Takes a lease's machine back, then ends the lease and hands its
    /// host to whoever waits.
    pub(super) async fn finish(&self, on: LeaseTarget) {
        let Some(host) = self
            .lock()
            .leases
            .get(&on.lease)
            .map(|held| held.lease.host)
        else {
            return;
        };
        let closed = match self.backend(host) {
            Ok(backend) => {
                self.edit(|state, touched| {
                    let alone = state.active_on(host) <= 1 && !state.cleaning.contains(&host);
                    if let Some(host) = state.hosts.get_mut(&host)
                        && alone
                    {
                        host.state = TestHostState::Resetting;
                        touched.host(host.id);
                    }
                });
                backend.close(&on).await
            }
            Err(error) => Err(error),
        };
        let wall = self.0.clock.wall();
        let starts = self.edit(|state, touched| {
            if let Some(held) = state.leases.get_mut(&on.lease) {
                held.lease.state = LeaseState::Ended;
                held.lease.ended = Some(held.ending.unwrap_or(LeaseEnd::Closed));
                touched.lease(on.lease);
            }
            match closed {
                Err(error) => {
                    if let Some(host) = state.hosts.get_mut(&host) {
                        host.state = TestHostState::Failed;
                        host.error = Some(error);
                        touched.host(host.id);
                    }
                }
                Ok(()) => {
                    state.settle(host, touched);
                    if let Some(host) = state.hosts.get_mut(&host)
                        && host.state == TestHostState::Warm
                    {
                        host.last_reset = Some(wall);
                    }
                }
            }
            self.pump(state, touched)
        });
        self.start_all(starts);
    }

    /// Ends a lease for `reason` and answers once it has ended.
    pub(super) async fn end(&self, id: LeaseId, reason: LeaseEnd) {
        if let Some(on) = self.edit(|state, touched| state.begin_end(id, reason, touched)) {
            self.finish(on).await;
            return;
        }
        self.wait(None, |state| {
            let held = state.leases.get(&id)?;
            (held.lease.state != LeaseState::Ended).then_some(())
        })
        .await;
    }

    /// Waits until `pending` answers none, or until `deadline`. Answers
    /// whether it got there before the deadline.
    async fn wait(
        &self,
        deadline: Option<Instant>,
        pending: impl Fn(&State) -> Option<()>,
    ) -> bool {
        loop {
            let mut changed = pin!(self.0.changed.notified());
            changed.as_mut().enable();
            if pending(&self.lock()).is_none() {
                return true;
            }
            match deadline {
                Some(at) => {
                    if self.0.clock.now() >= at {
                        return false;
                    }
                    tokio::select! {
                        () = changed => {}
                        () = self.0.clock.sleep_until(at) => {}
                    }
                }
                None => changed.await,
            }
        }
    }

    /// `lease_open`: the caller's lease of `kind`, waiting in line for it
    /// at most `wait_seconds`.
    async fn ask(
        &self,
        caller: LeaseCaller,
        kind: TestKind,
        minutes: Option<u16>,
        wait_seconds: u16,
    ) -> Result<ToolResult, String> {
        let settings = self.settings();
        let length = minutes
            .unwrap_or(settings.default_minutes)
            .clamp(1, settings.longest_minutes.max(1));
        let wall = self.0.clock.wall();
        let id = self.edit(|state, touched| {
            let hosts: Vec<_> = state
                .hosts
                .values()
                .filter(|host| host.kind == kind)
                .filter(|host| host.backend != TestHostBackend::MacHost || settings.mac_host)
                .collect();
            if hosts.is_empty() || !settings.kinds.contains(&kind) {
                return Err(None);
            }
            if !hosts.iter().any(|host| {
                matches!(
                    host.state,
                    TestHostState::Building
                        | TestHostState::Warm
                        | TestHostState::Leased
                        | TestHostState::Resetting
                )
            }) {
                let lines: Vec<String> = hosts.iter().map(|host| host_line(host)).collect();
                return Err(Some(format!(
                    "No {} host here is ready to lease:\n{}",
                    kind_label(kind),
                    lines.join("\n")
                )));
            }
            let host = hosts[0].id;
            if let Some(held) = state.leases.values().find(|held| {
                held.lease.holder == caller.actor
                    && held.lease.kind == kind
                    && matches!(
                        held.lease.state,
                        LeaseState::Queued | LeaseState::Starting | LeaseState::Ready
                    )
            }) {
                return Ok(held.lease.id);
            }
            let id = LeaseId::generate();
            let idle = Duration::from_secs(u64::from(settings.idle_minutes) * 60);
            let lease = TestLease {
                id,
                device: self.0.device,
                host,
                kind,
                holder: caller.actor,
                permission: caller.permission,
                started_at: wall,
                ends_at: wall.after(Duration::from_secs(u64::from(length) * 60)),
                idle_ends_at: wall.after(idle),
                state: LeaseState::Queued,
                target: None,
                ended: None,
                recent: Vec::new(),
            };
            state.leases.insert(
                id,
                Held {
                    lease,
                    minutes: length,
                    deadlines: None,
                    ending: None,
                    calls: 0,
                },
            );
            state.queues.entry(kind).or_default().push_back(id);
            touched.lease(id);
            Ok(id)
        });
        let id = match id {
            Ok(id) => id,
            Err(Some(why)) => return Err(why),
            Err(None) => return Err(self.elsewhere(kind)),
        };
        let starts = self.edit(|state, touched| self.pump(state, touched));
        self.start_all(starts);
        let wait = Duration::from_secs(u64::from(wait_seconds.min(MAX_WAIT_SECONDS)));
        let deadline = self.0.clock.now() + wait;
        let granted = self
            .wait(Some(deadline), |state| {
                let held = state.leases.get(&id)?;
                (held.lease.state == LeaseState::Queued).then_some(())
            })
            .await;
        if !granted {
            let place = self.edit(|state, touched| {
                let queue = state.queues.get_mut(&kind)?;
                let place = queue.iter().position(|queued| *queued == id)?;
                queue.remove(place);
                state.leases.remove(&id);
                touched.gone(id);
                Some(place + 1)
            });
            if let Some(place) = place {
                return Err(format!(
                    "Every {} host is busy. You were number {place} in line after {} seconds. \
                     Call lease_open again to keep waiting.",
                    kind_label(kind),
                    wait.as_secs()
                ));
            }
        }
        self.wait(None, |state| {
            let held = state.leases.get(&id)?;
            matches!(
                held.lease.state,
                LeaseState::Queued | LeaseState::Starting | LeaseState::Ending
            )
            .then_some(())
        })
        .await;
        let state = self.lock();
        let held = state
            .leases
            .get(&id)
            .ok_or_else(|| format!("Lease {id} is gone. Call lease_open again."))?;
        match held.lease.state {
            LeaseState::Ready => Ok(ToolResult::Text(quick_start(
                &held.lease,
                held.minutes,
                settings.idle_minutes,
            ))),
            _ => Err(not_ready(&held.lease)),
        }
    }

    /// What a caller reads when this device doesn't offer `kind`.
    fn elsewhere(&self, kind: TestKind) -> String {
        let label = kind_label(kind);
        let peers: Vec<String> = match self.0.db.peer_book() {
            Ok(Some(book)) => book
                .peers
                .into_iter()
                .filter(|peer| peer.test_kinds.contains(&kind))
                .map(|peer| peer.name)
                .collect(),
            _ => Vec::new(),
        };
        if peers.is_empty() {
            format!("This device doesn't offer a {label} host, and no linked device does.")
        } else {
            format!(
                "This device doesn't offer a {label} host. {} does; use start_agent to start an \
                 agent there and ask it to lease one.",
                peers.join(", ")
            )
        }
    }

    /// The holder's ready lease, for one call. It doesn't go idle until
    /// [`TestHosts::done`].
    fn using(
        &self,
        caller: &LeaseCaller,
        id: LeaseId,
    ) -> Result<(LeaseTarget, Arc<dyn TestBackend>), String> {
        let (target, host) = self.edit(|state, touched| {
            let held = state.held_by(caller, id)?;
            match (held.lease.state, &held.lease.target) {
                (LeaseState::Ready, Some(target)) => {
                    held.calls += 1;
                    touched.lease(id);
                    Ok((target.clone(), held.lease.host))
                }
                _ => Err(not_ready(&held.lease)),
            }
        })?;
        Ok((LeaseTarget { lease: id, target }, self.backend(host)?))
    }

    /// Ends one call on a lease, which counts from now for going idle.
    fn done(&self, id: LeaseId, ran: Option<LeaseCommand>) {
        let idle = Duration::from_secs(u64::from(self.settings().idle_minutes) * 60);
        let now = self.0.clock.now();
        let wall = self.0.clock.wall();
        self.edit(|state, touched| {
            let Some(held) = state.leases.get_mut(&id) else {
                return;
            };
            held.calls = held.calls.saturating_sub(1);
            if let Some((_, idle_at)) = &mut held.deadlines {
                *idle_at = now + idle;
                held.lease.idle_ends_at = wall.after(idle);
            }
            if let Some(ran) = ran {
                held.lease.recent.push(ran);
                let over = held.lease.recent.len().saturating_sub(RECENT_COMMANDS);
                held.lease.recent.drain(..over);
            }
            touched.lease(id);
        });
    }

    /// A ready lease's screen, as a PNG no larger than the lease tools
    /// show.
    async fn screen_png(
        &self,
        on: &LeaseTarget,
        backend: &dyn TestBackend,
    ) -> Result<ToolImage, String> {
        let png = backend.screenshot(on).await?;
        let png = thumbnail::png_within(&png, MAX_SCREEN_EDGE)
            .map_err(|err| format!("The screenshot couldn't be read: {err}"))?;
        Ok(ToolImage {
            mime: "image/png".into(),
            data: STANDARD.encode(png),
        })
    }

    async fn call(&self, caller: LeaseCaller, call: LeaseCall) -> Result<ToolResult, String> {
        match call {
            LeaseCall::ListTestHosts => Ok(ToolResult::Text(self.list(&caller))),
            LeaseCall::Open {
                kind,
                minutes,
                wait_seconds,
            } => self.ask(caller, kind, minutes, wait_seconds).await,
            LeaseCall::Run {
                lease,
                command,
                timeout_seconds,
            } => {
                let (on, backend) = self.using(&caller, lease)?;
                let seconds = timeout_seconds.clamp(1, MAX_RUN_SECONDS);
                let at = self.0.clock.wall();
                let output = backend
                    .run(&on, &command, Duration::from_secs(u64::from(seconds)))
                    .await;
                let exit_code = output.as_ref().ok().and_then(|output| output.exit_code);
                self.done(
                    lease,
                    Some(LeaseCommand {
                        at,
                        command,
                        exit_code,
                    }),
                );
                Ok(ToolResult::Text(run_output(&output?, seconds)))
            }
            LeaseCall::Screenshot { lease } => {
                let (on, backend) = self.using(&caller, lease)?;
                let image = self.screen_png(&on, backend.as_ref()).await;
                self.done(lease, None);
                Ok(ToolResult::Mixed {
                    text: format!("The screen of lease {lease}."),
                    images: vec![image?],
                })
            }
            LeaseCall::Install { lease, source } => {
                let (on, backend) = self.using(&caller, lease)?;
                let installed = backend.install(&on, source).await;
                self.done(lease, None);
                installed.map(ToolResult::Text)
            }
            LeaseCall::Close { lease } => {
                let ended = {
                    let mut state = self.lock();
                    let held = state.held_by(&caller, lease)?;
                    held.lease.state == LeaseState::Ended
                };
                if ended {
                    return Err(not_ready(&self.lock().leases[&lease].lease));
                }
                self.end(lease, LeaseEnd::Closed).await;
                Ok(ToolResult::Text(format!("Lease {lease} is closed.")))
            }
        }
    }

    /// `list_test_hosts`: the hosts of the kinds you picked, the caller's
    /// leases, and the kinds linked devices offer.
    fn list(&self, caller: &LeaseCaller) -> String {
        let settings = self.settings();
        let state = self.lock();
        let mut hosts: Vec<_> = state
            .hosts
            .values()
            .filter(|host| settings.kinds.contains(&host.kind))
            .filter(|host| host.backend != TestHostBackend::MacHost || settings.mac_host)
            .collect();
        hosts.sort_by_key(|host| {
            (
                TestKind::ALL.iter().position(|kind| *kind == host.kind),
                host.id,
            )
        });
        let mut text = if hosts.is_empty() {
            "This device offers no test hosts.".to_owned()
        } else {
            let lines: Vec<String> = hosts.iter().map(|host| host_line(host)).collect();
            format!("Test hosts on this device:\n{}", lines.join("\n"))
        };
        let mut mine: Vec<_> = state
            .leases
            .values()
            .filter(|held| {
                held.lease.holder == caller.actor && held.lease.state != LeaseState::Ended
            })
            .collect();
        mine.sort_by_key(|held| held.lease.started_at);
        if !mine.is_empty() {
            text.push_str("\nYour leases:");
            for held in mine {
                text.push_str(&format!("\n- {}", not_ready(&held.lease)));
            }
        }
        drop(state);
        if let Ok(Some(book)) = self.0.db.peer_book() {
            for peer in book.peers.iter().filter(|peer| !peer.test_kinds.is_empty()) {
                let kinds: Vec<&str> = peer
                    .test_kinds
                    .iter()
                    .map(|kind| kind_label(*kind))
                    .collect();
                text.push_str(&format!(
                    "\n{} offers {}; start an agent there with start_agent to lease one.",
                    peer.name,
                    kinds.join(", ")
                ));
            }
        }
        text
    }

    /// Ends a lease from the app.
    pub async fn end_by_you(&self, id: LeaseId) -> ActionOutcome {
        let live = self
            .lock()
            .leases
            .get(&id)
            .map(|held| held.lease.state != LeaseState::Ended);
        match live {
            Some(true) => {
                self.end(id, LeaseEnd::EndedByYou).await;
                ActionOutcome::Done
            }
            Some(false) => refused("That lease has ended already."),
            None => refused("There's no such lease on this device."),
        }
    }

    /// A ready lease's screen, for the app. Looking doesn't count as a call.
    pub async fn screen(&self, id: LeaseId) -> ActionOutcome {
        let ready = {
            let state = self.lock();
            state
                .leases
                .get(&id)
                .and_then(|held| match (&held.lease.state, &held.lease.target) {
                    (LeaseState::Ready, Some(target)) => Some((
                        LeaseTarget {
                            lease: id,
                            target: target.clone(),
                        },
                        held.lease.host,
                    )),
                    _ => None,
                })
        };
        let Some((on, host)) = ready else {
            return refused("That lease isn't ready, so there's no screen to show.");
        };
        let backend = match self.backend(host) {
            Ok(backend) => backend,
            Err(reason) => return refused(&reason),
        };
        match self.screen_png(&on, backend.as_ref()).await {
            Ok(image) => ActionOutcome::LeaseScreen {
                image,
                at: self.0.clock.wall(),
            },
            Err(reason) => refused(&reason),
        }
    }
}

fn refused(reason: &str) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.to_owned(),
    }
}

impl Leases for TestHosts {
    fn run(&self, caller: LeaseCaller, call: LeaseCall) -> LeaseAnswer<'_> {
        Box::pin(self.call(caller, call))
    }
}
