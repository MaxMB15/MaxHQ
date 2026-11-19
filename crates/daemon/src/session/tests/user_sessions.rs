// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The commands you run in an agent's session, as cards in its timeline,
//! and what a message you send does to them.

use maxhq_core::context::Shows;
use maxhq_protocol::context::SessionRefBlock;
use maxhq_protocol::ids::TerminalId;
use maxhq_protocol::terminal::{FreshReason, SessionBlock, Terminal, TerminalKind, TerminalStatus};
use maxhq_term::Transcript;

use super::deletes::add_sub;
use super::restarts::server;
use super::*;
use crate::terminals::access::Opener;
use crate::terminals::{BlockChange, RunNote, SessionAsk, SessionRequest, SessionResult};

/// Where the tests' sessions run, as the device writes it.
const FOLDER: &str = "~/code/example-app";

impl Harness {
    /// Sends a session request as the app does, through the terminals'
    /// door, and gives its key.
    fn through_terminals(
        &self,
        make: impl FnOnce(IdempotencyKey) -> ClientMessage,
    ) -> IdempotencyKey {
        let key = IdempotencyKey::generate();
        self.phone.client.open_terminal(make(key), Opener::App);
        key
    }

    /// Asks for `command` in `agent`'s session, as the app does for a `!`.
    fn bang(&self, agent: AgentId, command: &str, private: bool, fresh: bool) -> IdempotencyKey {
        let command = command.to_owned();
        self.through_terminals(|key| ClientMessage::SessionRun {
            key,
            agent,
            command,
            private,
            fresh,
            cols: 100,
            rows: 30,
        })
    }

    /// The request the manager handed the terminals for `key`.
    async fn session_request(&self, key: IdempotencyKey) -> SessionRequest {
        let terminals = self.fakes.terminals.clone();
        let asked = || (terminals.sessions().into_iter()).find(|asked| asked.key == key);
        eventually("the ask for the session", || asked().is_some()).await;
        asked().expect("it was asked a moment ago")
    }

    /// Runs `command` as block `block` of the session `terminal`, as the
    /// terminals would, and gives the card it went into.
    async fn wrote(
        &mut self,
        agent: AgentId,
        terminal: TerminalId,
        block: u32,
        command: &str,
        private: bool,
    ) -> ItemId {
        let key = self.bang(agent, command, private, false);
        self.session_request(key).await;
        self.fakes.terminals.note(RunNote::Session {
            key,
            agent,
            result: SessionResult::Wrote {
                terminal,
                block,
                command: command.into(),
                cwd: FOLDER.into(),
                private,
                at: Timestamp::now(),
            },
        });
        match self.phone.result(key).await {
            ActionOutcome::SessionRan {
                terminal: ran,
                item,
                block: got,
            } => {
                assert_eq!((ran, got), (terminal, block));
                item
            }
            other => panic!("expected a block, got {other:?}"),
        }
    }

    /// Says block `block` of the session `terminal` ended, having written
    /// `text`.
    fn block_ended(
        &self,
        agent: AgentId,
        terminal: TerminalId,
        block: u32,
        at: Timestamp,
        text: &str,
    ) {
        self.fakes.terminals.note(RunNote::Block {
            agent,
            terminal,
            block,
            change: BlockChange::Ended {
                exit: Some(0),
                at,
                cwd_after: None,
                output: Transcript {
                    text: text.into(),
                    lines: 1,
                    truncated: false,
                },
                secret_prompts: 0,
            },
        });
    }

    /// Sends `text` with these pills, as a person does, the way `mode`
    /// says.
    async fn send_refs(
        &mut self,
        agent: AgentId,
        text: &str,
        refs: Vec<InlineRef>,
        mode: SendMode,
    ) -> ActionOutcome {
        let text = text.to_owned();
        self.act(|key| ClientMessage::SendInput {
            key,
            agent,
            input: UserInput {
                text,
                attachments: Vec::new(),
                refs,
            },
            mode,
        })
        .await
    }

