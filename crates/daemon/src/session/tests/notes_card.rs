// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! An agent in a container and the project's notes: its first notes call
//! asks you, Allow covers the rest of its run, Deny refuses only that call,
//! and an agent on the host never asks.

use maxhq_core::tools::agents::WAIT as CARD_WAIT;
use maxhq_protocol::agents::LeaveContainer;
use maxhq_protocol::ids::NoteId;
use maxhq_protocol::model::Project;
use maxhq_protocol::timeline::Consent;
use serde_json::{json, Value};

use super::restarts::server;
use super::*;
use crate::session::consent::{ALLOW, DENY, REMEMBER, REMEMBER_LABEL};
use crate::session::note_tools::{NOTES_DENIED, NOTES_UNANSWERED};

/// What a call Never refuses reads, in part.
const KEPT_IN: &str = "The project's notes are shared with agents outside the container";

/// The project the container is in.
fn shop() -> ProjectId {
    ProjectId::derive("shop")
}

/// A container in the project Shop, with no agents yet.
fn in_shop() -> Harness {
    let mut h = Harness::on(server());
    h.keep(Entity::Project(Project {
        id: shop(),
        name: "Shop".into(),
        repo: None,
        default_vendor: None,
        default_account: None,
        default_workspace: None,
    }));
    h.container(Some(shop()));
    h
}

/// The note id a `create_note` answer gives.
fn made(answer: &str) -> NoteId {
    answer
        .trim_start_matches("Made the note ")
        .trim_end_matches('.')
        .parse()
        .unwrap_or_else(|_| panic!("no id in {answer:?}"))
}

impl Harness {
    /// The notes cards open now.
    fn notes_cards(&self) -> Vec<AttentionEvent> {
        self.hub
            .snapshot()
            .entities
            .into_iter()
            .filter_map(|entity| match entity {
                Entity::Attention(event) if event.summary.contains("use the project's notes") => {
                    Some(event)
                }
                _ => None,
            })
            .collect()
    }

    /// Waits for the one notes card there should be.
    async fn notes_card(&self) -> AttentionEvent {
        eventually("a notes card", || !self.notes_cards().is_empty()).await;
        let cards = self.notes_cards();
        assert_eq!(cards.len(), 1, "{cards:#?}");
        cards.into_iter().next().expect("there's one")
    }

    /// Waits for every notes card to close.
    async fn notes_closed(&self) {
        eventually("the notes card closes", || self.notes_cards().is_empty()).await;
    }
}

/// A card's fact, by its label.
fn fact(consent: &Consent, label: &str) -> Option<String> {
    (consent.facts.iter())
        .find(|fact| fact.label == label)
        .map(|fact| fact.value.clone())
}

#[tokio::test]
async fn the_first_notes_call_asks_and_allow_covers_the_rest_of_the_run() {
    let mut h = in_shop();
    let (lead, session) = h.start_boxed_claude(Some("Plan it")).await;
    let create = json!({"title": "Release plan", "body": "ship\n"});
    h.ask_in(session, lead, "n1", "create_note", create).await;
    let card = h.notes_card().await;
    assert!(!h.replied("n1"), "the call waits for the card");

    let question = h.reach_question(lead).await;
    let labels: Vec<&str> = question.options.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(labels, [ALLOW, DENY]);
    let consent = question.consent.expect("the card names the risk");
    assert_eq!(
        consent.title,
        "Let this container agent use the project's notes?"
    );
    for said in ["other devices", "internet access", "read what it writes"] {
        assert!(consent.risk.contains(said), "{said}: {}", consent.risk);
    }
    assert_eq!(fact(&consent, "Project").as_deref(), Some("Shop"));
    assert_eq!(fact(&consent, "Container").as_deref(), Some("mhq-box"));
    assert_eq!(
        fact(&consent, "Call").as_deref(),
        Some("Create note: Release plan")
    );
    assert!(fact(&consent, "Agent").is_some(), "{:#?}", consent.facts);
    assert_eq!(
        consent.remember, None,
        "it holds no permission to keep it on"
    );

    assert_eq!(
        h.pick_reach(card.id, ALLOW, false).await,
        AnswerOutcome::Accepted
    );
    let note = made(&h.answer("n1").await.expect("the note is made"));
    h.notes_closed().await;

    // Calls of other kinds go without asking for the rest of the run.
    let listed = h
        .call_in(session, lead, "n2", "list_notes", json!({}))
        .await
        .expect("the list goes");
    let listed: Value = serde_json::from_str(&listed).unwrap();
    assert_eq!(listed[0]["title"], "Release plan");
    let read = json!({"note": note.to_string()});
    let read = h
        .call_in(session, lead, "n3", "read_note", read)
        .await
        .expect("the read goes");
    assert!(read.ends_with("ship\n"), "{read}");
    let edit = json!({"note": "Release plan", "old_string": "ship", "new_string": "ship it"});
    h.call_in(session, lead, "n4", "edit_note", edit)
        .await
        .expect("the edit goes");
    assert!(h.notes_cards().is_empty(), "Allow asks nothing more");

    // Another agent in the same container asks for itself, and the card
    // names the note it reads by id by its title.
    let (beside, beside_session) = h.start_boxed_claude(Some("Beside it")).await;
    let read = json!({"note": note.to_string()});
    h.ask_in(beside_session, beside, "o1", "read_note", read)
        .await;
    h.notes_card().await;
    assert!(!h.replied("o1"), "another agent still asks");
    let consent = (h.reach_question(beside).await.consent).expect("a consent card");
    assert_eq!(
        fact(&consent, "Call").as_deref(),
        Some("Read note: Release plan")
    );
}

