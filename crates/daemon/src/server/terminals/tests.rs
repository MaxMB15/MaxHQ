// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Who reaches terminals through a connection: the app and proven phones,
//! a phone only as far as its rules go, and nobody else.

use maxhq_core::prove::Kind;
use maxhq_protocol::ids::{AgentId, IdempotencyKey, PhoneId, TerminalId};
use maxhq_protocol::message::{ActionOutcome, ErrorCode};
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::terminal::{Terminal, TerminalKind};
use maxhq_protocol::timeline::ItemId;
use maxhq_protocol::{ClientMessage, ProtocolRange, ServerMessage};

use super::{NOT_FOR_TERMINALS, SESSION_THROUGH_RUNS};
use crate::server::proof::unproven;
use crate::server::tests::{hello, serve, Served, TestClient};
use crate::server::PHONE_CANT;
use crate::terminals::access::{Opener, PHONE_CANT_RUN, PHONE_NOT_ITS_OWN};
use crate::terminals::{NO_SUCH_TERMINAL, STILL_RUNNING, UNREACHABLE};

/// What an attach to a terminal with no holder behind it reads, once it
/// got past who may watch.
const ENDED: &str = "That terminal has ended, and its output is gone.";

fn run() -> TerminalKind {
    TerminalKind::Run {
        command: "pnpm test".into(),
        private: false,
    }
}

fn error(code: ErrorCode, message: &str) -> ServerMessage {
    ServerMessage::Error {
        code,
        message: message.to_owned(),
    }
}

fn refused(reason: &str) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.to_owned(),
    }
}

/// A terminal as the hub shows it.
fn published(served: &Served, terminal: TerminalId) -> Terminal {
    match served.hub.get(&EntityKey::Terminal(terminal)) {
        Some(Entity::Terminal(terminal)) => terminal,
        other => panic!("expected the terminal, got {other:?}"),
    }
}

/// Every request a connection makes of `terminal`, opening one for `agent`
/// first, then of `agent`'s session.
fn requests(agent: AgentId, terminal: TerminalId) -> Vec<ClientMessage> {
    let key = IdempotencyKey::generate;
    vec![
        ClientMessage::TerminalOpen {
            key: key(),
            agent,
            kind: run(),
            cols: 80,
            rows: 24,
        },
        ClientMessage::TerminalAttach {
            terminal,
            after: None,
        },
        ClientMessage::TerminalDetach { terminal },
        ClientMessage::TerminalResize {
            terminal,
            cols: 100,
            rows: 30,
        },
        ClientMessage::TerminalInterrupt {
            key: key(),
            terminal,
            force: false,
        },
        ClientMessage::TerminalClose {
            key: key(),
            terminal,
            force: true,
        },
        ClientMessage::TerminalRename {
            key: key(),
            terminal,
            title: "mine".into(),
        },
        ClientMessage::TerminalMove {
            key: key(),
            terminal,
        },
        ClientMessage::SessionRun {
            key: key(),
            agent,
            command: "whoami".into(),
            private: false,
            fresh: false,
            cols: 80,
            rows: 24,
        },
        ClientMessage::SessionShare {
            key: key(),
            agent,
            item: ItemId(format!("session-{terminal}-1")),
            block: 1,
            private: false,
        },
        ClientMessage::SessionCd {
            key: key(),
            agent,
            path: "~/code/example-app".into(),
        },
        ClientMessage::SessionEnd {
            key: key(),
            agent,
            force: true,
        },
    ]
}

/// A `!` command for `agent`'s session, under a new key.
fn session_run(agent: AgentId) -> ClientMessage {
    ClientMessage::SessionRun {
        key: IdempotencyKey::generate(),
        agent,
        command: "ls".into(),
        private: false,
        fresh: false,
        cols: 80,
        rows: 24,
    }
}

fn session_end(agent: AgentId, force: bool) -> ClientMessage {
    ClientMessage::SessionEnd {
        key: IdempotencyKey::generate(),
        agent,
        force,
    }
}

/// Sends a keyed request and gives its outcome.
async fn ask(client: &mut TestClient, message: ClientMessage) -> ActionOutcome {
    let key = message.action_key().expect("a keyed request");
    client.send(&message).await;
    client.outcome(key).await
}

