// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The dispatcher's chat tools: it names the chat it's in, moves a message
//! that's about something else to another chat, and finds earlier chats.
//! Each chat runs its own agent, so a chat a message moves to answers it
//! with its own history.

use maxhq_protocol::ids::ProjectId;
use serde_json::{json, Value};

use super::agents::{flag, id, number, text};
use super::{fields, Call, Tool};

/// What the dispatcher reads about its chats, after
/// [`super::dispatch::INSTRUCTIONS`].
pub const INSTRUCTIONS: &str = "You also hold plain conversations, and each conversation \
    is a chat of its own. After your first answer in a chat, and again when its topic \
    shifts, call name_chat with a short title, a one line summary and the project it's \
    about, if one.\n\n\
    When a message is clearly about something other than this chat, call switch_chat: with \
    the `chat` number of an earlier chat on that topic, or with `new` and a title. MaxHQ \
    moves the message there and that chat answers it, so end your turn after switch_chat \
    without answering or saying anything. When you're unsure, stay and answer here. A \
    follow up, a question about your last answer or a request that builds on this chat \
    stays.\n\n\
    A chat's first message can start with a block that lists the user's recent chats, \
    newest first, each with its number, title, age and summary. If the message carries on \
    one of them, switch to it. To look further back, call find_chats with a few words: it \
    searches the titles, summaries and full text of every chat, archived ones too.\n\n\
    When the user asks you to save the chat to Notes, write a note with create_note: a \
    short title, and a Markdown body with what was asked, decided and found, as someone \
    reading it later needs it. A note belongs to a project, so name the chat's project with \
    name_chat first. If the chat isn't about one, ask the user which project.";

/// The most characters a chat's title may have.
pub const MAX_CHAT_TITLE: usize = 80;

/// The most characters a chat's summary may have.
pub const MAX_CHAT_SUMMARY: usize = 160;

/// The most characters a find_chats query may have.
pub const MAX_CHAT_QUERY: usize = 200;

/// A call to one of the chat tools.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatCall {
    /// Names the caller's chat.
    Name {
        /// Its title, at most [`MAX_CHAT_TITLE`] characters.
        title: String,
        /// What it's about, in one line, at most [`MAX_CHAT_SUMMARY`].
        summary: String,
        /// The project it's about, if one.
        project: Option<ProjectId>,
    },
    /// Moves the user's last message to another chat.
    Switch(SwitchTo),
    /// Finds earlier chats.
    Find {
        /// What to look for, trimmed.
        query: String,
    },
}

/// The chat a message moves to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwitchTo {
    /// An earlier chat, by its number.
    Chat(u32),
    /// A new chat with this title.
    New(String),
}

/// A short title for each of the chat tools.
pub(super) fn title(tool: Tool) -> &'static str {
    match tool {
        Tool::NameChat => "Name this chat",
        Tool::SwitchChat => "Move the message to another chat",
        Tool::FindChats => "Find earlier chats",
        _ => unreachable!("not a chat tool"),
    }
}

/// What the dispatcher reads about each of the chat tools.
pub(super) fn description(tool: Tool) -> &'static str {
    match tool {
        Tool::NameChat => {
            "Names the chat you're in, as the user's list of chats shows it: a short title, a \
             one line summary, and the project it's about by the id list_targets gives, if \
             one. Call it after your first answer and when the topic shifts. A title the user \
             gave stays, and only the summary and project change."
        }
        Tool::SwitchChat => {
            "Moves the user's last message to another chat, which answers it there with its \
             own history: an earlier chat by its `chat` number, or a new one with `new` and a \
             `title`. The user sees where it went and can keep it here instead. End your turn \
             after it without answering."
        }
        Tool::FindChats => {
            "Finds the user's earlier chats, archived ones too, by a few words: a title match \
             first, then a summary match, then the most matches in the full text. Gives up to \
             8, each with its number, title, summary and age. Move a message to one with \
             switch_chat."
        }
        _ => unreachable!("not a chat tool"),
    }
}

/// The JSON schema for each of the chat tools' arguments.
pub(super) fn input_schema(tool: Tool) -> Value {
    match tool {
        Tool::NameChat => json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "minLength": 1, "maxLength": MAX_CHAT_TITLE, "description": "A short title, in sentence case."},
                "summary": {"type": "string", "maxLength": MAX_CHAT_SUMMARY, "description": "What the chat is about, in one line."},
                "project": {"type": "string", "description": "The project's id, as list_targets gives it, if the chat is about one."},
            },
            "required": ["title", "summary"],
            "additionalProperties": false,
        }),
        Tool::SwitchChat => json!({
            "type": "object",
            "properties": {
                "chat": {"type": "integer", "minimum": 1, "description": "The earlier chat's number. Leave it out with `new`."},
                "new": {"type": "boolean", "description": "Start a new chat for the message."},
                "title": {"type": "string", "minLength": 1, "maxLength": MAX_CHAT_TITLE, "description": "With `new`, the new chat's title."},
            },
            "additionalProperties": false,
        }),
        Tool::FindChats => json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "minLength": 1, "maxLength": MAX_CHAT_QUERY, "description": "A few words to look for."},
            },
            "required": ["query"],
            "additionalProperties": false,
        }),
        _ => unreachable!("not a chat tool"),
    }
}

