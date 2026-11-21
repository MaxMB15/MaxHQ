// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The dispatcher's chats. Each conversation with the dispatcher is a chat
//! that runs its own agent. A request soon after the chat's last one goes
//! there, and one after a longer pause, as the device's settings set it,
//! starts a new chat whose first message lists the recent ones. The
//! dispatcher moves a request about something else to another chat with
//! `switch_chat`, and Keep here moves it back.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::fmt::Write as _;

use maxhq_core::handoff;
use maxhq_core::pill_text::Carried;
use maxhq_core::timeline_find;
use maxhq_core::tools::chats::{one_line, ChatCall, SwitchTo, MAX_CHAT_TITLE};
use maxhq_core::tools::notes::NoteCall;
use maxhq_core::tools::Call;
use maxhq_protocol::dispatch::{ChatAct, DispatcherChat};
use maxhq_protocol::ids::{AccountId, AgentId, IdempotencyKey, ProjectId, SessionId};
use maxhq_protocol::message::{ActionOutcome, ClientMessage, SendMode, UserInput};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::warn;

use super::agent_tools::{failure, Later, Reply, NO_CONN};
use super::held::is_busy;
use super::idem::{Outcome, Seen};
use super::{refused, Input, Manager};
use crate::drive::{ToolCall, ToolResult};

/// How many recent chats a new chat's first message lists.
const INDEX_CHATS: usize = 30;

/// How many of the newest chats `find_chats` reads through in full.
const READ_CHATS: usize = 50;

/// How many chats `find_chats` gives at most.
const FOUND: usize = 8;

/// What opens the list of recent chats in front of a new chat's first
/// message.
const INDEX_HEAD: &str = "The user's recent chats with you, newest first. If the message \
                          below carries on one of them, call switch_chat with its number. \
                          find_chats finds older ones.";

/// What a chat you keep a moved request in is sent.
const KEEP_HERE: &str = "Keep this here. Answer my last message in this chat.";

/// What a chat you save to Notes is sent.
const SAVE_NOTE: &str = "Save this chat to Notes.";

/// Why a call from an agent that isn't a chat is refused.
const NOT_A_CHAT: &str = "MaxHQ doesn't keep this conversation as a chat.";

/// Why a note from a chat with no project is refused.
const NO_PROJECT: &str = "This chat has no project, and a note belongs to one. Name the \
                          chat's project with name_chat first, or ask the user which project.";

/// What the manager keeps of the dispatcher's chats.
#[derive(Default)]
pub(super) struct Chats {
    /// Each chat, by its agent.
    rows: HashMap<AgentId, Chat>,
    /// The requests that start a chat, until they're done.
    starting: HashMap<IdempotencyKey, Starting>,
}

/// One chat.
struct Chat {
    /// What's stored of it.
    record: Record,
    /// Your last request in it, which `switch_chat` moves. A restart
    /// forgets it.
    last: Option<UserInput>,
    /// The chat the dispatcher moved that request to, until you write
    /// again.
    moved: Option<Moved>,
    /// Whether you kept that request here, so it doesn't move again.
    kept: bool,
    /// Whether it holds only its first request yet. Such a chat that moved
    /// the request away goes once you write again.
    stub: bool,
}

impl Chat {
    fn new(record: Record) -> Self {
        Self {
            record,
            last: None,
            moved: None,
            kept: false,
            stub: false,
        }
    }
}

/// Where a request went.
#[derive(Clone, Copy)]
struct Moved {
    /// The chat it went to.
    to: AgentId,
    /// Whether the move started that chat.
    started: bool,
}

/// What's stored of a chat.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Record {
    agent: AgentId,
    number: u32,
    title: String,
    summary: String,
    project: Option<ProjectId>,
    active_at: Timestamp,
    archived: bool,
    /// Whether you named it, so the dispatcher's names leave the title be.
    named: bool,
}

/// A request that starts a chat.
struct Starting {
    /// The request.
    input: UserInput,
    /// The recent chats its first message lists, after a pause.
    index: Option<String>,
    /// The title `switch_chat` gave the chat.
    title: String,
    /// The chat a switch moved the request out of, and the call to answer
    /// once the new chat starts.
    from: Option<(AgentId, Reply)>,
}

