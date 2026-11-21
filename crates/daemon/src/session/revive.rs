// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Keeping holders connected and bringing agents back. A dropped
//! connection reconnects from the last line read. A CLI that dies cuts its
//! turn off: the cut-off items end as interrupted, and the agent comes
//! back with a message that names them instead of running them again.
//! After three resumes in a row with no progress the agent waits for you.
//! Only a resume after a crash cut something off counts. A Claude CLI that
//! ends with nothing cut off comes back without a continue message, unless
//! it ends five times in five minutes.
//! A Codex app-server restarts after a short backoff, and stops
//! restarting once it crashes five times in five minutes. A holder that
//! can't be reached is ended before its agents come back, and when it
//! can't be confirmed as ended they wait for you instead.
//!
//! Starting a holder and connecting to it run in a task that posts how it
//! went. Until then, actions for that holder wait in order.

use std::collections::HashSet;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use futures::stream::{BoxStream, StreamExt};
use maxhq_protocol::attention::{AttentionEvent, AttentionKind};
use maxhq_protocol::ids::{AgentId, AttentionId, SessionId, WorkspaceId};
use maxhq_protocol::message::{ActionOutcome, SendMode};
use maxhq_protocol::model::{Activity, DeviceSettings, PauseReason, PermissionMode, Vendor};
use maxhq_protocol::state::{Change, Entity};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{Delivery, ItemBody, ItemStatus, TimelineEvent, TimelineItem};
use tokio::sync::mpsc;
use tracing::warn;
use uuid::Uuid;

use super::drivers::STOP_GRACE;
use super::folders;
use super::held::{
    internal_key, is_busy, Held, Hold, HoldRecord, Internal, Open, Reaching, Server, ServerKey,
    Target, Waiting, Written,
};
use super::holders::{self, ChildState, Connection, Ended, Event, Holders, Line};
use super::idem::Outcome;
use super::loading::Later;
use super::places::{fit_launch, kind_of, source};
use super::reads::{read_history, ReadKind, Reader, Source};
use super::recovery::{continue_message, cut_off, Next, HANDSHAKE_LIMIT, MAX_CRASHES};
use super::vendors::{one_process_per_agent, speaks_acp};
use super::{refused, Input, Manager};
use crate::db::HolderRow;
use crate::drive::claude::own_mcp;
use crate::drive::{
    acp, Action, ActionKey, ChildExit, Effects, HeldLine, Login, LoginAuth, Place, PlaceHost,
    Staged,
};
use crate::transfer::in_guest_attachments;

/// Connections in a row that may end before a line arrives. After that the
/// holder counts as gone.
pub(super) const MAX_RETRIES: u32 = 3;

/// How a holder's failure ends when the program it was to run isn't
/// there.
const NOT_FOUND: &str = "(os error 2)";

/// What MaxHQ tells you when it stops bringing an agent back.
const NO_PROGRESS: &str = "MaxHQ restarted this agent 3 times in a row and it made no progress, \
                           so it's paused. Send a message to carry on.";

/// What MaxHQ tells you when it couldn't make sure a holder it lost has
/// ended. `what` names the process.
fn stranded(what: &str, reason: &str) -> String {
    format!(
        "MaxHQ lost touch with {what} and couldn't make sure it stopped ({reason}), so it \
         didn't start another. Check that it has ended, then send a message to carry on."
    )
}

/// Why a CLI that keeps ending with nothing cut off stopped coming back.
fn idle_ends(vendor: Vendor) -> String {
    format!(
        "{} ended on its own 5 times in 5 minutes, so MaxHQ stopped restarting it. Send a \
         message to try again.",
        vendor.name()
    )
}

/// What starting a Claude or Cursor CLI needs.
pub(super) struct AgentSpawn {
    /// The agent it runs.
    pub lead: AgentId,
    /// The account it runs under.
    pub login: Login,
    /// Where it runs.
    pub place: Place,
    /// Which CLI, and the session it runs.
    pub cli: Cli,
    /// The model to run.
    pub model: Option<String>,
    /// The effort level to run at, for a Claude CLI.
    pub effort: Option<String>,
    /// The permission mode to run in.
    pub mode: PermissionMode,
}

/// The CLI an agent's holder runs, with its session.
pub(super) enum Cli {
    /// A Claude CLI.
    Claude {
        /// Its session UUID.
        uuid: Uuid,
        /// Whether it resumes that session rather than starting it.
        resume: bool,
        /// Whether to read the session's history while the holder starts.
        seed: bool,
    },
    /// An ACP CLI, such as Cursor's `agent acp`, with what it loads.
    Acp {
        /// Its vendor, which picks its flavor.
        vendor: Vendor,
        /// What it loads.
        kept: acp::Kept,
    },
}

impl Cli {
    fn vendor(&self) -> Vendor {
        match self {
            Cli::Claude { .. } => Vendor::Claude,
            Cli::Acp { vendor, .. } => *vendor,
        }
    }
}

/// What a dead CLI's lead agent was doing.
pub(super) struct Cut {
    /// The lead.
    pub lead: AgentId,
    /// Whether it or a sub-agent was mid-turn.
    pub busy: bool,
    /// The tool items they had running.
    pub items: Vec<TimelineItem>,
}

/// How an app-server's end goes on for its agents.
struct CodexEnd {
    /// You stopped it.
    stopping: bool,
    /// It was lost and may still run, for this reason.
    stranded: Option<String>,
    /// It died with its machine.
    restarted: bool,
}

/// How a task reaching a holder did.
pub(super) enum Reach {
    /// The holder couldn't be started.
    Failed(String),
    /// The holder is connected.
    Connected(Box<Connection>),
    /// The holder started, or already ran, but couldn't be connected to.
    Unreachable(String),
}

impl Manager {
    /// Hands a holder's new driver each of its lead agents as stored, so
    /// reattaching isn't activity: a restarted `maxd`, a resumed CLI and a
    /// new app-server leave each agent's times and read state as they
    /// were. Only what the CLI does from here moves them, and a turn that
    /// ended while nothing watched still shows unread. Call it once the
    /// holder is in `holds`, before anything reaches it.
    pub(super) fn met_again(&mut self, session: SessionId) {
        let leads = self.leads_of(session);
        let Some(hold) = self.holds.get_mut(&session) else {
            return;
        };
        for lead in leads {
            if let Some(held) = self.held.get(&lead) {
                hold.driver.met_again(&held.agent);
            }
        }
    }

    /// Starts a holder from `launch`, if there is one, then connects to it
    /// in a task that posts how that went. Lines after `after` replay
    /// first, so 0 replays every line the holder kept. `seed` reads an
    /// agent's history on the way. Until the task posts, actions for the
    /// holder wait.
    pub(super) fn reach(
        &mut self,
        session: SessionId,
        reaching: Reaching,
        launch: Option<holders::Launch>,
        after: u64,
        seed: Option<(AgentId, Source)>,
    ) {
        let Some(workspace) = self.holds.get(&session).map(|hold| hold.record.workspace) else {
            return;
        };
        let holders = self.holders_at(workspace);
        let Some(hold) = self.holds.get_mut(&session) else {
            return;
        };
        hold.reaching = Some(reaching);
        hold.retry_at = None;
        let Some(holders) = holders else {
            // The guest's link is down, so this waits for it to come up.
            hold.waiting = Some(Waiting {
                launch,
                after,
                seed,
                end: None,
            });
            return;
        };
        hold.waiting = None;
        let check = launch
            .is_some()
            .then(|| hold.folder_check.clone())
            .flatten();
        let mut launch = launch;
        let vendors = self.vendors.clone();
        let places = self.places.clone();
        let scanning = self.places.clone();
        let tx = self.tx.clone();
        let db = self.db.clone();
        let now = self.clock.wall();
        let bytes = self.limits.disk;
        tokio::spawn(async move {
            let seed = seed.map(|(agent, source)| {
                tokio::task::spawn_blocking(move || {
                    read_history(&*vendors, &*places, &source, agent, bytes)
                })
            });
            let verdict = match (check, launch.as_mut()) {
                (Some((agent, check)), Some(launch)) => Some(Box::new(
                    folders::gate(agent, check, db, scanning, now, launch).await,
                )),
                _ => None,
            };
            let reach = reach_holder(holders, session, launch, after).await;
            let seed = match seed {
                Some(read) => read.await.ok().flatten(),
                None => None,
            };
            let _ = tx.send(Input::Reached {
                session,
                reach,
                seed,
                verdict,
            });
        });
    }

