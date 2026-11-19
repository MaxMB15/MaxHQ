// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::time::Duration;

use maxhq_core::tools::Tool;
use maxhq_hold::{Dirs, Mark, ShellCommand};
use maxhq_protocol::frame::{Frame, FrameDecoder};
use maxhq_protocol::ids::PhoneId;
use tokio::io::AsyncReadExt;
use tokio::time::timeout;

use super::access::PHONE_NOT_ITS_OWN;
use super::*;
use crate::testing::TempDir;

const WAIT: Duration = Duration::from_secs(10);

/// Terminals in a folder of their own, and what they tell the manager.
struct Fixture {
    terminals: Terminals,
    notes: mpsc::UnboundedReceiver<RunNote>,
    hub: Hub,
    _dir: TempDir,
}

fn fixture() -> Fixture {
    let dir = TempDir::new();
    let hub = crate::secrets::testing::hub();
    let holds = maxhq_hold::Terminals::new(Dirs {
        sessions: dir.path().join("terminals"),
        sockets: dir.path().join("holders"),
    });
    let wiring = Terminals::wire(holds, hub.clone());
    Fixture {
        terminals: wiring.terminals.expect("real terminals"),
        notes: wiring.notes,
        hub,
        _dir: dir,
    }
}

fn run() -> TerminalKind {
    TerminalKind::Run {
        command: "pnpm test".into(),
        private: false,
    }
}

/// A terminal as the hub shows it.
fn published(hub: &Hub, terminal: TerminalId) -> Option<Terminal> {
    match hub.get(&EntityKey::Terminal(terminal)) {
        Some(Entity::Terminal(terminal)) => Some(terminal),
        _ => None,
    }
}

/// A wire to nowhere, for an attach that never gets as far as sending.
fn nowhere() -> Sender {
    maxhq_wire::spawn(tokio::io::sink()).0
}

fn refused(reason: &str) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.to_owned(),
    }
}

#[tokio::test]
async fn a_request_for_a_terminal_nobody_holds_says_so() {
    let f = fixture();
    let terminal = TerminalId::generate();
    let attached = f
        .terminals
        .attach(1, Caller::App, nowhere(), terminal, None);
    assert_eq!(attached.await, Err(Refusal::unknown()));
    let typed = f
        .terminals
        .input(1, Caller::App, terminal, b"ls\r".to_vec());
    assert_eq!(typed, Err(Refusal::unknown()));
    let resized = f.terminals.resize(1, Caller::App, terminal, 100, 30);
    assert_eq!(resized, Err(Refusal::unknown()));
    let missing = refused(NO_SUCH_TERMINAL);
    assert_eq!(f.terminals.interrupt(Caller::App, terminal), missing);
    assert_eq!(f.terminals.rename(Caller::App, terminal, "dev"), missing);
    assert_eq!(f.terminals.move_run(Caller::App, terminal), missing);
    // A close sent again after the terminal went finds it gone, which is
    // what it asked for.
    assert_eq!(
        f.terminals.close(Caller::App, terminal, false),
        ActionOutcome::Done
    );
    // Nothing reaches the connection as a bad message but the ID.
    assert_eq!(
        Refusal::unknown().into_error(),
        (ErrorCode::BadMessage, NO_SUCH_TERMINAL.to_owned())
    );
}

