// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Terminals the manager opens for its agents, and the commands you run
//! with `!` as items in their timelines.

use std::sync::{Mutex, MutexGuard};

use maxhq_hold::{RunOutput, TermExit};
use maxhq_protocol::ids::TerminalId;
use maxhq_protocol::terminal::{Terminal, TerminalKind, TerminalStatus};
use tokio::sync::mpsc;

use super::deletes::add_sub;
use super::restarts::server;
use super::*;
use crate::terminals::access::Opener;
use crate::terminals::{Host, OpenRequest, RunNote, SessionRequest, Wiring};

#[derive(Default)]
struct Calls {
    opens: Vec<OpenRequest>,
    released: Vec<TerminalId>,
    ended: Vec<AgentId>,
    sessions: Vec<SessionRequest>,
    /// Where notes go to the manager that runs now.
    notes: Option<mpsc::UnboundedSender<RunNote>>,
}

/// Terminals that note what the manager asks, and say what a test tells
/// them to. Clones share their state, so they outlive a restart.
#[derive(Clone, Default)]
pub(super) struct FakeTerminals {
    calls: Arc<Mutex<Calls>>,
}

impl FakeTerminals {
    /// Wires them to a new manager.
    pub(super) fn wiring(&self) -> Wiring {
        let (tx, rx) = mpsc::unbounded_channel();
        self.lock().notes = Some(tx);
        Wiring {
            host: Arc::new(self.clone()),
            notes: rx,
            terminals: None,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Calls> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(super) fn note(&self, note: RunNote) {
        let calls = self.lock();
        let notes = calls.notes.as_ref().expect("a manager is wired");
        notes.send(note).expect("the manager listens");
    }

    pub(super) fn opens(&self) -> Vec<OpenRequest> {
        self.lock().opens.clone()
    }

    fn released(&self) -> Vec<TerminalId> {
        self.lock().released.clone()
    }

    pub(super) fn ended(&self) -> Vec<AgentId> {
        self.lock().ended.clone()
    }

    pub(super) fn sessions(&self) -> Vec<SessionRequest> {
        self.lock().sessions.clone()
    }
}

impl Host for FakeTerminals {
    fn open(&self, request: OpenRequest) {
        self.lock().opens.push(request);
    }

    fn release(&self, terminal: TerminalId) {
        self.lock().released.push(terminal);
    }

    fn end_agent(&self, agent: AgentId) {
        self.lock().ended.push(agent);
    }

    fn session(&self, request: SessionRequest) {
        self.lock().sessions.push(request);
    }
}

fn run(command: &str) -> TerminalKind {
    TerminalKind::Run {
        command: command.into(),
        private: false,
    }
}

/// How a run ended, with what it wrote.
fn exit(code: i32, text: &str) -> TermExit {
    TermExit {
        code: Some(code),
        signal: None,
        at: Timestamp::now(),
        output: Some(RunOutput {
            text: text.into(),
            lines: u32::try_from(text.lines().count()).unwrap(),
            truncated: false,
        }),
    }
}

/// A message the agent's CLI wrote at `at`.
fn said(agent: AgentId, id: &str, at: u64) -> TimelineItem {
    let mut item = reply(agent, id, id, ItemStatus::Completed);
    item.started_at = Some(Timestamp(at));
    item
}

fn the_run(update: TimelineUpdate) -> TimelineItem {
    let (_, events) = events(update);
    match events.as_slice() {
        [TimelineEvent::Upsert { item }] => item.clone(),
        other => panic!("expected one run, got {other:?}"),
    }
}

fn process(item: &TimelineItem) -> TerminalStatus {
    match &item.body {
        ItemBody::UserRun { process, .. } => *process,
        other => panic!("not a run: {other:?}"),
    }
}

impl Harness {
    /// Asks the manager for a terminal for `agent`, as the app.
    fn ask_terminal(&self, agent: AgentId, kind: TerminalKind) -> IdempotencyKey {
        let key = IdempotencyKey::generate();
        let message = ClientMessage::TerminalOpen {
            key,
            agent,
            kind,
            cols: 100,
            rows: 30,
        };
        self.phone.client.open_terminal(message, Opener::App);
        key
    }

    async fn try_terminal(&mut self, agent: AgentId, kind: TerminalKind) -> ActionOutcome {
        let key = self.ask_terminal(agent, kind);
        self.phone.result(key).await
    }

    /// Opens a terminal for `agent`: waits for the manager to ask for it,
    /// then says it opened, as the terminals do.
    async fn open_terminal(&mut self, agent: AgentId, kind: TerminalKind) -> Terminal {
        let terminals = self.fakes.terminals.clone();
        let from = terminals.opens().len();
        let key = self.ask_terminal(agent, kind);
        eventually("the ask for a terminal", || terminals.opens().len() > from).await;
        let request = terminals.opens().remove(from);
        assert_eq!(request.key, key);
        let terminal = Terminal {
            id: TerminalId::generate(),
            agent,
            device: self.host.id,
            kind: request.kind,
            moved: false,
            title: "make".into(),
            cwd: request.cwd.display().to_string(),
            status: TerminalStatus::Running,
            busy: false,
            marks: false,
            password_prompt: false,
            reads_keys: false,
            full_screen: false,
            app_cursor: false,
            waiting: false,
            answers: false,
            started_at: Timestamp::now(),
            cols: request.cols,
            rows: request.rows,
        };
        terminals.note(RunNote::Opened {
            key,
            agent,
            result: Ok(terminal.clone()),
        });
        assert_eq!(
            self.phone.result(key).await,
            ActionOutcome::TerminalOpened {
                terminal: terminal.id
            }
        );
        terminal
    }
}

#[tokio::test]
async fn a_run_shows_where_the_timeline_stood_and_ends_with_what_it_wrote() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let say = Say {
        timeline: vec![upsert(agent, said(agent, "m1", 10))],
        ..Say::default()
    };
    h.say(session, &say).await;
    let (_, items, _) = reset(h.open(agent, None, 50).await);
    assert_eq!(ids(&items), ["m1"]);

    let terminal = h.open_terminal(agent, run("make")).await;
    let request = &h.fakes.terminals.opens()[0];
    assert_eq!(request.cwd, h.folder);
    assert_eq!((request.cols, request.rows), (100, 30));
    assert_eq!(request.opener, Opener::App);
    let item = the_run(h.phone.timeline(agent).await);
    assert_eq!(item.id.0, format!("run-{}", terminal.id));
    assert_eq!(item.status, ItemStatus::Running);
    assert_eq!(process(&item), TerminalStatus::Running);
    let ItemBody::UserRun {
        command,
        cwd,
        output,
        ..
    } = &item.body
    else {
        panic!("not a run: {item:?}");
    };
    assert_eq!(command, "make");
    assert_eq!(*cwd, h.folder.display().to_string());
    assert!(output.is_empty());
    let kept = h.db.user_run::<TimelineItem>(agent, &item.id.0).unwrap();
    assert_eq!(kept.after.as_deref(), Some("m1"));

    h.fakes.terminals.note(RunNote::Ended {
        agent,
        terminal: terminal.id,
        exit: Some(exit(0, "built\n")),
    });
    let item = the_run(h.phone.timeline(agent).await);
    assert_eq!(item.status, ItemStatus::Completed);
    assert_eq!(
        process(&item),
        TerminalStatus::Exited {
            code: Some(0),
            signal: None,
            stopped: false,
        }
    );
    let ItemBody::UserRun { output, lines, .. } = &item.body else {
        panic!("not a run: {item:?}");
    };
    assert_eq!((output.as_str(), *lines), ("built\n", 1));
    eventually("the release", || {
        h.fakes.terminals.released() == [terminal.id]
    })
    .await;
}

#[tokio::test]
async fn a_run_between_two_messages_stays_there_after_a_restart() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let (uuid, _) = *h.vendors.launches().last().expect("the agent launched");
    let history = || vec![said(agent, "m1", 10), said(agent, "m2", 30)];
    h.vendors.set_history(&uuid.to_string(), history());
    let first = Say {
        timeline: vec![upsert(agent, said(agent, "m1", 10))],
        ..Say::default()
    };
    h.say(session, &first).await;
    let terminal = h.open_terminal(agent, run("make")).await;
    let second = Say {
        timeline: vec![upsert(agent, said(agent, "m2", 30))],
        ..Say::default()
    };
    h.say(session, &second).await;
    h.fakes.terminals.note(RunNote::Ended {
        agent,
        terminal: terminal.id,
        exit: Some(exit(2, "error\n")),
    });
    eventually("the run's end kept", || {
        h.db.user_run::<TimelineItem>(agent, &format!("run-{}", terminal.id))
            .is_some_and(|row| row.record.status == ItemStatus::Failed)
    })
    .await;