    /// The card `item` as kept.
    fn session_card(&self, agent: AgentId, item: &ItemId) -> TimelineItem {
        (self.db.user_run::<TimelineItem>(agent, &item.0))
            .expect("the card is kept")
            .record
    }

    /// The pills the last message sent to `agent` carried.
    fn last_refs(&self, agent: AgentId) -> Vec<InlineRef> {
        (self.vendors.sent_refs().into_iter())
            .rev()
            .find(|(to, _)| *to == agent)
            .map(|(_, refs)| refs)
            .expect("a message went to the agent")
    }
}

/// A card's blocks, whether it's sealed, and whether its pill went along.
fn card_of(item: &TimelineItem) -> (&[SessionBlock], bool, bool) {
    match &item.body {
        ItemBody::UserSession {
            blocks,
            sealed,
            sent,
            ..
        } => (blocks, *sealed, *sent),
        other => panic!("not a session card: {other:?}"),
    }
}

/// The pill for the card `item`, with a block for each command, as a
/// client makes one.
fn pill(item: Option<&ItemId>, commands: &[&str]) -> InlineRef {
    InlineRef::Session {
        device_name: "devbox".into(),
        blocks: (commands.iter())
            .map(|command| SessionRefBlock {
                command: (*command).into(),
                cwd: FOLDER.into(),
                cwd_after: None,
                exit_code: Some(0),
                running: false,
                duration_ms: Some(5),
                lines_total: 1,
                text: format!("what {command} wrote"),
            })
            .collect(),
        item: item.cloned(),
    }
}

/// The commands each session pill in `refs` holds.
fn pill_commands(refs: &[InlineRef]) -> Vec<Vec<String>> {
    (refs.iter())
        .filter_map(|made| match made {
            InlineRef::Session { blocks, .. } => {
                Some(blocks.iter().map(|block| block.command.clone()).collect())
            }
            _ => None,
        })
        .collect()
}

/// A message the agent's CLI wrote at `at`.
fn said(agent: AgentId, id: &str, at: u64) -> TimelineItem {
    let mut item = reply(agent, id, id, ItemStatus::Completed);
    item.started_at = Some(Timestamp(at));
    item
}

/// A shell open for `agent`, as the terminals say it opened.
fn opened(h: &Harness, agent: AgentId, kind: TerminalKind) -> Terminal {
    Terminal {
        id: TerminalId::generate(),
        agent,
        device: h.host.id,
        kind,
        moved: false,
        title: "zsh".into(),
        cwd: h.folder.display().to_string(),
        status: TerminalStatus::Running,
        busy: false,
        marks: true,
        password_prompt: false,
        started_at: Timestamp::now(),
        cols: 100,
        rows: 30,
    }
}

