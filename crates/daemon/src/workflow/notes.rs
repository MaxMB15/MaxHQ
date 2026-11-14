// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What binding needs from agents' timelines, which live in the session
//! manager: each agent's first prompt and the commands it finished. Kept in
//! memory only, and never sent to the app.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use maxhq_protocol::ids::AgentId;
use maxhq_protocol::timeline::{ItemBody, ItemStatus, TimelineItem};

/// How much of a first prompt is kept.
pub const PROMPT_HEAD: usize = 4 * 1024;
/// How much of one command is kept.
const COMMAND_HEAD: usize = 4 * 1024;
/// How many commands are kept per agent, newest last.
const COMMANDS: usize = 200;

/// Each agent's prompt head and finished commands. Cheap to clone.
#[derive(Clone, Default)]
pub struct Notes(Arc<Mutex<HashMap<AgentId, Note>>>);

#[derive(Default)]
struct Note {
    prompt: Option<String>,
    commands: VecDeque<String>,
}

impl Notes {
    fn lock(&self) -> MutexGuard<'_, HashMap<AgentId, Note>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Takes in an item the agent's timeline just took. A first message
    /// sets the prompt head, and a finished command is kept.
    pub fn item(&self, agent: AgentId, item: &TimelineItem, finished: bool) {
        match &item.body {
            ItemBody::UserMessage { text, .. } => {
                let mut notes = self.lock();
                let note = notes.entry(agent).or_default();
                if note.prompt.is_none() {
                    note.prompt = Some(head(text, PROMPT_HEAD).to_owned());
                }
            }
            ItemBody::Command { command, .. } if finished => {
                let mut notes = self.lock();
                push(&mut notes.entry(agent).or_default().commands, command);
            }
            _ => {}
        }
    }

    /// Reads a whole timeline again, as after its history was replaced.
    pub fn history<'a>(&self, agent: AgentId, items: impl Iterator<Item = &'a TimelineItem>) {
        let mut note = Note::default();
        for item in items {
            match &item.body {
                ItemBody::UserMessage { text, .. } if note.prompt.is_none() => {
                    note.prompt = Some(head(text, PROMPT_HEAD).to_owned());
                }
                ItemBody::Command { command, .. }
                    if matches!(item.status, ItemStatus::Completed | ItemStatus::Failed) =>
                {
                    push(&mut note.commands, command);
                }
                _ => {}
            }
        }
        self.lock().insert(agent, note);
    }

    /// The first few kilobytes of the agent's first prompt.
    pub fn prompt_head(&self, agent: AgentId) -> Option<String> {
        self.lock().get(&agent).and_then(|n| n.prompt.clone())
    }

    /// Calls `visit` with each command the agent finished, oldest first.
    pub fn each_command(&self, agent: AgentId, visit: &mut dyn FnMut(&str)) {
        if let Some(note) = self.lock().get(&agent) {
            for command in &note.commands {
                visit(command);
            }
        }
    }

    /// Forgets the agents `keep` doesn't name.
    pub fn retain(&self, keep: impl Fn(AgentId) -> bool) {
        self.lock().retain(|agent, _| keep(*agent));
    }
}

fn push(commands: &mut VecDeque<String>, command: &str) {
    if commands.len() == COMMANDS {
        commands.pop_front();
    }
    commands.push_back(head(command, COMMAND_HEAD).to_owned());
}

/// At most `max` bytes of `text`, cut at a character.
fn head(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
pub(crate) mod tests {
    use maxhq_protocol::timeline::{Delivery, ItemId};

    use super::*;

    fn item(agent: AgentId, status: ItemStatus, body: ItemBody) -> TimelineItem {
        TimelineItem {
            id: ItemId("1".into()),
            agent,
            turn: None,
            parent: None,
            ordinal: 0,
            status,
            started_at: None,
            completed_at: None,
            body,
        }
    }

    fn said(agent: AgentId, text: &str) -> TimelineItem {
        let body = ItemBody::UserMessage {
            text: text.to_owned(),
            attachments: Vec::new(),
            refs: Vec::new(),
            delivery: Delivery::Queued,
        };
        item(agent, ItemStatus::Completed, body)
    }

    pub(crate) fn ran(agent: AgentId, command: &str, status: ItemStatus) -> TimelineItem {
        let body = ItemBody::Command {
            command: command.to_owned(),
            cwd: None,
            exit_code: None,
            output: String::new(),
            output_truncated: false,
            background: false,
        };
        item(agent, status, body)
    }

    fn commands(notes: &Notes, agent: AgentId) -> Vec<String> {
        let mut seen = Vec::new();
        notes.each_command(agent, &mut |c| seen.push(c.to_owned()));
        seen
    }

    #[test]
    fn the_first_prompt_is_kept_cut_at_a_character() {
        let notes = Notes::default();
        let agent = AgentId::generate();
        let long = format!("{}é and more", "x".repeat(PROMPT_HEAD - 1));
        notes.item(agent, &said(agent, &long), true);
        notes.item(agent, &said(agent, "a later message"), true);
        assert_eq!(notes.prompt_head(agent), Some("x".repeat(PROMPT_HEAD - 1)));
    }

    #[test]
    fn only_finished_commands_are_kept() {
        let notes = Notes::default();
        let agent = AgentId::generate();
        notes.item(agent, &ran(agent, "make plan", ItemStatus::Running), false);
        notes.item(agent, &ran(agent, "make plan", ItemStatus::Completed), true);
        assert_eq!(commands(&notes, agent), ["make plan"]);
    }

    #[test]
    fn a_replaced_history_is_read_again() {
        let notes = Notes::default();
        let agent = AgentId::generate();
        notes.item(agent, &said(agent, "old"), true);
        let items = [
            said(agent, "first"),
            ran(agent, "one", ItemStatus::Completed),
            ran(agent, "two", ItemStatus::Running),
            ran(agent, "three", ItemStatus::Failed),
        ];
        notes.history(agent, items.iter());
        assert_eq!(notes.prompt_head(agent).as_deref(), Some("first"));
        assert_eq!(commands(&notes, agent), ["one", "three"]);

        notes.retain(|_| false);
        assert_eq!(notes.prompt_head(agent), None);
    }
}
