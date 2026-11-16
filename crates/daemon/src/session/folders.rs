// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Keeping an agent from its folder's files until you trust them. The check
//! runs on the way to every Claude and Cursor start, and before Codex loads
//! a thread. An agent never waits for your answer: it starts restricted, and
//! a card asks. A trusted answer starts it again between turns. See
//! [`crate::folder_trust`].

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use maxhq_protocol::attention::{AnswerOutcome, AttentionAnswer};
use maxhq_protocol::guest::{FolderScan, ScannedFile};
use maxhq_protocol::ids::{AgentId, AttentionId, ClientId, IdempotencyKey, WorkspaceId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::Vendor;
use maxhq_protocol::state::{Change, Entity};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{
    Delivery, ItemBody, ItemStatus, QuestionAnswer, TimelineEvent, TimelineItem,
};
use serde::{Deserialize, Serialize};
use tracing::warn;

use super::held::{internal_key, Internal, Open, Target};
use super::holders::{self, HolderPlaces};
use super::{Input, Manager};
use crate::db::Db;
use crate::drive::{Action, ActionKey, FolderTrust, Place, PlaceHost};
use crate::folder_trust::{
    self, Card, CURSOR_DATA_VAR, NOTED_LABEL, QUESTION, RESTRICTED_ARGS, RESTRICTED_LABEL,
    TRUST_LABEL,
};
use crate::recipe;
use crate::skills::deliver;

/// Where to check a folder, and for which CLI.
#[derive(Clone)]
pub struct FolderCheck {
    vendor: Vendor,
    cwd: PathBuf,
    locked: Vec<String>,
    /// The workspace whose guest reads the folder, when it isn't here.
    guest: Option<WorkspaceId>,
}

/// An agent kept from its folder's files, or told of them. Stored, since
/// holders outlive `maxd` and a card has to too.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Restricted {
    /// The project the folder belongs to, for one CLI.
    key: String,
    /// What's in it.
    fingerprint: String,
    /// The files, for the pair stored if you trust them.
    files: Vec<ScannedFile>,
    /// The card, until it's answered.
    card: Option<Card>,
}

/// What a folder's scan comes to for one CLI, before any agent has it.
pub struct Judged {
    vendor: Vendor,
    cwd: PathBuf,
    /// The part of the scan the CLI reads.
    scan: FolderScan,
    fingerprint: String,
    /// Set when there's nothing to keep from the agent, or you trusted it.
    fine: bool,
    /// What you last trusted, for a card.
    before: Option<Vec<ScannedFile>>,
    /// Codex only: the folders it looks the folder's trust up for.
    paths: Vec<String>,
}

/// How a check came out.
pub struct Verdict {
    agent: AgentId,
    restricted: Option<Restricted>,
}

/// Checks the folder, keeping `launch` from its files unless it's fine. The
/// result is stored first, so a `maxd` that dies now still knows.
pub async fn gate(
    agent: AgentId,
    check: FolderCheck,
    db: Db,
    places: Arc<dyn HolderPlaces>,
    now: Timestamp,
    launch: &mut holders::Launch,
) -> Verdict {
    let (vendor, guest) = (check.vendor, check.guest.is_some());
    let from_guest = read_guest(&check, &*places).await;
    let decided = tokio::task::spawn_blocking(move || {
        let judged = judge(check, from_guest, &db);
        verdict(agent, judged, &db, now)
    })
    .await;
    // A check that couldn't finish isn't an answer, so the flags stay on.
    let verdict = decided.unwrap_or_else(|err| {
        warn!(%err, %agent, "the folder check didn't finish");
        Verdict {
            agent,
            restricted: Some(Restricted {
                key: String::new(),
                fingerprint: String::new(),
                files: Vec::new(),
                card: None,
            }),
        }
    });
    if verdict.restricted.is_some() {
        restrict(vendor, guest, agent, launch);
    }
    verdict
}