#[tokio::test]
async fn a_bang_goes_into_a_card_where_the_timeline_stood_and_fills_in_as_it_runs() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let say = Say {
        timeline: vec![upsert(agent, said(agent, "m1", 10))],
        ..Say::default()
    };
    h.say(session, &say).await;
    reset(h.open(agent, None, 50).await);

    let key = h.bang(agent, "pnpm test", false, false);
    let asked = h.session_request(key).await;
    assert_eq!(asked.agent, agent);
    assert_eq!(asked.cwd, h.folder);
    assert_eq!((asked.cols, asked.rows), (100, 30));
    assert_eq!(asked.opener, Opener::App);
    assert!(
        matches!(
            &asked.ask,
            SessionAsk::Run { command, private: false, fresh: None } if command == "pnpm test"
        ),
        "{:?}",
        asked.ask
    );
    let terminal = TerminalId::generate();
    let at = Timestamp::now();
    h.fakes.terminals.note(RunNote::Session {
        key,
        agent,
        result: SessionResult::Wrote {
            terminal,
            block: 1,
            command: "pnpm test".into(),
            cwd: FOLDER.into(),
            private: false,
            at,
        },
    });
    let item = ItemId(format!("session-{terminal}-1"));
    assert_eq!(
        h.phone.result(key).await,
        ActionOutcome::SessionRan {
            terminal,
            item: item.clone(),
            block: 1
        }
    );
    let (_, shown) = events(h.phone.timeline(agent).await);
    assert!(
        matches!(shown.as_slice(), [TimelineEvent::Upsert { item: card }] if card.id == item),
        "{shown:?}"
    );
    let kept = h.db.user_run::<TimelineItem>(agent, &item.0).unwrap();
    assert_eq!(kept.after.as_deref(), Some("m1"));
    let card = kept.record;
    assert_eq!(card.status, ItemStatus::Running);
    let ItemBody::UserSession {
        terminal: its,
        device,
        ..
    } = &card.body
    else {
        panic!("not a card: {card:?}");
    };
    assert_eq!((*its, *device), (terminal, h.host.id));
    let (blocks, sealed, sent) = card_of(&card);
    assert!(!sealed && !sent);
    assert_eq!(blocks.len(), 1);
    let block = &blocks[0];
    assert_eq!((block.id, block.command.as_str()), (1, "pnpm test"));
    assert_eq!((block.cwd.as_str(), block.started_at), (FOLDER, at));
    assert_eq!(block.process, TerminalStatus::Running);
    assert_eq!(block.start, None);

    // The shell marks the start, then the end.
    h.fakes.terminals.note(RunNote::Block {
        agent,
        terminal,
        block: 1,
        change: BlockChange::Started {
            start: 4096,
            at: Timestamp(at.0 + 10),
            cwd: Some("~/code/example-app/apps/web".into()),
        },
    });
    eventually("the start kept", || {
        card_of(&h.session_card(agent, &item)).0[0].start == Some(4096)
    })
    .await;
    h.fakes.terminals.note(RunNote::Block {
        agent,
        terminal,
        block: 1,
        change: BlockChange::Ended {
            exit: Some(1),
            at: Timestamp(at.0 + 1510),
            cwd_after: Some(FOLDER.into()),
            output: Transcript {
                text: "Tests: 1 failed".into(),
                lines: 1,
                truncated: false,
            },
            secret_prompts: 0,
        },
    });
    eventually("the end kept", || {
        h.session_card(agent, &item).status == ItemStatus::Completed
    })
    .await;
    let card = h.session_card(agent, &item);
    let block = &card_of(&card).0[0];
    assert_eq!(
        block.process,
        TerminalStatus::Exited {
            code: Some(1),
            signal: None
        }
    );
    assert_eq!(block.cwd, "~/code/example-app/apps/web");
    assert_eq!(block.cwd_after.as_deref(), Some(FOLDER));
    assert_eq!(block.duration_ms, Some(1500));
    assert_eq!((block.output.as_str(), block.lines), ("Tests: 1 failed", 1));
    assert_eq!(card.completed_at, Some(Timestamp(at.0 + 1510)));

    // The next command joins the same card, which runs again.
    assert_eq!(h.wrote(agent, terminal, 2, "git status", false).await, item);
    let card = h.session_card(agent, &item);
    assert_eq!(card.status, ItemStatus::Running);
    let ids: Vec<u32> = card_of(&card).0.iter().map(|block| block.id).collect();
    assert_eq!(ids, [1, 2]);
}

