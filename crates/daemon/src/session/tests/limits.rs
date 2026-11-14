// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Carrying on after a usage limit: the message a Claude Code or Codex
//! agent gets once a limit that cut its turn off resets, and when it
//! doesn't.

use std::time::Duration;

use maxhq_protocol::model::{DeviceSettingChange, InboxChange};

use super::*;
use crate::session::recovery::LIMIT_MESSAGE;

/// Sets the device's default for Claude Code or Codex agents.
async fn default_for(h: &mut Harness, vendor: Vendor, value: bool) {
    let change = match vendor {
        Vendor::Claude => DeviceSettingChange::ContinueClaudeAtLimit { value },
        Vendor::Codex => DeviceSettingChange::ContinueCodexAtLimit { value },
        other => panic!("no default for {other:?}"),
    };
    let outcome = h
        .act(|key| ClientMessage::UpdateDeviceSettings {
            key,
            device: device().id,
            changes: vec![change],
        })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
}

/// Turns the agent's own switch on or off.
async fn switch(h: &mut Harness, agent: AgentId, on: bool) {
    let change = InboxChange::ContinueAtLimit { on };
    let outcome = h
        .act(|key| ClientMessage::SetInbox { key, agent, change })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    assert_eq!(h.agent(agent).inbox.continue_at_limit, Some(on));
}

/// How long a limit in these tests lasts.
const LIMIT: Duration = Duration::from_secs(60 * 60);

/// A wait for a limit that resets in an hour.
fn limited(h: &Harness) -> Activity {
    Activity::WaitingForLimitReset {
        resets_at: Some(Timestamp(h.clock.wall().0 + LIMIT.as_millis() as u64)),
    }
}

/// The clock moves just past the reset of a limit that started now.
fn reset(h: &Harness) {
    h.clock.advance(LIMIT + Duration::from_secs(1));
}

/// The clock moves past the check a minute after the reset.
fn checked(h: &Harness) {
    h.clock.advance(Duration::from_secs(2 * 60));
}

/// The agent's CLI reports it doing `activity`.
async fn doing(h: &Harness, agent: AgentId, session: SessionId, activity: Activity) {
    let say = Say {
        agents: vec![h.reported(agent, activity)],
        ..Say::default()
    };
    h.say(session, &say).await;
}

/// The limit cuts the agent's turn off, then resets with nothing waiting.
async fn cut_and_reset(h: &Harness, agent: AgentId, session: SessionId) {
    doing(h, agent, session, Activity::Busy).await;
    doing(h, agent, session, limited(h)).await;
    reset(h);
    doing(h, agent, session, Activity::Idle).await;
}

/// How many carry-on messages went to the CLI.
fn carry_ons(h: &Harness, session: SessionId) -> usize {
    h.sent(session)
        .iter()
        .filter(|text| *text == LIMIT_MESSAGE)
        .count()
}

#[tokio::test]
async fn a_turn_the_limit_cuts_off_carries_on_once_it_resets() {
    let mut h = Harness::new();
    default_for(&mut h, Vendor::Claude, true).await;
    let (agent, session) = h.start_claude(Some("Fix the build")).await;
    doing(&h, agent, session, Activity::Busy).await;
    doing(&h, agent, session, limited(&h)).await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(carry_ons(&h, session), 0, "nothing goes while it waits");

    // The same wait reported again changes nothing.
    let wait = limited(&h);
    doing(&h, agent, session, wait.clone()).await;
    doing(&h, agent, session, wait).await;
    reset(&h);
    doing(&h, agent, session, Activity::Idle).await;
    eventually("the carry-on", || carry_ons(&h, session) == 1).await;
    // The check after the reset doesn't send a second.
    checked(&h);
    tokio::time::sleep(QUIET).await;
    assert_eq!(carry_ons(&h, session), 1);

    // Each turn the limit cuts off gets one.
    cut_and_reset(&h, agent, session).await;
    eventually("the second carry-on", || carry_ons(&h, session) == 2).await;
}

