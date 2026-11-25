// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Taking over a session from your terminal. A `sleep` each test starts
//! stands in for the terminal's CLI.

use std::ffi::OsStr;
use std::process::{Command, Stdio};
use std::sync::mpsc;

use maxhq_protocol::message::TakeOverMode;
use maxhq_protocol::processes::ProcessSignal;

use super::*;
use crate::ports::processes::send;

/// A child that stands in for the CLI, reaped as soon as it quits.
struct Cli {
    pid: u32,
    quit: mpsc::Receiver<()>,
}

impl Cli {
    fn start(program: impl AsRef<OsStr>, args: &[&str]) -> Self {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let (tx, quit) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = child.wait();
            let _ = tx.send(());
        });
        Self { pid, quit }
    }

    fn sleep() -> Self {
        Self::start("sleep", &["120"])
    }

    /// Waits for it to quit, while the manager goes on.
    async fn quit(&self) -> bool {
        eventually("the CLI quitting", || self.quit.try_recv().is_ok()).await;
        true
    }

    /// Whether it still runs a while later.
    async fn runs(&self) -> bool {
        tokio::time::sleep(Duration::from_millis(500)).await;
        self.quit.try_recv().is_err()
    }
}

impl Drop for Cli {
    fn drop(&mut self) {
        if self.quit.try_recv().is_err() {
            let _ = send(self.pid, ProcessSignal::Kill);
        }
    }
}

/// An outside session whose CLI is `cli`.
fn in_terminal(h: &Harness, session: &str, activity: Activity, cli: &Cli) -> AgentId {
    let id = h.outside(session, activity);
    let mut agent = h.agent(id);
    agent.pid = Some(cli.pid);
    h.hub.apply([Change::Upsert {
        value: Entity::Agent(agent),
    }]);
    id
}

fn take_over(mode: TakeOverMode, agent: AgentId) -> impl FnOnce(IdempotencyKey) -> ClientMessage {
    move |key| ClientMessage::TakeOver { key, agent, mode }
}

#[tokio::test]
async fn stop_now_stops_the_cli_and_continues_here() {
    let mut h = Harness::new();
    let uuid = Uuid::now_v7();
    let session = uuid.to_string();
    let cli = Cli::sleep();
    let agent = in_terminal(&h, &session, Activity::Busy, &cli);
    h.vendors.set_history(&session, Vec::new());

    let outcome = h.act(take_over(TakeOverMode::Now, agent)).await;
    assert_eq!(outcome, ActionOutcome::Done);
    assert!(cli.quit().await);
    assert_eq!(h.vendors.launches(), [(uuid, true)]);
    let shown = h.agent(agent);
    assert_eq!(shown.control, Control::Held);
    assert!(!shown.taking_over);
}

#[tokio::test]
async fn after_this_turn_waits_for_the_turn_to_end() {
    let mut h = Harness::new();
    let uuid = Uuid::now_v7();
    let session = uuid.to_string();
    let cli = Cli::sleep();
    let agent = in_terminal(&h, &session, Activity::Busy, &cli);
    h.vendors.set_history(&session, Vec::new());

    let outcome = h.act(take_over(TakeOverMode::AfterTurn, agent)).await;
    assert_eq!(outcome, ActionOutcome::Done);
    assert!(h.agent(agent).taking_over);
    assert!(cli.runs().await, "the turn goes on");
    assert!(h.vendors.launches().is_empty());

    let mut idle = h.agent(agent);
    idle.activity = Activity::Idle;
    h.hub.apply([Change::Upsert {
        value: Entity::Agent(idle),
    }]);
    assert!(cli.quit().await);
    eventually("the session continuing here", || {
        h.agent(agent).control == Control::Held
    })
    .await;
    assert_eq!(h.vendors.launches(), [(uuid, true)]);
    assert!(!h.agent(agent).taking_over);
}

#[tokio::test]
async fn a_wait_can_be_cancelled() {
    let mut h = Harness::new();
    let session = Uuid::now_v7().to_string();
    let cli = Cli::sleep();
    let agent = in_terminal(&h, &session, Activity::Busy, &cli);

    let outcome = h.act(take_over(TakeOverMode::AfterTurn, agent)).await;
    assert_eq!(outcome, ActionOutcome::Done);
    let outcome = h.act(take_over(TakeOverMode::Cancel, agent)).await;
    assert_eq!(outcome, ActionOutcome::Done);
    assert!(!h.agent(agent).taking_over);

    let mut idle = h.agent(agent);
    idle.activity = Activity::Idle;
    h.hub.apply([Change::Upsert {
        value: Entity::Agent(idle),
    }]);
    assert!(cli.runs().await);
    assert!(h.vendors.launches().is_empty());
}

#[tokio::test]
async fn an_idle_session_is_taken_over_at_once() {
    let mut h = Harness::new();
    let uuid = Uuid::now_v7();
    let session = uuid.to_string();
    let cli = Cli::sleep();
    let agent = in_terminal(&h, &session, Activity::Idle, &cli);
    h.vendors.set_history(&session, Vec::new());

    let outcome = h.act(take_over(TakeOverMode::AfterTurn, agent)).await;
    assert_eq!(outcome, ActionOutcome::Done);
    assert!(cli.quit().await);
    assert_eq!(h.agent(agent).control, Control::Held);
}

#[tokio::test]
async fn only_the_cli_itself_is_stopped() {
    let mut h = Harness::new();
    let session = Uuid::now_v7().to_string();
    // A shell that kept the CLI's pid isn't the CLI.
    let bin = crate::testing::TempDir::new();
    let shell = crate::testing::fake_program(bin.path(), "shell", "#!/bin/sh\nsleep 120; :\n");
    let other = Cli::start(&shell, &[]);
    let agent = in_terminal(&h, &session, Activity::Busy, &other);
    let outcome = h.act(take_over(TakeOverMode::Now, agent)).await;
    assert!(reason(&outcome).contains("another process"), "{outcome:?}");
    assert!(other.runs().await);
    assert_eq!(h.agent(agent).control, Control::Observed);
}

#[tokio::test]
async fn a_cli_that_already_quit_is_continued() {
    let mut h = Harness::new();
    let uuid = Uuid::now_v7();
    let session = uuid.to_string();
    let cli = Cli::sleep();
    let agent = in_terminal(&h, &session, Activity::Busy, &cli);
    send(cli.pid, ProcessSignal::Kill).unwrap();
    assert!(cli.quit().await);
    h.vendors.set_history(&session, Vec::new());

    let outcome = h.act(take_over(TakeOverMode::Now, agent)).await;
    assert_eq!(outcome, ActionOutcome::Done);
    assert_eq!(h.vendors.launches(), [(uuid, true)]);
}