#[tokio::test]
async fn a_bang_the_session_cant_take_runs_fresh_and_says_why() {
    let mut h = Harness::new();
    let (agent, _) = h.start_claude(None).await;
    reset(h.open(agent, None, 50).await);

    // The terminals find the session busy, and open a fresh shell.
    let key = h.bang(agent, "pnpm dev", false, false);
    h.session_request(key).await;
    h.fakes.terminals.note(RunNote::Session {
        key,
        agent,
        result: SessionResult::Fresh(FreshReason::Busy),
    });
    let terminal = opened(
        &h,
        agent,
        TerminalKind::Run {
            command: "pnpm dev".into(),
            private: false,
        },
    );
    h.fakes.terminals.note(RunNote::Opened {
        key,
        agent,
        result: Ok(terminal.clone()),
    });
    assert_eq!(
        h.phone.result(key).await,
        ActionOutcome::TerminalOpened {
            terminal: terminal.id
        }
    );
    let run =
        h.db.user_run::<TimelineItem>(agent, &format!("run-{}", terminal.id));
    assert!(
        matches!(
            run.as_ref().map(|row| &row.record.body),
            Some(ItemBody::UserRun {
                fresh: Some(FreshReason::Busy),
                ..
            })
        ),
        "{:?}",
        run.map(|row| row.record)
    );

    // A line break, an exec, or the app asking sends it fresh whatever the
    // session does.
    for (command, asked, reason) in [
        ("echo a\necho b", false, FreshReason::Lines),
        ("exec zsh", false, FreshReason::Exec),
        ("ls", true, FreshReason::Asked),
    ] {
        let key = h.bang(agent, command, false, asked);
        let request = h.session_request(key).await;
        assert!(
            matches!(&request.ask, SessionAsk::Run { fresh: Some(got), .. } if *got == reason),
            "{command}: {:?}",
            request.ask
        );
    }

    // A `!` with nothing after it goes nowhere.
    let key = h.bang(agent, "  ", false, false);
    assert_eq!(
        reason(&h.phone.result(key).await),
        "Type a command after the !."
    );
    assert!(!h
        .fakes
        .terminals
        .sessions()
        .iter()
        .any(|asked| asked.key == key));
}

#[tokio::test]
async fn the_folder_chip_opens_the_session_or_moves_it() {
    let mut h = Harness::new();
    let (agent, _) = h.start_claude(None).await;
    reset(h.open(agent, None, 50).await);
    let cd = |path: &str| {
        let path = path.to_owned();
        move |key: IdempotencyKey| ClientMessage::SessionCd { key, agent, path }
    };

    // With no session, the terminals open one there.
    let key = h.through_terminals(cd("~/code/example-app/apps/web"));
    let asked = h.session_request(key).await;
    assert!(
        matches!(&asked.ask, SessionAsk::Cd { path } if path == "~/code/example-app/apps/web"),
        "{:?}",
        asked.ask
    );
    assert_eq!((asked.cols, asked.rows), (80, 24));
    let session = opened(&h, agent, TerminalKind::Session);
    h.fakes.terminals.note(RunNote::Session {
        key,
        agent,
        result: SessionResult::Opened(session.clone()),
    });
    assert_eq!(
        h.phone.result(key).await,
        ActionOutcome::TerminalOpened {
            terminal: session.id
        }
    );

    // On an idle session the move is a block anyone can see.
    let key = h.through_terminals(cd("~/code/example-app"));
    h.session_request(key).await;
    h.fakes.terminals.note(RunNote::Session {
        key,
        agent,
        result: SessionResult::Wrote {
            terminal: session.id,
            block: 1,
            command: "cd -- ~/code/example-app".into(),
            cwd: "~/code/example-app/apps/web".into(),
            private: false,
            at: Timestamp::now(),
        },
    });
    assert!(matches!(
        h.phone.result(key).await,
        ActionOutcome::SessionRan { block: 1, .. }
    ));

    // A busy one says so.
    let busy = "The session is busy. Wait for pnpm dev to finish, or stop it.";
    let key = h.through_terminals(cd("/tmp"));
    h.session_request(key).await;
    h.fakes.terminals.note(RunNote::Session {
        key,
        agent,
        result: SessionResult::Refused(busy.into()),
    });
    assert_eq!(reason(&h.phone.result(key).await), busy);
}

