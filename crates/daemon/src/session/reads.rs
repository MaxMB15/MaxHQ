// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Reading the CLIs' own history files off the manager's loop. Each read
//! runs on a blocking thread and posts what it read back to the loop, where
//! only the newest read of each agent counts.

use std::io;
use std::path::PathBuf;

use maxhq_protocol::ids::{AccountId, AgentId, WorkspaceId};
use maxhq_protocol::model::Vendor;
use maxhq_protocol::timeline::{Delivery, ItemBody, TimelineEvent, TimelineItem};
use tracing::{debug, warn};

use super::holders::HolderPlaces;
use super::vendors::{History, Vendors};
use super::{Input, Manager, POLL_EVERY};

/// What a history read is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReadKind {
    /// Fills a held agent's timeline again, after lines rotated away.
    Seed,
    /// Fills an outside session's timeline when a client first opens it.
    Outside,
    /// Reads what an outside session wrote since the last read.
    Poll,
}

/// The session a history belongs to.
pub(super) struct Source {
    /// The session's vendor.
    pub vendor: Vendor,
    /// The account's config folder.
    pub config_dir: PathBuf,
    /// The Claude session UUID or the Codex thread ID.
    pub session: String,
    /// The Claude sub-agent of `session` whose own transcript holds the
    /// history, or none for the session's.
    pub subagent: Option<String>,
    /// The container workspace whose guest keeps the history, or none for
    /// this device.
    pub workspace: Option<WorkspaceId>,
    /// The account the session runs under, which what a login that ran out
    /// shows names, when MaxHQ knows it.
    pub account: Option<AccountId>,
}

/// Where a read gets its items.
pub(super) enum Reader {
    /// A reader to open first.
    Open(Source),
    /// A reader an earlier read opened.
    Kept(Box<dyn History>),
}

/// A finished history read.
pub(super) struct Read {
    agent: AgentId,
    token: u64,
    kind: ReadKind,
    /// The reader, to keep for the next read. It's none when the vendor
    /// keeps no history MaxHQ can read.
    history: Option<Box<dyn History>>,
    /// What it read, if there was a reader.
    items: Option<io::Result<Vec<TimelineItem>>>,
}

/// Reads a session's whole history, or at most `bytes` of its newest.
/// It blocks, so it runs off the loop. None means the vendor keeps no
/// history MaxHQ can read, or the guest that keeps it can't be reached.
pub(super) fn read_history(
    vendors: &dyn Vendors,
    places: &dyn HolderPlaces,
    source: &Source,
    agent: AgentId,
    bytes: Option<u64>,
) -> Option<io::Result<Vec<TimelineItem>>> {
    let mut history = open_history(vendors, places, source, agent, bytes)?;
    history.limit(bytes);
    Some(history.read().map(settled))
}

/// What a history says, with every message as one the agent has. Only
/// `maxd` queues a message, when you send it, and a queued message is sent
/// again after a crash. A container's guest keeps its agent's history and
/// can say anything in it, so nothing read back gets to wait in the queue.
fn settled(mut items: Vec<TimelineItem>) -> Vec<TimelineItem> {
    for item in &mut items {
        if let ItemBody::UserMessage { delivery, .. } = &mut item.body
            && matches!(delivery, Delivery::Queued | Delivery::Steer)
        {
            *delivery = Delivery::Sent;
        }
    }
    items
}

/// A reader for a session's history: this device's files, or the guest's
/// for a session held in a container. The guest's reader takes at most
/// `bytes` of the newest history in its first read, since the guest
/// starts reading as it opens.
fn open_history(
    vendors: &dyn Vendors,
    places: &dyn HolderPlaces,
    source: &Source,
    agent: AgentId,
    bytes: Option<u64>,
) -> Option<Box<dyn History>> {
    match (source.workspace, source.subagent.as_deref()) {
        (Some(workspace), None) => places.history(
            workspace,
            source.vendor,
            source.config_dir.to_string_lossy().into_owned(),
            source.session.clone(),
            agent,
            bytes,
        ),
        // A guest reads only a session's own history.
        (Some(_), Some(_)) => None,
        (None, subagent) => vendors
            .history(
                source.vendor,
                &source.config_dir,
                &source.session,
                subagent,
                agent,
            )
            .map(|mut history| {
                if let Some(account) = source.account {
                    history.account(account);
                }
                history
            }),
    }
}

impl Manager {
    /// Reads an agent's history on a blocking thread. A newer read of the
    /// same agent makes this one stale.
    pub(super) fn read(&mut self, agent: AgentId, kind: ReadKind, reader: Reader) {
        self.next_read += 1;
        let token = self.next_read;
        self.reads.insert(agent, (token, kind));
        let vendors = self.vendors.clone();
        let places = self.places.clone();
        let tx = self.tx.clone();
        let bytes = self.limits.disk;
        tokio::task::spawn_blocking(move || {
            let mut history = match reader {
                Reader::Open(source) => open_history(&*vendors, &*places, &source, agent, bytes)
                    .map(|mut history| {
                        history.limit(bytes);
                        history
                    }),
                Reader::Kept(history) => Some(history),
            };
            let items = history.as_mut().map(|history| history.read().map(settled));
            let read = Read {
                agent,
                token,
                kind,
                history,
                items,
            };
            let _ = tx.send(Input::Read(Box::new(read)));
        });
    }

    /// Whether a read is still filling an outside session's timeline, or a
    /// held agent's after a restart, so a client that opens it waits for
    /// the reset the read sends.
    pub(super) fn filling(&self, agent: AgentId) -> bool {
        self.loading.waits(agent)
            || self
                .reads
                .get(&agent)
                .is_some_and(|(_, kind)| *kind == ReadKind::Outside)
    }

    /// Takes in a finished read, unless a newer one replaced it.
    pub(super) fn landed(&mut self, read: Read) {
        let Read {
            agent,
            token,
            kind,
            history,
            items,
        } = read;
        if self
            .reads
            .get(&agent)
            .is_none_or(|(newest, _)| *newest != token)
        {
            return;
        }
        self.reads.remove(&agent);
        // An outside session still open waits for its reader to come back.
        let open = self.outside.get(&agent).is_some_and(Option::is_none);
        match kind {
            ReadKind::Seed => match items {
                Some(Ok(items)) => {
                    self.timeline_mut(agent).replace_history(items);
                    self.reset_viewers(agent);
                }
                Some(Err(err)) => warn!(%err, %agent, "couldn't read the agent's history"),
                None => {}
            },
            ReadKind::Outside => {
                if let Some(timeline) = self.timelines.get_mut(&agent) {
                    match items {
                        Some(Ok(items)) => timeline.replace_history(items),
                        Some(Err(err)) => {
                            debug!(%err, %agent, "couldn't read the session's history");
                        }
                        None => {}
                    }
                    // Clients that opened the timeline wait for this.
                    self.reset_viewers(agent);
                }
            }
            ReadKind::Poll if open => match items {
                Some(Ok(items)) => self.timeline_events(
                    items
                        .into_iter()
                        .map(|item| (agent, TimelineEvent::Upsert { item }))
                        .collect(),
                ),
                Some(Err(err)) => debug!(%err, %agent, "couldn't read the session's history"),
                None => {}
            },
            ReadKind::Poll => {}
        }
        if open && kind != ReadKind::Seed {
            match history {
                Some(history) => {
                    self.outside.insert(agent, Some(history));
                }
                None => {
                    self.outside.remove(&agent);
                }
            }
        }
        if !self.outside.is_empty() && self.next_poll.is_none() {
            self.next_poll = Some(self.clock.now() + POLL_EVERY);
        }
    }
}