/// Starts a CLI without its folder's files: Claude with flags, and Cursor
/// with a data folder for `agent` alone, in the config folder it has where
/// it runs. Codex's trust goes with its thread instead.
fn restrict(vendor: Vendor, guest: bool, agent: AgentId, launch: &mut holders::Launch) {
    match vendor {
        Vendor::Claude => launch.args.extend(RESTRICTED_ARGS.map(OsString::from)),
        Vendor::Cursor => {
            let config = if guest {
                PathBuf::from(recipe::CURSOR_CONFIG)
            } else {
                // With no home, a folder no Cursor has used before.
                std::env::home_dir().map_or_else(
                    || std::env::temp_dir().join("maxhq-cursor"),
                    |home| home.join(".cursor"),
                )
            };
            let dir = folder_trust::cursor_data_dir(&config, agent);
            launch.env.retain(|(name, _)| name != CURSOR_DATA_VAR);
            launch
                .env
                .push((CURSOR_DATA_VAR.into(), dir.into_os_string()));
        }
        Vendor::Codex | Vendor::Acp => {}
    }
}

/// Reads a guest's folder through its guest. A folder here is read later,
/// off the loop.
async fn read_guest(check: &FolderCheck, places: &dyn HolderPlaces) -> Option<FolderScan> {
    let workspace = check.guest?;
    let cwd = check.cwd.display().to_string();
    let read = places.folder_scan(workspace, cwd.clone(), check.locked.clone());
    Some(folder_trust::from_guest(read, workspace, &cwd).await)
}

/// Reads the folder and judges it, off the loop.
async fn check_folder(check: FolderCheck, db: Db, places: Arc<dyn HolderPlaces>) -> Option<Judged> {
    let from_guest = read_guest(&check, &*places).await;
    tokio::task::spawn_blocking(move || judge(check, from_guest, &db))
        .await
        .inspect_err(|err| warn!(%err, "the folder check didn't finish"))
        .ok()
}

/// What a scan comes to for the check's CLI. It blocks, reading the folder
/// when the guest hasn't.
fn judge(check: FolderCheck, from_guest: Option<FolderScan>, db: &Db) -> Judged {
    let here = from_guest.is_none();
    let all = from_guest.unwrap_or_else(|| folder_trust::scan_here(&check.cwd, &check.locked));
    let paths = match check.vendor {
        Vendor::Codex => folder_trust::codex_paths(&check.cwd, &all.key, here),
        _ => Vec::new(),
    };
    let scan = folder_trust::only(&all, check.vendor);
    let fingerprint = folder_trust::fingerprint(&scan.files);
    let fine = !scan.failed
        && (scan.files.is_empty()
            || db
                .folder_trusted(&scan.key, &fingerprint)
                .unwrap_or_else(|err| {
                    warn!(%err, "couldn't read the trusted folders");
                    false
                }));
    let before = if fine {
        None
    } else {
        db.last_trusted_files(&scan.key).unwrap_or_else(|err| {
            warn!(%err, "couldn't read what was last trusted");
            None
        })
    };
    Judged {
        vendor: check.vendor,
        cwd: check.cwd,
        scan,
        fingerprint,
        fine,
        before,
        paths,
    }
}

/// What a judged folder comes to for an agent, stored.
fn verdict(agent: AgentId, judged: Judged, db: &Db, now: Timestamp) -> Verdict {
    let Judged {
        vendor,
        cwd,
        scan,
        fingerprint,
        fine,
        before,
        ..
    } = judged;
    if fine {
        if let Err(err) = db.remove_restricted(agent) {
            warn!(%err, %agent, "couldn't note the agent has its folder's files");
        }
        return Verdict {
            agent,
            restricted: None,
        };
    }
    let card = folder_trust::card(agent, vendor, &cwd, &scan, before.as_deref(), now);
    let restricted = Restricted {
        key: scan.key,
        fingerprint,
        files: scan.files,
        card: Some(card),
    };
    if let Err(err) = db.put_restricted(agent, &restricted) {
        warn!(%err, %agent, "couldn't store the restricted agent");
    }
    Verdict {
        agent,
        restricted: Some(restricted),
    }
}

