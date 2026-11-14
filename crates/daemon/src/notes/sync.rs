// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Bringing other devices' notes up to date. One task swaps a project's
//! versions with a peer, through `maxhq_core::notes::sync::exchange`:
//! after each save, to every peer it can reach; once at start and every
//! hour for every project with notes; and when a link to a peer comes
//! up. One exchange per peer and project runs at a time, and a change
//! that comes while it runs has it run once more. A failed one tries
//! again after a second, doubling up to ten minutes, unless a newer
//! change comes first. A peer that keeps no notes for the project, or
//! runs a `maxd` too old for notes, isn't asked again until the next
//! change.

use std::collections::HashMap;
use std::time::Duration;

use maxhq_core::notes::sync::{exchange, Answer, Moved, NotesEnd};
use maxhq_protocol::agents::PeerRequest;
use maxhq_protocol::ids::{DeviceId, ProjectId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::notes::{NoteVersion, VersionHash};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tracing::debug;

use super::{Notes, NOT_KEPT};
use crate::peers::Peers;

/// How often every project's notes are swapped with every peer, in case
/// a change was missed.
const EVERY: Duration = Duration::from_secs(60 * 60);

/// The first wait before a failed exchange runs again.
const RETRY_FIRST: Duration = Duration::from_secs(1);

/// The longest wait before a failed exchange runs again.
const RETRY_MOST: Duration = Duration::from_secs(10 * 60);

/// What a `maxd` too old for a note request answers. One from before
/// notes refuses the request as unknown, and one from while they were
/// being built says it doesn't keep them.
const TOO_OLD: [&str; 2] = [
    "This maxd is too old for that; update it.",
    "This maxd doesn't keep notes yet.",
];

/// A project whose notes changed.
#[derive(Clone, Copy, Debug)]
pub struct Changed {
    /// The project.
    pub project: ProjectId,
    /// The device the change came from, which has it already.
    pub from: Option<DeviceId>,
}

/// A peer and a project to swap notes for.
type Key = (DeviceId, ProjectId);

impl Notes {
    /// Starts swapping notes with the devices `peers` links to. Only the
    /// first call counts.
    pub fn sync_with(&self, peers: Peers) {
        let (tx, rx) = mpsc::unbounded_channel();
        if self.0.changed.set(tx).is_ok() {
            tokio::spawn(run(self.clone(), peers, rx));
        }
    }
}

/// Runs exchanges as changes, links and the hour call for them, until
/// `maxd` stops.
async fn run(notes: Notes, peers: Peers, mut changes: mpsc::UnboundedReceiver<Changed>) {
    let mut links = peers.links();
    let mut up = links.borrow_and_update().clone();
    let (done, mut ended) = mpsc::unbounded_channel();
    let mut slots = Slots::default();
    // The first tick is now, which makes the round at start.
    let mut hourly = tokio::time::interval(EVERY);
    hourly.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let due = slots.next_due();
        let mut wanted: Vec<Key> = Vec::new();
        tokio::select! {
            Some(change) = changes.recv() => {
                for device in peers.note_targets() {
                    if Some(device) != change.from {
                        wanted.push((device, change.project));
                    }
                }
            }
            Ok(()) = links.changed() => {
                let now = links.borrow_and_update().clone();
                let new: Vec<DeviceId> = now.difference(&up).copied().collect();
                up = now;
                if !new.is_empty() {
                    for project in notes.projects().await.unwrap_or_default() {
                        wanted.extend(new.iter().map(|device| (*device, project)));
                    }
                }
            }
            _ = hourly.tick() => {
                let targets = peers.note_targets();
                for project in notes.projects().await.unwrap_or_default() {
                    wanted.extend(targets.iter().map(|device| (*device, project)));
                }
            }
            Some((key, result)) = ended.recv() => {
                log(key, &result);
                if slots.finished(key, ended_as(&result), Instant::now()) {
                    start(&notes, &peers, &done, key);
                }
            }
            () = sleep_until(due), if due.is_some() => {
                for key in slots.take_due(Instant::now()) {
                    start(&notes, &peers, &done, key);
                }
            }
        }
        for key in wanted {
            if slots.want(key) {
                start(&notes, &peers, &done, key);
            }
        }
    }
}

/// Sleeps until `due`, or forever with none.
async fn sleep_until(due: Option<Instant>) {
    match due {
        Some(due) => tokio::time::sleep_until(due).await,
        None => std::future::pending().await,
    }
}