    h.restart().await;
    let (_, items, _) = reset(h.open(agent, None, 50).await);
    let run = format!("run-{}", terminal.id);
    assert_eq!(ids(&items), ["m1", run.as_str(), "m2"]);
    assert_eq!(
        process(&items[1]),
        TerminalStatus::Exited {
            code: Some(2),
            signal: None,
            stopped: false,
        }
    );
}

#[tokio::test]
async fn a_run_that_ended_while_its_timeline_was_read_shows_its_end() {
    let mut h = Harness::new();
    let (agent, _) = h.start_claude(None).await;
    let (uuid, _) = *h.vendors.launches().last().expect("the agent launched");
    let uuid = uuid.to_string();
    h.vendors.set_history(&uuid, Vec::new());
    let terminal = h.open_terminal(agent, run("make")).await;
    h.vendors.hold_reads(&uuid);
    h.restart().await;
    eventually("the history read", || h.vendors.waiting_reads() == 1).await;

    // The terminals tell the new manager the run ended while it was down.
    h.fakes.terminals.note(RunNote::Ended {
        agent,
        terminal: terminal.id,
        exit: Some(exit(0, "")),
    });
    eventually("the release", || {
        h.fakes.terminals.released() == [terminal.id]
    })
    .await;
    let kept =
        h.db.user_run::<TimelineItem>(agent, &format!("run-{}", terminal.id));
    assert_eq!(
        kept.map(|row| row.record.status),
        Some(ItemStatus::Completed)
    );
    h.vendors.release_reads(&uuid);
    let (_, items, _) = reset(h.open(agent, None, 50).await);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].status, ItemStatus::Completed);
}