/// The label picked on a card's `question`. A card has no buttons, so
/// that's the one way to answer it, from its agent's feed.
pub(super) fn picked<'a>(answer: &'a AttentionAnswer, question: &str) -> Option<&'a str> {
    let AttentionAnswer::Questions { answers } = answer else {
        return None;
    };
    let answer = answers.iter().find(|answer| answer.id == question)?;
    match answer.values.as_slice() {
        [only] => Some(only),
        _ => None,
    }
}

/// Whether an answer ticks a card's box, which comes back as an answer of
/// its own under the box's `id`.
pub(super) fn ticked(answer: &AttentionAnswer, id: &str) -> bool {
    let AttentionAnswer::Questions { answers } = answer else {
        return false;
    };
    answers
        .iter()
        .any(|answer| answer.id == id && !answer.values.is_empty())
}

impl Manager {
    /// What a start of `vendor`'s CLI in `place` has to check.
    pub(super) fn folder_check(&self, vendor: Vendor, place: &Place) -> FolderCheck {
        FolderCheck {
            vendor,
            cwd: place.cwd.clone(),
            locked: deliver::locked_names(place.workspace),
            guest: match place.host {
                PlaceHost::Local => None,
                PlaceHost::Guest { workspace } => Some(workspace),
            },
        }
    }