#[tokio::test]
async fn nothing_carries_on_with_the_default_off() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(Some("Fix the build")).await;
    cut_and_reset(&h, agent, session).await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(carry_ons(&h, session), 0, "it's off at first");

    default_for(&mut h, Vendor::Claude, true).await;
    default_for(&mut h, Vendor::Claude, false).await;
    cut_and_reset(&h, agent, session).await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(carry_ons(&h, session), 0);
}

#[tokio::test]
async fn each_vendor_has_its_own_default() {
    let mut h = Harness::new();
    default_for(&mut h, Vendor::Codex, true).await;
    let (agent, session) = h.start_claude(Some("Fix the build")).await;
    cut_and_reset(&h, agent, session).await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(carry_ons(&h, session), 0);
}

#[tokio::test]
async fn an_agents_own_switch_beats_the_default() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(Some("Fix the build")).await;
    switch(&mut h, agent, true).await;
    cut_and_reset(&h, agent, session).await;
    eventually("the carry-on", || carry_ons(&h, session) == 1).await;

    default_for(&mut h, Vendor::Claude, true).await;
    switch(&mut h, agent, false).await;
    cut_and_reset(&h, agent, session).await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(carry_ons(&h, session), 1);
}

#[tokio::test]
async fn a_switch_turned_while_waiting_counts() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(Some("Fix the build")).await;
    doing(&h, agent, session, Activity::Busy).await;
    doing(&h, agent, session, limited(&h)).await;
    switch(&mut h, agent, true).await;
    reset(&h);
    doing(&h, agent, session, Activity::Idle).await;
    eventually("the carry-on", || carry_ons(&h, session) == 1).await;

    default_for(&mut h, Vendor::Claude, true).await;
    doing(&h, agent, session, Activity::Busy).await;
    doing(&h, agent, session, limited(&h)).await;
    switch(&mut h, agent, false).await;
    reset(&h);
    doing(&h, agent, session, Activity::Idle).await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(carry_ons(&h, session), 1);
}

#[tokio::test]
async fn a_message_you_queued_goes_instead() {
    let mut h = Harness::new();
    default_for(&mut h, Vendor::Claude, true).await;
    let (agent, session) = h.start_claude(Some("Fix the build")).await;
    doing(&h, agent, session, Activity::Busy).await;
    doing(&h, agent, session, limited(&h)).await;
    reset(&h);
    let say = Say {
        agents: vec![h.reported(agent, Activity::Idle)],
        timeline: vec![upsert(agent, queued(agent, "q1", "Then push"))],
        ..Say::default()
    };
    h.say(session, &say).await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(carry_ons(&h, session), 0);
}

#[tokio::test]
async fn a_message_that_went_at_the_reset_goes_instead() {
    let mut h = Harness::new();
    default_for(&mut h, Vendor::Claude, true).await;
    let (agent, session) = h.start_claude(Some("Fix the build")).await;
    doing(&h, agent, session, Activity::Busy).await;
    doing(&h, agent, session, limited(&h)).await;
    reset(&h);
    doing(&h, agent, session, Activity::Busy).await;
    doing(&h, agent, session, Activity::Idle).await;
    checked(&h);
    tokio::time::sleep(QUIET).await;
    assert_eq!(carry_ons(&h, session), 0);
}

#[tokio::test]
async fn only_a_wait_that_cuts_a_turn_off_carries_on() {
    let mut h = Harness::new();
    default_for(&mut h, Vendor::Claude, true).await;
    let (agent, session) = h.start_claude(Some("Fix the build")).await;
    doing(&h, agent, session, Activity::Idle).await;
    doing(&h, agent, session, limited(&h)).await;
    reset(&h);
    doing(&h, agent, session, Activity::Idle).await;
    checked(&h);
    tokio::time::sleep(QUIET).await;
    assert_eq!(carry_ons(&h, session), 0);
}

#[tokio::test]
async fn a_codex_agent_carries_on_too() {
    let mut h = Harness::new();
    default_for(&mut h, Vendor::Codex, true).await;
    let (agent, server) = h.start_codex("thread-1").await;
    for activity in [
        Activity::Busy,
        Activity::WaitingForLimitReset { resets_at: None },
        Activity::Idle,
    ] {
        let say = Say {
            agents: vec![h.codex_agent("thread-1", activity)],
            ..Say::default()
        };
        h.say(server, &say).await;
    }
    eventually("the carry-on", || carry_ons(&h, server) == 1).await;
    assert_eq!(h.agent(agent).id, agent);
}