#[tokio::test]
async fn a_run_moved_to_the_drawer_says_so() {
    let mut h = Harness::new();
    let (agent, _) = h.start_claude(None).await;
    reset(h.open(agent, None, 50).await);
    let terminal = h.open_terminal(agent, run("npm run dev")).await;
    the_run(h.phone.timeline(agent).await);
    h.fakes.terminals.note(RunNote::Moved {
        agent,
        terminal: terminal.id,
    });
    let item = the_run(h.phone.timeline(agent).await);
    assert!(matches!(item.body, ItemBody::UserRun { moved: true, .. }));
    assert_eq!(process(&item), TerminalStatus::Running);
}

#[tokio::test]
async fn a_shell_opens_in_the_agents_folder_with_no_item() {
    let mut h = Harness::new();
    let (agent, _) = h.start_claude(None).await;
    reset(h.open(agent, None, 50).await);
    let shell = h.open_terminal(agent, TerminalKind::Shell).await;
    assert_eq!(h.fakes.terminals.opens()[0].cwd, h.folder);
    h.phone
        .quiet("a timeline item", |message| {
            matches!(message, ServerMessage::Timeline { .. })
        })
        .await;
    assert!(h
        .db
        .user_run::<TimelineItem>(agent, &format!("run-{}", shell.id))
        .is_none());
}

#[tokio::test]
async fn a_terminal_opens_only_for_a_lead_this_device_runs_here() {
    let mut h = Harness::new();
    let outcome = h.try_terminal(AgentId::derive("nobody"), run("ls")).await;
    assert!(
        reason(&outcome).contains("MaxHQ runs on this device"),
        "{outcome:?}"
    );

    let (lead, session) = h.start_claude(None).await;
    let sub = add_sub(&h, lead, session).await;
    let outcome = h.try_terminal(sub.id, TerminalKind::Shell).await;
    assert!(
        reason(&outcome).contains("agent that started this one"),
        "{outcome:?}"
    );
    assert!(h.fakes.terminals.opens().is_empty());
}

