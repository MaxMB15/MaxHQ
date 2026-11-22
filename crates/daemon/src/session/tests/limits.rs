// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Carrying on after a usage limit: the message a Claude Code or Codex
//! agent gets once a limit that cut its turn off resets, or a banked reset
//! you use lifts it, and when it doesn't.

use std::time::Duration;

use maxhq_protocol::model::{DeviceSettingChange, InboxChange};

use super::*;
use crate::session::recovery::LIMIT_MESSAGE;
use crate::usage::codex::FakeHome;

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

/// A Codex account of this device kept in `fake`'s folder, so a reset
/// used on it goes through the stand-in app-server. Each test names its
/// own, as only one reset at a time goes for an account.
fn codex_account(h: &mut Harness, name: &str, fake: &FakeHome) -> AccountId {
    let id = AccountId::derive(name);
    h.keep(Entity::Account(Account {
        id,
        device: h.host.id,
        vendor: Vendor::Codex,
        label: name.into(),
        config_dir: fake.home.display().to_string(),
        email: None,
        organization: None,
        plan: None,
        models: Vec::new(),
        usage: None,
        usage_failure: None,
        acp: None,
        login: LoginKind::ConfigDir,
        origin: None,
        model_server: None,
    }));
    id
}

/// Never lets go of an idle CLI, so each agent keeps its app-server
/// through the hour these tests move the clock on.
async fn keep_clis(h: &mut Harness) {
    let outcome = h
        .act(|key| ClientMessage::UpdateDeviceSettings {
            key,
            device: device().id,
            changes: vec![DeviceSettingChange::IdleReleaseMinutes { value: 0 }],
        })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
}

/// How many carry-on messages went to any CLI.
fn all_carry_ons(h: &Harness) -> usize {
    h.holders
        .sessions()
        .into_iter()
        .map(|session| carry_ons(h, session))
        .sum()
}

/// Starts a busy Codex agent on `thread` under `account`.
async fn start_codex_on(h: &mut Harness, account: AccountId, thread: &str) -> (AgentId, SessionId) {
    let mut spec = h.spec(Vendor::Codex, Some("Ship it"));
    spec.account = account;
    h.start_codex_with(thread, spec).await
}

/// The app-server reports the agent on `thread` doing `activity`.
async fn thread_doing(h: &Harness, server: SessionId, thread: &str, activity: Activity) {
    let say = Say {
        agents: vec![h.codex_agent(thread, activity)],
        ..Say::default()
    };
    h.say(server, &say).await;
}

/// The limit cuts the turn on `thread` off, an hour before it resets.
async fn thread_cut(h: &Harness, server: SessionId, thread: &str) {
    thread_doing(h, server, thread, Activity::Busy).await;
    thread_doing(h, server, thread, limited(h)).await;
}

/// The limit cuts the turn on `thread` off, and its wait ends long before
/// the reset.
async fn thread_cut_and_ended_early(h: &Harness, server: SessionId, thread: &str) {
    thread_cut(h, server, thread).await;
    thread_doing(h, server, thread, Activity::Busy).await;
    thread_doing(h, server, thread, Activity::Idle).await;
}

/// Uses a banked reset on `account` as the app's Use reset does, with
/// the stand-in app-server answering `answer`.
async fn use_reset(h: &Harness, fake: &FakeHome, account: AccountId, answer: &str) {
    fake.write("consume.json", &serde_json::json!({ "outcome": answer }));
    let program = fake.program.clone();
    let sessions = h.sessions.clone();
    let outcome = crate::usage::codex::reset::use_credit_with(
        h.hub.clone(),
        IdempotencyKey::generate(),
        account,
        move || Some(program.clone()),
        move |account| sessions.reset_used(account),
    )
    .await;
    assert!(
        matches!(outcome, ActionOutcome::ResetCredit { .. }),
        "{answer}: {outcome:?}"
    );
}

#[tokio::test]
async fn a_reset_you_use_carries_on_a_waiting_agent_now() {
    let mut h = Harness::new();
    keep_clis(&mut h).await;
    let fake = FakeHome::new("reset-wakes-waiting");
    let account = codex_account(&mut h, "reset-wakes-waiting", &fake);
    default_for(&mut h, Vendor::Codex, true).await;
    let (_, server) = start_codex_on(&mut h, account, "thread-1").await;
    thread_cut(&h, server, "thread-1").await;

    use_reset(&h, &fake, account, "reset").await;
    // Its app-server reads the limits again, finds room and ends the wait,
    // an hour before the reset time.
    h.logged("limits lifted").await;
    thread_doing(&h, server, "thread-1", Activity::Idle).await;
    eventually("the carry-on", || carry_ons(&h, server) == 1).await;

    // The reset time and the check after it send no second.
    reset(&h);
    checked(&h);
    tokio::time::sleep(QUIET).await;
    assert_eq!(all_carry_ons(&h), 1, "only once");
    assert_eq!(h.count("limits lifted"), 1);
}

