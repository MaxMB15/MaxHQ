// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What the driver keeps for each thread: its settings, its turn, the items
//! still open and the messages waiting to go out.

use std::collections::{HashMap, HashSet, VecDeque};

use maxhq_core::handoff;
use maxhq_protocol::context::InlineRef;
use maxhq_protocol::ids::{AgentId, IdempotencyKey};
use maxhq_protocol::model::{AgentLink, ContextUsage, PermissionMode};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{
    AttachmentRef, Delivery, ItemBody, ItemId, ItemStatus, TimelineItem,
};
use serde_json::Value;

use super::kept::Kept;
use super::{items, Seen};
use crate::drive::{Message, Place, Staged};

/// The most lines kept for a thread while it resumes.
pub const BUFFERED: usize = 5000;

/// Where a thread's turn stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Turn {
    /// No turn is running.
    Idle,
    /// `turn/start` went out and its turn ID hasn't come back yet.
    Starting,
    /// The turn with this ID is running.
    Active(String),
}

/// An item that started and hasn't completed.
#[derive(Clone, Debug)]
pub struct Open {
    /// Codex's type for it, lower case without underscores.
    pub kind: String,
    /// The item as last sent.
    pub item: TimelineItem,
    /// Whether it was sent at all. Reasoning waits for its first text, so
    /// an empty one never shows.
    pub shown: bool,
}

/// What a user message Codex echoed shows, taken from the message as you
/// typed it rather than as Codex got it.
#[derive(Clone, Debug)]
pub struct Echo {
    /// What you typed.
    pub text: String,
    /// Its pills.
    pub refs: Vec<InlineRef>,
    /// The attachments, as clients know them.
    pub attachments: Vec<AttachmentRef>,
    /// How it went in.
    pub delivery: Delivery,
}

/// A message on its way to Codex. It shows as a local item, and Codex's
/// echo of it as a user message updates that item.
#[derive(Clone, Debug)]
pub struct Outgoing {
    /// The local item's ID.
    pub local: ItemId,
    /// The `clientUserMessageId` it goes out with.
    pub client_id: String,
    /// What you typed.
    pub text: String,
    /// Its pills.
    pub refs: Vec<InlineRef>,
    /// Its attachments.
    pub attachments: Vec<Staged>,
    /// How it goes in.
    pub delivery: Delivery,
    /// The key to answer once it's out, when the action is still waiting.
    pub key: Option<IdempotencyKey>,
    /// When you sent it.
    pub at: Timestamp,
}

impl Outgoing {
    /// A new message with a fresh local ID.
    pub fn new(message: Message, key: Option<IdempotencyKey>, at: Timestamp) -> Self {
        let client_id = uuid::Uuid::now_v7().to_string();
        Self {
            local: items::local_id(&client_id),
            client_id,
            text: message.text,
            refs: message.refs,
            attachments: message.attachments,
            delivery: Delivery::Sent,
            key,
            at,
        }
    }

    /// The attachments as clients know them.
    pub fn shown(&self) -> Vec<AttachmentRef> {
        self.attachments
            .iter()
            .map(|staged| staged.attachment.clone())
            .collect()
    }

    /// The local item that shows the message on `agent`'s timeline.
    pub fn item(&self, agent: AgentId, status: ItemStatus) -> TimelineItem {
        TimelineItem {
            id: self.local.clone(),
            agent,
            turn: None,
            parent: None,
            ordinal: 0,
            status,
            started_at: Some(self.at),
            completed_at: None,
            body: ItemBody::UserMessage {
                text: self.typed().to_owned(),
                attachments: self.shown(),
                refs: self.refs.clone(),
                delivery: self.delivery,
            },
        }
    }

    /// The text as you typed it, without a handoff in front of it.
    fn typed(&self) -> &str {
        handoff::strip(&self.text)
    }

    /// What the echo shows.
    pub fn echo(&self) -> Echo {
        Echo {
            text: self.typed().to_owned(),
            refs: self.refs.clone(),
            attachments: self.shown(),
            delivery: self.delivery,
        }
    }
}