    /// A task finished reaching a holder. Once it's connected, the actions
    /// that waited for it run.
    pub(super) async fn reached(
        &mut self,
        session: SessionId,
        reach: Reach,
        seed: Option<io::Result<Vec<TimelineItem>>>,
        verdict: Option<Box<folders::Verdict>>,
    ) {
        let Some(hold) = self.holds.get_mut(&session) else {
            return;
        };
        let Some(reaching) = hold.reaching.take() else {
            return;
        };
        let parked = std::mem::take(&mut hold.parked);
        let asked = hold.env_ask.take();
        let record = hold.record.clone();
        let restarted = hold.restarted;
        if let Some(lead) = record.agent
            && let Some(seed) = seed
        {
            match seed {
                Ok(items) => {
                    self.timeline_mut(lead).replace_history(items);
                    self.reset_viewers(lead);
                }
                Err(err) => warn!(%err, agent = %lead, "couldn't read the agent's history"),
            }
        }
        let fresh = !matches!(reaching, Reaching::Again | Reaching::Restored);
        match reach {
            Reach::Connected(connection) => {
                if fresh {
                    self.replace_stranded(|old| replaces(&record, old));
                }
                self.wire(session, *connection, fresh).await;
                if let Some(verdict) = verdict {
                    self.folder_checked(*verdict);
                }
                if fresh && let Some(lead) = record.agent {
                    self.workflow_env_reached(lead, asked);
                }
                self.came_up(session, reaching);
                self.work.extend(parked);
            }
            Reach::Unreachable(err) if record.workspace.is_some() && !fresh && !restarted => {
                // A guest's holder runs on while its link comes and goes,
                // so it's tried again rather than counted as gone, unless
                // its container restarted under it.
                warn!(%err, %session, "couldn't reach the holder in its guest");
                self.retry_later(session, reaching, parked);
            }
            Reach::Unreachable(err) => {
                warn!(%err, %session, "couldn't reach the holder");
                if fresh {
                    self.replace_stranded(|old| replaces(&record, old));
                }
                self.refuse_parked(parked, "couldn't reach the agent's process");
                if let Reaching::Start(key) | Reaching::Continue(key) = reaching
                    && let Some(lead) = record.agent
                {
                    // The agent is held, and comes back like after a crash.
                    self.publish(lead);
                    let outcome = match reaching {
                        Reaching::Start(_) => ActionOutcome::Started { agent: lead },
                        _ => ActionOutcome::Done,
                    };
                    self.finish(key, Outcome::Action(outcome));
                }
                self.lost(session);
            }
            Reach::Failed(err) => self.not_started(session, reaching, parked, &err),
        }
    }

    /// Starts reading a holder connection. `fresh` says the CLI just
    /// started.
    async fn wire(&mut self, session: SessionId, connection: Connection, fresh: bool) {
        let now = self.clock.now();
        let Some(hold) = self.holds.get_mut(&session) else {
            return;
        };
        hold.generation += 1;
        hold.writer = Some(connection.writer.clone());
        hold.holder_pid = Some(connection.hello.holder_pid);
        hold.pid = match connection.hello.child {
            ChildState::Running { pid } => Some(pid),
            ChildState::Exited(_) => None,
        };
        if fresh && hold.record.vendor == Vendor::Codex {
            hold.handshake_by = Some(now + HANDSHAKE_LIMIT);
        }
        if let Some(read) = hold.read_before.take()
            && connection.hello.first_seq > hold.last_seq + 1
        {
            // The holder trimmed the open turn's first line away, so
            // reading goes on from where the previous run stopped.
            tracing::debug!(
                %session,
                checkpoint = hold.last_seq + 1,
                first_seq = connection.hello.first_seq,
                "the holder no longer keeps the open turn's first line",
            );
            hold.last_seq = read;
        }
        let stopping = hold.stopping;
        let pending: Vec<HeldLine> = connection
            .hello
            .pending
            .into_iter()
            .map(holders::held_line)
            .collect();
        let generation = hold.generation;
        // The rules come first, so prompts the holder still has are
        // answered by them too.
        self.give_command_rules(session);
        let Some(hold) = self.holds.get_mut(&session) else {
            return;
        };
        hold.driver.keep_items(self.limits.items);
        let mut fx = Effects::default();
        hold.driver.connected(fresh, &pending, &mut fx);
        pump(self.tx.clone(), session, generation, connection.events);
        self.apply_fx(session, fx).await;
        if stopping {
            // It was asked to stop while it was being reached.
            if let Some(hold) = self.holds.get_mut(&session) {
                hold.handshake_by = None;
            }
            let writer = connection.writer;
            tokio::spawn(async move {
                if let Err(err) = writer.stop(STOP_GRACE).await {
                    warn!(%err, %session, "couldn't stop the agent's process");
                }
            });
        }
        for lead in self.leads_of(session) {
            self.publish(lead);
        }
    }

    /// Finishes what a holder was reached for, now that it's connected.
    fn came_up(&mut self, session: SessionId, reaching: Reaching) {
        let Some((record, restarted)) = self
            .holds
            .get(&session)
            .map(|hold| (hold.record.clone(), hold.restarted))
        else {
            return;
        };
        match reaching {
            Reaching::Again => {}
            Reaching::Restored => {
                // One whose container restarted only replays what it kept
                // before it ends.
                if restarted {
                    return;
                }
                // The holder outlived whatever wind-down was planned, so
                // its agents' flags no longer apply.
                for lead in self.leads_of(session) {
                    self.drop_flag(lead);
                }
                // A new driver knows none of the threads its app-server
                // runs, so each one is adopted again.
                if record.vendor == Vendor::Codex {
                    for lead in self.codex_leads(record.server_key()) {
                        self.follow_up(lead);
                    }
                }
            }
            Reaching::Start(key) | Reaching::Continue(key) => {
                let Some(lead) = record.agent else {
                    return;
                };
                self.settle_resume(lead);
                self.publish(lead);
                let outcome = match reaching {
                    Reaching::Start(_) => ActionOutcome::Started { agent: lead },
                    _ => ActionOutcome::Done,
                };
                self.finish(key, Outcome::Action(outcome));
            }
            Reaching::Resume => {
                let Some(lead) = record.agent else {
                    return;
                };
                if let Some(held) = self.held.get_mut(&lead) {
                    held.down_since = None;
                }
                self.save(lead);
                self.publish(lead);
                self.follow_up(lead);
                self.settle_resume(lead);
            }
            Reaching::Codex { restart } => {
                for lead in self.codex_leads(record.server_key()) {
                    if let Some(held) = self.held.get_mut(&lead) {
                        held.down_since = None;
                        held.fault = None;
                        self.mark_dirty(lead);
                        self.publish(lead);
                    }
                }
                if restart {
                    self.follow_up_owed(record.server_key());
                }
            }
        }
    }

    /// Claude says nothing until it gets a message, so a resume with
    /// nothing to send is done once its holder is connected.
    fn settle_resume(&mut self, lead: AgentId) {
        if let Some(held) = self.held.get_mut(&lead)
            && held.owed.is_none()
            && held.resuming
        {
            held.resuming = false;
            self.mark_dirty(lead);
            self.publish(lead);
        }
    }

    /// Why the holder for `record` couldn't `verb` its CLI, from what it
    /// said, naming the agent's vendor. MaxHQ installs nothing for a local
    /// ACP agent, so one whose program a container lacks says so.
    fn start_failure(&self, record: &HoldRecord, verb: &str, err: &str) -> String {
        let agent = match record.vendor {
            Vendor::Acp => self.acp_agent(record.account),
            _ => None,
        };
        match agent {
            Some(agent) if record.workspace.is_some() && err.ends_with(NOT_FOUND) => {
                format!("{} isn't installed in this container", agent.program)
            }
            Some(agent) => format!("couldn't {verb} {}: {err}", agent.name),
            None => format!("couldn't {verb} {}: {err}", record.vendor.name()),
        }
    }

