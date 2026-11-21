// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The notes tools: an agent lists, reads, makes and edits its own
//! project's notes. They need no permission, since an agent reads its
//! project's files anyway. An agent in a container is the exception: the
//! notes are shared with agents outside it, so its first notes call asks
//! you, as its permission's Containers setting says, and Allow covers the
//! rest of its run. Each call runs off the loop, as the notes read and
//! write SQLite.

use maxhq_core::tools::notes::{Edit, NoteCall};
use maxhq_core::tools::Call;
use maxhq_protocol::agents::{LeaveContainer, Permission, ToolCall};
use maxhq_protocol::ids::{AgentId, DeviceId, NoteId, ProjectId, WorkspaceId};
use maxhq_protocol::notes::{NoteAuthor, NoteSummary};
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::Question;
use serde_json::json;

use super::agent_tools::{Calling, Later, Reply};
use super::consent::{consent_card, inbox_line, Pending, MISLED};
use super::{Input, Manager};
use crate::drive::codex::requests::visible;
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

/// Why an agent in a container whose permission keeps it in can't use the
/// notes.
const KEPT_IN: &str = "You run in a container, and your permission doesn't let you reach outside \
                       it. The project's notes are shared with agents outside the container, so \
                       you can't use them. Ask the user to allow more.";

/// What a notes call reads when you say no.
pub(super) const NOTES_DENIED: &str = "The user didn't allow you to use the project's notes, \
                                       which are shared outside your container, so this call \
                                       didn't go.";

/// What a notes call reads when nobody answered in time.
pub(super) const NOTES_UNANSWERED: &str = "The user didn't answer in time, so this call to the \
                                           project's notes, which are shared outside your \
                                           container, didn't go.";

/// What a notes card asks.
const NOTES_TITLE: &str = "Let this container agent use the project's notes?";

/// What Allow and Deny do on a notes card.
const NOTES_ANSWERS: [&str; 2] = [
    "Lets it use the project's notes for the rest of this run.",
    "Refuses this call. Its next notes call asks again.",
];

/// What could go wrong when an agent in a container uses the notes.
fn notes_risk() -> String {
    format!(
        "The project's notes are shared with you and with agents outside the container, here \
         and on your other devices. {MISLED} it could send them out through its internet \
         access, and agents outside the container read what it writes."
    )
}

impl Manager {
    /// Runs a notes call from the agent tools. An agent in a container goes
    /// first by its permission's Containers setting, unless you let it use
    /// the notes for this run already: Always allow lets it through, Never
    /// refuses, and Ask holds the call for your answer.
    pub(super) fn notes_call(
        &mut self,
        reply: Reply,
        caller: Calling,
        call: NoteCall,
        raw: ToolCall,
    ) {
        let asks = caller.container.is_some()
            && (caller.agent).is_some_and(|agent| !self.agent_tools.notes.contains(&agent));
        if let Some(project) = (caller.agent)
            .filter(|_| asks)
            .and_then(|agent| self.note_project(agent))
        {
            let leave = (caller.permission)
                .and_then(|id| self.agent_tools.permissions.get(&id))
                .map_or(LeaveContainer::Ask, |permission| permission.leave_container);
            match leave {
                LeaveContainer::Allow => {}
                LeaveContainer::Never => return self.reply_now(reply, Err(KEPT_IN.into())),
                LeaveContainer::Ask => {
                    let asked = self.note_asked(project, &call);
                    let pending = Pending {
                        reply,
                        caller,
                        call: Call::Note(call),
                        raw,
                    };
                    return self.ask_for_notes(pending, |this, feed, permission| {
                        this.notes_question(feed, caller.container, project, asked, permission)
                    });
                }
            }
        }
        self.note_tool(reply, caller, call);
    }

    /// Runs a notes tool call for the caller's project.
    pub(super) fn note_tool(&mut self, reply: Reply, caller: Calling, call: NoteCall) {
        let Some((agent, project)) = caller
            .agent
            .and_then(|agent| Some((agent, self.note_project(agent)?)))
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

    /// The project whose notes an agent's calls reach.
    fn note_project(&self, agent: AgentId) -> Option<ProjectId> {
        self.project_of(agent).or_else(|| self.chat_project(agent))
    }

    /// A notes card's inbox line and its question, for the agent whose
    /// `feed` asks, in `container`, wanting to do `asked`.
    fn notes_question(
        &self,
        feed: AgentId,
        container: Option<(DeviceId, WorkspaceId)>,
        project: ProjectId,
        (wants, call): (String, String),
        permission: Option<&Permission>,
    ) -> (String, Question) {
        let title = self.agent_title(feed);
        let container = self.container_named(container);
        let opening = format!("{title} runs in the container {container} and asks to {wants}.");
        let details = vec![
            ("Agent", title.clone()),
            ("Project", self.project_named(project)),
            ("Container", container),
            ("Call", call),
        ];
        let remember = permission.and_then(|permission| self.remember_box(feed, permission));
        let question = consent_card(
            NOTES_TITLE,
            &opening,
            &notes_risk(),
            details,
            NOTES_ANSWERS,
            remember,
        );
        let summary = inbox_line(&format!("Let {title} use the project's notes?"));
        (summary, question)
    }

    /// What a notes call asks to do, after "asks to", and the call as a
    /// card's detail, such as `Read note: Release plan`.
    fn note_asked(&self, project: ProjectId, call: &NoteCall) -> (String, String) {
        let (verb, detail, note) = match call {
            NoteCall::List => {
                return ("list the project's notes".into(), "List notes".into());
            }
            NoteCall::Read { note } => ("read", "Read", self.note_named(project, note)),
            NoteCall::Create { title, .. } => ("create", "Create", visible(title)),
            NoteCall::Edit(edit) => ("edit", "Edit", self.note_named(project, &edit.note)),
        };
        (
            format!("{verb} the note {note}"),
            format!("{detail} note: {note}"),
        )
    }

    /// A note as a card names it: its title when the agent named it by the
    /// id of one this device holds, and else what the agent named.
    fn note_named(&self, project: ProjectId, named: &str) -> String {
        let key = EntityKey::ProjectNotes {
            project,
            device: self.device,
        };
        let title = named
            .trim()
            .parse::<NoteId>()
            .ok()
            .and_then(|id| match self.hub.get(&key) {
                Some(Entity::ProjectNotes(held)) => held
                    .notes
                    .into_iter()
                    .find(|note| note.note == id)
                    .map(|note| note.title),
                _ => None,
            });
        visible(title.as_deref().unwrap_or(named))
    }

    /// A project's name, for a card.
    fn project_named(&self, project: ProjectId) -> String {
        match self.hub.get(&EntityKey::Project(project)) {
            Some(Entity::Project(found)) => visible(&found.name),
            _ => project.to_string(),
        }
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