#[tokio::test]
async fn a_message_seals_the_cards_and_takes_only_shared_blocks_along() {
    let mut h = Harness::new();
    let (agent, _) = h.start_claude(None).await;
    reset(h.open(agent, None, 50).await);
    let terminal = TerminalId::generate();
    let item = h.wrote(agent, terminal, 1, "cd apps/web", false).await;
    assert_eq!(
        h.wrote(agent, terminal, 2, "export TOKEN=hidden", false)
            .await,
        item
    );

    // You keep the second block from the agent.
    let share = |item: &ItemId, block: u32, private: bool| {
        let item = item.clone();
        move |key: IdempotencyKey| ClientMessage::SessionShare {
            key,
            agent,
            item,
            block,
            private,
        }
    };
    let key = h.through_terminals(share(&item, 2, true));
    assert_eq!(h.phone.result(key).await, ActionOutcome::Done);
    assert!(card_of(&h.session_card(agent, &item)).0[1].private);
    let no_block = "No session card here has that block.";
    let key = h.through_terminals(share(&item, 9, true));
    assert_eq!(reason(&h.phone.result(key).await), no_block);
    let key = h.through_terminals(share(&ItemId("session-nowhere-1".into()), 1, true));
    assert_eq!(reason(&h.phone.result(key).await), no_block);

    // A client that sends the hidden block anyway gets it dropped.
    let sent = pill(Some(&item), &["cd apps/web", "export TOKEN=hidden"]);
    let outcome = h
        .send_refs(agent, "Why \u{FFFC}?", vec![sent], SendMode::Queue)
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    assert_eq!(pill_commands(&h.last_refs(agent)), [["cd apps/web"]]);
    let card = h.session_card(agent, &item);
    assert_eq!((card_of(&card).1, card_of(&card).2), (true, true));

    // A sealed card's blocks stay as they are, and the next command starts
    // a new card.
    let key = h.through_terminals(share(&item, 1, true));
    assert_eq!(
        reason(&h.phone.result(key).await),
        "Your message sealed that card, so its blocks stay as they are."
    );
    let next = h.wrote(agent, terminal, 3, "pnpm build", false).await;
    assert_eq!(next.0, format!("session-{terminal}-3"));

    // A message without its pill seals it too, as not sent.
    let outcome = h
        .send_refs(agent, "Thanks", Vec::new(), SendMode::Queue)
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    let card = h.session_card(agent, &next);
    assert_eq!((card_of(&card).1, card_of(&card).2), (true, false));
}

#[tokio::test]
async fn a_message_queued_mid_turn_seals_the_card_at_once() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(Some("Fix the build")).await;
    let say = Say {
        timeline: vec![upsert(
            agent,
            reply(agent, "r1", "Working on it", ItemStatus::Running),
        )],
        ..Say::default()
    };
    h.say(session, &say).await;
    let terminal = TerminalId::generate();
    let item = h.wrote(agent, terminal, 1, "git status", false).await;

    let outcome = h
        .send_refs(agent, "And the tests", Vec::new(), SendMode::Queue)
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    let card = h.session_card(agent, &item);
    assert_eq!((card_of(&card).1, card_of(&card).2), (true, false));
    // The turn still runs, and the next `!` starts a new card.
    let (_, items, _) = reset(h.open(agent, None, 50).await);
    let reply = items
        .iter()
        .find(|item| item.id.0 == "r1")
        .expect("the reply");
    assert_eq!(reply.status, ItemStatus::Running);
    let next = h.wrote(agent, terminal, 2, "git diff", false).await;
    assert_ne!(next, item);
    assert!(!card_of(&h.session_card(agent, &next)).1);
}

