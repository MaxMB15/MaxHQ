// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The notes tools: an agent lists, reads, makes and edits its own
//! project's notes. They need no permission, since an agent reads its
//! project's files anyway. Each runs off the loop, as the notes read and
//! write SQLite.

use maxhq_core::tools::notes::{Edit, NoteCall};
use maxhq_protocol::ids::{AgentId, DeviceId, NoteId, ProjectId};
use maxhq_protocol::notes::{NoteAuthor, NoteSummary};
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use serde_json::json;

use super::agent_tools::{Calling, Later, Reply};
use super::{Input, Manager};
use crate::drive::ToolResult;
use crate::hub::Hub;
use crate::notes::{Notes, Save};

/// Why the notes tools refuse a caller outside a project.
const NO_PROJECT: &str = "This agent isn't in a project, so it has no notes.";

/// Why a note can't be found by what the caller named.
const NOT_FOUND: &str = "No note in this project has that id or title. list_notes lists them.";

/// Why a title names more than one note.
const SAME_TITLE: &str = "More than one note has that title. Name it by the id list_notes gives.";

/// Why a deleted note can't be read or edited.
const DELETED: &str = "That note was deleted.";

impl Manager {
    /// Runs a notes tool call for the caller's project.
    pub(super) fn note_tool(&mut self, reply: Reply, caller: Calling, call: NoteCall) {
        let Some((agent, project)) = caller
            .agent
            .and_then(|agent| Some((agent, self.project_of(agent)?)))
        else {
            return self.reply_now(reply, Err(NO_PROJECT.into()));
        };
        let notes = self.notes.clone();
        let hub = self.hub.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = run(&notes, &hub, project, agent, call)
                .await
                .map(ToolResult::Text);
            let later = Later::Answer {
                reply,
                result,
                release: None,
            };
            let _ = tx.send(Input::Agents(Box::new(later)));
        });
    }
}

/// Answers one notes call.
async fn run(
    notes: &Notes,
    hub: &Hub,
    project: ProjectId,
    agent: AgentId,
    call: NoteCall,
) -> Result<String, String> {
    let by = Some(NoteAuthor {
        device: notes.me(),
        agent: Some(agent),
    });
    match call {
        NoteCall::List => {
            let listed: Vec<_> = notes
                .summaries(project)
                .await?
                .into_iter()
                .filter(|note| !note.deleted)
                .map(|note| {
                    json!({
                        "id": note.note.to_string(),
                        "title": note.title,
                        "bytes": note.size,
                        "changed_at": when(note.at),
                        "changed_by": author(hub, note.by),
                    })
                })
                .collect();
            if listed.is_empty() {
                return Ok("This project has no notes yet.".into());
            }
            Ok(serde_json::to_string_pretty(&listed).unwrap_or_default())
        }
        NoteCall::Read { note } => {
            let found = find(notes, project, &note).await?;
            let version = notes.read(project, found.note, None).await?;
            Ok(format!(
                "Note {} \"{}\", last changed {} by {}:\n\n{}",
                version.note,
                version.title,
                when(version.at),
                author(hub, version.by),
                version.body
            ))
        }
        NoteCall::Create { title, body } => {
            let note = NoteId::generate();
            notes
                .save(Save {
                    project,
                    note,
                    base: None,
                    title,
                    body,
                    deleted: false,
                    at: Timestamp::now(),
                    by,
                })
                .await?;
            Ok(format!("Made the note {note}."))
        }
        NoteCall::Edit(edit) => {
            let found = find(notes, project, &edit.note).await?;
            let head = notes.read(project, found.note, None).await?;
            let body = Edit::apply(&edit, &head.body)?;
            let (_, merged) = notes
                .save(Save {
                    project,
                    note: head.note,
                    base: Some(head.hash),
                    title: head.title,
                    body,
                    deleted: false,
                    at: Timestamp::now(),
                    by,
                })
                .await?;
            Ok(if merged {
                format!(
                    "Edited the note {}. It changed at the same time elsewhere, and MaxHQ \
                     merged both changes; read it again before another edit.",
                    head.note
                )
            } else {
                format!("Edited the note {}.", head.note)
            })
        }
    }
}

/// The live note the caller named: by its id, or by a title only one live
/// note has.
async fn find(notes: &Notes, project: ProjectId, named: &str) -> Result<NoteSummary, String> {
    let all = notes.summaries(project).await?;
    if let Ok(id) = named.parse::<NoteId>()
        && let Some(note) = all.iter().find(|note| note.note == id)
    {
        return if note.deleted {
            Err(DELETED.into())
        } else {
            Ok(note.clone())
        };
    }
    let mut titled = all
        .into_iter()
        .filter(|note| !note.deleted && note.title == named);
    match (titled.next(), titled.next()) {
        (Some(note), None) => Ok(note),
        (Some(_), Some(_)) => Err(SAME_TITLE.into()),
        (None, _) => Err(NOT_FOUND.into()),
    }
}

/// A time as the tools write it.
fn when(at: Timestamp) -> String {
    i64::try_from(at.0)
        .ok()
        .and_then(|ms| jiff::Timestamp::from_millisecond(ms).ok())
        .map_or_else(|| at.0.to_string(), |at| at.to_string())
}

/// Who changed a note, as the tools write it.
fn author(hub: &Hub, by: Option<NoteAuthor>) -> String {
    let Some(by) = by else {
        return "someone unknown".into();
    };
    let device = device_name(hub, by.device);
    match by.agent {
        Some(agent) => format!("agent {agent} on {device}"),
        None => format!("the user on {device}"),
    }
}

/// A device's name, or its id when this device doesn't know it.
pub(super) fn device_name(hub: &Hub, device: DeviceId) -> String {
    match hub.get(&EntityKey::Device(device)) {
        Some(Entity::Device(found)) => found.name,
        _ => device.to_string(),
    }
}