/// Runs one exchange in a task of its own, and says when it ends.
fn start(
    notes: &Notes,
    peers: &Peers,
    done: &mpsc::UnboundedSender<(Key, Result<Moved, String>)>,
    key: Key,
) {
    let (notes, peers, done) = (notes.clone(), peers.clone(), done.clone());
    tokio::spawn(async move {
        let (device, project) = key;
        let here = Here {
            notes: &notes,
            peer: device,
        };
        let there = There {
            peers: &peers,
            device,
        };
        let result = exchange(&here, &there, project).await;
        let _ = done.send((key, result));
    });
}

fn log((device, project): Key, result: &Result<Moved, String>) {
    match result {
        Ok(moved) => {
            debug!(%device, %project, to_here = moved.to_a, to_there = moved.to_b, "swapped notes")
        }
        Err(err) => debug!(%device, %project, %err, "couldn't swap notes"),
    }
}

/// How an exchange ended, for what comes next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ended {
    /// Both ends hold the same versions.
    Done,
    /// The peer keeps no notes for the project, or can't.
    Settled,
    /// It failed, and runs again later.
    Failed,
}

fn ended_as(result: &Result<Moved, String>) -> Ended {
    match result {
        Ok(_) => Ended::Done,
        Err(reason) if reason == NOT_KEPT || TOO_OLD.contains(&reason.as_str()) => Ended::Settled,
        Err(_) => Ended::Failed,
    }
}

/// The wait before the `failures`th try again.
fn backoff(failures: u32) -> Duration {
    let doubled = RETRY_FIRST.saturating_mul(1 << failures.saturating_sub(1).min(16));
    doubled.min(RETRY_MOST)
}

/// Where each peer and project's exchange stands.
#[derive(Default)]
struct Slots(HashMap<Key, Slot>);

#[derive(Debug, Default)]
struct Slot {
    /// An exchange is running.
    running: bool,
    /// A change came while it ran, so it runs once more.
    again: bool,
    /// How many times in a row it failed.
    failures: u32,
    /// When it tries again after failing.
    due: Option<Instant>,
}

impl Slots {
    /// Asks for an exchange. Says whether to start one now: not while one
    /// runs, which runs once more after instead. A wait to try again ends.
    fn want(&mut self, key: Key) -> bool {
        let slot = self.0.entry(key).or_default();
        if slot.running {
            slot.again = true;
            return false;
        }
        slot.due = None;
        slot.running = true;
        true
    }

    /// Takes an exchange's end. Says whether to start another now.
    fn finished(&mut self, key: Key, ended: Ended, now: Instant) -> bool {
        let Some(slot) = self.0.get_mut(&key) else {
            return false;
        };
        slot.running = false;
        match ended {
            Ended::Done => {
                slot.failures = 0;
                if std::mem::take(&mut slot.again) {
                    slot.running = true;
                    return true;
                }
                self.0.remove(&key);
            }
            Ended::Settled => {
                self.0.remove(&key);
            }
            Ended::Failed => {
                slot.failures += 1;
                slot.again = false;
                slot.due = Some(now + backoff(slot.failures));
            }
        }
        false
    }

    /// The soonest try again.
    fn next_due(&self) -> Option<Instant> {
        self.0.values().filter_map(|slot| slot.due).min()
    }

    /// The exchanges due to try again by `now`, marked running.
    fn take_due(&mut self, now: Instant) -> Vec<Key> {
        let mut keys = Vec::new();
        for (key, slot) in &mut self.0 {
            if !slot.running && slot.due.is_some_and(|due| due <= now) {
                slot.due = None;
                slot.running = true;
                keys.push(*key);
            }
        }
        keys
    }
}

/// This device's notes as one end of an exchange with `peer`.
struct Here<'a> {
    notes: &'a Notes,
    peer: DeviceId,
}

impl NotesEnd for Here<'_> {
    fn hashes(&self, project: ProjectId) -> Answer<'_, Vec<VersionHash>> {
        Box::pin(self.notes.hashes(project))
    }

    fn versions(
        &self,
        project: ProjectId,
        hashes: Vec<VersionHash>,
    ) -> Answer<'_, Vec<NoteVersion>> {
        Box::pin(self.notes.versions(project, hashes))
    }

    fn put(&self, project: ProjectId, versions: Vec<NoteVersion>) -> Answer<'_, u32> {
        Box::pin(self.notes.put(project, versions, Some(self.peer)))
    }
}

/// Another device's notes, over the link to it.
struct There<'a> {
    peers: &'a Peers,
    device: DeviceId,
}

