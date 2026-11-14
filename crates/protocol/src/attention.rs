// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Things an agent needs from you, and your answers.
//!
//! The desktop app, the phone and the CLI's own prompt can all answer the same
//! event. The first answer wins, and the others get [`AnswerOutcome::AlreadyAnswered`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::{AgentId, AttentionId, ClientId};
use crate::time::Timestamp;
use crate::timeline::QuestionAnswer;

/// `text` with what it would hide spelled out, as in `[U+202E]`: control characters
/// other than a line break or a tab, soft hyphens, zero-width and direction marks, and
/// tag characters. Those can change what a prompt seems to say without showing anything.
pub fn show_hidden(text: &str) -> String {
    let mut shown = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\u{0}'..='\u{8}'
            | '\u{b}'..='\u{1f}'
            | '\u{7f}'..='\u{9f}'
            | '\u{ad}'
            | '\u{61c}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{e0000}'..='\u{e007f}' => {
                shown.push_str(&format!("[U+{:04X}]", c as u32));
            }
            c => shown.push(c),
        }
    }
    shown
}

/// What kind of attention an agent needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    /// Asks to run a tool, such as a shell command or a file edit.
    Permission,
    /// Asks you a question.
    Question,
    /// Finished a turn.
    Done,
    /// Stopped on an error.
    Error,
}

/// One answer you can pick.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct AttentionOption {
    /// Sent back in [`AttentionAnswer::Option`].
    pub id: String,
    /// Shown on the button.
    pub label: String,
    /// What the agent says this option does, where it says: `allow_once`,
    /// `allow_always`, `reject_once` or `reject_always`. It decides over the
    /// label, which can say anything. An older peer leaving it out reads as
    /// not said, and the id and label decide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub kind: Option<String>,
}

/// Something an agent needs from you.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct AttentionEvent {
    /// Stable, so a notification can open the event days later.
    pub id: AttentionId,
    /// The agent asking.
    pub agent: AgentId,
    /// What it's asking for.
    pub kind: AttentionKind,
    /// One line for the notification and the inbox.
    pub summary: String,
    /// The answers you can pick, in order.
    pub options: Vec<AttentionOption>,
    /// Whether a typed answer is allowed as well as the options.
    pub accepts_text: bool,
    /// When the agent asked.
    pub created_at: Timestamp,
    /// When the question stops mattering, such as when the CLI's own prompt
    /// times out.
    pub expires_at: Option<Timestamp>,
}

/// Your answer to an event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AttentionAnswer {
    /// Picks one of the event's options.
    Option {
        /// An [`AttentionOption::id`] from the event.
        id: String,
    },
    /// A typed answer.
    Text {
        /// What you typed.
        text: String,
    },
    /// Answers to an agent's questions, as a
    /// [`crate::timeline::ItemBody::Question`] asks them.
    Questions {
        /// One for each question you answered.
        answers: Vec<QuestionAnswer>,
    },
    /// Turns the request down without an answer and ends the turn, as Esc
    /// does in the CLI, so a message you queued starts the next turn. Its
    /// item ends [`crate::timeline::ItemStatus::Declined`] with nothing
    /// decided or answered. A `maxd` from before
    /// [`crate::message::DISMISS_SINCE`] can't read it.
    Dismiss,
}

/// What happened to an answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AnswerOutcome {
    /// Yours was first, and the agent has it.
    Accepted,
    /// Someone answered first.
    AlreadyAnswered {
        /// When the first answer arrived.
        answered_at: Timestamp,
        /// The client that sent it.
        answered_by: ClientId,
    },
    /// The event expired before the answer arrived.
    Expired,
    /// `maxd` has no event with that ID.
    UnknownEvent,
    /// An outcome from a newer `maxd`. The answer arrived, and what it came
    /// to needs a newer app to show.
    Unknown,
}