/// One Codex thread: a top-level agent, or a sub-agent when it has a
/// parent.
#[derive(Debug)]
pub struct Thread {
    /// Codex's thread ID.
    pub id: String,
    /// The agent it is.
    pub agent: AgentId,
    /// The thread that spawned it, for a sub-agent.
    pub parent: Option<String>,
    /// Where it runs. A thread first seen in replayed lines has none until
    /// the manager adopts it, and a sub-agent takes its parent's.
    pub place: Option<Place>,
    /// The model it runs.
    pub model: Option<String>,
    /// A model you picked that hasn't reached Codex yet.
    pub wanted_model: Option<String>,
    /// The effort level you picked, or none for the model's default. Each
    /// turn sends it, so it holds through a resume.
    pub effort: Option<String>,
    /// The mode you asked for.
    pub requested: Option<PermissionMode>,
    /// The mode Codex reports, or `None` for a mix MaxHQ never sets.
    pub effective: Option<PermissionMode>,
    /// Whether settings wait to go out between turns.
    pub dirty: bool,
    /// The title.
    pub title: Option<String>,
    /// Whether you set the title, so Codex's own name doesn't replace it.
    pub titled: bool,
    /// How full the context window is.
    pub context: Option<ContextUsage>,
    /// When it started.
    pub started_at: Timestamp,
    /// When a turn last started or ended.
    pub last_activity_at: Timestamp,
    /// Whether the app-server has it loaded, so a turn can start at once.
    pub loaded: bool,
    /// Whether `thread/resume` is under way. Its lines wait in `buffer`.
    pub resuming: bool,
    /// Lines that came while it resumed.
    pub buffer: Kept,
    /// Items a history read found finished, by Codex's item ID, until the
    /// lines kept during the resume have run. Their lines read again after
    /// a restart don't start them over.
    pub settled: HashSet<String>,
    /// A system error Codex reported.
    pub errored: Option<String>,
    /// The turn.
    pub turn: Turn,
    /// The holder's seq for the first line that showed the running turn.
    pub turn_from: Option<u64>,
    /// An interrupt that waits for the turn's ID.
    pub interrupt_wanted: bool,
    /// When the last turn ended.
    pub ended_at: Option<Timestamp>,
    /// Whether the cache is assumed cold since then.
    pub cold: bool,
    /// Set while the account is out of its limit, with when it resets.
    pub limited: Option<Option<Timestamp>>,
    /// The limit-wait item on the timeline while `limited` is set.
    pub limit_item: Option<TimelineItem>,
    /// Items started and not completed, by Codex's item ID.
    pub open: HashMap<String, Open>,
    /// What echoed user messages show, by Codex's item ID.
    pub echoes: HashMap<String, Echo>,
    /// Messages waiting for the turn to end.
    pub queue: VecDeque<Outgoing>,
    /// Messages sent and not echoed yet, oldest first.
    pub sent: VecDeque<Outgoing>,
    /// Steers waiting for the turn's ID.
    pub steers: Vec<Outgoing>,
    /// Whether you stopped it.
    pub stopped: bool,
    /// How `maxd` reaches it.
    pub link: AgentLink,
}

impl Thread {
    /// A thread with nothing known about it yet.
    pub fn new(id: String, agent: AgentId, at: Timestamp) -> Self {
        Self {
            id,
            agent,
            parent: None,
            place: None,
            model: None,
            wanted_model: None,
            effort: None,
            requested: None,
            effective: None,
            dirty: false,
            title: None,
            titled: false,
            context: None,
            started_at: at,
            last_activity_at: at,
            loaded: true,
            resuming: false,
            buffer: Kept::default(),
            settled: HashSet::new(),
            errored: None,
            turn: Turn::Idle,
            turn_from: None,
            interrupt_wanted: false,
            ended_at: None,
            cold: false,
            limited: None,
            limit_item: None,
            open: HashMap::new(),
            echoes: HashMap::new(),
            queue: VecDeque::new(),
            sent: VecDeque::new(),
            steers: Vec::new(),
            stopped: false,
            link: AgentLink::Live,
        }
    }

    /// Whether a turn is running or starting.
    pub fn busy(&self) -> bool {
        self.turn != Turn::Idle
    }

    /// Notes activity at `at`. A line read again after a restart is older
    /// than what `maxd` stored, so it leaves the time where it is.
    pub fn touch(&mut self, at: Timestamp) {
        self.last_activity_at = self.last_activity_at.max(at);
    }

    /// The running turn's ID.
    pub fn active_turn(&self) -> Option<&str> {
        match &self.turn {
            Turn::Active(turn) => Some(turn),
            _ => None,
        }
    }

    /// Marks `turn` as running, shown first by the line with `seq`. A line
    /// read again after a restart moves the start back to it.
    pub fn run(&mut self, turn: &str, seq: u64) {
        if self.active_turn() == Some(turn) {
            self.turn_from = Some(self.turn_from.map_or(seq, |from| from.min(seq)));
        } else {
            self.turn = Turn::Active(turn.to_owned());
            self.turn_from = Some(seq);
        }
    }

    /// Keeps a line for after the resume, dropping it once the buffer is
    /// full.
    pub fn keep(&mut self, text: &str, method: &str, params: &Value, seen: Seen) {
        self.buffer.keep(text, method, params, seen, BUFFERED);
    }

    /// Takes the echo for a user message Codex sent back: the message sent
    /// with its client ID, or else the oldest one sent.
    pub fn take_sent(&mut self, client_id: Option<&str>) -> Option<Outgoing> {
        if let Some(client_id) = client_id
            && let Some(index) = self
                .sent
                .iter()
                .position(|message| message.client_id == client_id)
        {
            return self.sent.remove(index);
        }
        self.sent.pop_front()
    }
}