#[tokio::test]
async fn a_terminal_for_an_agent_in_a_container_is_refused() {
    let mut h = Harness::on(server());
    h.container(None);
    let (agent, _) = h.start_boxed_claude(None).await;
    let outcome = h.try_terminal(agent, run("ls")).await;
    assert!(reason(&outcome).contains("containers"), "{outcome:?}");
    assert!(h.fakes.terminals.opens().is_empty());
}

#[tokio::test]
async fn a_terminal_that_fails_to_open_says_why_and_adds_nothing() {
    let mut h = Harness::new();
    let (agent, _) = h.start_claude(None).await;
    reset(h.open(agent, None, 50).await);
    let key = h.ask_terminal(agent, run("make"));
    let terminals = h.fakes.terminals.clone();
    eventually("the ask for a terminal", || !terminals.opens().is_empty()).await;
    terminals.note(RunNote::Opened {
        key,
        agent,
        result: Err("Couldn't start the terminal: no shell".into()),
    });
    let outcome = h.phone.result(key).await;
    assert_eq!(reason(&outcome), "Couldn't start the terminal: no shell");
    h.phone
        .quiet("a timeline item", |message| {
            matches!(message, ServerMessage::Timeline { .. })
        })
        .await;

    // A resend gets the same answer, and opens nothing more.
    let message = ClientMessage::TerminalOpen {
        key,
        agent,
        kind: run("make"),
        cols: 80,
        rows: 24,
    };
    h.phone.client.open_terminal(message, Opener::App);
    assert_eq!(
        reason(&h.phone.result(key).await),
        "Couldn't start the terminal: no shell"
    );
    assert_eq!(terminals.opens().len(), 1);
}

#[tokio::test]
async fn stopping_an_agent_keeps_its_terminals_and_deleting_it_ends_them() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let outcome = h.act(|key| ClientMessage::StopAgent { key, agent }).await;
    assert_eq!(outcome, ActionOutcome::Done);
    eventually("the holder's files going", || h.holders.forgotten(session)).await;
    assert!(h.fakes.terminals.ended().is_empty());

    let outcome = h.act(|key| ClientMessage::DeleteAgent { key, agent }).await;
    assert_eq!(outcome, ActionOutcome::Done);
    assert_eq!(h.fakes.terminals.ended(), [agent]);
}

#[tokio::test]
async fn a_terminal_that_opens_after_its_agent_went_ends() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let outcome = h.act(|key| ClientMessage::StopAgent { key, agent }).await;
    assert_eq!(outcome, ActionOutcome::Done);
    eventually("the holder's files going", || h.holders.forgotten(session)).await;
    let key = h.ask_terminal(agent, run("make"));
    let terminals = h.fakes.terminals.clone();
    eventually("the ask for a terminal", || !terminals.opens().is_empty()).await;
    let outcome = h.act(|key| ClientMessage::DeleteAgent { key, agent }).await;
    assert_eq!(outcome, ActionOutcome::Done);

    let terminal = Terminal {
        id: TerminalId::generate(),
        agent,
        device: h.host.id,
        kind: run("make"),
        moved: false,
        title: "make".into(),
        cwd: h.folder.display().to_string(),
        status: TerminalStatus::Running,
        busy: false,
        marks: false,
        password_prompt: false,
        reads_keys: false,
        full_screen: false,
        app_cursor: false,
        waiting: false,
        answers: false,
        started_at: Timestamp::now(),
        cols: 80,
        rows: 24,
    };
    terminals.note(RunNote::Opened {
        key,
        agent,
        result: Ok(terminal),
    });
    let outcome = h.phone.result(key).await;
    assert!(reason(&outcome).contains("agent ended"), "{outcome:?}");
    // Once for the delete, once for the terminal that opened after it.
    assert_eq!(terminals.ended(), [agent, agent]);
    assert!(h.db.user_runs::<TimelineItem>(agent).unwrap().is_empty());
}
