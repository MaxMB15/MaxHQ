// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A phone whose switch is off can't put an agent in Auto or Full access
//! by any route: a plan it confirms, a plan's answer, the dispatcher acting
//! on its words, or an undo. Lower modes keep working, and the Mac can.

use base64::engine::general_purpose::STANDARD;
use maxhq_protocol::dispatch::PlanOutcome;
use serde_json::json;

use super::dispatcher::ASKS_FIRST;
use super::*;
use crate::phones::PHONE_CANT;
use crate::session::dispatcher_admin::PHONE_ASKED;

/// A phone the Mac never turned the switch on for.
fn switched_off() -> PhoneId {
    PhoneId::derive("switched off")
}

impl Harness {
    /// Makes the harness's client `phone`'s, and gives back the one it had.
    fn on_phone(&mut self, phone: PhoneId) -> Inbox {
        let phone = Inbox::from_phone(&self.sessions, phone);
        std::mem::replace(&mut self.phone, phone)
    }

    /// Starts a Claude agent in Ask first.
    async fn careful_agent(&mut self) -> AgentId {
        let spec = AgentSpec {
            permission: Some(PermissionMode::Ask),
            ..self.spec(Vendor::Claude, None)
        };
        started(
            self.act(|key| ClientMessage::StartAgent { key, spec })
                .await,
        )
    }

    /// The mode `agent` was asked to run in.
    fn mode_of(&self, agent: AgentId) -> Option<PermissionMode> {
        self.agent(agent)
            .permission
            .map(|permission| permission.requested)
    }

    /// Has the dispatcher in `talk` suggest `mode` for `agent`, as `key`,
    /// and gives the plan's number.
    async fn suggest_mode(
        &mut self,
        (dispatcher, session): (AgentId, SessionId),
        key: &str,
        agent: AgentId,
        mode: &str,
    ) -> u32 {
        let set =
            json!({"action": "set_mode", "agent": agent.to_string(), "mode": mode, "asked": false});
        let answer = self
            .call(session, dispatcher, key, "manage_agent", set)
            .await;
        assert_eq!(answer, Ok(ASKS_FIRST.into()));
        self.dispatcher().plan.expect("the plan").id
    }
}

/// Confirms `plan` as `inbox`.
async fn confirm(inbox: &mut Inbox, plan: u32) -> ActionOutcome {
    let key = IdempotencyKey::generate();
    inbox.send(ClientMessage::ConfirmPlan { key, plan });
    inbox.result(key).await
}

/// Answers `event` with the option `id` as `inbox`.
async fn pick(inbox: &mut Inbox, event: AttentionId, id: &str) -> AnswerOutcome {
    let key = IdempotencyKey::generate();
    inbox.send(ClientMessage::Answer {
        idempotency_key: key,
        event,
        answer: AttentionAnswer::Option { id: id.into() },
        refs: Vec::new(),
    });
    inbox.answered(key).await
}

#[tokio::test]
async fn a_phone_whose_switch_is_off_cant_confirm_a_plan_into_auto() {
    let mut h = Harness::with_dispatcher().await;
    let agent = h.careful_agent().await;
    let talk = h.dispatch("Tidy my agents").await;
    let plan = h.suggest_mode(talk, "m1", agent, "auto").await;

    let mut phone = Inbox::from_phone(&h.sessions, switched_off());
    assert_eq!(reason(&confirm(&mut phone, plan).await), PHONE_CANT);
    assert_eq!(h.mode_of(agent), Some(PermissionMode::Ask));
    assert_eq!(
        h.dispatcher().plan.map(|shown| shown.id),
        Some(plan),
        "the plan stays for the Mac"
    );

    // The Mac's confirm runs it.
    let outcome = h.act(|key| ClientMessage::ConfirmPlan { key, plan }).await;
    assert_eq!(
        outcome,
        ActionOutcome::PlanRan {
            results: vec![PlanOutcome::Done]
        }
    );
    eventually("Auto", || h.mode_of(agent) == Some(PermissionMode::Auto)).await;

    // A plan that lowers the mode runs from the phone.
    let plan = h.suggest_mode(talk, "m2", agent, "plan").await;
    assert_eq!(
        confirm(&mut phone, plan).await,
        ActionOutcome::PlanRan {
            results: vec![PlanOutcome::Done]
        }
    );
    eventually("Plan only", || {
        h.mode_of(agent) == Some(PermissionMode::Plan)
    })
    .await;
}

#[tokio::test]
async fn a_phone_whose_switch_is_on_confirms_a_plan_into_auto() {
    let mut h = Harness::with_dispatcher().await;
    let agent = h.careful_agent().await;
    let talk = h.dispatch("Tidy my agents").await;
    let plan = h.suggest_mode(talk, "m1", agent, "auto").await;

    // Every test shares one phone store, so this phone's key is its own.
    let ssh_public = crate::phones::openssh_for_tests(&[150; 32]);
    let id = PhoneId::derive(&ssh_public);
    let phones = crate::phones::for_tests();
    let added = phones.add(id, "iPhone", &ssh_public, &STANDARD.encode([8u8; 32]));
    assert!(
        matches!(
            added,
            ActionOutcome::Done | ActionOutcome::PhoneAdded { .. }
        ),
        "{added:?}"
    );
    assert_eq!(phones.set_everything(id, true), ActionOutcome::Done);

    let mut phone = Inbox::from_phone(&h.sessions, id);
    assert_eq!(
        confirm(&mut phone, plan).await,
        ActionOutcome::PlanRan {
            results: vec![PlanOutcome::Done]
        }
    );
    eventually("Auto", || h.mode_of(agent) == Some(PermissionMode::Auto)).await;
}