#[tokio::test]
async fn the_app_reaches_terminals_and_hears_why_a_request_fails() {
    let served = serve();
    let (mut app, server) = TestClient::greeted(&served.socket).await;
    assert!(server.capabilities.terminals);

    // A terminal opens only for an agent MaxHQ runs here.
    let open = ClientMessage::TerminalOpen {
        key: IdempotencyKey::generate(),
        agent: AgentId::derive("nobody"),
        kind: run(),
        cols: 80,
        rows: 24,
    };
    let opened = ask(&mut app, open).await;
    assert!(
        matches!(&opened, ActionOutcome::Refused { reason } if reason.contains("agents MaxHQ runs on this device")),
        "{opened:?}"
    );

    // So does a session, which opens through `!` and the folder chip, not
    // as a terminal of its own.
    let nobody = AgentId::derive("nobody");
    let open_session = ClientMessage::TerminalOpen {
        key: IdempotencyKey::generate(),
        agent: nobody,
        kind: TerminalKind::Session,
        cols: 80,
        rows: 24,
    };
    assert_eq!(
        ask(&mut app, open_session).await,
        refused(SESSION_THROUGH_RUNS)
    );
    let ran = ask(&mut app, session_run(nobody)).await;
    assert!(
        matches!(&ran, ActionOutcome::Refused { reason } if reason.contains("agents MaxHQ runs on this device")),
        "{ran:?}"
    );
    let share = ClientMessage::SessionShare {
        key: IdempotencyKey::generate(),
        agent: nobody,
        item: ItemId("session-nowhere-1".into()),
        block: 1,
        private: true,
    };
    assert_eq!(
        ask(&mut app, share).await,
        refused("No session card here has that block.")
    );
    // Ending a session that isn't there finds what it asked for.
    assert_eq!(
        ask(&mut app, session_end(nobody, false)).await,
        ActionOutcome::Done
    );

    // One nobody holds is a bad ID, whatever the request.
    let nobodys = TerminalId::generate();
    let missing = || error(ErrorCode::BadMessage, NO_SUCH_TERMINAL);
    app.send(&ClientMessage::TerminalAttach {
        terminal: nobodys,
        after: Some(41),
    })
    .await;
    assert_eq!(app.recv().await, missing());
    app.send(&ClientMessage::TerminalResize {
        terminal: nobodys,
        cols: 100,
        rows: 30,
    })
    .await;
    assert_eq!(app.recv().await, missing());
    app.type_into(nobodys, b"ls\r").await;
    assert_eq!(app.recv().await, missing());
    let key = IdempotencyKey::generate;
    for message in [
        ClientMessage::TerminalInterrupt {
            key: key(),
            terminal: nobodys,
            force: false,
        },
        ClientMessage::TerminalRename {
            key: key(),
            terminal: nobodys,
            title: "dev".into(),
        },
        ClientMessage::TerminalMove {
            key: key(),
            terminal: nobodys,
        },
    ] {
        assert_eq!(ask(&mut app, message).await, refused(NO_SUCH_TERMINAL));
    }
    // A close sent again after the terminal went finds what it asked for.
    let close = ClientMessage::TerminalClose {
        key: key(),
        terminal: nobodys,
        force: false,
    };
    assert_eq!(ask(&mut app, close).await, ActionOutcome::Done);
    app.send(&ClientMessage::TerminalDetach { terminal: nobodys })
        .await;
    app.expect_quiet().await;

    // One it holds, whose holder can't be reached right now.
    let agent = AgentId::derive("a");
    let held = served.terminals.plant(agent, run(), Opener::App);
    let rename = ClientMessage::TerminalRename {
        key: key(),
        terminal: held,
        title: "  tests  ".into(),
    };
    assert_eq!(ask(&mut app, rename).await, ActionOutcome::Done);
    assert_eq!(published(&served, held).title, "tests");
    let interrupt = ClientMessage::TerminalInterrupt {
        key: key(),
        terminal: held,
        force: false,
    };
    assert_eq!(ask(&mut app, interrupt).await, refused(UNREACHABLE));
    let close = |force| ClientMessage::TerminalClose {
        key: key(),
        terminal: held,
        force,
    };
    assert_eq!(ask(&mut app, close(false)).await, refused(STILL_RUNNING));
    assert_eq!(ask(&mut app, close(true)).await, refused(UNREACHABLE));
    let moved = ClientMessage::TerminalMove {
        key: key(),
        terminal: held,
    };
    assert_eq!(ask(&mut app, moved).await, ActionOutcome::Done);
    assert!(published(&served, held).moved);
    app.type_into(held, b"q").await;
    assert_eq!(app.recv().await, error(ErrorCode::Unreachable, UNREACHABLE));
    app.send(&ClientMessage::TerminalAttach {
        terminal: held,
        after: None,
    })
    .await;
    assert_eq!(app.recv().await, error(ErrorCode::BadMessage, ENDED));
}