    /// A holder that couldn't be started. What waited for it is refused,
    /// and an agent that was only going to exist with it is forgotten.
    fn not_started(
        &mut self,
        session: SessionId,
        reaching: Reaching,
        parked: Vec<(Target, Action)>,
        err: &str,
    ) {
        if matches!(reaching, Reaching::Again | Reaching::Restored) {
            // Nothing was started for these, so only a connect could fail.
            self.refuse_parked(parked, "couldn't reach the agent's process");
            self.lost(session);
            return;
        }
        let Some(record) = self.drop_hold(session) else {
            return;
        };
        match reaching {
            Reaching::Start(key) | Reaching::Continue(key) => {
                let reason = self.start_failure(&record, "start", err);
                warn!(%session, "{reason}");
                self.refuse_parked(parked, &reason);
                if let Some(lead) = record.agent {
                    self.unhold(lead);
                    if let Reaching::Start(_) = reaching {
                        if let Some(timeline) = self.timelines.remove(&lead) {
                            self.retired.insert(lead, timeline.seq().0);
                        }
                    } else {
                        // It's an outside session again.
                        self.reads.remove(&lead);
                        self.drop_unwatched([lead]);
                        if self.timelines.contains_key(&lead) && !self.outside.contains_key(&lead) {
                            self.open_outside(lead);
                        }
                        if !self.filling(lead) {
                            self.reset_viewers(lead);
                        }
                    }
                }
                self.finish(key, Outcome::Action(refused(reason)));
            }
            Reaching::Resume => {
                let reason = self.start_failure(&record, "restart", err);
                if let Some(lead) = record.agent {
                    if let Some(held) = self.held.get_mut(&lead) {
                        held.session = None;
                    }
                    self.fail_resume(lead, reason.clone());
                }
                self.refuse_parked(parked, &reason);
            }
            Reaching::Codex { restart } => {
                let server_key = record.server_key();
                if let Some(server) = self.servers.get_mut(&server_key) {
                    server.session = None;
                }
                let reason = self.start_failure(&record, "start", err);
                warn!(account = %server_key.account, "{reason}");
                if restart {
                    if let Some(server) = self.servers.get_mut(&server_key) {
                        server.waiting = parked;
                    }
                    self.codex_crashed(server_key);
                } else {
                    self.refuse_parked(parked, &reason);
                }
            }
            Reaching::Again | Reaching::Restored => {}
        }
    }

    /// Forgets a holder that never started.
    fn drop_hold(&mut self, session: SessionId) -> Option<HoldRecord> {
        let hold = self.holds.remove(&session)?;
        self.dirty_seq.remove(&session);
        if let Err(err) = self.db.remove_holder(session) {
            warn!(%err, %session, "couldn't forget the holder");
        }
        Some(hold.record)
    }

    /// Refuses the actions that waited for a holder that didn't come up.
    pub(super) fn refuse_parked(&mut self, parked: Vec<(Target, Action)>, reason: &str) {
        let failed = Written {
            already: false,
            failed: true,
        };
        for (_, action) in parked {
            self.result(action.key(), refused(reason), failed);
        }
    }

    /// Handles what a holder connection read.
    pub(super) async fn held_event(
        &mut self,
        session: SessionId,
        generation: u64,
        event: Option<Event>,
    ) {
        if self
            .holds
            .get(&session)
            .is_none_or(|hold| hold.generation != generation)
        {
            return;
        }
        match event {
            Some(Event::Line(line)) => self.line(session, line).await,
            Some(Event::Exited(exit)) => {
                let exit = holders::child_exit(exit);
                if exit.code.is_none() && exit.signal.is_none() {
                    // The holder died before its CLI, which may still run.
                    self.end_lost(session, exit);
                } else {
                    self.exited(session, exit).await;
                }
            }
            None => self.stream_ended(session),
        }
    }

    /// Hands a line to the driver. A jump in seq means lines rotated away
    /// before they were read, so the timeline resets from the CLI's own
    /// history.
    async fn line(&mut self, session: SessionId, line: Line) {
        let Some(hold) = self.holds.get_mut(&session) else {
            return;
        };
        if line.seq <= hold.last_seq {
            return;
        }
        let gap = line.seq > hold.last_seq + 1;
        let live = !line.replayed;
        hold.last_seq = line.seq;
        hold.retries = 0;
        if live {
            hold.handshake_by = None;
        }
        let own = match hold.record.vendor {
            vendor if one_process_per_agent(vendor) => hold.record.agent,
            _ => None,
        };
        let mut fx = Effects::default();
        hold.driver.line(&holders::held_line(line), &mut fx);
        self.dirty_seq.insert(session);
        self.flush_soon();
        if live
            && let Some(lead) = own
            && let Some(held) = self.held.get_mut(&lead)
            && held.resuming
        {
            held.resuming = false;
            self.mark_dirty(lead);
            self.publish(lead);
        }
        self.apply_fx(session, fx).await;
        if gap {
            for lead in self.leads_of(session) {
                self.reseed(lead);
            }
        }
    }

    /// Reads an agent's history again and resets its timeline from it,
    /// once the read is done.
    pub(super) fn reseed(&mut self, lead: AgentId) {
        if self.loading.waits(lead) {
            self.wait_for(lead, Later::Reseed);
            return;
        }
        let Some(held) = self.held.get(&lead) else {
            return;
        };
        let source = source(held);
        self.read(lead, ReadKind::Seed, Reader::Open(source));
    }

    /// The connection dropped while the holder may still run, so connect
    /// again from the last line read.
    fn stream_ended(&mut self, session: SessionId) {
        let Some(hold) = self.holds.get_mut(&session) else {
            return;
        };
        hold.writer = None;
        let workspace = hold.record.workspace;
        let after = hold.last_seq;
        if hold.restarted {
            // Its machine restarted, so nothing runs it any more.
            self.post(Input::Lost { session });
            return;
        }
        if let Some(workspace) = workspace
            && !self.link_up(workspace)
        {
            // The guest's link went down, which drops every connection to
            // its holders. They reconnect once it's back.
            self.reach(session, Reaching::Again, None, after, None);
            for lead in self.leads_of(session) {
                self.publish(lead);
            }
            return;
        }
        let Some(hold) = self.holds.get_mut(&session) else {
            return;
        };
        hold.retries += 1;
        let retries = hold.retries;
        for lead in self.leads_of(session) {
            self.publish(lead);
        }
        if retries > MAX_RETRIES {
            warn!(%session, "the holder keeps dropping its connection, so maxd counts it as gone");
            self.post(Input::Lost { session });
            return;
        }
        self.reach(session, Reaching::Again, None, after, None);
    }

    /// A holder can't be reached. Its CLI counts as ended with no code,
    /// once nothing still runs for it.
    pub(super) fn lost(&mut self, session: SessionId) {
        let exit = ChildExit {
            code: None,
            signal: None,
            at: self.clock.wall(),
            stderr_tail: String::new(),
            with_machine: false,
        };
        self.end_lost(session, exit);
    }