#[tokio::test]
async fn a_phone_whose_switch_is_off_asking_for_auto_waits_for_the_mac() {
    let mut h = Harness::with_dispatcher().await;
    let agent = h.careful_agent().await;
    let mac = h.on_phone(switched_off());
    let (dispatcher, session) = h.dispatch("Set my agent to auto, then plan only").await;
    h.phone = mac;

    // The phone's words don't count for Auto, so it waits in the plan.
    let auto =
        json!({"action": "set_mode", "agent": agent.to_string(), "mode": "auto", "asked": true});
    let answer = h
        .call(session, dispatcher, "m1", "manage_agent", auto)
        .await;
    assert_eq!(answer, Ok(ASKS_FIRST.into()));
    let plan = h.dispatcher().plan.expect("the plan");
    assert_eq!(plan.items[0].reason, PHONE_ASKED);
    assert_eq!(h.mode_of(agent), Some(PermissionMode::Ask));

    // A lower mode the phone asked for runs at once.
    let lower =
        json!({"action": "set_mode", "agent": agent.to_string(), "mode": "plan", "asked": true});
    let answer = h
        .call(session, dispatcher, "m2", "manage_agent", lower)
        .await;
    assert_ne!(answer, Ok(ASKS_FIRST.into()));
    eventually("Plan only", || {
        h.mode_of(agent) == Some(PermissionMode::Plan)
    })
    .await;

    // The Mac's confirm runs the Auto it held back.
    let plan = h.dispatcher().plan.expect("the plan").id;
    let outcome = h.act(|key| ClientMessage::ConfirmPlan { key, plan }).await;
    assert_eq!(
        outcome,
        ActionOutcome::PlanRan {
            results: vec![PlanOutcome::Done]
        }
    );
    eventually("Auto", || h.mode_of(agent) == Some(PermissionMode::Auto)).await;
}

#[tokio::test]
async fn a_phone_whose_switch_is_off_cant_undo_back_into_auto() {
    let mut h = Harness::with_dispatcher().await;
    // The Mac starts agents in Auto.
    let (agent, _) = h.start_claude(None).await;
    assert_eq!(h.mode_of(agent), Some(PermissionMode::Auto));
    let (dispatcher, session) = h.dispatch("Set my agent to plan only").await;
    let lower =
        json!({"action": "set_mode", "agent": agent.to_string(), "mode": "plan", "asked": true});
    h.call(session, dispatcher, "m1", "manage_agent", lower)
        .await
        .expect("it lowered the mode");
    eventually("Plan only", || {
        h.mode_of(agent) == Some(PermissionMode::Plan)
    })
    .await;
    let [action] = h.dispatcher().undo[..] else {
        panic!("no undo");
    };

    let mut phone = Inbox::from_phone(&h.sessions, switched_off());
    let key = IdempotencyKey::generate();
    phone.send(ClientMessage::UndoDispatcherAction { key, action });
    assert_eq!(reason(&phone.result(key).await), PHONE_CANT);
    assert_eq!(
        h.dispatcher().undo,
        vec![action],
        "the Mac can still undo it"
    );

    assert_eq!(h.undo(action).await, ActionOutcome::Done);
    eventually("Auto", || h.mode_of(agent) == Some(PermissionMode::Auto)).await;
}

#[tokio::test]
async fn a_phone_whose_switch_is_off_cant_answer_a_plan_into_auto() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let mut phone = Inbox::from_phone(&h.sessions, switched_off());

    // Claude's plan prompt also takes "auto", which no card offers.
    let plan = h.prompt(agent, "Ready to code?");
    h.ask(session, &plan).await;
    assert_eq!(
        pick(&mut phone, plan.id, "auto").await,
        AnswerOutcome::UnknownEvent
    );
    assert!(h.holders.writes(session).is_empty());
    assert!(h.hub.get(&EntityKey::Attention(plan.id)).is_some());
    assert_eq!(
        pick(&mut phone, plan.id, "accept_edits").await,
        AnswerOutcome::Accepted
    );
    assert_eq!(h.holders.writes(session).len(), 1);

    // The Mac can.
    let plan = h.prompt(agent, "Ready to code?");
    h.ask(session, &plan).await;
    assert_eq!(
        pick(&mut h.phone, plan.id, "auto").await,
        AnswerOutcome::Accepted
    );

    // A question whose option reads "auto" is only words.
    let question = AttentionEvent {
        kind: AttentionKind::Question,
        ..h.prompt(agent, "Which mode should the docs name?")
    };
    h.ask(session, &question).await;
    assert_eq!(
        pick(&mut phone, question.id, "auto").await,
        AnswerOutcome::Accepted
    );
}