#[tokio::test]
async fn agents_tools_and_unproven_connections_never_reach_a_terminal() {
    let served = serve();
    let agent = AgentId::derive("a");
    let shell = served
        .terminals
        .plant(agent, TerminalKind::Shell, Opener::App);
    let before = published(&served, shell);

    // `maxd mcp`, which agents' tool calls come through.
    let mut tools = TestClient::connect(&served.socket).await;
    let ClientMessage::Hello(mut greeting) = hello(ProtocolRange::OURS) else {
        unreachable!("the helper says hello")
    };
    greeting.role = maxhq_protocol::agents::ClientRole::Mcp;
    tools.send(&ClientMessage::Hello(greeting)).await;
    assert!(matches!(tools.recv().await, ServerMessage::Hello(_)));
    for message in requests(agent, shell) {
        tools.send(&message).await;
        let answer = tools.recv().await;
        assert_eq!(
            answer,
            error(
                ErrorCode::NotAllowed,
                "this connection may only call agent tools"
            ),
            "{message:?}"
        );
    }
    tools.type_into(shell, b"whoami\r").await;
    assert_eq!(
        tools.recv().await,
        error(ErrorCode::NotAllowed, NOT_FOR_TERMINALS)
    );

    // A connection that proved no key.
    let (mut stranger, _) = TestClient::unproven(&served.socket).await;
    let refusal = unproven(&served.hub.device().name);
    for message in requests(agent, shell) {
        stranger.send(&message).await;
        let answer = stranger.recv().await;
        let expected = match message.action_key() {
            Some(key) => ServerMessage::ActionResult {
                key,
                outcome: refused(&refusal),
            },
            None => error(ErrorCode::NotAllowed, &refusal),
        };
        assert_eq!(answer, expected, "{message:?}");
    }
    stranger.type_into(shell, b"whoami\r").await;
    assert_eq!(
        stranger.recv().await,
        error(ErrorCode::NotAllowed, NOT_FOR_TERMINALS)
    );

    assert_eq!(published(&served, shell), before);
}