    /// Ends whatever may still run for a holder the manager lost, in a task
    /// of its own, then handles `exit`. Until then its agents stay down, so
    /// a new holder never runs beside the old one.
    pub(super) fn end_lost(&mut self, session: SessionId, exit: ChildExit) {
        let Some(hold) = self.holds.get_mut(&session) else {
            return;
        };
        if hold.ending {
            return;
        }
        let workspace = hold.record.workspace;
        let Some(holders) = self.holders_at(workspace) else {
            // Only the guest can end its holder, so this waits for its link.
            if let Some(hold) = self.holds.get_mut(&session) {
                hold.writer = None;
                hold.waiting = Some(Waiting {
                    launch: None,
                    after: hold.last_seq,
                    seed: None,
                    end: Some(exit),
                });
            }
            return;
        };
        let Some(hold) = self.holds.get_mut(&session) else {
            return;
        };
        hold.ending = true;
        hold.waiting = None;
        let holder_pid = hold.holder_pid;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let ended = holders.end(session, holder_pid).await;
            let _ = tx.send(Input::Ended {
                session,
                exit,
                ended,
            });
        });
    }

    /// A holder the manager lost has ended, or couldn't be confirmed as
    /// ended. Then its agents don't come back on their own.
    pub(super) async fn ended(&mut self, session: SessionId, exit: ChildExit, ended: Ended) {
        let Some(hold) = self.holds.get_mut(&session) else {
            return;
        };
        hold.ending = false;
        if let Ended::Unconfirmed { pid, reason } = ended {
            warn!(%session, ?pid, "{reason}, so maxd won't replace the holder on its own");
            let what = match hold.record.vendor {
                Vendor::Claude => "this agent's Claude process",
                Vendor::Codex => "this account's Codex process",
                Vendor::Cursor => "this agent's Cursor process",
                Vendor::Acp => "this agent's ACP process",
            };
            hold.stranded = Some(stranded(what, &reason));
        }
        self.exited(session, exit).await;
    }

    /// A holder's CLI ended. Its turns are cut off, and its agents come
    /// back unless you stopped or paused them.
    pub(super) async fn exited(&mut self, session: SessionId, mut exit: ChildExit) {
        if let Some(hold) = self.holds.get_mut(&session)
            && hold.jobs > 0
        {
            // The driver's jobs may hold lines the CLI printed before it
            // ended, so the end waits for them.
            hold.exit = Some(exit);
            return;
        }
        let Some(hold) = self.holds.get(&session) else {
            return;
        };
        let stopping = hold.stopping;
        let relaunch = hold.relaunch;
        let vendor = hold.record.vendor;
        let server_key = hold.record.server_key();
        let stranded = hold.stranded.clone();
        // A holder that died with its device or container comes back as
        // after a restart, unless it may still run.
        let restarted = hold.restarted && stranded.is_none();
        exit.with_machine = restarted;
        let mut released = self.take_release(session, stranded.is_some());
        // What was running has to be read before the driver closes it.
        let cuts: Vec<Cut> = self
            .leads_of(session)
            .into_iter()
            .map(|lead| self.cut(lead))
            .collect();

        let mut fx = Effects::default();
        if let Some(hold) = self.holds.get_mut(&session) {
            hold.driver.exited(&exit, &mut fx);
        }
        if released.is_some() {
            super::idle::quiet_exit(&mut fx);
        }
        self.apply_fx(session, fx).await;

        if let Some(hold) = self.holds.remove(&session) {
            if stranded.is_some() {
                // Its row and files stay, so a restarted `maxd` tries to end
                // it again before anything replaces it.
                self.stranded.insert(session, hold.record);
            } else {
                if let Some(writer) = hold.writer {
                    // Asked for now, so a spawn that follows comes after it.
                    let forgetting = writer.forget();
                    tokio::spawn(async move {
                        let _writer = writer;
                        if let Err(err) = forgetting.await {
                            warn!(%err, %session, "couldn't remove the holder's files");
                        }
                    });
                }
                if let Err(err) = self.db.remove_holder(session) {
                    warn!(%err, %session, "couldn't forget the holder");
                }
                own_mcp::forget(session);
            }
        }
        self.dirty_seq.remove(&session);
        let routed: Vec<_> = self
            .routed
            .iter()
            .filter(|(_, routed)| **routed == session)
            .map(|(key, _)| *key)
            .collect();
        let failed = Written {
            already: false,
            failed: true,
        };
        for key in routed {
            self.result(
                key,
                refused("the agent's process ended before it answered"),
                failed,
            );
        }
        for cut in &cuts {
            self.interrupt(cut.lead);
        }
        match vendor {
            _ if one_process_per_agent(vendor) => {
                for cut in cuts {
                    if let Some(parked) = released.take() {
                        self.released(cut, parked);
                    } else if relaunch && stranded.is_none() {
                        self.relaunched(vendor, cut);
                    } else if restarted {
                        self.agent_restarted(cut, stopping);
                    } else {
                        self.agent_exited(vendor, cut, stopping, stranded.as_deref());
                    }
                }
            }
            _ => {
                let end = CodexEnd {
                    stopping,
                    stranded,
                    restarted,
                };
                self.codex_exited(server_key, cuts, end);
            }
        }
    }

    /// Whether a lead or its sub-agents are mid-turn.
    pub(super) fn mid_turn(&self, lead: AgentId) -> bool {
        self.held
            .get(&lead)
            .is_some_and(|held| is_busy(&held.agent.activity))
            || self
                .subs_of(lead)
                .iter()
                .filter_map(|sub| self.subs.get(sub))
                .any(|sub| is_busy(&sub.activity))
    }

    /// Whether a lead or its sub-agents were mid-turn, and the tool items
    /// they had running.
    pub(super) fn cut(&self, lead: AgentId) -> Cut {
        let busy = self.mid_turn(lead);
        let mut agents = self.subs_of(lead);
        agents.insert(0, lead);
        let items = cut_off(
            agents
                .iter()
                .filter_map(|agent| self.timelines.get(agent))
                .flat_map(|timeline| timeline.kept()),
        );
        Cut { lead, busy, items }
    }

    /// Ends what a dead CLI left running: items stop as interrupted,
    /// prompts close and the agents go idle.
    fn interrupt(&mut self, lead: AgentId) {
        let now = self.clock.wall();
        let mut agents = self.subs_of(lead);
        agents.push(lead);
        for agent in &agents {
            let Some(timeline) = self.timelines.get(agent) else {
                continue;
            };
            let events: Vec<(AgentId, TimelineEvent)> = timeline
                .kept()
                .filter(|item| matches!(item.status, ItemStatus::Running | ItemStatus::Waiting))
                .cloned()
                .map(|mut item| {
                    item.status = ItemStatus::Interrupted;
                    item.completed_at = item.completed_at.or(Some(now));
                    match &mut item.body {
                        ItemBody::AssistantMessage { streaming, .. }
                        | ItemBody::Reasoning { streaming, .. }
                        | ItemBody::Plan { streaming, .. } => *streaming = false,
                        _ => {}
                    }
                    (*agent, TimelineEvent::Upsert { item })
                })
                .collect();
            self.timeline_events(events);
        }
        let events: Vec<AttentionId> = self
            .attention
            .iter()
            .filter(|(_, open)| !open.ours && agents.contains(&open.agent))
            .map(|(id, _)| *id)
            .collect();
        for event in events {
            self.close_attention(event);
        }
        for agent in &agents {
            if let Some(sub) = self.subs.get_mut(agent)
                && is_busy(&sub.activity)
            {
                sub.activity = Activity::Idle;
            }
        }
        if let Some(held) = self.held.get_mut(&lead)
            && is_busy(&held.agent.activity)
        {
            held.agent.activity = Activity::Idle;
        }
    }

    /// A CLI the manager stopped to start again with other flags. Its
    /// agent comes back on the same session, unless you paused it since.
    fn relaunched(&mut self, vendor: Vendor, cut: Cut) {
        let lead = cut.lead;
        let Some(held) = self.held.get_mut(&lead) else {
            return;
        };
        if held.paused.is_some() {
            self.agent_exited(vendor, cut, true, None);
            return;
        }
        held.session = None;
        self.resume_agent(lead);
    }

    /// A Claude or Cursor CLI ended. It comes back on its own unless you
    /// stopped it, it keeps making no progress, or its holder is
    /// `stranded`: lost and maybe still running.
    fn agent_exited(&mut self, vendor: Vendor, cut: Cut, stopping: bool, stranded: Option<&str>) {
        let now = self.clock.wall();
        let lead = cut.lead;
        let Some(held) = self.held.get_mut(&lead) else {
            return;
        };
        held.session = None;
        held.down_since = Some(now);
        let parked = stopping || held.paused.is_some();
        if parked {
            held.paused.get_or_insert(PauseReason::ByYou);
        } else if cut.busy {
            held.owed = Some(continue_message(&cut.items));
        }
        if let Some(reason) = stranded {
            held.fault = Some(reason.to_owned());
            held.resuming = false;
            self.notice(lead, reason);
            self.save(lead);
            self.publish(lead);
            return;
        }
        if parked {
            held.resuming = false;
            self.save(lead);
            self.publish(lead);
            return;
        }
        // Only a resume with something cut off counts toward pausing. One
        // that cut nothing off, such as a device restart while the agent
        // was idle, comes back as it was.
        let next = if held.owed.is_some() {
            held.resumes.after_crash()
        } else if self
            .idle_ends
            .entry(lead)
            .or_default()
            .crashed(self.clock.now())
            .is_some()
        {
            Next::Resume
        } else {
            warn!(%lead, "{} ended on its own {MAX_CRASHES} times in 5 minutes", vendor.name());
            let reason = idle_ends(vendor);
            held.fault = Some(reason.clone());
            held.resuming = false;
            self.idle_ends.remove(&lead);
            self.notice(lead, &reason);
            self.save(lead);
            self.publish(lead);
            return;
        };
        match next {
            Next::Pause => self.pause_no_progress(lead),
            Next::Resume => {
                let name = vendor.name();
                self.notice(
                    lead,
                    &format!("{name} stopped unexpectedly, so MaxHQ restarted it."),
                );
                self.resume_agent(lead);
            }
        }
    }

    /// A Codex app-server ended. Its threads need adopting again, and the
    /// ones it cut off mid-turn come back once it restarts. One that's
    /// `stranded`, lost and maybe still running, isn't restarted until you
    /// act.
    fn codex_exited(&mut self, server_key: ServerKey, cuts: Vec<Cut>, end: CodexEnd) {
        let CodexEnd {
            stopping,
            stranded,
            restarted,
        } = end;
        let now = self.clock.wall();
        if let Some(server) = self.servers.get_mut(&server_key) {
            server.session = None;
        }
        for cut in cuts {
            let lead = cut.lead;
            self.adopting.remove(&lead);
            if restarted {
                if let Some(held) = self.held.get_mut(&lead) {
                    held.adopt = true;
                }
                let resume = self.after_restart(&cut, stopping);
                if let Some(held) = self.held.get_mut(&lead) {
                    // Only a thread with something cut off comes back on its
                    // own. The rest are adopted when you next send.
                    held.resuming = resume && held.owed.is_some();
                }
                self.save(lead);
                self.publish(lead);
                continue;
            }
            let Some(held) = self.held.get_mut(&lead) else {
                continue;
            };
            held.adopt = true;
            held.down_since = Some(now);
            // Only a thread with something cut off comes back on its own,
            // and only its resume counts toward pausing.
            let parked = stopping || held.paused.is_some();
            if !parked && cut.busy {
                held.owed = Some(continue_message(&cut.items));
            }
            if parked || held.owed.is_none() || stranded.is_some() {
                held.resuming = false;
            } else {
                match held.resumes.after_crash() {
                    Next::Pause => {
                        self.pause_no_progress(lead);
                        continue;
                    }
                    Next::Resume => {
                        held.resuming = true;
                        self.notice(lead, "Codex stopped unexpectedly, so MaxHQ restarted it.");
                    }
                }
            }
            if let Some(reason) = &stranded {
                self.notice(lead, reason);
            }
            self.save(lead);
            self.publish(lead);
        }
        if let Some(reason) = stranded {
            self.codex_failed(server_key, reason);
        } else if restarted {
            // A restart isn't a crash, so it restarts at once and doesn't
            // count toward giving up.
            self.restart_soon(server_key);
        } else if !stopping {
            self.codex_crashed(server_key);
        }
    }

    /// Restarts an app-server on the next timer if anything needs it.
    fn restart_soon(&mut self, server_key: ServerKey) {
        let now = self.clock.now();
        let resuming = self.codex_leads(server_key).iter().any(|lead| {
            self.held
                .get(lead)
                .is_some_and(|held| held.paused.is_none() && held.resuming)
        });
        if let Some(server) = self.servers.get_mut(&server_key)
            && (resuming || !server.waiting.is_empty())
        {
            server.restart_at = Some(now);
        }
    }

    /// Counts an app-server crash, and plans a restart if anything needs
    /// one.
    fn codex_crashed(&mut self, server_key: ServerKey) {
        let now = self.clock.now();
        let leads = self.codex_leads(server_key);
        let resuming = leads.iter().any(|lead| {
            self.held
                .get(lead)
                .is_some_and(|held| held.paused.is_none() && held.resuming)
        });
        let Some(server) = self.servers.get_mut(&server_key) else {
            return;
        };
        if let Some(at) = server.restarts.crashed(now) {
            if resuming || !server.waiting.is_empty() {
                server.restart_at = Some(at);
            }
            return;
        }
        let reason =
            "Codex crashed 5 times in 5 minutes, so MaxHQ stopped restarting it".to_owned();
        warn!(account = %server_key.account, "{reason}");
        self.codex_failed(server_key, reason);
    }

    /// Stops restarting an account's app-server until you act, and shows
    /// its agents why.
    fn codex_failed(&mut self, server_key: ServerKey, reason: String) {
        let leads = self.codex_leads(server_key);
        let Some(server) = self.servers.get_mut(&server_key) else {
            return;
        };
        server.failed = Some(reason.clone());
        server.restart_at = None;
        let waiting = std::mem::take(&mut server.waiting);
        for lead in leads {
            if let Some(held) = self.held.get_mut(&lead) {
                held.fault = Some(reason.clone());
                held.resuming = false;
                self.save(lead);
                self.publish(lead);
            }
        }
        for (_, action) in waiting {
            self.result(action.key(), refused(reason.clone()), Written::default());
        }
    }

    /// Restarts an account's app-server once its backoff is over.
    pub(super) fn restart_codex(&mut self, server_key: ServerKey) {
        let Some(server) = self.servers.get_mut(&server_key) else {
            return;
        };
        server.restart_at = None;
        let waiting = std::mem::take(&mut server.waiting);
        match self.ensure_server(server_key, true) {
            Ok(session) => match self.holds.get_mut(&session) {
                // They run once the new app-server is up.
                Some(hold) if hold.reaching.is_some() => hold.parked.extend(waiting),
                _ => {
                    self.follow_up_owed(server_key);
                    self.work.extend(waiting);
                }
            },
            Err(reason) => {
                warn!(account = %server_key.account, "{reason}");
                if let Some(server) = self.servers.get_mut(&server_key) {
                    server.waiting = waiting;
                }
                self.codex_crashed(server_key);
            }
        }
    }

    /// Sends each of an account's Codex agents what a crash left it owing.
    fn follow_up_owed(&mut self, server_key: ServerKey) {
        for lead in self.codex_leads(server_key) {
            let owes = self.held.get(&lead).is_some_and(|held| {
                held.paused.is_none() && (held.resuming || held.owed.is_some())
            }) && !self.limit_waits.contains_key(&lead);
            if owes {
                self.follow_up(lead);
            }
        }
    }

    /// The account's app-server, started if it isn't running. `restart`
    /// says it replaces one that crashed.
    pub(super) fn ensure_server(
        &mut self,
        server_key: ServerKey,
        restart: bool,
    ) -> Result<SessionId, String> {
        let server = self
            .servers
            .get(&server_key)
            .ok_or_else(|| "Codex isn't set up for this account".to_owned())?;
        if let Some(session) = server.session
            && self.holds.contains_key(&session)
        {
            return Ok(session);
        }
        let login = server.login.clone();
        let guest = server_key.workspace;
        self.check_sign_in(&login, guest.is_some())?;
        let launch = self.vendors.codex_launch(&login);
        let launch = fit_launch(launch, Vendor::Codex, &login, guest.is_some());
        let session = SessionId::generate();
        let driver = self.vendors.codex_server(&login, self.device);
        let mut record = HoldRecord::new(Vendor::Codex, login.account, login.config_dir.clone());
        record.workspace = guest;
        record.boot = self.boot_at(guest);
        record.login = kind_of(&login.auth);
        record.note_launch(&launch);
        // Stored first, so a restarted `maxd` knows a holder it may have
        // started.
        self.store_holder(session, &record);
        self.holds.insert(session, Hold::new(record, driver));
        if let Some(server) = self.servers.get_mut(&server_key) {
            server.session = Some(session);
            server.failed = None;
            server.restart_at = None;
        }
        for lead in self.codex_leads(server_key) {
            if let Some(held) = self.held.get_mut(&lead) {
                // A new app-server has no threads loaded.
                held.adopt = true;
            }
        }
        self.met_again(session);
        let launch = holders::launch(session, launch);
        self.reach(session, Reaching::Codex { restart }, Some(launch), 0, None);
        Ok(session)
    }

    /// Starts a held Claude or ACP agent's CLI again, resuming its
    /// session.
    pub(super) fn resume_agent(&mut self, lead: AgentId) {
        if self.loading.waits(lead) {
            self.wait_for(lead, Later::Resume);
            return;
        }
        self.give_agent_key(lead);
        let Some(held) = self.held.get(&lead) else {
            return;
        };
        let cli = match held.vendor() {
            Vendor::Claude => {
                let Ok(uuid) = Uuid::parse_str(&held.vendor_session) else {
                    self.fail_resume(lead, "the agent has no Claude session to resume".to_owned());
                    return;
                };
                // A session the agent just moved to has no transcript to
                // resume until its handoff goes.
                let fresh = held
                    .handoff
                    .as_ref()
                    .is_some_and(|handoff| handoff.since.is_none())
                    || self.trust_fresh.contains(&lead);
                Cli::Claude {
                    uuid,
                    resume: !fresh,
                    seed: false,
                }
            }
            Vendor::Codex => return,
            vendor => Cli::Acp {
                vendor,
                kept: self.acp_kept(held),
            },
        };
        let login = match self.login_of(held) {
            Ok(login) => login,
            Err(reason) => {
                self.fail_resume(lead, reason);
                return;
            }
        };
        // An agent from before `maxd` named every level comes back at its
        // model's default, named, so the level shown is the one it runs at.
        let effort = held
            .effort
            .clone()
            .or_else(|| self.model_effort(held.vendor(), held.account, held.model.as_deref()));
        let spawn = AgentSpawn {
            lead,
            login,
            place: self.place(held),
            cli,
            model: held.model.clone(),
            effort: effort.clone(),
            mode: held.mode,
        };
        self.trust_fresh.remove(&lead);
        let session = self.spawn_agent(spawn, Reaching::Resume);
        if let Some(held) = self.held.get_mut(&lead) {
            held.session = Some(session);
            held.effort = effort;
            held.resuming = true;
            held.fault = None;
            held.released = false;
        }
        self.save(lead);
        self.publish(lead);
    }

    /// Starts a Claude or Cursor CLI in a new holder, for `reaching`, and
    /// returns the holder. The caller links the agent to it.
    pub(super) fn spawn_agent(&mut self, spawn: AgentSpawn, reaching: Reaching) -> SessionId {
        let AgentSpawn {
            lead,
            login,
            place,
            cli,
            model,
            effort,
            mode,
        } = spawn;
        let vendor = cli.vendor();
        let guest = match place.host {
            PlaceHost::Guest { workspace } => Some(workspace),
            PlaceHost::Local => None,
        };
        // Codex checks its folder when it loads a thread, not here.
        let check = matches!(vendor, Vendor::Claude | Vendor::Cursor)
            .then(|| (lead, self.folder_check(vendor, &place)));
        let claude = matches!(cli, Cli::Claude { .. });
        let (launch, driver, vendor_session, seed) = match cli {
            Cli::Claude { uuid, resume, seed } => {
                let mut launch = self.vendors.claude_launch(
                    &login,
                    &place,
                    uuid,
                    resume,
                    model.as_deref(),
                    mode,
                );
                crate::drive::claude::add_effort(&mut launch, effort.as_deref());
                let driver = self.vendors.claude_session(&login, &place, lead, mode);
                let seed = seed.then(|| {
                    let source = Source {
                        vendor,
                        config_dir: login.config_dir.clone(),
                        session: uuid.to_string(),
                        subagent: None,
                        workspace: guest,
                        account: Some(login.account),
                    };
                    (lead, source)
                });
                (launch, driver, Some(uuid.to_string()), seed)
            }
            Cli::Acp { vendor, kept } => {
                let launch = self.vendors.acp_launch(vendor, &login, &place, mode);
                let session = kept.session.clone();
                let driver = self
                    .vendors
                    .acp_session(vendor, &login, &place, lead, kept, mode);
                (launch, driver, session, None)
            }
        };
        let mut launch = fit_launch(launch, vendor, &login, guest.is_some());
        // After fitting, so the card warns of each variable that replaces
        // what the launch ends up with.
        let ask = claude
            .then(|| self.give_workflow_env(lead, &place, &mut launch))
            .flatten();
        // A holder is single use, so each start gets a new one.
        let session = SessionId::generate();
        let mut record = HoldRecord::new(vendor, login.account, login.config_dir.clone());
        record.agent = Some(lead);
        record.workspace = guest;
        // A guest whose link is down has no boot to note yet. The reach
        // waits for the link, which notes it then.
        record.boot = self.boot_at(guest);
        record.login = kind_of(&login.auth);
        record.vendor_session = vendor_session;
        record.note_launch(&launch);
        // Stored first, so a restarted `maxd` knows a holder it may have
        // started.
        self.store_holder(session, &record);
        let mut hold = Hold::new(record, driver);
        hold.folder_check = check;
        hold.env_ask = ask;
        self.holds.insert(session, hold);
        self.met_again(session);
        let launch = holders::launch(session, launch);
        self.reach(session, reaching, Some(launch), 0, seed);
        session
    }

    /// Forgets the stranded holders a new one replaces. You chose to carry
    /// on, so a restarted `maxd` no longer tries to end them.
    fn replace_stranded(&mut self, replaced: impl Fn(&HoldRecord) -> bool) {
        let sessions: Vec<SessionId> = self
            .stranded
            .iter()
            .filter(|(_, record)| replaced(record))
            .map(|(session, _)| *session)
            .collect();
        for session in sessions {
            self.stranded.remove(&session);
            if let Err(err) = self.db.remove_holder(session) {
                warn!(%err, %session, "couldn't forget the holder");
            }
        }
    }

    /// A resume that couldn't start.
    fn fail_resume(&mut self, lead: AgentId, reason: String) {
        warn!(agent = %lead, "{reason}");
        if let Some(held) = self.held.get_mut(&lead) {
            held.fault = Some(reason);
            held.resuming = false;
        }
        self.save(lead);
        self.publish(lead);
    }

    /// Too many resumes made no progress, so the agent waits for you.
    pub(super) fn pause_no_progress(&mut self, lead: AgentId) {
        let id = AttentionId::generate();
        let Some(held) = self.held.get_mut(&lead) else {
            return;
        };
        held.paused = Some(PauseReason::ResumesMadeNoProgress);
        held.resuming = false;
        let old = held.notice.replace(id);
        if let Some(old) = old {
            self.close_attention(old);
        }
        self.open_notice(lead, id);
        self.notice(lead, NO_PROGRESS);
        self.save(lead);
        self.publish(lead);
    }

    /// Shows MaxHQ's own attention event saying the agent paused.
    fn open_notice(&mut self, lead: AgentId, id: AttentionId) {
        let event = AttentionEvent {
            id,
            agent: lead,
            kind: AttentionKind::Error,
            summary: NO_PROGRESS.to_owned(),
            options: Vec::new(),
            accepts_text: false,
            created_at: self.clock.wall(),
            expires_at: None,
        };
        self.attention.insert(
            id,
            Open {
                agent: lead,
                ours: true,
                expires_at: None,
            },
        );
        self.hub.apply_held([Change::Upsert {
            value: Entity::Attention(event),
        }]);
    }

    /// Sends a revived agent what it still needs: the continue message a
    /// crash left it owing, then the messages that were queued. A Codex
    /// thread with nothing to send is adopted, so its timeline follows the
    /// app-server again.
    pub(super) fn follow_up(&mut self, lead: AgentId) {
        if self.loading.waits(lead) {
            self.wait_for(lead, Later::FollowUp);
            return;
        }
        let Some(held) = self.held.get(&lead) else {
            return;
        };
        if held.paused.is_some() {
            return;
        }
        let adopt = held.vendor() == Vendor::Codex && held.adopt;
        let mut sent = false;
        if let Some(text) = held.owed.clone() {
            let key = internal_key();
            self.internal.insert(key, Internal::Owed(lead));
            self.work.push_back((
                Target::Agent(lead),
                Action::Send {
                    key,
                    agent: lead,
                    text,
                    refs: Vec::new(),
                    attachments: Vec::new(),
                    mode: SendMode::Queue,
                },
            ));
            sent = true;
        }
        sent |= self.requeue(lead);
        if adopt && !sent {
            self.adopt_now(lead);
        }
    }

    /// Sends the messages a dead CLI had queued again, since they died
    /// with it. Returns whether it sent any. A container's agent gets only
    /// the ones whose files could be in its container.
    fn requeue(&mut self, lead: AgentId) -> bool {
        let guest = self
            .held
            .get(&lead)
            .is_some_and(|held| held.guest.is_some());
        let Some(timeline) = self.timelines.get(&lead) else {
            return false;
        };
        let queued: Vec<TimelineItem> = timeline
            .kept()
            .filter(|item| {
                item.status == ItemStatus::Pending
                    && matches!(
                        item.body,
                        ItemBody::UserMessage {
                            delivery: Delivery::Queued,
                            ..
                        }
                    )
            })
            .cloned()
            .collect();
        if queued.is_empty() {
            return false;
        }
        self.timeline_events(
            queued
                .iter()
                .map(|item| {
                    (
                        lead,
                        TimelineEvent::Remove {
                            id: item.id.clone(),
                        },
                    )
                })
                .collect(),
        );
        let mut sent = false;
        for item in queued {
            if guest && !names_guest_files(&item) {
                warn!(agent = %lead, "dropped a queued message naming files outside the container");
                continue;
            }
            let ItemBody::UserMessage {
                text,
                refs,
                attachments,
                ..
            } = item.body
            else {
                continue;
            };
            let attachments = attachments
                .into_iter()
                .map(|attachment| {
                    let path = PathBuf::from(&attachment.path);
                    Staged {
                        text: crate::sidecar::text_of(&path),
                        path,
                        attachment,
                    }
                })
                .collect();
            let key = internal_key();
            self.internal.insert(key, Internal::Plain);
            self.work.push_back((
                Target::Agent(lead),
                Action::Send {
                    key,
                    agent: lead,
                    text,
                    refs,
                    attachments,
                    mode: SendMode::Queue,
                },
            ));
            sent = true;
        }
        sent
    }

    /// Turns setup-tokens on for a device that held one before the setting
    /// came, so it carries on as it did. Settings stored since name the
    /// setting, so this happens once and never undoes your choice.
    fn keep_setup_tokens(&mut self) {
        if !self.credentials.holds_claude_token() {
            return;
        }
        match self.db.settings_name(self.device, "claude_setup_token") {
            Ok(false) => {}
            Ok(true) => return,
            Err(err) => {
                warn!(%err, "couldn't read the device settings");
                return;
            }
        }
        let settings = DeviceSettings {
            claude_setup_token: true,
            ..self.device_settings()
        };
        if let Err(err) = self.db.set_device_settings(&settings) {
            warn!(%err, "couldn't store the device settings");
        }
        self.hub.apply([Change::Upsert {
            value: Entity::DeviceSettings(settings),
        }]);
        tracing::info!("turned setup-tokens on, since this device holds one");
    }

    /// Picks up where the previous run of `maxd` stopped: settings,
    /// answers, held agents and their holders. Agents whose CLI died while
    /// `maxd` was down come back as after any crash. Their timelines are
    /// read once the loop runs, so it takes requests meanwhile.
    pub(super) async fn restore(&mut self) {
        let attachments = self.attachments.clone();
        tokio::task::spawn_blocking(move || {
            let removed = attachments.clean(SystemTime::now());
            if removed > 0 {
                tracing::info!(removed, "removed staged attachments nobody sent");
            }
        });

        match self.db.device_settings(self.device) {
            Ok(Some(settings)) => self.hub.apply([Change::Upsert {
                value: Entity::DeviceSettings(settings),
            }]),
            Ok(None) => {}
            Err(err) => warn!(%err, "couldn't read the device settings"),
        }
        self.keep_setup_tokens();
        // A Mac on the charger holds from here, without waiting for an agent.
        let settings = self.device_settings();
        self.power.set_mode(settings.keep_awake);
        self.keep_to(&settings);

        if let Err(err) = self.db.prune_answers(self.answers_since()) {
            warn!(%err, "couldn't prune old answers");
        }
        match self.db.answers() {
            Ok(rows) => self.answers = rows.into_iter().map(|row| (row.event, row)).collect(),
            Err(err) => warn!(%err, "couldn't read the answers given"),
        }

        let rows: Vec<Held> = self.db.held().unwrap_or_else(|err| {
            warn!(%err, "couldn't read the held agents");
            Vec::new()
        });
        // An agent in a container workspace that's gone went with it, as
        // when the workspace was deleted by a `maxd` that kept its agents.
        let saved = self.saved_workspaces();
        let (rows, gone): (Vec<Held>, Vec<Held>) = rows.into_iter().partition(|held| {
            !held
                .guest
                .is_some_and(|workspace| self.workspace_gone(saved.as_ref(), workspace))
        });
        for held in &gone {
            let lead = held.agent.id;
            if let Err(err) = self.db.remove_held(lead) {
                warn!(%err, agent = %lead, "couldn't forget the agent");
            }
            self.drop_flag(lead);
            self.forget_inbox(lead);
        }
        if !gone.is_empty() {
            tracing::info!(
                dropped = gone.len(),
                "dropped the agents of workspaces that are gone"
            );
        }
        // Only a start or an observer makes a folder's workspace and project,
        // and no observer reads the config folders MaxHQ's own agents run
        // under. So an agent in a folder here brings its folder back, under
        // the same IDs, before it shows. A container's comes from the
        // workspace manager.
        let folders: Vec<(AgentId, WorkspaceId, PathBuf)> = rows
            .iter()
            .filter(|held| held.guest.is_none())
            .map(|held| (held.agent.id, held.agent.workspace, held.cwd.clone()))
            .collect();
        let hub = self.hub.clone();
        let located = tokio::task::spawn_blocking(move || {
            for (agent, workspace, folder) in folders {
                let found = hub.locate_folder(&folder);
                if found != workspace {
                    warn!(%agent, %workspace, %found, "the agent's folder has another workspace now");
                }
            }
        });
        if let Err(err) = located.await {
            warn!(%err, "couldn't bring back the agents' folders");
        }
        for mut held in rows {
            let id = held.agent.id;
            held.session = None;
            if held.vendor() == Vendor::Codex {
                held.adopt = true;
                let login = self.codex_login(held.account, held.config_dir.clone(), held.login);
                self.servers
                    .entry(held.server_key())
                    .or_insert_with(|| Server::new(login));
            }
            let queued = std::mem::take(&mut held.queued);
            let timeline = self.timeline_mut(id);
            // The messages you queued are only MaxHQ's until they're sent,
            // so they come back from the stored agent, onto the end of its
            // history once that's read.
            for item in queued {
                if held.guest.is_some() && !names_guest_files(&item) {
                    warn!(agent = %id, "dropped a stored message naming files outside the container");
                    continue;
                }
                if !timeline.contains(&item.id) {
                    timeline.apply(TimelineEvent::Upsert { item });
                }
            }
            if held.paused == Some(PauseReason::ResumesMadeNoProgress)
                && let Some(notice) = held.notice
            {
                self.held.insert(id, held);
                self.open_notice(id, notice);
            } else {
                self.held.insert(id, held);
            }
        }

        self.restore_restricted();
        self.restore_env_cards();
        self.restore_task_cards();
        // Each timeline is read off the loop, a few at a time, so a long
        // one doesn't sit in memory beside every other. Holders wait for
        // their agents' timelines, so a CLI that died while `maxd` was down
        // is cut off from its whole timeline.
        self.load_timelines();
        // Before the holders, whose places say which agent is the
        // dispatcher.
        self.restore_chats();

        let rows: Vec<HolderRow<HoldRecord>> = self.db.holders().unwrap_or_else(|err| {
            warn!(%err, "couldn't read the holders");
            Vec::new()
        });
        let alive: Option<HashSet<SessionId>> = match self.holders.list().await {
            Ok(list) => Some(list.into_iter().collect()),
            Err(err) => {
                warn!(%err, "couldn't list the holders, so maxd tries each one");
                None
            }
        };
        let host_boot = self.hub.device().boot_id.clone();
        for row in rows {
            let HolderRow {
                session,
                record,
                last_seq,
                checkpoint,
            } = row;
            // So did a holder there, such as its Codex app-server.
            if record
                .workspace
                .is_some_and(|workspace| self.workspace_gone(saved.as_ref(), workspace))
            {
                let _ = self.db.remove_holder(session);
                continue;
            }
            let driver = match record.vendor {
                vendor if one_process_per_agent(vendor) => {
                    let Some(held) = record.agent.and_then(|agent| self.held.get(&agent)) else {
                        let _ = self.db.remove_holder(session);
                        continue;
                    };
                    let place = self.place(held);
                    // A setup-token that's gone stops the next launch, not
                    // the reading of one that runs.
                    let login = self.login_of(held).unwrap_or_else(|_| Login {
                        account: held.account,
                        config_dir: held.config_dir.clone(),
                        auth: LoginAuth::ConfigDir,
                    });
                    let (mode, agent) = (held.mode, held.agent.id);
                    let driver = match record.vendor {
                        vendor if speaks_acp(vendor) => {
                            let kept = self.acp_kept(held);
                            self.vendors
                                .acp_session(vendor, &login, &place, agent, kept, mode)
                        }
                        _ => self.vendors.claude_session(&login, &place, agent, mode),
                    };
                    if let Some(held) = self.held.get_mut(&agent) {
                        held.session = Some(session);
                    }
                    driver
                }
                _ => {
                    let login =
                        self.codex_login(record.account, record.config_dir.clone(), record.login);
                    let server = self
                        .servers
                        .entry(record.server_key())
                        .or_insert_with(|| Server::new(login));
                    server.session = Some(session);
                    let login = server.login.clone();
                    self.vendors.codex_server(&login, self.device)
                }
            };
            let guest = record.workspace;
            let listed = alive.as_ref().is_none_or(|alive| alive.contains(&session));
            // A holder on this device from an earlier boot died with it. A
            // guest's holders are checked once its link is up.
            let restarted = guest.is_none()
                && !alive.as_ref().is_some_and(|alive| alive.contains(&session))
                && record.boot.as_ref().is_some_and(|boot| *boot != host_boot);
            let mut hold = Hold::new(record, driver);
            hold.restarted = restarted;
            hold.reaching = Some(Reaching::Restored);
            // A turn still open is read again from its first line, so the
            // new driver rebuilds it whole rather than from mid-stream.
            hold.last_seq = match checkpoint {
                Some(first) if (1..=last_seq).contains(&first) => {
                    hold.read_before = Some(last_seq);
                    first - 1
                }
                _ => last_seq,
            };
            let after = hold.last_seq;
            self.holds.insert(session, hold);
            self.met_again(session);
            let alive = guest.is_some() || (listed && !restarted);
            self.loading.wait_holder(session, alive.then_some(after));
        }

        // An agent that waited for its limit to reset when `maxd` stopped
        // goes on waiting.
        let now = self.clock.wall();
        let waits: Vec<(AgentId, Timestamp)> = self
            .held
            .values()
            .filter(|held| held.paused.is_none() && (held.owed.is_some() || held.resuming))
            .filter_map(|held| Some((held.agent.id, held.resume_after.filter(|at| *at > now)?)))
            .collect();
        for (lead, at) in waits {
            let left = Duration::from_millis(at.0.saturating_sub(now.0));
            self.limit_waits.insert(lead, self.clock.now() + left);
            if let Some(held) = self.held.get_mut(&lead) {
                held.resuming = false;
            }
        }

        // A Claude or Cursor agent that should run but has no holder lost
        // it while `maxd` was down.
        let orphans: Vec<AgentId> = self
            .held
            .values()
            .filter(|held| {
                held.own_process()
                    && held.session.is_none()
                    && held.paused.is_none()
                    && held.fault.is_none()
                    && !held.released
                    && !self.limit_waits.contains_key(&held.agent.id)
            })
            .map(|held| held.agent.id)
            .collect();
        for lead in orphans {
            self.resume_agent(lead);
        }

        // An app-server still running has its threads adopted once it's
        // reached. With none running, only the threads a crash cut off
        // start one.
        let keys: Vec<ServerKey> = self.servers.keys().copied().collect();
        for key in keys {
            let held = self
                .servers
                .get(&key)
                .and_then(|server| server.session)
                .is_some_and(|session| self.holds.contains_key(&session));
            if !held {
                self.follow_up_owed(key);
            }
        }
        self.release_holders();

        let leads: Vec<AgentId> = self.held.keys().copied().collect();
        for lead in leads {
            self.publish(lead);
        }
    }

    /// The container workspaces with a row, whether its record parses or
    /// not. None when that isn't known for sure, and then no agent is
    /// dropped as gone.
    fn saved_workspaces(&self) -> Option<HashSet<WorkspaceId>> {
        let ids = match self.db.workspace_ids() {
            Ok(ids) => ids,
            Err(err) => {
                warn!(%err, "couldn't read the saved workspaces, so no agent is dropped");
                return None;
            }
        };
        let mut saved = HashSet::new();
        for id in ids {
            match id.parse() {
                Ok(workspace) => {
                    saved.insert(workspace);
                }
                Err(err) => {
                    warn!(id, %err, "a saved workspace's id doesn't parse, so no agent is dropped");
                    return None;
                }
            }
        }
        Some(saved)
    }

    /// Whether a container workspace is gone: it has no row among `saved`
    /// and the workspace manager has no record of it. Nothing is gone when
    /// `saved` isn't known.
    fn workspace_gone(&self, saved: Option<&HashSet<WorkspaceId>>, workspace: WorkspaceId) -> bool {
        saved.is_some_and(|saved| !saved.contains(&workspace))
            && self.workspaces.container(workspace).is_none()
    }
}