impl Manager {
    /// Whether an agent is one of the dispatcher's chats.
    pub(super) fn is_chat(&self, agent: AgentId) -> bool {
        self.dispatcher.chats.rows.contains_key(&agent)
    }

    /// Makes a chat the conversation the header shows. An agent that isn't
    /// one of the dispatcher's chats never takes its place.
    pub(super) fn adopt_chat(&mut self, chat: AgentId) {
        if self.is_chat(chat) {
            self.dispatcher.agent = Some(chat);
        }
    }

    /// Handles a client's act on one chat.
    pub(super) fn chat_message(&mut self, conn: u64, message: ClientMessage) {
        let ClientMessage::ActOnChat {
            key,
            agent: chat,
            action: act,
        } = message
        else {
            return;
        };
        match self.keys.begin(key, conn) {
            Seen::New => {}
            Seen::Running => return,
            Seen::Done(outcome) => {
                self.tell(conn, outcome.message(key));
                return;
            }
        }
        if let Some(outcome) = self.act_on_chat(key, chat, act) {
            self.finish(key, Outcome::Action(outcome));
        }
    }

    /// Picks, renames, archives or saves a chat, or keeps the request
    /// moved out of it there.
    fn act_on_chat(
        &mut self,
        key: IdempotencyKey,
        chat: AgentId,
        act: ChatAct,
    ) -> Option<ActionOutcome> {
        if !self.dispatcher.chats.rows.contains_key(&chat) {
            return Some(refused("That chat isn't here any more."));
        }
        let now = self.clock.wall();
        match act {
            ChatAct::Pick => {
                if let Err(reason) = self.pick_chat(chat) {
                    return Some(refused(reason));
                }
            }
            ChatAct::Rename { title } => {
                let title = one_line(title, MAX_CHAT_TITLE);
                if title.is_empty() {
                    return Some(refused("Give the chat a title."));
                }
                self.edit_chat(chat, |row| {
                    row.record.title = title;
                    row.record.named = true;
                });
            }
            ChatAct::Archive { archived } => self.edit_chat(chat, |row| {
                row.record.archived = archived;
                if !archived {
                    row.record.active_at = now;
                }
            }),
            ChatAct::KeepHere => return self.keep_here(key, chat),
            ChatAct::SaveNote => {
                if let Err(reason) = self.pick_chat(chat) {
                    return Some(refused(reason));
                }
                // The request to save isn't yours to move.
                self.edit_chat(chat, |row| row.last = None);
                let input = typed(SAVE_NOTE);
                let send = ClientMessage::SendInput {
                    key,
                    agent: chat,
                    input,
                    mode: SendMode::Queue,
                };
                return self.action(key, send);
            }
        }
        Some(ActionOutcome::Done)
    }