#[tokio::test]
async fn only_a_person_seals_and_every_sender_gets_the_scrubbed_pill() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let terminal = TerminalId::generate();
    let item = h.wrote(agent, terminal, 1, "ls", false).await;
    h.wrote(agent, terminal, 2, "cat .env", true).await;
    let both = || pill(Some(&item), &["ls", "cat .env"]);

    // An agent's tools send through a connection no person is behind.
    let mut tool = Inbox {
        client: h.sessions.tool_client(ClientId::derive("agent-tools")),
        kept: Vec::new(),
    };
    let key = IdempotencyKey::generate();
    tool.send(ClientMessage::SendInput {
        key,
        agent,
        input: UserInput {
            text: "See \u{FFFC}".into(),
            attachments: Vec::new(),
            refs: vec![both()],
        },
        mode: SendMode::Queue,
    });
    assert_eq!(tool.result(key).await, ActionOutcome::Done);
    assert_eq!(pill_commands(&h.last_refs(agent)), [["ls"]]);
    assert!(!card_of(&h.session_card(agent, &item)).1);

    // Allowing a prompt seals nothing.
    let asked = h.prompt(agent, "Run cargo test?");
    h.ask(session, &asked).await;
    let key = IdempotencyKey::generate();
    h.phone.send(answer(key, asked.id));
    assert_eq!(h.phone.answered(key).await, AnswerOutcome::Accepted);
    assert!(!card_of(&h.session_card(agent, &item)).1);

    // A typed answer that takes the pill along seals its card, and the
    // agent reads only the shared block.
    let asked = h.prompt(agent, "Which one?");
    h.ask(session, &asked).await;
    let typed = "This: \u{FFFC}";
    let key = IdempotencyKey::generate();
    h.phone.send(ClientMessage::Answer {
        idempotency_key: key,
        event: asked.id,
        answer: AttentionAnswer::Text { text: typed.into() },
        refs: vec![both()],
    });
    assert_eq!(h.phone.answered(key).await, AnswerOutcome::Accepted);
    let text = maxhq_core::context::encode(typed, &[pill(Some(&item), &["ls"])], &[], agent).text;
    assert!(!text.contains(".env"), "{text}");
    let sent = serde_json::json!(AttentionAnswer::Text { text });
    h.logged(&format!("answered {sent}")).await;
    let card = h.session_card(agent, &item);
    assert_eq!((card_of(&card).1, card_of(&card).2), (true, true));
}

#[tokio::test]
async fn a_card_stays_in_place_and_open_across_a_restart() {
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
    let terminal = TerminalId::generate();
    let item = h.wrote(agent, terminal, 1, "pnpm test", false).await;
    let second = Say {
        timeline: vec![upsert(agent, said(agent, "m2", 30))],
        ..Say::default()
    };
    h.say(session, &second).await;

    h.restart().await;
    let (_, items, _) = reset(h.open(agent, None, 50).await);
    assert_eq!(ids(&items), ["m1", item.0.as_str(), "m2"]);

    // The block that ran across the restart ends in its card.
    let at = card_of(&h.session_card(agent, &item)).0[0].started_at;
    h.block_ended(agent, terminal, 1, Timestamp(at.0 + 20), "ok");
    eventually("the end kept", || {
        h.session_card(agent, &item).status == ItemStatus::Completed
    })
    .await;
    // The card is still open, so the next command joins it.
    assert_eq!(h.wrote(agent, terminal, 2, "pnpm lint", false).await, item);
}