#[tokio::test]
async fn a_phone_runs_nothing_and_manages_only_its_own_runs_until_its_switch_is_on() {
    use base64::Engine;

    let phones = crate::phones::for_tests();
    let served = serve();
    let (mut app, _) = TestClient::greeted(&served.socket).await;
    // The phones store is one for every test, so this key is this test's.
    const SEED: [u8; 32] = [241; 32];
    let ssh_public = crate::phones::openssh_for_tests(&SEED);
    let phone = PhoneId::derive(&ssh_public);
    let add = ClientMessage::AddPhone {
        key: IdempotencyKey::generate(),
        phone,
        name: "iPhone".into(),
        ssh_public,
        push_public: base64::engine::general_purpose::STANDARD.encode([8u8; 32]),
    };
    let added = ask(&mut app, add).await;
    assert!(
        matches!(
            added,
            ActionOutcome::Done | ActionOutcome::PhoneAdded { .. }
        ),
        "{added:?}"
    );
    let mut own = TestClient::connect(&served.socket).await;
    own.send(&ClientMessage::Phone { phone }).await;
    own.send(&hello(ProtocolRange::OURS)).await;
    let ServerMessage::Hello(server) = own.recv().await else {
        panic!("expected a hello");
    };
    let proved = own
        .prove_as(Kind::Phone, &SEED, &server, &hello(ProtocolRange::OURS))
        .await;
    assert_eq!(proved, ActionOutcome::Done);

    let agent = AgentId::derive("a");
    let apps = served.terminals.plant(agent, run(), Opener::App);
    let shell = served
        .terminals
        .plant(agent, TerminalKind::Shell, Opener::App);
    let its_own = served
        .terminals
        .plant(agent, run(), Opener::Phone { phone });
    let key = IdempotencyKey::generate;

    // It watches any run and sends it Ctrl-C. These have no holder to
    // reach, so each gets as far as trying. A shell it doesn't watch.
    let cant_run = || error(ErrorCode::NotAllowed, PHONE_CANT_RUN);
    for terminal in [apps, its_own] {
        own.send(&ClientMessage::TerminalAttach {
            terminal,
            after: None,
        })
        .await;
        assert_eq!(own.recv().await, error(ErrorCode::BadMessage, ENDED));
        let interrupt = ClientMessage::TerminalInterrupt {
            key: key(),
            terminal,
            force: false,
        };
        assert_eq!(ask(&mut own, interrupt).await, refused(UNREACHABLE));
    }
    own.send(&ClientMessage::TerminalAttach {
        terminal: shell,
        after: None,
    })
    .await;
    assert_eq!(own.recv().await, cant_run());
    let interrupt = ClientMessage::TerminalInterrupt {
        key: key(),
        terminal: shell,
        force: false,
    };
    assert_eq!(ask(&mut own, interrupt).await, refused(PHONE_CANT_RUN));
    // It types into and sizes no terminal, not even a run it opened, and
    // manages only the runs it opened.
    for terminal in [apps, shell, its_own] {
        own.type_into(terminal, b"y").await;
        assert_eq!(own.recv().await, cant_run());
        own.send(&ClientMessage::TerminalResize {
            terminal,
            cols: 40,
            rows: 20,
        })
        .await;
        assert_eq!(own.recv().await, cant_run());
    }
    for terminal in [apps, shell] {
        let rename = ClientMessage::TerminalRename {
            key: key(),
            terminal,
            title: "mine".into(),
        };
        assert_eq!(ask(&mut own, rename).await, refused(PHONE_NOT_ITS_OWN));
    }
    let rename = ClientMessage::TerminalRename {
        key: key(),
        terminal: its_own,
        title: "mine".into(),
    };
    assert_eq!(ask(&mut own, rename).await, ActionOutcome::Done);
    // Nor does it open a shell or a run, run a `!` command, share a
    // session's block or move a session. It ends a session only once its
    // switch is on.
    let open_shell = || ClientMessage::TerminalOpen {
        key: key(),
        agent,
        kind: TerminalKind::Shell,
        cols: 80,
        rows: 24,
    };
    assert_eq!(ask(&mut own, open_shell()).await, refused(PHONE_CANT));
    let commands = requests(agent, its_own).into_iter().filter(|message| {
        matches!(
            message,
            ClientMessage::TerminalOpen { .. }
                | ClientMessage::SessionRun { .. }
                | ClientMessage::SessionShare { .. }
                | ClientMessage::SessionCd { .. }
        )
    });
    for message in commands {
        assert_eq!(
            ask(&mut own, message.clone()).await,
            refused(PHONE_CANT_RUN),
            "{message:?}"
        );
    }
    assert_eq!(
        ask(&mut own, session_end(agent, true)).await,
        refused(PHONE_NOT_ITS_OWN)
    );

    // The Mac turns its switch on, and the phone may use every terminal.
    let switch = |everything: bool| ClientMessage::SetPhoneEverything {
        key: IdempotencyKey::generate(),
        phone,
        everything,
    };
    assert_eq!(ask(&mut app, switch(true)).await, ActionOutcome::Done);
    assert!(phones.everything(phone));
    for terminal in [apps, shell, its_own] {
        own.type_into(terminal, b"y").await;
        assert_eq!(own.recv().await, error(ErrorCode::Unreachable, UNREACHABLE));
    }
    let opened = ask(&mut own, open_shell()).await;
    assert_ne!(opened, refused(PHONE_CANT));
    // It runs `!` commands, and gets as far as the agent, which isn't here.
    let ran = ask(&mut own, session_run(agent)).await;
    assert!(
        matches!(&ran, ActionOutcome::Refused { reason } if reason.contains("agents MaxHQ runs on this device")),
        "{ran:?}"
    );
    assert_eq!(
        ask(&mut own, session_end(agent, true)).await,
        ActionOutcome::Done
    );

    // Off again, it types into nothing.
    assert_eq!(ask(&mut app, switch(false)).await, ActionOutcome::Done);
    own.type_into(apps, b"y").await;
    assert_eq!(own.recv().await, cant_run());
}
