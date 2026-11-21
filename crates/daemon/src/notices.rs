// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The notification log: every alert about this device's own agents, by
//! the rules the Mac's notifications and the phone pushes share, kept in
//! `maxd.db` so the app's inbox can list what came before. The newest
//! [`NOTICES_KEPT`] stay. A client lists, marks read and clears them with
//! [`ClientMessage::Notices`](maxhq_protocol::ClientMessage::Notices).

use std::sync::{Arc, Mutex, PoisonError};

use maxhq_core::notify::{project_name, Alert, Notification, NotificationSender, Notifier, Panes};
use maxhq_core::store::Store;
use maxhq_protocol::attention::{show_hidden, AttentionKind};
use maxhq_protocol::ids::{AgentId, DeviceId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::notices::{Notice, NoticeCall, NoticeKind, NoticePage, NOTICES_KEPT};
use maxhq_protocol::phones::PushKind;
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use tracing::warn;

use crate::db::Db;
use crate::hub::{Hub, Start};

/// The refusal for a log on another device.
const OTHER_DEVICE: &str = "This device keeps only its own notifications.";

/// What a row keeps besides its agent and times. JSON, so it can grow
/// without a migration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    kind: NoticeKind,
    title: String,
    project: Option<String>,
    summary: Option<String>,
}

/// A notice before it has a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raised {
    agent: AgentId,
    at: Timestamp,
    record: Record,
}

/// Keeps a notice, then drops the oldest past [`NOTICES_KEPT`].
fn record(conn: &Connection, raised: &Raised) -> rusqlite::Result<()> {
    let record = serde_json::to_string(&raised.record)
        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
    conn.execute(
        "INSERT INTO notices (agent, at, read_at, record) VALUES (?1, ?2, NULL, ?3)",
        params![raised.agent.to_string(), raised.at.0 as i64, record],
    )?;
    conn.execute(
        "DELETE FROM notices WHERE id NOT IN \
         (SELECT id FROM notices ORDER BY id DESC LIMIT ?1)",
        params![NOTICES_KEPT],
    )?;
    Ok(())
}