#[tokio::test]
async fn every_vendor_reads_only_the_shared_blocks() {
    let mut h = Harness::new();
    let (claude, _) = h.start_claude(None).await;
    let (codex, _) = h.start_codex("thread-1").await;
    let account = h.acp_agent(false);
    let mut spec = h.spec(Vendor::Acp, None);
    spec.account = account;
    let acp = started(h.act(|key| ClientMessage::StartAgent { key, spec }).await);

    for (agent, shows) in [
        (claude, Shows::Tool),
        (codex, Shows::Tool),
        (acp, Shows::Fence),
    ] {
        let terminal = TerminalId::generate();
        let item = h.wrote(agent, terminal, 1, "cd apps/web", false).await;
        h.wrote(agent, terminal, 2, "export TOKEN=canary-1f2e", true)
            .await;
        h.wrote(agent, terminal, 3, "pnpm test", false).await;
        let commands = ["cd apps/web", "export TOKEN=canary-1f2e", "pnpm test"];
        let text = "Why does \u{FFFC} fail?";
        let outcome = h
            .send_refs(
                agent,
                text,
                vec![pill(Some(&item), &commands)],
                SendMode::Queue,
            )
            .await;
        assert_eq!(outcome, ActionOutcome::Done, "{shows:?}");
        let refs = h.last_refs(agent);
        assert_eq!(pill_commands(&refs), [["cd apps/web", "pnpm test"]]);
        let encoded = maxhq_core::context::encode_for(text, &refs, &[], agent, shows).text;
        assert!(
            encoded.starts_with("Why does [Shell session 1: 2 commands] fail?"),
            "{encoded}"
        );
        assert!(
            encoded.contains("I ran these on devbox, in one shell:"),
            "{encoded}"
        );
        assert!(encoded.contains("$ pnpm test"), "{encoded}");
        assert!(!encoded.contains("canary"), "{encoded}");
    }

    // A pill whose card isn't here keeps no blocks, though it stays in
    // its place.
    let elsewhere = pill(Some(&ItemId("session-elsewhere-1".into())), &["whoami"]);
    let outcome = h
        .send_refs(claude, "See \u{FFFC}", vec![elsewhere], SendMode::Queue)
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    let empty: [[&str; 0]; 1] = [[]];
    assert_eq!(pill_commands(&h.last_refs(claude)), empty);
    let none = pill(None, &["whoami"]);
    let outcome = h
        .send_refs(claude, "See \u{FFFC}", vec![none], SendMode::Queue)
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    assert_eq!(pill_commands(&h.last_refs(claude)), empty);
}

#[tokio::test]
async fn a_new_agent_reads_only_the_shared_blocks_of_its_first_message() {
    let mut h = Harness::new();
    let (lead, _) = h.start_claude(None).await;
    let terminal = TerminalId::generate();
    let item = h.wrote(lead, terminal, 1, "make", false).await;
    h.wrote(lead, terminal, 2, "cat ~/.netrc", true).await;

    let mut spec = h.spec(Vendor::Claude, Some("Look at \u{FFFC}"));
    if let Some(first) = &mut spec.first {
        first.refs = vec![pill(Some(&item), &["make", "cat ~/.netrc"])];
    }
    let agent = started(h.act(|key| ClientMessage::StartAgent { key, spec }).await);
    eventually("the first message", || {
        h.vendors.sent_refs().iter().any(|(to, _)| *to == agent)
    })
    .await;
    assert_eq!(pill_commands(&h.last_refs(agent)), [["make"]]);
}

#[tokio::test]
async fn a_session_is_only_for_a_lead_this_device_runs_here() {
    let mut h = Harness::new();
    let nobody = AgentId::derive("nobody");
    let key = h.bang(nobody, "ls", false, false);
    let outcome = h.phone.result(key).await;
    assert!(
        reason(&outcome).contains("MaxHQ runs on this device"),
        "{outcome:?}"
    );

    let (lead, session) = h.start_claude(None).await;
    let sub = add_sub(&h, lead, session).await;
    let key = h.bang(sub.id, "ls", false, false);
    let outcome = h.phone.result(key).await;
    assert!(
        reason(&outcome).contains("agent that started this one"),
        "{outcome:?}"
    );
    let key = h.through_terminals(|key| ClientMessage::SessionCd {
        key,
        agent: sub.id,
        path: FOLDER.into(),
    });
    let outcome = h.phone.result(key).await;
    assert!(
        reason(&outcome).contains("agent that started this one"),
        "{outcome:?}"
    );
    assert!(h.fakes.terminals.sessions().is_empty());
}