#[tokio::test]
async fn a_reset_already_redeemed_carries_on_an_idle_agent_once() {
    let mut h = Harness::new();
    keep_clis(&mut h).await;
    let fake = FakeHome::new("reset-wakes-redeemed");
    let account = codex_account(&mut h, "reset-wakes-redeemed", &fake);
    let (agent, server) = start_codex_on(&mut h, account, "thread-1").await;
    switch(&mut h, agent, true).await;
    thread_cut_and_ended_early(&h, server, "thread-1").await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(carry_ons(&h, server), 0, "nothing goes before the reset");

    use_reset(&h, &fake, account, "alreadyRedeemed").await;
    eventually("the carry-on", || carry_ons(&h, server) == 1).await;
    // An idle agent's app-server has no wait to end.
    assert_eq!(h.count("limits lifted"), 0);

    // Another reset, and then the check after the reset time, find it
    // already carried on.
    use_reset(&h, &fake, account, "reset").await;
    reset(&h);
    checked(&h);
    tokio::time::sleep(QUIET).await;
    assert_eq!(all_carry_ons(&h), 1, "only once");
}

#[tokio::test]
async fn a_reset_you_use_leaves_an_agent_with_its_switch_off_as_it_was() {
    let mut h = Harness::new();
    keep_clis(&mut h).await;
    let fake = FakeHome::new("reset-wakes-off");
    let account = codex_account(&mut h, "reset-wakes-off", &fake);
    let (off, server) = start_codex_on(&mut h, account, "thread-1").await;
    let (waiting, _) = start_codex_on(&mut h, account, "thread-2").await;
    switch(&mut h, off, false).await;
    switch(&mut h, waiting, false).await;
    thread_cut_and_ended_early(&h, server, "thread-1").await;
    thread_cut(&h, server, "thread-2").await;

    use_reset(&h, &fake, account, "reset").await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(all_carry_ons(&h), 0);
    assert_eq!(h.count("limits lifted"), 0, "its app-server is left alone");

    // Each kept its reset time, so a switch turned on before then counts
    // there, as without the reset.
    switch(&mut h, off, true).await;
    thread_doing(&h, server, "thread-2", Activity::Idle).await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(
        carry_ons(&h, server),
        0,
        "nothing goes before the reset time"
    );
    reset(&h);
    checked(&h);
    eventually("the carry-on at the reset time", || {
        carry_ons(&h, server) == 1
    })
    .await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(all_carry_ons(&h), 1, "the one still off gets none");
}

#[tokio::test]
async fn a_reset_you_use_carries_on_only_that_accounts_agents() {
    let mut h = Harness::new();
    keep_clis(&mut h).await;
    let fake = FakeHome::new("reset-wakes-used");
    let used = codex_account(&mut h, "reset-wakes-used", &fake);
    let other_fake = FakeHome::new("reset-wakes-other");
    let other = codex_account(&mut h, "reset-wakes-other", &other_fake);
    default_for(&mut h, Vendor::Codex, true).await;
    default_for(&mut h, Vendor::Claude, true).await;
    let (_, server) = start_codex_on(&mut h, used, "thread-1").await;
    let (_, other_server) = start_codex_on(&mut h, other, "thread-2").await;
    assert_ne!(server, other_server, "each account has its own app-server");
    let (claude, claude_session) = h.start_claude(Some("Fix the build")).await;
    thread_cut(&h, server, "thread-1").await;
    thread_cut(&h, other_server, "thread-2").await;
    cut_and_ended_early(&h, claude, claude_session).await;

    use_reset(&h, &fake, used, "reset").await;
    h.logged("limits lifted").await;
    // Both waits end before their reset time, the other's by a turn its
    // CLI starts.
    thread_doing(&h, server, "thread-1", Activity::Idle).await;
    thread_doing(&h, other_server, "thread-2", Activity::Busy).await;
    thread_doing(&h, other_server, "thread-2", Activity::Idle).await;
    eventually("the carry-on", || carry_ons(&h, server) == 1).await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(carry_ons(&h, other_server), 0, "another account's");
    assert_eq!(carry_ons(&h, claude_session), 0, "another vendor's");
    assert_eq!(
        h.count("limits lifted"),
        1,
        "only the used account's app-server"
    );

    // The others carry on at their own reset time.
    reset(&h);
    checked(&h);
    eventually("the other's carry-on", || carry_ons(&h, other_server) == 1).await;
    eventually("Claude's carry-on", || carry_ons(&h, claude_session) == 1).await;
    assert_eq!(carry_ons(&h, server), 1);
    tokio::time::sleep(QUIET).await;
    assert_eq!(all_carry_ons(&h), 3, "one each");
}

#[tokio::test]
async fn a_reset_that_changes_nothing_carries_on_nobody() {
    let mut h = Harness::new();
    keep_clis(&mut h).await;
    let fake = FakeHome::new("reset-wakes-nothing");
    let account = codex_account(&mut h, "reset-wakes-nothing", &fake);
    default_for(&mut h, Vendor::Codex, true).await;
    let (_, server) = start_codex_on(&mut h, account, "thread-1").await;
    start_codex_on(&mut h, account, "thread-2").await;
    thread_cut_and_ended_early(&h, server, "thread-1").await;
    thread_cut(&h, server, "thread-2").await;

    for answer in ["nothingToReset", "noCredit"] {
        use_reset(&h, &fake, account, answer).await;
    }
    tokio::time::sleep(QUIET).await;
    assert_eq!(all_carry_ons(&h), 0);
    assert_eq!(h.count("limits lifted"), 0);

    // They carry on at the reset time, as before.
    reset(&h);
    thread_doing(&h, server, "thread-2", Activity::Idle).await;
    checked(&h);
    eventually("both carry-ons", || carry_ons(&h, server) == 2).await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(all_carry_ons(&h), 2, "one each");
}