#[tokio::test]
async fn deny_refuses_the_call_with_the_reason_and_the_next_call_asks_again() {
    let mut h = in_shop();
    let (lead, session) = h.start_boxed_claude(Some("Plan it")).await;
    h.ask_in(session, lead, "d1", "list_notes", json!({})).await;
    let card = h.notes_card().await;
    assert_eq!(
        h.pick_reach(card.id, DENY, false).await,
        AnswerOutcome::Accepted
    );
    let reason = h.answer("d1").await.expect_err("Deny refuses");
    assert_eq!(reason, NOTES_DENIED);
    h.notes_closed().await;

    let read = json!({"note": "Release plan"});
    h.ask_in(session, lead, "d2", "read_note", read).await;
    let card = h.notes_card().await;
    tokio::time::sleep(QUIET).await;
    assert!(!h.replied("d2"), "the next call asks again");
    let consent = (h.reach_question(lead).await.consent).expect("a consent card");
    assert_eq!(
        fact(&consent, "Call").as_deref(),
        Some("Read note: Release plan")
    );
    assert_eq!(
        h.pick_reach(card.id, ALLOW, false).await,
        AnswerOutcome::Accepted
    );
    let reason = h.answer("d2").await.expect_err("there's no such note");
    assert!(reason.contains("No note in this project"), "{reason}");
}

#[tokio::test]
async fn a_notes_card_nobody_answers_refuses_its_calls_once_the_wait_runs_out() {
    let mut h = in_shop();
    let (lead, session) = h.start_boxed_claude(Some("Plan it")).await;
    // Two calls at once share one card.
    h.ask_in(session, lead, "t1", "list_notes", json!({})).await;
    h.notes_card().await;
    h.ask_in(session, lead, "t2", "read_note", json!({"note": "Plan"}))
        .await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.notes_cards().len(), 1, "one card asks for both");
    h.clock.advance(Duration::from_secs(CARD_WAIT - 1));
    tokio::time::sleep(QUIET).await;
    assert!(!h.replied("t1") && !h.replied("t2"), "they still wait");
    h.clock.advance(Duration::from_secs(2));
    for key in ["t1", "t2"] {
        let reason = h.answer(key).await.expect_err("nobody answered");
        assert_eq!(reason, NOTES_UNANSWERED);
    }
    h.notes_closed().await;
    assert_eq!(h.reach_item(lead).await.status, ItemStatus::Cancelled);
}

#[tokio::test]
async fn one_answer_lets_every_waiting_notes_call_go() {
    let mut h = in_shop();
    let (lead, session) = h.start_boxed_claude(Some("Plan it")).await;
    h.ask_in(session, lead, "w1", "list_notes", json!({})).await;
    let card = h.notes_card().await;
    let create = json!({"title": "Plan", "body": ""});
    h.ask_in(session, lead, "w2", "create_note", create).await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.notes_cards().len(), 1, "one card asks for both");
    assert_eq!(
        h.pick_reach(card.id, ALLOW, false).await,
        AnswerOutcome::Accepted
    );
    h.answer("w1").await.expect("the list goes");
    h.answer("w2").await.expect("the note is made");
    h.notes_closed().await;
}