    /// Codex reads a folder's files when it loads a thread, so a start or
    /// an adopt checks the folder first, off the loop, and comes back with
    /// the trust it's to load with. Returns the action when it's checked.
    pub(super) fn check_codex(&mut self, target: Target, action: Action) -> Option<Action> {
        let place = match &action {
            Action::Start { place, .. } | Action::Adopt { place, .. } if place.trust.is_none() => {
                place
            }
            _ => return Some(action),
        };
        let check = self.folder_check(Vendor::Codex, place);
        let db = self.db.clone();
        let places = self.places.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let judged = check_folder(check, db, places).await.map(Box::new);
            let _ = tx.send(Input::FolderChecked {
                target,
                action: Box::new(action),
                judged,
            });
        });
        None
    }

    /// A Codex folder check finished, so the action goes on with its trust.
    /// The card waits for the thread, which names the agent of a new one.
    pub(super) fn codex_checked(
        &mut self,
        target: Target,
        mut action: Action,
        judged: Option<Box<Judged>>,
    ) {
        let key = action.key();
        let (place, agent) = match &mut action {
            Action::Start { place, agent, .. } => (place, *agent),
            Action::Adopt { place, agent, .. } => (place, Some(*agent)),
            _ => return,
        };
        let trust = match &judged {
            Some(judged) => FolderTrust {
                trusted: judged.fine,
                paths: judged.paths.clone(),
            },
            // A check that couldn't finish isn't an answer.
            None => {
                let cwd = place.cwd.display().to_string();
                FolderTrust {
                    trusted: false,
                    paths: folder_trust::codex_paths(&place.cwd, &cwd, false),
                }
            }
        };
        place.trust = Some(trust);
        if let Some(judged) = judged {
            self.codex_checks.insert(key, (agent, judged));
        }
        self.work.push_front((target, action));
    }

    /// A Codex start or adopt ended. Once its thread is loaded, its agent
    /// gets the card its folder's check asked for.
    pub(super) fn codex_loaded(&mut self, key: IdempotencyKey, outcome: &ActionOutcome, ok: bool) {
        let Some((agent, judged)) = self.codex_checks.remove(&key) else {
            return;
        };
        let started = match outcome {
            ActionOutcome::Started { agent } => Some(*agent),
            _ => None,
        };
        if let Some(agent) = agent.or(started).filter(|_| ok) {
            let verdict = verdict(agent, *judged, &self.db, self.clock.wall());
            self.folder_checked(verdict);
        }
    }

    /// A CLI started after its folder's check.
    pub(super) fn folder_checked(&mut self, verdict: Verdict) {
        let Verdict { agent, restricted } = verdict;
        let old = self.restricted.remove(&agent);
        let Some(mut new) = restricted else {
            if let Some(old) = old {
                self.close_folder_card(agent, &old, None);
            }
            return;
        };
        let id = folder_trust::card_id(agent, &new.fingerprint);
        if let Some(old) = old.filter(|old| old.fingerprint != new.fingerprint) {
            self.close_folder_card(agent, &old, None);
        }
        if self.answers.contains_key(&id) {
            new.card = None;
            self.store_restricted(agent, &new);
        } else if !self.attention.contains_key(&id)
            && let Some(card) = new.card.clone()
        {
            self.open_folder_card(card);
        }
        self.restricted.insert(agent, new);
    }

    fn store_restricted(&self, agent: AgentId, restricted: &Restricted) {
        if let Err(err) = self.db.put_restricted(agent, restricted) {
            warn!(%err, %agent, "couldn't store the restricted agent");
        }
    }

    /// Shows a card, with its question in the agent's timeline.
    pub(super) fn open_folder_card(&mut self, card: Card) {
        let Card { event, item } = card;
        let agent = event.agent;
        self.attention.insert(
            event.id,
            Open {
                agent,
                ours: true,
                expires_at: event.expires_at,
            },
        );
        self.hub.apply_held([Change::Upsert {
            value: Entity::Attention(event),
        }]);
        self.timeline_events(vec![(agent, TimelineEvent::Upsert { item })]);
    }

    /// Closes an agent's card, showing what you picked or taking the question
    /// away if the card no longer applies.
    fn close_folder_card(&mut self, agent: AgentId, restricted: &Restricted, picked: Option<&str>) {
        let Some(card) = &restricted.card else {
            return;
        };
        self.close_attention(card.event.id);
        let event = match picked {
            Some(picked) => {
                let mut item = card.item.clone();
                answered(
                    &mut item,
                    QUESTION,
                    vec![picked.to_owned()],
                    self.clock.wall(),
                );
                TimelineEvent::Upsert { item }
            }
            None => TimelineEvent::Remove {
                id: card.item.id.clone(),
            },
        };
        self.timeline_events(vec![(agent, event)]);
    }

    /// Whether an event is a card still waiting.
    pub(super) fn is_folder_card(&self, event: AttentionId) -> bool {
        self.card_owner(event).is_some()
    }

    fn card_owner(&self, event: AttentionId) -> Option<AgentId> {
        self.restricted.iter().find_map(|(agent, restricted)| {
            restricted
                .card
                .as_ref()
                .is_some_and(|card| card.event.id == event)
                .then_some(*agent)
        })
    }

    /// Answers a card. Only a person can, so an agent's tool route hears
    /// there's no such event, and an answer it doesn't offer leaves it open.
    pub(super) fn answer_folder_card(
        &mut self,
        conn: u64,
        key: IdempotencyKey,
        event: AttentionId,
        answer: &AttentionAnswer,
        by: ClientId,
    ) -> AnswerOutcome {
        let person = self.clients.get(&conn).is_some_and(|viewer| viewer.person);
        let Some(agent) = self.card_owner(event).filter(|_| person) else {
            return AnswerOutcome::UnknownEvent;
        };
        let Some(mut restricted) = self.restricted.get(&agent).cloned() else {
            return AnswerOutcome::UnknownEvent;
        };
        let Some(pick) = picked(answer, QUESTION)
            .filter(|pick| restricted.card.as_ref().is_some_and(|c| c.offers(pick)))
        else {
            return AnswerOutcome::UnknownEvent;
        };
        if pick == TRUST_LABEL || pick == NOTED_LABEL {
            let stored = self.db.trust_folder(
                &restricted.key,
                &restricted.fingerprint,
                self.clock.wall(),
                by,
                &restricted.files,
            );
            if let Err(err) = stored {
                warn!(%err, %agent, "couldn't store the trusted folder");
                return AnswerOutcome::UnknownEvent;
            }
            if pick == TRUST_LABEL {
                self.trusted(&restricted, key, by);
            } else {
                self.noted(&restricted, key, by);
            }
        } else {
            self.record_answer(event, key, by);
            self.close_folder_card(agent, &restricted, Some(RESTRICTED_LABEL));
            restricted.card = None;
            self.store_restricted(agent, &restricted);
            self.restricted.insert(agent, restricted);
        }
        AnswerOutcome::Accepted
    }

    /// Answers the card of every agent kept from the same files, as you
    /// answered one of them, and returns those agents.
    fn answer_same(
        &mut self,
        pair: &Restricted,
        key: IdempotencyKey,
        by: ClientId,
        picked: &str,
    ) -> Vec<AgentId> {
        let same: Vec<AgentId> = self
            .restricted
            .iter()
            .filter(|(_, other)| other.key == pair.key && other.fingerprint == pair.fingerprint)
            .map(|(agent, _)| *agent)
            .collect();
        for agent in &same {
            let Some(restricted) = self.restricted.get(agent).cloned() else {
                continue;
            };
            if let Some(card) = &restricted.card {
                self.record_answer(card.event.id, key, by);
            }
            self.close_folder_card(*agent, &restricted, Some(picked));
            if let Some(restricted) = self.restricted.get_mut(agent) {
                restricted.card = None;
            }
        }
        same
    }

    /// You trusted these files, so every agent kept from them gets them, once
    /// it's between turns.
    fn trusted(&mut self, pair: &Restricted, key: IdempotencyKey, by: ClientId) {
        for agent in self.answer_same(pair, key, by, TRUST_LABEL) {
            if let Some(restricted) = self.restricted.get(&agent) {
                self.store_restricted(agent, restricted);
            }
            self.trust_restarts.insert(agent);
        }
    }

    /// You saw what these files run, on a card from before Cursor could
    /// start without them, so it shows no more. The agent ran with them.
    fn noted(&mut self, pair: &Restricted, key: IdempotencyKey, by: ClientId) {
        for agent in self.answer_same(pair, key, by, NOTED_LABEL) {
            self.restricted.remove(&agent);
            if let Err(err) = self.db.remove_restricted(agent) {
                warn!(%err, %agent, "couldn't forget the restricted agent");
            }
        }
    }

    /// Starts each agent you trusted the folder of again once it's idle.
    pub(super) fn restart_for_trust(&mut self) {
        if self.trust_restarts.is_empty() {
            return;
        }
        let marked: Vec<AgentId> = self.trust_restarts.iter().copied().collect();
        for agent in marked {
            // Whether it has a transcript isn't known until its timeline is
            // in.
            if self.loading.waits(agent) {
                continue;
            }
            if self
                .held
                .get(&agent)
                .is_some_and(|held| held.vendor() == Vendor::Codex)
            {
                self.reload_for_trust(agent);
                continue;
            }
            let session = self.held.get(&agent).and_then(|held| held.session);
            if session.is_some() && self.mid_turn(agent) {
                continue;
            }
            self.trust_restarts.remove(&agent);
            // One that stopped on its own gets the files when it starts.
            if let Some(session) = session {
                tracing::info!(%agent, "starting an idle agent again with its folder's files");
                if !self.has_transcript(agent) {
                    self.trust_fresh.insert(agent);
                }
                self.stop_to_relaunch(session);
            }
        }
    }

    /// Codex reads a folder's files when it loads a thread, so an idle one
    /// is let go, and adopted again once that's done. One that isn't loaded
    /// gets them at its next adopt.
    fn reload_for_trust(&mut self, agent: AgentId) {
        let Some(held) = self.held.get(&agent) else {
            return;
        };
        if self.mid_turn(agent) || self.adopting.contains_key(&agent) {
            return;
        }
        let loaded = !held.adopt && self.session_of(held).is_some();
        // A thread with no turn yet can't be loaded again, so it waits for
        // its first.
        if loaded && !self.has_transcript(agent) {
            return;
        }
        self.trust_restarts.remove(&agent);
        if loaded {
            tracing::info!(%agent, "loading an idle thread again with its folder's files");
            let key = internal_key();
            self.internal.insert(key, Internal::Reload(agent));
            self.work
                .push_back((Target::Agent(agent), Action::Stop { key, agent }));
        }
    }

    /// A trusted Codex agent's thread was let go, so it's adopted again,
    /// unless it's paused, which adopts it when it's back.
    pub(super) fn reloaded(&mut self, agent: AgentId, ok: bool) {
        if !ok {
            warn!(%agent, "couldn't let the thread go to load its folder's files");
            return;
        }
        let Some(held) = self.held.get_mut(&agent) else {
            return;
        };
        held.adopt = true;
        let paused = held.paused.is_some();
        self.save(agent);
        if !paused {
            self.adopt_now(agent);
        }
    }

    /// Whether the CLI wrote a transcript, which it does at its first message.
    /// Resuming a session without one fails.
    pub(super) fn has_transcript(&self, agent: AgentId) -> bool {
        self.timelines.get(&agent).is_some_and(|timeline| {
            timeline.items().any(|item| {
                matches!(
                    item.body,
                    ItemBody::UserMessage {
                        delivery: Delivery::Sent | Delivery::Steer,
                        ..
                    }
                )
            })
        })
    }

    /// Brings the cards back after a restart, and goes on with a restart a
    /// trusted answer asked for.
    pub(super) fn restore_restricted(&mut self) {
        let rows: Vec<(AgentId, Restricted)> = self.db.restricted().unwrap_or_else(|err| {
            warn!(%err, "couldn't read the restricted agents");
            Vec::new()
        });
        for (agent, mut restricted) in rows {
            if !self.held.contains_key(&agent) {
                if let Err(err) = self.db.remove_restricted(agent) {
                    warn!(%err, %agent, "couldn't forget the restricted agent");
                }
                continue;
            }
            let id = folder_trust::card_id(agent, &restricted.fingerprint);
            if self.answers.contains_key(&id) {
                restricted.card = None;
            }
            let trusted = self
                .db
                .folder_trusted(&restricted.key, &restricted.fingerprint)
                .unwrap_or(false);
            if trusted {
                restricted.card = None;
                self.trust_restarts.insert(agent);
            }
            if let Some(card) = restricted.card.clone() {
                self.open_folder_card(card);
            }
            self.restricted.insert(agent, restricted);
        }
    }

    /// An agent went, and its card with it.
    pub(super) fn forget_restricted(&mut self, agent: AgentId) {
        self.trust_restarts.remove(&agent);
        self.trust_fresh.remove(&agent);
        if let Some(old) = self.restricted.remove(&agent) {
            self.close_folder_card(agent, &old, None);
        }
        if let Err(err) = self.db.remove_restricted(agent) {
            warn!(%err, %agent, "couldn't forget the restricted agent");
        }
    }
}

/// Marks a card's `question` as answered with `values`.
pub(super) fn answered(
    item: &mut TimelineItem,
    question: &str,
    values: Vec<String>,
    now: Timestamp,
) {
    if let ItemBody::Question { answers, .. } = &mut item.body {
        *answers = Some(vec![QuestionAnswer {
            id: question.to_owned(),
            values,
        }]);
    }
    item.status = ItemStatus::Completed;
    item.completed_at = Some(now);
}
