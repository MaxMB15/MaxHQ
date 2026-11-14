// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The notes tools' arguments: what each takes, its checks, and how an
//! edit changes a note's body. Each acts on the notes of the calling
//! agent's own project, and none needs a permission.

use maxhq_protocol::notes::{MAX_NOTE_BODY, MAX_NOTE_TITLE};
use serde_json::{json, Map, Value};

use super::agents::text;
use super::{fields, Call, Tool};

/// A call to one of the notes tools.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoteCall {
    /// List the project's notes.
    List,
    /// Read a note.
    Read {
        /// The note, by its id or its title.
        note: String,
    },
    /// Make a note.
    Create {
        /// Its title, trimmed.
        title: String,
        /// Its body, as given.
        body: String,
    },
    /// Change a note's body.
    Edit(Edit),
}

/// A change to a note's body: `old_string` replaced with `new_string`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    /// The note, by its id or its title.
    pub note: String,
    /// The text to replace, exactly as the body has it.
    pub old_string: String,
    /// What replaces it.
    pub new_string: String,
    /// Whether every match is replaced, not just the one.
    pub replace_all: bool,
}

impl Edit {
    /// The body with the edit made. It's refused when `old_string` isn't
    /// in the body, or is there more than once without `replace_all`.
    pub fn apply(&self, body: &str) -> Result<String, String> {
        let found = body.matches(self.old_string.as_str()).count();
        match found {
            0 => Err(
                "`old_string` isn't in the note. Read it again and copy the text exactly.".into(),
            ),
            1 => Ok(body.replacen(&self.old_string, &self.new_string, 1)),
            _ if self.replace_all => Ok(body.replace(&self.old_string, &self.new_string)),
            found => Err(format!(
                "`old_string` is in the note {found} times. Give more of the text around it so \
                 it's found once, or set `replace_all`."
            )),
        }
    }
}

/// A short title for each notes tool.
pub(super) fn title(tool: Tool) -> &'static str {
    match tool {
        Tool::ListNotes => "List notes",
        Tool::ReadNote => "Read a note",
        Tool::CreateNote => "Create a note",
        Tool::EditNote => "Edit a note",
        _ => unreachable!("not a notes tool"),
    }
}

/// What the agent reads about each notes tool.
pub(super) fn description(tool: Tool) -> &'static str {
    match tool {
        Tool::ListNotes => {
            "Lists your project's notes: each one's id, title and size, and when and by whom \
             it last changed. The user and the project's agents share these notes on every \
             device."
        }
        Tool::ReadNote => {
            "Reads one of your project's notes in Markdown. Name it by the id list_notes \
             gives, or by its title when only one note has it."
        }
        Tool::CreateNote => {
            "Makes a note in your project with a title and a Markdown body, and gives its id. \
             The user sees it with the project's notes."
        }
        Tool::EditNote => {
            "Changes one of your project's notes: replaces `old_string` with `new_string` in \
             its body. `old_string` must be in the note exactly once, unless `replace_all` is \
             set to replace each one. Read the note first. If someone changed it at the same \
             time, MaxHQ merges both changes."
        }
        _ => unreachable!("not a notes tool"),
    }
}