    /// Makes `chat` the conversation now.
    fn pick_chat(&mut self, chat: AgentId) -> Result<(), &'static str> {
        if self.dispatcher.starting.is_some() {
            return Err("The dispatcher is still starting.");
        }
        if self.dispatcher.agent != Some(chat) {
            self.dispatcher.agent = Some(chat);
            self.dispatcher.plan = None;
            self.dispatcher.carried = Carried::default();
            self.publish_dispatcher();
        }
        let now = self.clock.wall();
        self.edit_chat(chat, |row| {
            row.record.active_at = now;
            row.record.archived = false;
        });
        Ok(())
    }

    /// Moves the request the dispatcher moved out of `source` back, and has
    /// `source` answer it. A chat the move started goes, and one that was
    /// there before stops answering it.
    fn keep_here(&mut self, key: IdempotencyKey, source: AgentId) -> Option<ActionOutcome> {
        let Some(row) = self.dispatcher.chats.rows.get(&source) else {
            return Some(refused("That chat isn't here any more."));
        };
        let Some(Moved { to, started }) = row.moved else {
            return Some(refused("Nothing moved out of that chat."));
        };
        if self.dispatcher.starting.is_some() {
            return Some(refused("The dispatcher is still starting."));
        }
        let mut carried = Carried::default();
        if let Some(last) = &row.last {
            carried.keep(&last.refs);
        }
        self.dispatcher.agent = Some(source);
        self.dispatcher.plan = None;
        self.dispatcher.carried = carried;
        let now = self.clock.wall();
        self.edit_chat(source, |row| {
            row.moved = None;
            row.kept = true;
            row.stub = false;
            row.record.active_at = now;
            row.record.archived = false;
        });
        if started {
            let delete = IdempotencyKey::derive(&format!("dispatcher-keep:{key}"));
            self.message(
                NO_CONN,
                ClientMessage::DeleteAgent {
                    key: delete,
                    agent: to,
                },
            );
        } else {
            self.edit_chat(to, |row| row.last = None);
            if self
                .held
                .get(&to)
                .is_some_and(|held| is_busy(&held.agent.activity))
            {
                let stop = IdempotencyKey::derive(&format!("dispatcher-keep:{key}"));
                self.message(
                    NO_CONN,
                    ClientMessage::Interrupt {
                        key: stop,
                        agent: to,
                    },
                );
            }
        }
        self.publish_dispatcher();
        let send = ClientMessage::SendInput {
            key,
            agent: source,
            input: typed(KEEP_HERE),
            mode: SendMode::Queue,
        };
        self.action(key, send)
    }

    /// Whether a request to the dispatcher starts a new chat: when asked,
    /// when there's no chat now, or when the chat now has been quiet longer
    /// than the device's [`maxhq_protocol::model::DeviceSettings::chat_gap`],
    /// if it has one. A new chat after a pause lists the recent ones in its
    /// first message. Writing clears where the dispatcher moved your last
    /// request.
    pub(super) fn route_chat(
        &mut self,
        key: IdempotencyKey,
        input: &UserInput,
        current: Option<AgentId>,
        new: bool,
    ) -> bool {
        // A switch to a new chat routed it already.
        if self.dispatcher.chats.starting.contains_key(&key) {
            return new;
        }
        self.settle_moves();
        let now = self.clock.wall();
        let gap = self.device_settings().chat_gap();
        // The chat was last active when you last wrote or it last did
        // something, such as answer.
        let recent = current.filter(|agent| {
            let Some(row) = self.dispatcher.chats.rows.get(agent) else {
                return false;
            };
            let held = self.held.get(agent).map(|held| held.agent.last_activity_at);
            let last = held.map_or(row.record.active_at, |at| at.max(row.record.active_at));
            gap.is_none_or(|gap| u128::from(now.0.saturating_sub(last.0)) < gap.as_millis())
        });
        if let Some(chat) = recent.filter(|_| !new) {
            self.edit_chat(chat, |row| {
                row.record.active_at = now;
                row.record.archived = false;
                row.last = Some(input.clone());
                row.kept = false;
                row.stub = false;
            });
            return false;
        }
        let index = if new { None } else { self.chat_index(now) };
        let starting = Starting {
            input: input.clone(),
            index,
            title: String::new(),
            from: None,
        };
        self.dispatcher.chats.starting.insert(key, starting);
        true
    }

    /// Clears where the dispatcher moved your last request, and deletes a
    /// chat that held nothing but a request it moved away.
    fn settle_moves(&mut self) {
        let moved: Vec<(AgentId, bool)> = self
            .dispatcher
            .chats
            .rows
            .iter()
            .filter(|(_, row)| row.moved.is_some())
            .map(|(chat, row)| (*chat, row.stub))
            .collect();
        for (chat, stub) in moved {
            self.edit_chat(chat, |row| row.moved = None);
            if stub {
                let key = IdempotencyKey::derive(&format!("dispatcher-stub:{chat}"));
                self.message(NO_CONN, ClientMessage::DeleteAgent { key, agent: chat });
            }
        }
    }

    /// The recent chats a new chat's first message lists, newest first.
    /// None until a chat has a title.
    fn chat_index(&self, now: Timestamp) -> Option<String> {
        let mut records: Vec<&Record> = self
            .dispatcher
            .chats
            .rows
            .values()
            .map(|row| &row.record)
            .filter(|record| !record.title.is_empty())
            .collect();
        if records.is_empty() {
            return None;
        }
        records.sort_by_key(|record| Reverse(record.active_at));
        let mut index = INDEX_HEAD.to_owned();
        for record in records.into_iter().take(INDEX_CHATS) {
            let age = age(now, record.active_at);
            let _ = write!(index, "\n#{} {}, {age}", record.number, record.title);
            if record.archived {
                index.push_str(", archived");
            }
            if !record.summary.is_empty() {
                let _ = write!(index, ": {}", record.summary);
            }
        }
        Some(index)
    }

    /// A new chat's first message: after a pause, the recent chats go in
    /// front of it, where the timeline doesn't show them.
    pub(super) fn chat_index_for(&mut self, key: IdempotencyKey, text: String) -> String {
        let index = self
            .dispatcher
            .chats
            .starting
            .get_mut(&key)
            .and_then(|starting| starting.index.take());
        match index {
            Some(index) => handoff::wrap(&index, &text),
            None => text,
        }
    }

    /// Hears that a request finished. One that started a chat lists it,
    /// and a switch that started one answers the chat it left. A switch
    /// whose chat didn't start leaves the request where it was.
    pub(super) fn chats_done(&mut self, key: IdempotencyKey, outcome: &Outcome) {
        let Some(starting) = self.dispatcher.chats.starting.remove(&key) else {
            return;
        };
        let Starting {
            input, title, from, ..
        } = starting;
        let Outcome::Action(ActionOutcome::Started { agent }) = outcome else {
            if let Some((source, reply)) = from {
                self.dispatcher.agent = Some(source);
                self.publish_dispatcher();
                let reason = match outcome {
                    Outcome::Action(outcome) => failure(outcome),
                    Outcome::Answer(_) => "it didn't start".into(),
                };
                let reason = format!("The new chat didn't start, so answer here: {reason}");
                self.reply_now(reply, Err(reason));
            }
            return;
        };
        let agent = *agent;
        let number = self.next_number();
        let record = Record {
            agent,
            number,
            title: title.clone(),
            summary: String::new(),
            project: None,
            active_at: self.clock.wall(),
            archived: false,
            named: false,
        };
        let chat = Chat {
            last: Some(input),
            stub: from.is_none(),
            ..Chat::new(record)
        };
        self.dispatcher.chats.rows.insert(agent, chat);
        self.store_chat(agent);
        self.publish_chat(agent);
        if let Some((source, reply)) = from {
            let moved = Moved {
                to: agent,
                started: true,
            };
            self.edit_chat(source, |row| row.moved = Some(moved));
            self.reply_now(reply, Ok(moved_to(number, &title)));
        }
    }

    /// The number a new chat gets.
    fn next_number(&self) -> u32 {
        let rows = self.dispatcher.chats.rows.values();
        rows.map(|row| row.record.number).max().unwrap_or(0) + 1
    }

    /// Runs the dispatcher's call to MaxHQ's tools: its chat tools and a
    /// note here, the rest in [`Manager::dispatcher_tool`].
    pub(super) fn dispatcher_call(&mut self, session: SessionId, call: ToolCall) {
        let lead = self.lead_of(call.agent);
        match call.call {
            Call::Chat(chat) => {
                let reply = Reply::Driver {
                    session,
                    key: call.key,
                };
                match chat {
                    ChatCall::Name {
                        title,
                        summary,
                        project,
                    } => {
                        let named = self.name_chat(lead, title, summary, project);
                        self.reply_now(reply, named);
                    }
                    ChatCall::Switch(to) => self.switch_chat(reply, lead, to, &call.call_id),
                    ChatCall::Find { query } => self.find_chats(reply, lead, &query),
                }
            }
            Call::Note(note @ NoteCall::Create { .. }) => {
                let reply = Reply::Driver {
                    session,
                    key: call.key,
                };
                if self.chat_project(lead).is_none() {
                    return self.reply_now(reply, Err(NO_PROJECT.into()));
                }
                self.note_tool(reply, self.agent_caller(lead), note);
            }
            other => self.dispatcher_tool(
                session,
                ToolCall {
                    call: other,
                    ..call
                },
            ),
        }
    }

    /// Names the caller's chat. A title you gave stays.
    fn name_chat(
        &mut self,
        chat: AgentId,
        title: String,
        summary: String,
        project: Option<ProjectId>,
    ) -> Result<String, String> {
        if !self.dispatcher.chats.rows.contains_key(&chat) {
            return Err(NOT_A_CHAT.into());
        }
        if title.is_empty() {
            return Err("Give a `title`.".into());
        }
        if let Some(project) = project
            && !matches!(
                self.hub.get(&EntityKey::Project(project)),
                Some(Entity::Project(_))
            )
        {
            return Err(format!(
                "There's no project with id {project}. list_targets lists them."
            ));
        }
        self.edit_chat(chat, |row| {
            if !row.record.named {
                row.record.title = title;
            }
            row.record.summary = summary;
            row.record.project = project.or(row.record.project);
            row.stub = false;
        });
        Ok("Named the chat.".into())
    }

    /// Moves the user's last request in `source` to another chat, which
    /// answers it. A chat it moves to that ran before resumes with it.
    fn switch_chat(&mut self, reply: Reply, source: AgentId, to: SwitchTo, call: &str) {
        let checked = self.switchable(source);
        let (input, account) = match checked {
            Ok(found) => found,
            Err(reason) => return self.reply_now(reply, Err(reason.into())),
        };
        let key = IdempotencyKey::derive(&format!("dispatcher-switch:{source}:{call}"));
        let title = match to {
            SwitchTo::New(title) => title,
            SwitchTo::Chat(number) => {
                let moved = self.switch_to(key, source, number, input);
                return self.reply_now(reply, moved);
            }
        };
        if !matches!(self.keys.begin(key, NO_CONN), Seen::New) {
            return self.reply_now(reply, Err("That move ran already.".into()));
        }
        let starting = Starting {
            input: input.clone(),
            index: None,
            title,
            from: Some((source, reply)),
        };
        self.dispatcher.chats.starting.insert(key, starting);
        // The chat the request leaves runs on to end its turn.
        self.dispatcher.agent = None;
        self.owner_words.sent(key, &input.text);
        if let Some(outcome) = self.ask_dispatcher(key, input, account, true) {
            self.finish(key, Outcome::Action(outcome));
        }
    }

    /// The request `source` may move and the account it runs as, or why
    /// it may not move one.
    fn switchable(&self, source: AgentId) -> Result<(UserInput, AccountId), &'static str> {
        let row = self.dispatcher.chats.rows.get(&source).ok_or(NOT_A_CHAT)?;
        if self.dispatcher.agent != Some(source) {
            return Err("Only the chat the user writes in now can move a message.");
        }
        if row.kept {
            return Err("The user kept the message in this chat, so answer it here.");
        }
        let input = row
            .last
            .clone()
            .ok_or("There's no message of the user's here to move, so answer in this chat.")?;
        let held = self.held.get(&source).ok_or(NOT_A_CHAT)?;
        Ok((input, held.account))
    }

    /// Moves a request to the chat numbered `number`, which resumes with
    /// it.
    fn switch_to(
        &mut self,
        key: IdempotencyKey,
        source: AgentId,
        number: u32,
        input: UserInput,
    ) -> Result<String, String> {
        let found = self
            .dispatcher
            .chats
            .rows
            .values()
            .find(|row| row.record.number == number)
            .map(|row| (row.record.agent, row.record.title.clone()));
        let Some((target, title)) = found else {
            return Err(format!(
                "There's no chat #{number}. find_chats finds earlier chats."
            ));
        };
        if target == source {
            return Err("That's this chat, so answer here.".into());
        }
        if !self.held.contains_key(&target) {
            return Err(NOT_A_CHAT.into());
        }
        let mut carried = Carried::default();
        carried.keep(&input.refs);
        self.dispatcher.agent = Some(target);
        self.dispatcher.plan = None;
        self.dispatcher.carried = carried;
        let now = self.clock.wall();
        let moved = input.clone();
        self.edit_chat(target, |row| {
            row.record.active_at = now;
            row.record.archived = false;
            row.last = Some(moved);
            row.kept = false;
            row.stub = false;
        });
        let moved = Moved {
            to: target,
            started: false,
        };
        self.edit_chat(source, |row| row.moved = Some(moved));
        self.publish_dispatcher();
        self.owner_words.sent(key, &input.text);
        let send = ClientMessage::SendInput {
            key,
            agent: target,
            input,
            mode: SendMode::Queue,
        };
        self.message(NO_CONN, send);
        Ok(moved_to(number, &title))
    }

    /// Finds the chats other than `caller` that match `query`: a title
    /// match first, then a summary match, then the most matches in the
    /// full text of the newest [`READ_CHATS`], newest first on a tie. The
    /// full text is read off the loop.
    fn find_chats(&mut self, reply: Reply, caller: AgentId, query: &str) {
        let words: Vec<String> = timeline_find::fold(query)
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        let mut records: Vec<&Record> = self
            .dispatcher
            .chats
            .rows
            .values()
            .map(|row| &row.record)
            .filter(|record| record.agent != caller)
            .collect();
        records.sort_by_key(|record| Reverse(record.active_at));
        let now = self.clock.wall();
        let found: Vec<_> = records
            .into_iter()
            .enumerate()
            .map(|(n, record)| {
                let title = hits(&record.title, &words);
                let summary = hits(&record.summary, &words);
                let body = (n < READ_CHATS).then(|| self.matches_in(record.agent, words.clone()));
                (chat_card(record, now), (title, summary), body)
            })
            .collect();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let mut ranked = Vec::new();
            for (card, (title, summary), body) in found {
                let body = match body {
                    Some(counted) => counted.await,
                    None => 0,
                };
                if title + summary + body > 0 {
                    ranked.push(((title, summary, body), card));
                }
            }
            ranked.sort_by_key(|(rank, _)| Reverse(*rank));
            let chats: Vec<Value> = ranked
                .into_iter()
                .take(FOUND)
                .map(|(_, card)| card)
                .collect();
            let text = if chats.is_empty() {
                "No earlier chat matches.".to_owned()
            } else {
                json!({"chats": chats, "note": "Move the message to one with switch_chat."})
                    .to_string()
            };
            let later = Later::Answer {
                reply,
                result: Ok(ToolResult::Text(text)),
                release: None,
            };
            let _ = tx.send(Input::Agents(Box::new(later)));
        });
    }

    /// The project a chat is about, which its notes go to.
    pub(super) fn chat_project(&self, agent: AgentId) -> Option<ProjectId> {
        let row = self.dispatcher.chats.rows.get(&self.lead_of(agent))?;
        row.record.project
    }

    /// Takes up the chats the last run kept, with a chat for each
    /// conversation from before chats were kept, and makes the one you
    /// wrote in last the conversation now. An agent that tools started in
    /// the dispatcher's folder doesn't become one.
    pub(super) fn restore_chats(&mut self) {
        if self.dispatcher.home().is_none() {
            return;
        }
        let stored = self.db.dispatcher_chats::<Record>().unwrap_or_else(|err| {
            warn!(%err, "couldn't read the dispatcher's chats");
            Vec::new()
        });
        for record in stored {
            let agent = record.agent;
            if self.in_home(agent) {
                self.dispatcher.chats.rows.insert(agent, Chat::new(record));
            } else if let Err(err) = self.db.remove_dispatcher_chat(agent) {
                warn!(%err, %agent, "couldn't forget the dispatcher chat");
            }
        }
        let mut unlisted: Vec<(Timestamp, AgentId, Timestamp)> = self
            .held
            .values()
            .filter(|held| held.agent.origin.is_none() && self.in_home(held.agent.id))
            .filter(|held| !self.dispatcher.chats.rows.contains_key(&held.agent.id))
            .map(|held| {
                let agent = &held.agent;
                (agent.started_at, agent.id, agent.last_activity_at)
            })
            .collect();
        unlisted.sort();
        for (_, agent, active_at) in unlisted {
            let record = Record {
                agent,
                number: self.next_number(),
                title: String::new(),
                summary: String::new(),
                project: None,
                active_at,
                archived: false,
                named: false,
            };
            self.dispatcher.chats.rows.insert(agent, Chat::new(record));
            self.store_chat(agent);
        }
        let rows = self.dispatcher.chats.rows.values();
        let newest = rows.max_by_key(|row| row.record.active_at);
        self.dispatcher.agent = newest.map(|row| row.record.agent);
        self.publish_dispatcher();
        let chats: Vec<AgentId> = self.dispatcher.chats.rows.keys().copied().collect();
        for chat in chats {
            self.publish_chat(chat);
        }
    }

    /// Forgets a chat whose agent is forgotten.
    pub(super) fn forget_chat(&mut self, agent: AgentId) {
        if self.dispatcher.chats.rows.remove(&agent).is_none() {
            return;
        }
        if let Err(err) = self.db.remove_dispatcher_chat(agent) {
            warn!(%err, %agent, "couldn't forget the dispatcher chat");
        }
        self.hub.apply([Change::Remove {
            key: EntityKey::DispatcherChat(agent),
        }]);
        let pointing: Vec<AgentId> = self
            .dispatcher
            .chats
            .rows
            .iter()
            .filter(|(_, row)| row.moved.is_some_and(|moved| moved.to == agent))
            .map(|(chat, _)| *chat)
            .collect();
        for chat in pointing {
            self.edit_chat(chat, |row| row.moved = None);
        }
        if self.dispatcher.agent == Some(agent) {
            self.dispatcher.agent = None;
            self.dispatcher.plan = None;
            self.publish_dispatcher();
        }
    }

    /// Changes a chat, stores it and shows it.
    fn edit_chat(&mut self, chat: AgentId, edit: impl FnOnce(&mut Chat)) {
        let Some(row) = self.dispatcher.chats.rows.get_mut(&chat) else {
            return;
        };
        edit(row);
        self.store_chat(chat);
        self.publish_chat(chat);
    }

    /// Stores a chat so a restarted `maxd` lists it.
    fn store_chat(&self, chat: AgentId) {
        let Some(row) = self.dispatcher.chats.rows.get(&chat) else {
            return;
        };
        if let Err(err) = self.db.put_dispatcher_chat(chat, &row.record) {
            warn!(%err, %chat, "couldn't store the dispatcher chat");
        }
    }

    /// Shows a chat as it is now.
    fn publish_chat(&self, chat: AgentId) {
        let Some(row) = self.dispatcher.chats.rows.get(&chat) else {
            return;
        };
        let record = row.record.clone();
        self.hub.apply([Change::Upsert {
            value: Entity::DispatcherChat(DispatcherChat {
                agent: record.agent,
                device: self.device,
                number: record.number,
                title: record.title,
                summary: record.summary,
                project: record.project,
                active_at: record.active_at,
                archived: record.archived,
                moved_to: row.moved.map(|moved| moved.to),
            }),
        }]);
    }
}