#[tokio::test]
async fn a_session_for_an_agent_in_a_container_is_refused() {
    let mut h = Harness::on(server());
    h.container(None);
    let (agent, _) = h.start_boxed_claude(None).await;
    let key = h.bang(agent, "ls", false, false);
    let outcome = h.phone.result(key).await;
    assert!(reason(&outcome).contains("containers"), "{outcome:?}");
    let key = h.through_terminals(|key| ClientMessage::SessionCd {
        key,
        agent,
        path: FOLDER.into(),
    });
    let outcome = h.phone.result(key).await;
    assert!(reason(&outcome).contains("containers"), "{outcome:?}");
    assert!(h.fakes.terminals.sessions().is_empty());
}

#[tokio::test]
async fn a_command_that_goes_in_after_its_agent_went_ends_the_session() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let outcome = h.act(|key| ClientMessage::StopAgent { key, agent }).await;
    assert_eq!(outcome, ActionOutcome::Done);
    eventually("the holder's files going", || h.holders.forgotten(session)).await;
    let key = h.bang(agent, "ls", false, false);
    h.session_request(key).await;
    let outcome = h.act(|key| ClientMessage::DeleteAgent { key, agent }).await;
    assert_eq!(outcome, ActionOutcome::Done);

    h.fakes.terminals.note(RunNote::Session {
        key,
        agent,
        result: SessionResult::Wrote {
            terminal: TerminalId::generate(),
            block: 1,
            command: "ls".into(),
            cwd: FOLDER.into(),
            private: false,
            at: Timestamp::now(),
        },
    });
    let outcome = h.phone.result(key).await;
    assert_eq!(
        reason(&outcome),
        "The agent ended before the session took that."
    );
    // Once for the delete, once for the session that took it after.
    assert_eq!(h.fakes.terminals.ended(), [agent, agent]);
    assert!(h.db.user_runs::<TimelineItem>(agent).unwrap().is_empty());
}

/// Waits until the manager remembers `want` of `agent`'s cards: how many
/// open ones, once read, and how many blocks that run.
async fn remembers(h: &Harness, agent: AgentId, want: (Option<usize>, usize)) {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let now = h.sessions.remembered(agent).await;
        if now == want {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the manager remembers {now:?}, not {want:?}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn the_manager_lets_go_of_cards_and_blocks_it_no_longer_needs() {
    let mut h = Harness::new();
    let (agent, _) = h.start_claude(None).await;
    reset(h.open(agent, None, 50).await);

    // A sealed card's block whose session runs stays until it ends.
    let live = opened(&h, agent, TerminalKind::Session);
    h.hub.apply([Change::Upsert {
        value: Entity::Terminal(live.clone()),
    }]);
    h.wrote(agent, live.id, 1, "pnpm dev", false).await;
    remembers(&h, agent, (Some(1), 1)).await;
    let outcome = h
        .send_refs(agent, "Thanks", Vec::new(), SendMode::Queue)
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    remembers(&h, agent, (Some(0), 1)).await;
    h.block_ended(agent, live.id, 1, Timestamp::now(), "ready");
    remembers(&h, agent, (Some(0), 0)).await;

    // One whose session ended goes as its card is sealed, and still ends
    // in that card when the end comes late.
    let gone = TerminalId::generate();
    let item = h.wrote(agent, gone, 1, "make", false).await;
    remembers(&h, agent, (Some(1), 1)).await;
    let outcome = h
        .send_refs(agent, "And now?", Vec::new(), SendMode::Queue)
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    remembers(&h, agent, (Some(0), 0)).await;
    h.block_ended(agent, gone, 1, Timestamp::now(), "built");
    eventually("the late end in the sealed card", || {
        let card = h.session_card(agent, &item);
        let (blocks, sealed, _) = card_of(&card);
        sealed && matches!(blocks[0].process, TerminalStatus::Exited { .. })
    })
    .await;

    // Deleting the agent lets go of its open cards and blocks.
    h.wrote(agent, live.id, 2, "ls", false).await;
    remembers(&h, agent, (Some(1), 1)).await;
    let outcome = h.act(|key| ClientMessage::DeleteAgent { key, agent }).await;
    assert_eq!(outcome, ActionOutcome::Done);
    remembers(&h, agent, (None, 0)).await;
}