#[tokio::test]
async fn a_cursor_agent_is_left_alone() {
    let mut h = Harness::new();
    h.cursor_account(LoginKind::ConfigDir);
    let spec = h.spec(Vendor::Cursor, Some("Fix the build"));
    let agent = started(h.act(|key| ClientMessage::StartAgent { key, spec }).await);
    let session = h.holders.last();
    switch(&mut h, agent, true).await;
    doing(&h, agent, session, Activity::Busy).await;
    let waiting = Activity::WaitingForLimitReset { resets_at: None };
    doing(&h, agent, session, waiting).await;
    doing(&h, agent, session, Activity::Idle).await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(carry_ons(&h, session), 0);
}

/// The limit cuts the agent's turn off, and its wait ends long before the
/// reset, as a turn the CLI starts by itself once made it.
async fn cut_and_ended_early(h: &Harness, agent: AgentId, session: SessionId) {
    doing(h, agent, session, Activity::Busy).await;
    doing(h, agent, session, limited(h)).await;
    doing(h, agent, session, Activity::Busy).await;
    doing(h, agent, session, Activity::Idle).await;
}

#[tokio::test]
async fn a_wait_that_ends_early_carries_on_after_the_reset() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(Some("Fix the build")).await;
    switch(&mut h, agent, true).await;
    cut_and_ended_early(&h, agent, session).await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(carry_ons(&h, session), 0, "nothing goes before the reset");
    reset(&h);
    tokio::time::sleep(QUIET).await;
    assert_eq!(carry_ons(&h, session), 0, "the check waits a minute");
    checked(&h);
    eventually("the carry-on", || carry_ons(&h, session) == 1).await;
    checked(&h);
    tokio::time::sleep(QUIET).await;
    assert_eq!(carry_ons(&h, session), 1, "only once");
}

#[tokio::test]
async fn the_check_after_the_reset_outlasts_a_restart_of_maxd() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(Some("Fix the build")).await;
    switch(&mut h, agent, true).await;
    cut_and_ended_early(&h, agent, session).await;
    h.restart().await;
    reset(&h);
    checked(&h);
    eventually("the carry-on", || carry_ons(&h, session) == 1).await;
}

#[tokio::test]
async fn an_agent_busy_at_the_check_gets_none() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(Some("Fix the build")).await;
    switch(&mut h, agent, true).await;
    doing(&h, agent, session, Activity::Busy).await;
    doing(&h, agent, session, limited(&h)).await;
    doing(&h, agent, session, Activity::Busy).await;
    reset(&h);
    checked(&h);
    tokio::time::sleep(QUIET).await;
    doing(&h, agent, session, Activity::Idle).await;
    checked(&h);
    tokio::time::sleep(QUIET).await;
    assert_eq!(carry_ons(&h, session), 0);
}

#[tokio::test]
async fn a_message_you_send_during_the_wait_ends_the_carry_on() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(Some("Fix the build")).await;
    switch(&mut h, agent, true).await;
    doing(&h, agent, session, Activity::Busy).await;
    doing(&h, agent, session, limited(&h)).await;
    assert_eq!(h.send(agent, "Stop there").await, ActionOutcome::Done);
    doing(&h, agent, session, Activity::Busy).await;
    doing(&h, agent, session, Activity::Idle).await;
    reset(&h);
    checked(&h);
    tokio::time::sleep(QUIET).await;
    assert_eq!(carry_ons(&h, session), 0);
}

#[tokio::test]
async fn a_switch_turned_off_before_the_check_counts() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(Some("Fix the build")).await;
    switch(&mut h, agent, true).await;
    cut_and_ended_early(&h, agent, session).await;
    switch(&mut h, agent, false).await;
    reset(&h);
    checked(&h);
    tokio::time::sleep(QUIET).await;
    assert_eq!(carry_ons(&h, session), 0);
}