/// The JSON schema for each notes tool's arguments.
pub(super) fn input_schema(tool: Tool) -> Value {
    let note = json!({
        "type": "string",
        "minLength": 1,
        "description": "The note's id, as list_notes gives it, or its title when only one note has it.",
    });
    let (properties, required) = match tool {
        Tool::ListNotes => (json!({}), json!([])),
        Tool::ReadNote => (json!({"note": note}), json!(["note"])),
        Tool::CreateNote => (
            json!({
                "title": {"type": "string", "minLength": 1, "maxLength": MAX_NOTE_TITLE, "description": "The note's title."},
                "body": {"type": "string", "description": "The note's body, in Markdown."},
            }),
            json!(["title", "body"]),
        ),
        Tool::EditNote => (
            json!({
                "note": note,
                "old_string": {"type": "string", "minLength": 1, "description": "The text to replace, exactly as the note has it."},
                "new_string": {"type": "string", "description": "The text to put in its place."},
                "replace_all": {"type": "boolean", "description": "Replace every match, not just the one. False when left out."},
            }),
            json!(["note", "old_string", "new_string"]),
        ),
        _ => unreachable!("not a notes tool"),
    };
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

/// Checks a notes tool's arguments.
pub(super) fn parse(tool: Tool, arguments: &Value) -> Result<Call, String> {
    let args = fields(arguments)?;
    let note = || text(&args, "note")?.ok_or_else(|| "Give a `note`.".to_owned());
    let call = match tool {
        Tool::ListNotes => NoteCall::List,
        Tool::ReadNote => NoteCall::Read { note: note()? },
        Tool::CreateNote => {
            let title = text(&args, "title")?.ok_or("Give a `title`.")?;
            if title.chars().count() > MAX_NOTE_TITLE {
                return Err(format!(
                    "`title` is longer than {MAX_NOTE_TITLE} characters."
                ));
            }
            let body = raw(&args, "body")?.unwrap_or_default();
            if body.len() > MAX_NOTE_BODY {
                return Err(format!("`body` is longer than {MAX_NOTE_BODY} bytes."));
            }
            NoteCall::Create { title, body }
        }
        Tool::EditNote => {
            let note = note()?;
            let old_string = raw(&args, "old_string")?
                .filter(|old| !old.is_empty())
                .ok_or("Give an `old_string`.")?;
            let new_string = raw(&args, "new_string")?.ok_or("Give a `new_string`.")?;
            if old_string == new_string {
                return Err("`old_string` and `new_string` are the same.".into());
            }
            let replace_all = super::agents::flag(&args, "replace_all")?.unwrap_or(false);
            NoteCall::Edit(Edit {
                note,
                old_string,
                new_string,
                replace_all,
            })
        }
        _ => unreachable!("not a notes tool"),
    };
    Ok(Call::Note(call))
}

/// A text argument as given, spaces and all, or none when left out.
fn raw(args: &Map<String, Value>, name: &str) -> Result<Option<String>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(format!("`{name}` must be text.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(old: &str, new: &str, replace_all: bool) -> Edit {
        Edit {
            note: "Plan".into(),
            old_string: old.into(),
            new_string: new.into(),
            replace_all,
        }
    }

    #[test]
    fn the_notes_tools_are_listed_for_agents_and_keys() {
        for tool in Tool::NOTES {
            assert_eq!(Tool::named(tool.name()), Some(tool));
            assert!(Tool::ALL.contains(&tool));
            assert!(tool.for_key() && !tool.runs_agents());
        }
    }

    #[test]
    fn each_notes_tool_checks_its_arguments() {
        assert_eq!(
            Tool::ListNotes.parse(&Value::Null),
            Ok(Call::Note(NoteCall::List))
        );
        assert_eq!(
            Tool::ReadNote.parse(&json!({"note": " Plan "})),
            Ok(Call::Note(NoteCall::Read {
                note: "Plan".into()
            }))
        );
        assert!(Tool::ReadNote
            .parse(&json!({}))
            .expect_err("refused")
            .contains("`note`"));
        assert_eq!(
            Tool::CreateNote.parse(&json!({"title": "Plan", "body": "  indented\n"})),
            Ok(Call::Note(NoteCall::Create {
                title: "Plan".into(),
                body: "  indented\n".into(),
            }))
        );
        assert!(Tool::CreateNote
            .parse(&json!({"title": "x".repeat(MAX_NOTE_TITLE + 1), "body": ""}))
            .is_err());
        assert_eq!(
            Tool::EditNote.parse(&json!({"note": "Plan", "old_string": " a", "new_string": ""})),
            Ok(Call::Note(NoteCall::Edit(edit(" a", "", false))))
        );
        assert!(Tool::EditNote
            .parse(&json!({"note": "Plan", "old_string": "", "new_string": "b"}))
            .expect_err("refused")
            .contains("`old_string`"));
        assert!(Tool::EditNote
            .parse(&json!({"note": "Plan", "old_string": "a", "new_string": "a"}))
            .is_err());
    }

    #[test]
    fn an_edit_replaces_one_match_or_says_how_many_it_found() {
        assert_eq!(edit("b", "x", false).apply("abc"), Ok("axc".into()));
        assert!(edit("z", "x", false)
            .apply("abc")
            .expect_err("refused")
            .contains("isn't in the note"));
        assert!(edit("a", "x", false)
            .apply("a a a")
            .expect_err("refused")
            .contains("3 times"));
        assert_eq!(edit("a", "x", true).apply("a a a"), Ok("x x x".into()));
    }
}