#[tokio::test]
async fn an_agent_on_the_host_never_asks() {
    let mut h = in_shop();
    // A container agent waits on its card meanwhile.
    let (lead, session) = h.start_boxed_claude(Some("Plan it")).await;
    h.ask_in(session, lead, "b1", "list_notes", json!({})).await;
    h.notes_card().await;

    let (agent, host) = h.start_claude(Some("On the host")).await;
    h.moves(host, agent, Activity::Idle).await;
    let answer = h
        .call(host, agent, "h1", "list_notes", json!({}))
        .await
        .expect("the list goes");
    assert_eq!(answer, "This project has no notes yet.");
    let create = json!({"title": "Plan", "body": "one\n"});
    h.call(host, agent, "h2", "create_note", create)
        .await
        .expect("the note is made");
    h.call(host, agent, "h3", "read_note", json!({"note": "Plan"}))
        .await
        .expect("the read goes");
    assert_eq!(h.notes_cards().len(), 1, "only the container agent's card");
    assert!(!h.replied("b1"));
}

#[tokio::test]
async fn a_remembered_answer_covers_notes_calls() {
    let mut h = in_shop();
    let (lead, session) = h.start_boxed_claude(Some("Plan it")).await;

    // Always allow, the box a card keeps, lets notes calls go with no card.
    h.reach_as(lead, LeaveContainer::Allow).await;
    let answer = h
        .call_in(session, lead, "a1", "list_notes", json!({}))
        .await
        .expect("the list goes");
    assert_eq!(answer, "This project has no notes yet.");
    assert!(h.notes_cards().is_empty(), "Always allow asks nobody");

    // Never refuses them with the reason, also with no card.
    h.reach_as(lead, LeaveContainer::Never).await;
    let reason = h
        .call_in(session, lead, "a2", "list_notes", json!({}))
        .await
        .expect_err("Never refuses");
    assert!(reason.contains(KEPT_IN), "{reason}");
    assert!(h.notes_cards().is_empty(), "Never asks nobody");

    // On Ask, the card's box keeps the answer on the permission, for every
    // call outside the container, notes included.
    h.reach_as(lead, LeaveContainer::Ask).await;
    h.ask_in(session, lead, "a3", "list_notes", json!({})).await;
    let card = h.notes_card().await;
    let consent = (h.reach_question(lead).await.consent).expect("a consent card");
    let remember = consent.remember.expect("the card offers its box");
    assert_eq!(remember.id, REMEMBER);
    assert_eq!(remember.label, REMEMBER_LABEL);
    for said in ["every later call", "the project's notes", "Containers"] {
        assert!(
            remember.settings.contains(said),
            "{said}: {}",
            remember.settings
        );
    }
    assert_eq!(
        h.pick_reach(card.id, ALLOW, true).await,
        AnswerOutcome::Accepted
    );
    h.answer("a3").await.expect("the list goes");
    assert_eq!(h.leave_now(), LeaveContainer::Allow);
}

#[tokio::test]
async fn a_restart_of_maxd_asks_again() {
    let mut h = in_shop();
    let (lead, session) = h.start_boxed_claude(Some("Plan it")).await;
    h.ask_in(session, lead, "r1", "list_notes", json!({})).await;
    let card = h.notes_card().await;
    assert_eq!(
        h.pick_reach(card.id, ALLOW, false).await,
        AnswerOutcome::Accepted
    );
    h.answer("r1").await.expect("the list goes");

    h.restart().await;
    eventually("the agent back in its holder", || {
        matches!(
            h.hub.get(&EntityKey::Agent(lead)),
            Some(Entity::Agent(back))
                if back.session == Some(session) && back.link == AgentLink::Live
        )
    })
    .await;
    h.ask_in(session, lead, "r2", "list_notes", json!({})).await;
    h.notes_card().await;
    assert!(!h.replied("r2"), "a restart asks again");
}