impl There<'_> {
    /// Sends a request and gives its outcome, or why it failed.
    async fn ask(&self, request: PeerRequest) -> Result<ActionOutcome, String> {
        match self.peers.request(self.device, request).await {
            Ok(ActionOutcome::Refused { reason }) => Err(reason),
            Ok(ActionOutcome::Unknown) => Err(TOO_OLD[0].to_owned()),
            Ok(outcome) => Ok(outcome),
            Err(unreachable) => Err(unreachable.to_string()),
        }
    }
}

/// The error for an answer that isn't the one asked for.
fn odd(outcome: &ActionOutcome) -> String {
    format!("A device answered a note request oddly: {outcome:?}")
}

impl NotesEnd for There<'_> {
    fn hashes(&self, project: ProjectId) -> Answer<'_, Vec<VersionHash>> {
        Box::pin(async move {
            match self.ask(PeerRequest::NoteHashes { project }).await? {
                ActionOutcome::NoteHashes { hashes } => Ok(hashes),
                other => Err(odd(&other)),
            }
        })
    }

    fn versions(
        &self,
        project: ProjectId,
        hashes: Vec<VersionHash>,
    ) -> Answer<'_, Vec<NoteVersion>> {
        Box::pin(async move {
            match self
                .ask(PeerRequest::NoteVersions { project, hashes })
                .await?
            {
                ActionOutcome::NoteVersions { versions } => Ok(versions),
                other => Err(odd(&other)),
            }
        })
    }

    fn put(&self, project: ProjectId, versions: Vec<NoteVersion>) -> Answer<'_, u32> {
        Box::pin(async move {
            match self
                .ask(PeerRequest::PutNoteVersions { project, versions })
                .await?
            {
                ActionOutcome::NotesPut { stored } => Ok(stored),
                other => Err(odd(&other)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Key {
        (DeviceId::derive("devbox"), ProjectId::derive("project"))
    }

    #[test]
    fn a_change_while_one_runs_runs_it_once_more() {
        let mut slots = Slots::default();
        let now = Instant::now();
        assert!(slots.want(key()));
        assert!(!slots.want(key()));
        assert!(!slots.want(key()));
        assert!(slots.finished(key(), Ended::Done, now));
        assert!(!slots.finished(key(), Ended::Done, now));
        assert!(slots.0.is_empty());
    }

    #[test]
    fn a_failure_waits_a_second_doubling_to_ten_minutes() {
        assert_eq!(backoff(1), Duration::from_secs(1));
        assert_eq!(backoff(2), Duration::from_secs(2));
        assert_eq!(backoff(5), Duration::from_secs(16));
        assert_eq!(backoff(10), Duration::from_secs(512));
        assert_eq!(backoff(11), RETRY_MOST);
        assert_eq!(backoff(1000), RETRY_MOST);

        let mut slots = Slots::default();
        let now = Instant::now();
        assert!(slots.want(key()));
        assert!(!slots.finished(key(), Ended::Failed, now));
        assert_eq!(slots.next_due(), Some(now + Duration::from_secs(1)));
        assert!(slots.take_due(now).is_empty());
        assert_eq!(slots.take_due(now + Duration::from_secs(1)), vec![key()]);
        assert!(!slots.finished(key(), Ended::Failed, now));
        assert_eq!(slots.next_due(), Some(now + Duration::from_secs(2)));
    }

    #[test]
    fn a_newer_change_replaces_a_wait_to_try_again() {
        let mut slots = Slots::default();
        let now = Instant::now();
        assert!(slots.want(key()));
        slots.finished(key(), Ended::Failed, now);
        assert!(slots.want(key()));
        assert_eq!(slots.next_due(), None);
        slots.finished(key(), Ended::Done, now);
        assert!(slots.0.is_empty());
    }

    #[test]
    fn a_peer_that_keeps_no_notes_isnt_asked_again() {
        let mut slots = Slots::default();
        assert!(slots.want(key()));
        assert!(!slots.want(key()));
        assert_eq!(ended_as(&Err(NOT_KEPT.into())), Ended::Settled);
        assert_eq!(ended_as(&Err(TOO_OLD[1].into())), Ended::Settled);
        assert_eq!(
            ended_as(&Err("Couldn't reach devbox.".into())),
            Ended::Failed
        );
        assert!(!slots.finished(key(), Ended::Settled, Instant::now()));
        assert_eq!(slots.next_due(), None);
        assert!(slots.0.is_empty());
    }
}