/// Starts a holder if there's a launch, then connects to it.
async fn reach_holder(
    holders: Arc<dyn Holders>,
    session: SessionId,
    launch: Option<holders::Launch>,
    after: u64,
) -> Reach {
    if let Some(launch) = launch
        && let Err(err) = holders.spawn(launch).await
    {
        return Reach::Failed(err.to_string());
    }
    match holders.connect(session, Some(after)).await {
        Ok(connection) => Reach::Connected(Box::new(connection)),
        Err(err) => Reach::Unreachable(err.to_string()),
    }
}

/// Whether every file a message names could be in a container. `maxd`
/// names only the paths uploads land at, so any other is a message it never
/// queued, and a driver must not name it to the container's agent.
fn names_guest_files(item: &TimelineItem) -> bool {
    let ItemBody::UserMessage { attachments, .. } = &item.body else {
        return true;
    };
    attachments
        .iter()
        .all(|attachment| in_guest_attachments(&attachment.path))
}

/// Whether a new holder takes the place of an old one: the same Claude or
/// Cursor agent's, or the same account's app-server.
fn replaces(new: &HoldRecord, old: &HoldRecord) -> bool {
    match new.vendor {
        vendor if one_process_per_agent(vendor) => old.agent.is_some() && old.agent == new.agent,
        _ => old.vendor == Vendor::Codex && old.server_key() == new.server_key(),
    }
}

/// Reads a holder connection in a task of its own, posting each event to
/// the manager.
fn pump(
    tx: mpsc::UnboundedSender<Input>,
    session: SessionId,
    generation: u64,
    mut events: BoxStream<'static, Event>,
) {
    tokio::spawn(async move {
        while let Some(event) = events.next().await {
            let exited = matches!(event, Event::Exited(_));
            let input = Input::Held {
                session,
                generation,
                event: Some(event),
            };
            if tx.send(input).is_err() || exited {
                return;
            }
        }
        let _ = tx.send(Input::Held {
            session,
            generation,
            event: None,
        });
    });
}