/// A message as you'd type it, with no pills or files.
fn typed(text: &str) -> UserInput {
    UserInput {
        text: text.to_owned(),
        attachments: Vec::new(),
        refs: Vec::new(),
    }
}

/// What `switch_chat` answers once the request moved.
fn moved_to(number: u32, title: &str) -> String {
    format!(
        "Moved the message to chat #{number}, \"{title}\", which answers it there. End your \
         turn now without answering."
    )
}

/// How many of `words`, already folded, show in `text`.
fn hits(text: &str, words: &[String]) -> u32 {
    let text = timeline_find::fold(text);
    words
        .iter()
        .filter(|word| text.contains(word.as_str()))
        .count() as u32
}

/// What `find_chats` gives of one chat.
fn chat_card(record: &Record, now: Timestamp) -> Value {
    json!({
        "chat": record.number,
        "title": record.title,
        "summary": record.summary,
        "age": age(now, record.active_at),
        "archived": record.archived,
    })
}

/// How long ago `at` was, in words.
fn age(now: Timestamp, at: Timestamp) -> String {
    let minutes = now.0.saturating_sub(at.0) / 60_000;
    let (count, unit) = match minutes {
        0 => return "just now".into(),
        1..60 => (minutes, "minute"),
        60..1_440 => (minutes / 60, "hour"),
        1_440..20_160 => (minutes / 1_440, "day"),
        _ => (minutes / 10_080, "week"),
    };
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{plural} ago")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_age_reads_in_the_largest_whole_unit() {
        let now = Timestamp(1_800_000_000_000);
        let ago = |minutes: u64| age(now, Timestamp(now.0 - minutes * 60_000));
        assert_eq!(ago(0), "just now");
        assert_eq!(ago(1), "1 minute ago");
        assert_eq!(ago(59), "59 minutes ago");
        assert_eq!(ago(60 * 3), "3 hours ago");
        assert_eq!(ago(1_440), "1 day ago");
        assert_eq!(ago(1_440 * 13), "13 days ago");
        assert_eq!(ago(1_440 * 21), "3 weeks ago");
        assert_eq!(age(now, Timestamp(now.0 + 5)), "just now");
    }

    #[test]
    fn a_hit_counts_each_word_once_in_any_case() {
        let words = vec!["login".to_owned(), "bug".to_owned()];
        assert_eq!(hits("Fixing the Login bug, the login", &words), 2);
        assert_eq!(hits("Release notes", &words), 0);
    }
}