#[tokio::test]
async fn requests_wait_while_the_terminals_a_previous_maxd_left_come_back() {
    let f = fixture();
    // With no return under way, nothing waits.
    timeout(Duration::from_secs(1), f.terminals.restored())
        .await
        .expect("a wait with no return under way");
    let restore = f.terminals.restoring();
    let waiting = tokio::spawn({
        let terminals = f.terminals.clone();
        async move { terminals.restored().await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!waiting.is_finished(), "a request went ahead of the return");
    restore.await;
    timeout(Duration::from_secs(1), waiting)
        .await
        .expect("the request waited on after the return")
        .unwrap();
}

#[tokio::test]
async fn a_phone_types_only_into_its_own_runs_until_its_switch_is_on() {
    let mut f = fixture();
    let agent = AgentId::derive("a");
    let mine = PhoneId::derive("mine");
    let me = Caller::Phone {
        phone: mine,
        everything: false,
    };
    let apps = f.terminals.plant(agent, run(), Opener::App);
    let others = f.terminals.plant(
        agent,
        run(),
        Opener::Phone {
            phone: PhoneId::derive("another"),
        },
    );
    let shell = f.terminals.plant(agent, TerminalKind::Shell, Opener::App);
    let own = f
        .terminals
        .plant(agent, run(), Opener::Phone { phone: mine });
    let not_its_own = || Err(Refusal::NotAllowed(PHONE_NOT_ITS_OWN.to_owned()));

    for terminal in [apps, others, shell] {
        assert_eq!(
            f.terminals.input(2, me, terminal, b"y".to_vec()),
            not_its_own()
        );
        assert_eq!(f.terminals.resize(2, me, terminal, 40, 20), not_its_own());
        assert_eq!(
            f.terminals.close(me, terminal, true),
            refused(PHONE_NOT_ITS_OWN)
        );
        assert_eq!(
            f.terminals.rename(me, terminal, "mine"),
            refused(PHONE_NOT_ITS_OWN)
        );
        assert_eq!(
            f.terminals.move_run(me, terminal),
            refused(PHONE_NOT_ITS_OWN)
        );
        // It watches any terminal and sends it Ctrl-C. These have no
        // holder to reach, so each gets as far as trying.
        assert_eq!(f.terminals.interrupt(me, terminal), refused(UNREACHABLE));
        let attached = f.terminals.attach(2, me, nowhere(), terminal, None).await;
        assert!(
            matches!(&attached, Err(Refusal::Unknown(reason)) if reason != NO_SUCH_TERMINAL),
            "{attached:?}"
        );
        let shown = published(&f.hub, terminal).expect("still shown");
        assert_eq!((shown.cols, shown.rows, shown.moved), (80, 24, false));
        assert_ne!(shown.title, "mine");
    }
    assert!(f.notes.try_recv().is_err(), "nothing moved");

    // Its own run it types into, sizes, renames and moves.
    let typed = f.terminals.input(2, me, own, b"y".to_vec());
    assert_eq!(typed, Err(Refusal::unreachable()));
    assert_eq!(f.terminals.resize(2, me, own, 40, 20), Ok(()));
    assert_eq!(f.terminals.rename(me, own, "tests"), ActionOutcome::Done);
    assert_eq!(f.terminals.move_run(me, own), ActionOutcome::Done);
    let shown = published(&f.hub, own).expect("still shown");
    assert_eq!(
        (shown.title.as_str(), shown.moved, shown.cols, shown.rows),
        ("tests", true, 40, 20)
    );
    assert!(matches!(
        f.notes.try_recv(),
        Ok(RunNote::Moved { agent: moved, terminal }) if moved == agent && terminal == own
    ));

    // With its switch on, it does what the app does.
    let trusted = Caller::Phone {
        phone: mine,
        everything: true,
    };
    for terminal in [apps, others, shell] {
        let typed = f.terminals.input(2, trusted, terminal, b"y".to_vec());
        assert_eq!(typed, Err(Refusal::unreachable()));
        assert_eq!(
            f.terminals.rename(trusted, terminal, "named"),
            ActionOutcome::Done
        );
    }
}

#[test]
fn the_opener_sizes_a_terminal_until_someone_types_then_the_last_to_type_does() {
    let note = Note {
        agent: AgentId::derive("a"),
        kind: TerminalKind::Shell,
        cwd: "~".into(),
        started_at: Timestamp(1),
        opener: Opener::App,
        moved: false,
        name: None,
    };
    let mut open = Open::new(note, None, DeviceId::derive("host"), TerminalId::generate());
    let size = |open: &Open| (open.entity.cols, open.entity.rows);
    open.sizer = Some(1);
    // A phone attaches and says its size, which waits while the opener's
    // applies.
    assert!(!open.resize(2, 40, 20));
    assert_eq!(size(&open), (80, 24));
    assert!(open.resize(1, 120, 30));
    assert_eq!(size(&open), (120, 30));
    // The phone types, so its size applies, and the opener's waits.
    assert!(open.typed(2));
    assert_eq!(size(&open), (40, 20));
    assert!(!open.typed(2));
    assert!(!open.resize(1, 100, 30));
    assert!(open.resize(2, 50, 25));
    // The Mac types again and gets its size back.
    assert!(open.typed(1));
    assert_eq!(size(&open), (100, 30));
    // A connection that never said a size takes over without changing it.
    assert!(!open.typed(3));
    assert_eq!(size(&open), (100, 30));
}

#[test]
fn a_connection_that_goes_no_longer_sizes_a_terminal() {
    let f = fixture();
    let terminal = f
        .terminals
        .plant(AgentId::derive("a"), TerminalKind::Shell, Opener::App);
    let size = || {
        let shown = published(&f.hub, terminal).expect("shown");
        (shown.cols, shown.rows)
    };
    // One a previous maxd left has nobody sizing it, so the first to ask
    // does.
    assert_eq!(
        f.terminals.resize(1, Caller::App, terminal, 120, 30),
        Ok(())
    );
    assert_eq!(size(), (120, 30));
    assert_eq!(f.terminals.resize(2, Caller::App, terminal, 40, 20), Ok(()));
    assert_eq!(size(), (120, 30));
    f.terminals.gone(1);
    assert_eq!(f.terminals.resize(2, Caller::App, terminal, 40, 20), Ok(()));
    assert_eq!(size(), (40, 20));
}

/// Hands a terminal `events` as its quiet link would, then drops the link.
async fn heard(terminals: &Terminals, terminal: TerminalId, events: Vec<TermEvent>) {
    let (tx, rx) = mpsc::channel(events.len().max(1));
    for event in events {
        tx.send(event).await.unwrap();
    }
    drop(tx);
    timeout(WAIT, terminals.clone().follow(terminal, rx))
        .await
        .expect("the link followed in time");
}

#[tokio::test]
async fn a_shell_says_whether_it_runs_something() {
    let f = fixture();
    let shell = f
        .terminals
        .plant(AgentId::derive("a"), TerminalKind::Shell, Opener::App);
    let busy = || published(&f.hub, shell).expect("a tab").busy;
    assert!(!busy());
    heard(&f.terminals, shell, vec![TermEvent::Busy(true)]).await;
    assert!(busy());
    heard(&f.terminals, shell, vec![TermEvent::Busy(false)]).await;
    assert!(!busy());
    // One that ended runs nothing, whatever it said last.
    let exit = TermExit {
        code: Some(0),
        signal: None,
        at: Timestamp(2),
        output: None,
    };
    heard(
        &f.terminals,
        shell,
        vec![TermEvent::Busy(true), TermEvent::Exit(exit)],
    )
    .await;
    assert!(!busy());
}

#[tokio::test]
async fn a_shell_that_marks_its_commands_is_busy_from_start_to_end() {
    let f = fixture();
    let shell = f
        .terminals
        .plant(AgentId::derive("a"), TerminalKind::Shell, Opener::App);
    let busy = || published(&f.hub, shell).expect("a tab").busy;
    let mark = |offset, mark| TermEvent::Mark {
        offset,
        at: Timestamp(offset),
        mark,
    };
    // Until its first mark, the holder says.
    heard(&f.terminals, shell, vec![TermEvent::Busy(true)]).await;
    assert!(busy());
    let prompt = vec![mark(0, Mark::Prompt), mark(7, Mark::Input)];
    heard(&f.terminals, shell, prompt).await;
    assert!(!busy());
    // From then on the marks do, whatever the holder thinks.
    let cwd = Some("/tmp/example-app".to_owned());
    let command = vec![
        mark(12, Mark::Command { cwd: cwd.clone() }),
        TermEvent::Busy(false),
    ];
    heard(&f.terminals, shell, command).await;
    assert!(busy());
    let done = vec![
        mark(40, Mark::Done { exit: Some(2) }),
        TermEvent::Busy(true),
        mark(44, Mark::Prompt),
    ];
    heard(&f.terminals, shell, done).await;
    assert!(!busy());
    let kept = f.terminals.lock()[&shell].commands.list().clone();
    assert_eq!(
        kept.front(),
        Some(&ShellCommand {
            prompt: Some(0),
            input: Some(7),
            start: Some(12),
            started_at: Some(Timestamp(12)),
            end: Some(40),
            ended_at: Some(Timestamp(40)),
            exit: Some(2),
            cwd,
        })
    );
    assert_eq!(kept.len(), 2, "and the prompt after it");
}

#[tokio::test]
async fn the_shell_integration_switch_decides_whether_shells_get_hooks() {
    let f = fixture();
    let zsh = || {
        let mut launch = TermLaunch::shell(TerminalId::generate(), "/".into(), 80, 24);
        launch.program = "/bin/zsh".into();
        launch
    };
    let mut on = zsh();
    f.terminals.integrate(&mut on);
    assert!(on.nonce.is_some());
    assert!(on.env.iter().any(|(key, _)| key == "ZDOTDIR"));
    let key = EntityKey::DeviceSettings(f.hub.device().id);
    let Some(Entity::DeviceSettings(settings)) = f.hub.get(&key) else {
        panic!("a new hub has settings");
    };
    assert!(settings.shell_integration, "on unless you turn it off");
    f.hub.apply([Change::Upsert {
        value: Entity::DeviceSettings(maxhq_protocol::model::DeviceSettings {
            shell_integration: false,
            ..settings
        }),
    }]);
    let mut off = zsh();
    f.terminals.integrate(&mut off);
    let plain = zsh();
    assert_eq!(off.nonce, None);
    assert_eq!((off.args, off.env), (plain.args, plain.env));
}

#[tokio::test]
async fn closing_a_terminal_that_runs_something_asks_first() {
    let f = fixture();
    let agent = AgentId::derive("a");
    let app = Caller::App;
    let ran = f.terminals.plant(agent, run(), Opener::App);
    assert_eq!(f.terminals.close(app, ran, false), refused(STILL_RUNNING));
    let shell = f.terminals.plant(agent, TerminalKind::Shell, Opener::App);
    heard(&f.terminals, shell, vec![TermEvent::Busy(true)]).await;
    assert_eq!(f.terminals.close(app, shell, false), refused(STILL_RUNNING));
    // Told to go ahead, it kills it, but this one's holder can't be
    // reached.
    assert_eq!(f.terminals.close(app, shell, true), refused(UNREACHABLE));
    // Back at its prompt, it doesn't ask.
    heard(&f.terminals, shell, vec![TermEvent::Busy(false)]).await;
    assert_eq!(f.terminals.close(app, shell, false), refused(UNREACHABLE));
    assert!(published(&f.hub, shell).is_some());
}

#[test]
fn an_ended_shell_stays_until_you_close_it() {
    let mut f = fixture();
    let shell = f
        .terminals
        .plant(AgentId::derive("a"), TerminalKind::Shell, Opener::App);
    f.terminals.ended(
        shell,
        Some(TermExit {
            code: Some(0),
            signal: None,
            at: Timestamp(2),
            output: None,
        }),
    );
    let shown = published(&f.hub, shell).expect("still a tab");
    assert_eq!(
        shown.status,
        TerminalStatus::Exited {
            code: Some(0),
            signal: None
        }
    );
    // A shell's end is the drawer's business, not the timeline's.
    assert!(f.notes.try_recv().is_err());
    assert_eq!(
        f.terminals.close(Caller::App, shell, false),
        ActionOutcome::Done
    );
    assert!(published(&f.hub, shell).is_none());
}

#[test]
fn a_run_goes_once_its_end_is_kept_unless_it_moved_to_the_drawer() {
    let mut f = fixture();
    let agent = AgentId::derive("a");
    let ran = f.terminals.plant(agent, run(), Opener::App);
    let moved = f.terminals.plant(agent, run(), Opener::App);
    assert_eq!(
        f.terminals.move_run(Caller::App, moved),
        ActionOutcome::Done
    );
    assert!(matches!(f.notes.try_recv(), Ok(RunNote::Moved { terminal, .. }) if terminal == moved));
    for terminal in [ran, moved] {
        f.terminals.ended(terminal, None);
        assert!(matches!(
            f.notes.try_recv(),
            Ok(RunNote::Ended { terminal: ended, exit: None, .. }) if ended == terminal
        ));
        f.terminals.release(terminal);
    }
    assert!(published(&f.hub, ran).is_none());
    // A moved run stays a tab that says it ended.
    let shown = published(&f.hub, moved).expect("still a tab");
    assert!(matches!(shown.status, TerminalStatus::Exited { .. }));
    assert_eq!(
        f.terminals.close(Caller::App, moved, false),
        ActionOutcome::Done
    );
    assert!(published(&f.hub, moved).is_none());
}

#[test]
fn an_agent_that_ends_takes_its_terminals_with_it() {
    let f = fixture();
    let (a, b) = (AgentId::derive("a"), AgentId::derive("b"));
    let shell = f.terminals.plant(a, TerminalKind::Shell, Opener::App);
    let moved = f.terminals.plant(a, run(), Opener::App);
    assert_eq!(
        f.terminals.move_run(Caller::App, moved),
        ActionOutcome::Done
    );
    let other = f.terminals.plant(b, TerminalKind::Shell, Opener::App);
    f.terminals.end_agent(a);
    assert!(published(&f.hub, shell).is_none());
    assert!(published(&f.hub, moved).is_none());
    assert!(published(&f.hub, other).is_some());
}

/// What a pump sent, decoded.
#[derive(Debug, PartialEq, Eq)]
enum Sent {
    Attached { from: u64, reset: bool },
    Output { offset: u64, bytes: Vec<u8> },
}

/// Runs a pump over `events`, as an attach does, and gives what reached
/// the connection.
async fn pumped(from: u64, reset: bool, events: Vec<TermEvent>) -> Vec<Sent> {
    let terminal = TerminalId::derive("pumped");
    let (writer, mut reader) = tokio::io::duplex(1 << 20);
    let (wire, _writing) = maxhq_wire::spawn(writer);
    let (tx, rx) = mpsc::channel(events.len().max(1));
    for event in events {
        tx.send(event).await.unwrap();
    }
    drop(tx);
    let pumping = tokio::spawn(pump(terminal, from, reset, rx, wire));
    let mut bytes = Vec::new();
    let read = timeout(WAIT, reader.read_to_end(&mut bytes)).await;
    read.expect("the pump in time").unwrap();
    pumping.await.unwrap();

    let mut decoder = FrameDecoder::new();
    decoder.push(&bytes);
    let mut sent = Vec::new();
    while let Some(frame) = decoder.next_frame().unwrap() {
        match frame {
            Frame::Message(json) => match serde_json::from_slice(json).unwrap() {
                ServerMessage::TerminalAttached {
                    terminal: to,
                    from,
                    reset,
                } => {
                    assert_eq!(to, terminal);
                    sent.push(Sent::Attached { from, reset });
                }
                other => panic!("expected an attach, got {other:?}"),
            },
            Frame::TerminalOutput {
                terminal: to,
                offset,
                bytes,
            } => {
                assert_eq!(to, terminal);
                assert!(bytes.len() <= MAX_TERMINAL_CHUNK);
                sent.push(Sent::Output {
                    offset,
                    bytes: bytes.to_vec(),
                });
            }
            other => panic!("expected terminal output, got {other:?}"),
        }
    }
    assert_eq!(decoder.buffered(), 0);
    sent
}

#[tokio::test]
async fn an_attach_says_where_output_starts_then_sends_it_in_chunks() {
    let printed: Vec<u8> = (0..MAX_TERMINAL_CHUNK * 2 + 5)
        .map(|i| (i % 251) as u8)
        .collect();
    let events = vec![
        TermEvent::Password(true),
        TermEvent::Output {
            offset: 10,
            data: printed.clone(),
        },
        TermEvent::Title("make".into()),
    ];
    let sent = pumped(10, false, events).await;
    let chunk = MAX_TERMINAL_CHUNK as u64;
    let expected = vec![
        Sent::Attached {
            from: 10,
            reset: false,
        },
        Sent::Output {
            offset: 10,
            bytes: printed[..MAX_TERMINAL_CHUNK].to_vec(),
        },
        Sent::Output {
            offset: 10 + chunk,
            bytes: printed[MAX_TERMINAL_CHUNK..MAX_TERMINAL_CHUNK * 2].to_vec(),
        },
        Sent::Output {
            offset: 10 + 2 * chunk,
            bytes: printed[MAX_TERMINAL_CHUNK * 2..].to_vec(),
        },
    ];
    assert_eq!(sent, expected);
}

#[tokio::test]
async fn a_connection_that_fell_behind_hears_it_starts_over() {
    let output = |offset: u64, text: &str| TermEvent::Output {
        offset,
        data: text.as_bytes().to_vec(),
    };
    let events = vec![
        output(0, "hello"),
        output(5, " world"),
        output(100, "later\r\n"),
    ];
    let sent = pumped(0, false, events).await;
    let expected = vec![
        Sent::Attached {
            from: 0,
            reset: false,
        },
        Sent::Output {
            offset: 0,
            bytes: b"hello".to_vec(),
        },
        Sent::Output {
            offset: 5,
            bytes: b" world".to_vec(),
        },
        Sent::Attached {
            from: 100,
            reset: true,
        },
        Sent::Output {
            offset: 100,
            bytes: b"later\r\n".to_vec(),
        },
    ];
    assert_eq!(sent, expected);

    // One that asked for more than the holder keeps starts over at once.
    let sent = pumped(50, true, vec![output(50, "$ ")]).await;
    assert_eq!(
        sent[0],
        Sent::Attached {
            from: 50,
            reset: true
        }
    );
}

/// No agent tool reads or writes a terminal: none is about one, and the
/// code that serves agents' tools, over `maxd mcp` or from another device,
/// never reaches the terminals. A tool that did would let an agent, or
/// what it read, type into your shell or read what you typed.
#[test]
fn no_agent_tool_reaches_a_terminal() {
    for tool in Tool::ALL.into_iter().chain(Tool::DISPATCHER) {
        let about = format!(
            "{} {} {} {}",
            tool.name(),
            tool.title(),
            tool.description(),
            tool.input_schema()
        )
        .to_lowercase();
        assert!(
            !about.contains("terminal"),
            "{} is about a terminal",
            tool.name()
        );
    }
    let sources = [
        (
            "core/src/tools.rs",
            include_str!("../../../core/src/tools.rs"),
        ),
        ("agents/mcp.rs", include_str!("../agents/mcp.rs")),
        (
            "session/agent_tools.rs",
            include_str!("../session/agent_tools.rs"),
        ),
        ("session/tools.rs", include_str!("../session/tools.rs")),
        (
            "session/note_tools.rs",
            include_str!("../session/note_tools.rs"),
        ),
        (
            "session/dispatcher.rs",
            include_str!("../session/dispatcher.rs"),
        ),
        ("peers/serve.rs", include_str!("../peers/serve.rs")),
    ];
    let reaches = [
        "terminals::",
        "Terminals",
        "ClientMessage::Terminal",
        "Frame::Terminal",
        "encode_terminal",
        "TermControl",
        "TermLink",
        "open_terminal",
        "terminal_open",
    ];
    for (file, source) in sources {
        for word in reaches {
            assert!(
                !source.contains(word),
                "{file} reaches terminals through {word}"
            );
        }
    }
}