/// The newest `limit` notices, at most [`NOTICES_KEPT`], and how many the
/// log holds. A row that doesn't read is skipped.
fn page(conn: &Connection, device: DeviceId, limit: u32) -> rusqlite::Result<NoticePage> {
    let (total, unread) = conn.query_row(
        "SELECT COUNT(*), COUNT(*) - COUNT(read_at) FROM notices",
        [],
        |row| Ok((row.get::<_, u32>(0)?, row.get::<_, u32>(1)?)),
    )?;
    let mut query = conn
        .prepare("SELECT id, agent, at, read_at, record FROM notices ORDER BY id DESC LIMIT ?1")?;
    let rows = query.query_map(params![limit.min(NOTICES_KEPT)], |row| {
        Ok((
            row.get::<_, u32>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    let mut notices = Vec::new();
    for row in rows {
        let (id, agent, at, read_at, record) = row?;
        let (Ok(agent), Ok(record)) = (agent.parse(), serde_json::from_str::<Record>(&record))
        else {
            continue;
        };
        let time = |ms: i64| Timestamp(u64::try_from(ms).unwrap_or(0));
        notices.push(Notice {
            id,
            device,
            agent,
            kind: record.kind,
            title: record.title,
            project: record.project,
            summary: record.summary,
            at: time(at),
            read_at: read_at.map(time),
        });
    }
    Ok(NoticePage {
        notices,
        total,
        unread,
    })
}

/// Marks the agent's unread notices read, or every one with no agent.
fn read(conn: &Connection, agent: Option<AgentId>, at: Timestamp) -> rusqlite::Result<()> {
    let at = at.0 as i64;
    match agent {
        Some(agent) => conn.execute(
            "UPDATE notices SET read_at = ?1 WHERE read_at IS NULL AND agent = ?2",
            params![at, agent.to_string()],
        ),
        None => conn.execute(
            "UPDATE notices SET read_at = ?1 WHERE read_at IS NULL",
            params![at],
        ),
    }
    .map(|_| ())
}

/// Answers a call on `device`'s log, which must be this one's.
pub async fn answer(db: Db, me: DeviceId, device: DeviceId, call: NoticeCall) -> ActionOutcome {
    if device != me {
        return ActionOutcome::Refused {
            reason: OTHER_DEVICE.to_owned(),
        };
    }
    let run = move || {
        let conn = db.lock();
        match call {
            NoticeCall::List { limit } => {
                page(&conn, me, limit).map(|page| ActionOutcome::Notices { page })
            }
            NoticeCall::Read { agent } => {
                read(&conn, agent, Timestamp::now()).map(|()| ActionOutcome::Done)
            }
            NoticeCall::Clear => conn
                .execute("DELETE FROM notices", [])
                .map(|_| ActionOutcome::Done),
        }
    };
    match tokio::task::spawn_blocking(run).await {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(err)) => {
            warn!(%err, "couldn't read or change the notification log");
            ActionOutcome::Refused {
                reason: format!("This device couldn't reach its notification log: {err}"),
            }
        }
        Err(err) => {
            warn!(%err, "the notification log's call ended early");
            ActionOutcome::Refused {
                reason: "maxd couldn't finish that. Its log says why.".to_owned(),
            }
        }
    }
}

/// Collects the alerts the notifier raises.
#[derive(Clone, Default)]
struct Collect(Arc<Mutex<Vec<(AgentId, Alert, PushKind)>>>);

impl Collect {
    fn take(&self) -> Vec<(AgentId, Alert, PushKind)> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl NotificationSender for Collect {
    // Only alerts about agents go in the log.
    fn deliver(&self, _: &Notification) {}

    fn alert(&self, notification: &Notification, alert: Alert, kind: PushKind) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push((
            notification.agent,
            alert,
            kind,
        ));
    }

    // The log keeps what was said, whatever cleared since.
    fn remove(&self, _: &[String]) {}

    fn remove_all(&self) {}
}

/// The rules, run on a copy of the hub's state, as the pushes run them.
pub struct Log {
    device: DeviceId,
    store: Store,
    notifier: Notifier,
    said: Collect,
}

impl Log {
    /// Starts with nothing seen, for `device`'s own agents.
    pub fn new(device: DeviceId) -> Self {
        let said = Collect::default();
        Self {
            device,
            store: Store::new(0),
            notifier: Notifier::new(Box::new(said.clone())),
            said,
        }
    }

    /// Takes the whole state, and what it raised, or nothing when `quiet`.
    pub fn reset(&mut self, entities: Vec<Entity>, quiet: bool) -> Vec<Raised> {
        if let Some(delta) = self.store.replace(|_| true, entities) {
            self.notifier
                .observe(&delta, &self.store, &Panes::default());
        }
        let raised = self.raised();
        if quiet {
            Vec::new()
        } else {
            raised
        }
    }

    /// Takes a change to the state, and what it raised.
    pub fn observe(&mut self, changes: Vec<Change>) -> Vec<Raised> {
        if let Some(delta) = self.store.apply(changes) {
            self.notifier
                .observe(&delta, &self.store, &Panes::default());
        }
        self.raised()
    }

    /// What the notifier raised about this device's agents, as notices.
    fn raised(&self) -> Vec<Raised> {
        let at = Timestamp::now();
        self.said
            .take()
            .into_iter()
            .filter_map(|(agent, alert, kind)| {
                let Some(Entity::Agent(agent)) = self.store.get(&EntityKey::Agent(agent)) else {
                    return None;
                };
                // A peer's agents are its own to log.
                match self.store.get(&EntityKey::Workspace(agent.workspace)) {
                    Some(Entity::Workspace(workspace)) if workspace.device == self.device => {}
                    _ => return None,
                }
                let prompt = match alert {
                    Alert::NeedsInput(id) => match self.store.get(&EntityKey::Attention(id)) {
                        Some(Entity::Attention(event)) => Some(event),
                        _ => None,
                    },
                    _ => None,
                };
                let kind = match (alert, kind, prompt.map(|event| event.kind)) {
                    (_, PushKind::Error, _) => NoticeKind::Error,
                    (Alert::NeedsInput(_), _, Some(AttentionKind::Question)) => {
                        NoticeKind::Question
                    }
                    (Alert::NeedsInput(_), _, Some(AttentionKind::Permission)) => {
                        NoticeKind::Permission
                    }
                    (Alert::NeedsInput(_), _, _) => NoticeKind::Input,
                    (Alert::Done, _, _) => NoticeKind::Finished,
                    (Alert::Limit, _, _) => NoticeKind::Limit,
                    (Alert::Resume, _, _) => NoticeKind::Resumed,
                };
                let summary = prompt
                    .map(|event| show_hidden(event.summary.trim()))
                    .filter(|summary| !summary.is_empty());
                Some(Raised {
                    agent: agent.id,
                    at,
                    record: Record {
                        kind,
                        title: show_hidden(&maxhq_core::notify::title(agent)),
                        project: project_name(agent, &self.store).map(|name| show_hidden(&name)),
                        summary,
                    },
                })
            })
            .collect()
    }
}

/// Keeps every notice raised, until the hub stops. Only the first state
/// is quiet: what was waiting when `maxd` started went in the log the run
/// before.
pub async fn watch(hub: Hub, db: Db) {
    let mut log = Log::new(hub.device().id);
    let mut quiet = true;
    loop {
        let subscription = hub.subscribe(None);
        let raised = match subscription.start {
            Start::Snapshot(snapshot) => log.reset(snapshot.entities, quiet),
            Start::Deltas(deltas) => deltas
                .into_iter()
                .flat_map(|delta| log.observe(delta.changes))
                .collect(),
        };
        quiet = false;
        keep(&db, raised).await;
        let mut deltas = subscription.deltas;
        loop {
            match deltas.recv().await {
                Ok(delta) => keep(&db, log.observe(delta.changes)).await,
                // Missed deltas: start again from the whole state.
                Err(RecvError::Lagged(_)) => break,
                Err(RecvError::Closed) => return,
            }
        }
    }
}

/// Writes notices to the log.
async fn keep(db: &Db, raised: Vec<Raised>) {
    if raised.is_empty() {
        return;
    }
    let db = db.clone();
    let write = move || {
        let conn = db.lock();
        raised.iter().try_for_each(|raised| record(&conn, raised))
    };
    match tokio::task::spawn_blocking(write).await {
        Ok(Ok(())) => {}
        Ok(Err(err)) => warn!(%err, "couldn't keep a notification in the log"),
        Err(err) => warn!(%err, "couldn't keep a notification in the log"),
    }
}

#[cfg(test)]
mod tests;
