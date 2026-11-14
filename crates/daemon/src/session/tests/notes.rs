// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The notes tools: an agent lists, reads, makes and edits its project's
//! notes without a permission, and an agent outside a project has none.

use maxhq_protocol::ids::{NoteId, ProjectId};
use maxhq_protocol::model::Workspace;
use maxhq_protocol::notes::NoteAuthor;
use maxhq_protocol::time::Timestamp;
use serde_json::{json, Value};

use super::*;
use crate::notes::{Notes, Save};

impl Harness {
    /// The project of an agent's workspace.
    fn project_of(&self, agent: AgentId) -> ProjectId {
        match self
            .hub
            .get(&EntityKey::Workspace(self.agent(agent).workspace))
        {
            Some(Entity::Workspace(workspace)) => workspace.project.expect("in a project"),
            other => panic!("no workspace: {other:?}"),
        }
    }

    /// The notes on this device, as the user's app reaches them.
    fn notes(&self) -> Notes {
        Notes::new(self.db.clone(), self.hub.clone())
    }
}

/// The note id a `create_note` answer gives.
fn made(answer: &str) -> NoteId {
    answer
        .trim_start_matches("Made the note ")
        .trim_end_matches('.')
        .parse()
        .unwrap_or_else(|_| panic!("no id in {answer:?}"))
}

#[tokio::test]
async fn an_agent_makes_reads_and_edits_its_projects_notes_without_a_permission() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let project = h.project_of(agent);

    let answer = h
        .call(session, agent, "k1", "list_notes", json!({}))
        .await
        .unwrap();
    assert_eq!(answer, "This project has no notes yet.");

    let answer = h
        .call(
            session,
            agent,
            "k2",
            "create_note",
            json!({"title": "Plan", "body": "one\ntwo\n"}),
        )
        .await
        .unwrap();
    let note = made(&answer);

    let listed: Value = serde_json::from_str(
        &h.call(session, agent, "k3", "list_notes", json!({}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(listed[0]["id"], note.to_string());
    assert_eq!(listed[0]["title"], "Plan");
    assert_eq!(listed[0]["bytes"], 8);
    assert!(listed[0]["changed_by"]
        .as_str()
        .unwrap()
        .starts_with(&format!("agent {agent} on ")));

    // By its title, or by its id.
    let read = h
        .call(session, agent, "k4", "read_note", json!({"note": "Plan"}))
        .await
        .unwrap();
    assert!(read.ends_with("one\ntwo\n"), "{read}");
    h.call(
        session,
        agent,
        "k5",
        "read_note",
        json!({"note": note.to_string()}),
    )
    .await
    .unwrap();

    let answer = h
        .call(
            session,
            agent,
            "k6",
            "edit_note",
            json!({"note": "Plan", "old_string": "two", "new_string": "TWO"}),
        )
        .await
        .unwrap();
    assert_eq!(answer, format!("Edited the note {note}."));
    let head = h.notes().read(project, note, None).await.unwrap();
    assert_eq!(head.body, "one\nTWO\n");
    assert_eq!(
        head.by,
        Some(NoteAuthor {
            device: h.hub.device().id,
            agent: Some(agent),
        })
    );
}

#[tokio::test]
async fn an_edit_that_doesnt_match_once_is_refused_with_the_count() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    h.call(
        session,
        agent,
        "k1",
        "create_note",
        json!({"title": "Plan", "body": "a a a"}),
    )
    .await
    .unwrap();
    let many = h
        .call(
            session,
            agent,
            "k2",
            "edit_note",
            json!({"note": "Plan", "old_string": "a", "new_string": "b"}),
        )
        .await
        .expect_err("refused");
    assert!(many.contains("3 times"), "{many}");
    let none = h
        .call(
            session,
            agent,
            "k3",
            "edit_note",
            json!({"note": "Plan", "old_string": "z", "new_string": "b"}),
        )
        .await
        .expect_err("refused");
    assert!(none.contains("isn't in the note"), "{none}");
    h.call(
        session,
        agent,
        "k4",
        "edit_note",
        json!({"note": "Plan", "old_string": "a", "new_string": "b", "replace_all": true}),
    )
    .await
    .unwrap();
    let read = h
        .call(session, agent, "k5", "read_note", json!({"note": "Plan"}))
        .await
        .unwrap();
    assert!(read.ends_with("b b b"), "{read}");
}

#[tokio::test]
async fn an_edit_merges_with_a_change_made_since_it_read() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let project = h.project_of(agent);
    let note = made(
        &h.call(
            session,
            agent,
            "k1",
            "create_note",
            json!({"title": "Plan", "body": "one\ntwo\nthree\n"}),
        )
        .await
        .unwrap(),
    );
    // The user changes the first line while the agent works.
    let notes = h.notes();
    let head = notes.read(project, note, None).await.unwrap();
    notes
        .save(Save {
            project,
            note,
            base: Some(head.hash),
            title: "Plan".into(),
            body: "ONE\ntwo\nthree\n".into(),
            deleted: false,
            at: Timestamp::now(),
            by: None,
        })
        .await
        .unwrap();
    // The agent's edit reads the head again, so it lands on the change.
    h.call(
        session,
        agent,
        "k2",
        "edit_note",
        json!({"note": note.to_string(), "old_string": "three", "new_string": "THREE"}),
    )
    .await
    .unwrap();
    assert_eq!(
        notes.read(project, note, None).await.unwrap().body,
        "ONE\ntwo\nTHREE\n"
    );
}

#[tokio::test]
async fn a_title_two_notes_share_or_a_deleted_note_is_refused() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let project = h.project_of(agent);
    for key in ["k1", "k2"] {
        h.call(
            session,
            agent,
            key,
            "create_note",
            json!({"title": "Plan", "body": key}),
        )
        .await
        .unwrap();
    }
    let both = h
        .call(session, agent, "k3", "read_note", json!({"note": "Plan"}))
        .await
        .expect_err("refused");
    assert!(both.contains("More than one note"), "{both}");

    let notes = h.notes();
    let first = notes.summaries(project).await.unwrap()[0].clone();
    notes
        .save(Save {
            project,
            note: first.note,
            base: Some(first.head),
            title: "Plan".into(),
            body: String::new(),
            deleted: true,
            at: Timestamp::now(),
            by: None,
        })
        .await
        .unwrap();
    let gone = h
        .call(
            session,
            agent,
            "k4",
            "read_note",
            json!({"note": first.note.to_string()}),
        )
        .await
        .expect_err("refused");
    assert!(gone.contains("deleted"), "{gone}");
    // The one left is found by its title, and a tombstone isn't listed.
    h.call(session, agent, "k5", "read_note", json!({"note": "Plan"}))
        .await
        .unwrap();
    let listed: Value = serde_json::from_str(
        &h.call(session, agent, "k6", "list_notes", json!({}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn an_agent_outside_a_project_has_no_notes() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let Some(Entity::Workspace(workspace)) =
        h.hub.get(&EntityKey::Workspace(h.agent(agent).workspace))
    else {
        panic!("the agent has no workspace");
    };
    h.keep(Entity::Workspace(Workspace {
        project: None,
        ..workspace
    }));
    let answer = h
        .call(session, agent, "k1", "list_notes", json!({}))
        .await
        .expect_err("refused");
    assert_eq!(answer, "This agent isn't in a project, so it has no notes.");
}