/// Checks a chat tool's arguments.
pub(super) fn parse(tool: Tool, arguments: &Value) -> Result<Call, String> {
    let args = fields(arguments)?;
    let call = match tool {
        Tool::NameChat => ChatCall::Name {
            title: one_line(
                text(&args, "title")?.ok_or("Give a `title`.")?,
                MAX_CHAT_TITLE,
            ),
            summary: one_line(
                text(&args, "summary")?.unwrap_or_default(),
                MAX_CHAT_SUMMARY,
            ),
            project: id(&args, "project")?,
        },
        Tool::SwitchChat => {
            let chat = number(&args, "chat", 1, u64::from(u32::MAX))?;
            let new = flag(&args, "new")?.unwrap_or(false);
            ChatCall::Switch(match (chat, new) {
                (Some(_), true) => return Err("Give `chat` or `new`, not both.".into()),
                (Some(chat), false) => SwitchTo::Chat(chat as u32),
                (None, true) => SwitchTo::New(one_line(
                    text(&args, "title")?.ok_or("Give the new chat a `title`.")?,
                    MAX_CHAT_TITLE,
                )),
                (None, false) => {
                    return Err("Give a `chat` number, or `new` with a `title`.".into())
                }
            })
        }
        Tool::FindChats => {
            let query = text(&args, "query")?.ok_or("Give a `query`.")?;
            if query.chars().count() > MAX_CHAT_QUERY {
                return Err(format!(
                    "`query` is longer than {MAX_CHAT_QUERY} characters."
                ));
            }
            ChatCall::Find { query }
        }
        _ => unreachable!("not a chat tool"),
    };
    Ok(Call::Chat(call))
}

/// The first line of `text`, at most `max` characters.
pub fn one_line(text: String, max: usize) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    line.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_keeps_one_line_of_each() {
        let project = ProjectId::derive("p");
        let long = "t".repeat(MAX_CHAT_TITLE + 5);
        assert_eq!(
            Tool::NameChat.parse(&json!({"title": format!("{long}\nmore"), "summary": " Fixing the login \n bug", "project": project.to_string()})),
            Ok(Call::Chat(ChatCall::Name {
                title: "t".repeat(MAX_CHAT_TITLE),
                summary: "Fixing the login".into(),
                project: Some(project),
            }))
        );
        assert!(Tool::NameChat.parse(&json!({"summary": "s"})).is_err());
    }

    #[test]
    fn a_switch_names_a_chat_or_a_new_one() {
        assert_eq!(
            Tool::SwitchChat.parse(&json!({"chat": 4})),
            Ok(Call::Chat(ChatCall::Switch(SwitchTo::Chat(4))))
        );
        assert_eq!(
            Tool::SwitchChat.parse(&json!({"new": true, "title": "Release notes"})),
            Ok(Call::Chat(ChatCall::Switch(SwitchTo::New(
                "Release notes".into()
            ))))
        );
        for (args, says) in [
            (json!({"chat": 4, "new": true}), "not both"),
            (json!({"new": true}), "`title`"),
            (json!({}), "`chat`"),
            (json!({"chat": 0}), "`chat`"),
        ] {
            let refused = Tool::SwitchChat.parse(&args).expect_err("refused");
            assert!(refused.contains(says), "{refused}");
        }
    }

    #[test]
    fn a_find_takes_a_short_query() {
        assert_eq!(
            Tool::FindChats.parse(&json!({"query": " login bug "})),
            Ok(Call::Chat(ChatCall::Find {
                query: "login bug".into()
            }))
        );
        let long = "q".repeat(MAX_CHAT_QUERY + 1);
        assert!(Tool::FindChats.parse(&json!({"query": long})).is_err());
    }

    #[test]
    fn the_chat_tools_are_the_dispatchers_alone() {
        for tool in [Tool::NameChat, Tool::SwitchChat, Tool::FindChats] {
            assert!(Tool::DISPATCHER.contains(&tool));
            assert!(!Tool::ALL.contains(&tool));
            assert!(!tool.for_key() && !tool.runs_agents());
            assert_eq!(Tool::named(tool.name()), Some(tool));
        }
        let instructions = super::super::dispatch::instructions();
        assert!(instructions.starts_with(super::super::dispatch::INSTRUCTIONS));
        assert!(instructions.ends_with(INSTRUCTIONS));
    }
}
