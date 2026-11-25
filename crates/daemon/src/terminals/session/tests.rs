// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::ffi::OsString;
use std::fs;
use std::io;
use std::process::ExitCode;

use maxhq_hold::{Detach, Dirs};
use maxhq_protocol::state::Change;
use tokio::time::timeout;

use super::*;
use crate::hub::Hub;
use crate::terminals::access::Caller;
use crate::testing::TempDir;

/// How long a real shell has for each step.
const WAIT: Duration = Duration::from_secs(20);

/// Where a holder these tests start lands. The test binary runs again with
/// only this test and the holder's arguments after `--`, and it becomes
/// the holder. Run with the others, it does nothing.
#[test]
fn holder_entry() {
    let args: Vec<OsString> = std::env::args_os().collect();
    let Some(pty) = args.iter().position(|arg| arg == "--pty") else {
        return;
    };
    let code = maxhq_hold::run_holder(&args[pty..]);
    std::process::exit(if code == ExitCode::SUCCESS { 0 } else { 1 });
}

/// Holders in `dir` that this test binary plays.
fn holds(dir: &Path) -> maxhq_hold::Terminals {
    let args = [
        "--exact",
        "terminals::session::tests::holder_entry",
        "--nocapture",
        "-q",
        "--test-threads=1",
        "--",
    ];
    maxhq_hold::Terminals::new(Dirs {
        sessions: dir.join("t"),
        sockets: dir.join("s"),
    })
    .with_holder(
        std::env::current_exe().expect("the test knows its program"),
        args.iter().map(OsString::from).collect(),
    )
    .with_detach(Detach::Setsid)
    .with_linger(Duration::from_secs(5))
}

/// Gives every shell `shell` and a home of its own, so no shell reads the
/// files of whoever runs the tests. A session gets `session` when set.
fn hook(home: &Path, shell: &str, session: Option<&str>) -> LaunchHook {
    let home = home.to_owned();
    let (shell, session) = (PathBuf::from(shell), session.map(PathBuf::from));
    Arc::new(move |launch: &mut TermLaunch| {
        launch.program = match (&session, launch.run) {
            (Some(session), false) => session.clone(),
            _ => shell.clone(),
        };
        let at = |name: &str| home.join(name).into_os_string();
        let set: [(&str, OsString); 13] = [
            ("HOME", home.clone().into_os_string()),
            ("ENV", "/dev/null".into()),
            ("BASH_ENV", "/dev/null".into()),
            ("ZDOTDIR", at("zdot")),
            ("XDG_CONFIG_HOME", at(".config")),
            ("XDG_DATA_HOME", at(".local/share")),
            ("XDG_DATA_DIRS", at("share")),
            ("TERM", "xterm-256color".into()),
            ("TERM_PROGRAM", "maxhq-test".into()),
            ("SHELL_SESSIONS_DISABLE", "1".into()),
            ("BASH_SILENCE_DEPRECATION_WARNING", "1".into()),
            ("TMOUT", "120".into()),
            ("HISTFILE", at(".history")),
        ];
        for (key, value) in set {
            launch.env.push((key.into(), value));
        }
    })
}

/// A session's terminals, in a folder of their own.
struct Fixture {
    terminals: Terminals,
    notes: mpsc::UnboundedReceiver<RunNote>,
    hub: Hub,
    agent: AgentId,
    /// Where the agent works, as the shell sees it.
    work: PathBuf,
    dir: TempDir,
}

fn fixture(shell: &str) -> Fixture {
    fixture_with(shell, None)
}

/// The first `name` on the PATH, where some systems, such as NixOS, keep
/// their only shells. Without one it says so and gives none, and the test
/// that needs it is skipped.
fn shell(name: &str) -> Option<String> {
    let found = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(name))
        .find(|path| path.is_file());
    if found.is_none() {
        eprintln!("note: no {name} on the PATH, so its session tests are skipped");
    }
    found.map(|path| path.to_string_lossy().into_owned())
}

/// A fixture whose shell is the first `name` on the PATH, or none.
fn fixture_on_path(name: &str) -> Option<Fixture> {
    shell(name).map(|shell| fixture(&shell))
}

fn fixture_with(shell: &str, session: Option<&str>) -> Fixture {
    let dir = TempDir::new();
    let root = fs::canonicalize(dir.path()).expect("the folder is there");
    let (home, work) = (root.join("home"), root.join("work"));
    for folder in [home.join("zdot"), home.join("share"), work.join("sub")] {
        fs::create_dir_all(folder).expect("folders are made");
    }
    let hub = crate::secrets::testing::hub();
    let wiring = Terminals::wire(holds(dir.path()), hub.clone());
    let terminals = wiring.terminals.expect("real terminals");
    terminals.set_launch_hook(hook(&home, shell, session));
    Fixture {
        terminals,
        notes: wiring.notes,
        hub,
        agent: AgentId::generate(),
        work,
        dir,
    }
}

impl Fixture {
    fn ask(&self, ask: SessionAsk) -> IdempotencyKey {
        let key = IdempotencyKey::generate();
        self.terminals.serve_session(SessionRequest {
            key,
            agent: self.agent,
            ask,
            cwd: self.work.clone(),
            cols: 80,
            rows: 24,
            opener: Opener::App,
            conn: 1,
        });
        key
    }

    fn run(&self, command: &str) -> IdempotencyKey {
        self.ask(SessionAsk::Run {
            command: command.into(),
            private: false,
            fresh: None,
        })
    }

    async fn note(&mut self) -> RunNote {
        timeout(WAIT, self.notes.recv())
            .await
            .expect("a note comes in time")
            .expect("the terminals still run")
    }

    /// What came of the request with `key`.
    async fn answer(&mut self, key: IdempotencyKey) -> SessionResult {
        loop {
            if let RunNote::Session {
                key: said, result, ..
            } = self.note().await
                && said == key
            {
                return result;
            }
        }
    }

    /// Runs `command` in the session, and waits for its block to end.
    async fn block(&mut self, command: &str) -> Ran {
        let key = self.run(command);
        let SessionResult::Wrote {
            terminal, block, ..
        } = self.answer(key).await
        else {
            panic!("{command} didn't go into the session");
        };
        self.ended(terminal, block).await
    }

    /// Waits for block `block` to end.
    async fn ended(&mut self, terminal: TerminalId, block: u32) -> Ran {
        let mut start = None;
        loop {
            let RunNote::Block {
                terminal: said,
                block: id,
                change,
                ..
            } = self.note().await
            else {
                continue;
            };
            assert_eq!(said, terminal);
            if id != block {
                continue;
            }
            match change {
                BlockChange::Started { start: at, .. } => start = Some(at),
                BlockChange::Ended {
                    exit,
                    cwd_after,
                    output,
                    secret_prompts,
                    stopped,
                    ..
                } => {
                    return Ran {
                        terminal,
                        start,
                        exit,
                        cwd_after,
                        output,
                        secret_prompts,
                        stopped,
                    };
                }
            }
        }
    }

    /// Waits for block `block` to start.
    async fn started(&mut self, block: u32) -> u64 {
        loop {
            if let RunNote::Block {
                block: id,
                change: BlockChange::Started { start, .. },
                ..
            } = self.note().await
                && id == block
            {
                return start;
            }
        }
    }

    /// Waits until `check` holds for the terminal.
    async fn until(&self, terminal: TerminalId, check: impl Fn(Option<&Open>) -> bool) {
        timeout(WAIT, async {
            while !check(self.terminals.lock().get(&terminal)) {
                tokio::time::sleep(LOOK).await;
            }
        })
        .await
        .expect("the terminal got there in time");
    }

    /// Ends the agent's terminals and lets their holders go.
    async fn end(&self) {
        self.terminals.end_agent(self.agent);
        let agent = self.agent;
        timeout(WAIT, async {
            loop {
                let left: Vec<(TerminalId, bool)> = (self.terminals.lock().iter())
                    .filter(|(_, open)| open.note.agent == agent)
                    .map(|(terminal, open)| (*terminal, open.running()))
                    .collect();
                if left.iter().all(|(_, running)| !running) {
                    for (terminal, _) in left {
                        self.terminals.finish(terminal);
                    }
                    return;
                }
                tokio::time::sleep(LOOK).await;
            }
        })
        .await
        .expect("the terminals ended in time");
    }

    fn shown(&self, path: &Path) -> String {
        shown(path)
    }
}

/// How a block ended.
#[derive(Debug)]
struct Ran {
    terminal: TerminalId,
    start: Option<u64>,
    exit: Option<i32>,
    cwd_after: Option<String>,
    output: Transcript,
    secret_prompts: u32,
    stopped: bool,
}

async fn cd_and_export_carry(name: &str) {
    let Some(mut f) = fixture_on_path(name) else {
        return;
    };
    let first = f.block("cd sub && export GREETING=hello").await;
    assert_eq!(first.exit, Some(0), "{first:?}");
    let sub = f.work.join("sub");
    assert_eq!(first.cwd_after, Some(f.shown(&sub)), "{first:?}");

    let key = f.run("echo \"$GREETING from $(basename \"$PWD\")\"; false");
    let SessionResult::Wrote {
        terminal,
        block,
        cwd,
        command,
        ..
    } = f.answer(key).await
    else {
        panic!("the second command didn't go into the session");
    };
    assert_eq!((terminal, block), (first.terminal, 2));
    assert_eq!(cwd, f.shown(&sub), "the block knows where it ran");
    assert!(command.starts_with("echo"));
    let second = f.ended(terminal, block).await;
    assert_eq!(second.exit, Some(1), "{second:?}");
    assert!(second.output.text.contains("hello from sub"), "{second:?}");
    assert!(
        !second.output.text.contains("echo"),
        "the line isn't output"
    );
    assert!(!second.output.truncated);
    assert_eq!(second.cwd_after, None, "the folder stayed");
    f.end().await;
}

#[tokio::test]
async fn commands_share_one_shell_so_cd_and_export_carry() {
    cd_and_export_carry("bash").await;
}

#[tokio::test]
async fn commands_share_one_zsh_so_cd_and_export_carry() {
    cd_and_export_carry("zsh").await;
}

/// Checks that a block's start is where hold says the command started, and
/// that what clients get from there starts with the command's mark.
async fn start_is_the_shells_mark(name: &str) {
    let Some(mut f) = fixture_on_path(name) else {
        return;
    };
    let ran = f.block("printf 'one\\ntwo\\n'").await;
    let start = ran.start.expect("the shell marked the start");
    assert_eq!(ran.exit, Some(0));
    assert!(ran.output.text.contains("one\ntwo"), "{ran:?}");
    f.until(ran.terminal, |open| {
        open.is_some_and(|open| {
            open.commands
                .list()
                .iter()
                .any(|marked| marked.start == Some(start))
        })
    })
    .await;
    let marked: Vec<u64> = (f.terminals.lock().get(&ran.terminal))
        .map(|open| {
            open.commands
                .list()
                .iter()
                .filter_map(|marked| marked.start)
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        marked.last(),
        Some(&start),
        "hold's last command is the block"
    );

    let Ok(Reached::Live(mut link)) = f
        .terminals
        .inner
        .holds
        .connect(ran.terminal, Some(start))
        .await
    else {
        panic!("the session's holder runs");
    };
    let mut seen = Vec::new();
    timeout(WAIT, async {
        while seen.len() < 8 {
            match link.events.recv().await {
                Some(TermEvent::Output { offset, data }) => {
                    if seen.is_empty() {
                        assert_eq!(offset, start, "the output starts where asked");
                    }
                    seen.extend(data);
                }
                Some(_) => {}
                None => panic!("the link closed"),
            }
        }
    })
    .await
    .expect("the output came in time");
    assert!(
        seen.starts_with(b"\x1b]133;C"),
        "the block starts with the command's mark: {:?}",
        String::from_utf8_lossy(&seen)
    );
    drop(link);
    f.end().await;
}

#[tokio::test]
async fn a_block_starts_where_hold_says_its_command_started() {
    start_is_the_shells_mark("bash").await;
}

#[tokio::test]
async fn a_zsh_block_starts_where_hold_says_its_command_started() {
    start_is_the_shells_mark("zsh").await;
}

#[tokio::test]
async fn a_busy_session_sends_a_command_fresh_and_asks_before_it_ends() {
    let Some(mut f) = fixture_on_path("bash") else {
        return;
    };
    let key = f.run("sleep 30");
    let SessionResult::Wrote {
        terminal, block, ..
    } = f.answer(key).await
    else {
        panic!("sleep didn't go into the session");
    };
    f.started(block).await;

    let key = f.run("echo fresh");
    assert!(matches!(
        f.answer(key).await,
        SessionResult::Fresh(FreshReason::Busy)
    ));
    let opened = loop {
        if let RunNote::Opened {
            key: said, result, ..
        } = f.note().await
            && said == key
        {
            break result.expect("the fresh shell opened");
        }
    };
    assert!(matches!(&opened.kind, TerminalKind::Run { command, .. } if command == "echo fresh"));
    assert_eq!(opened.cwd, f.shown(&f.work), "in the session's folder");

    // A folder can't wait for it either.
    let key = f.ask(SessionAsk::Cd { path: "sub".into() });
    let SessionResult::Refused(reason) = f.answer(key).await else {
        panic!("a busy session took a folder");
    };
    assert_eq!(
        reason,
        "The session is busy. Wait for sleep 30 to finish, or stop it."
    );

    let ActionOutcome::Refused { reason } = f.terminals.end_session(f.agent, false) else {
        panic!("a session that runs something ended without asking");
    };
    assert_eq!(reason, STILL_RUNNING);
    assert_eq!(f.terminals.end_session(f.agent, true), ActionOutcome::Done);
    let ran = f.ended(terminal, block).await;
    assert_eq!(ran.exit, None, "{ran:?}");
    f.until(terminal, |open| open.is_none()).await;
    f.end().await;
}

#[tokio::test]
async fn text_typed_at_the_prompt_sends_a_command_fresh_until_cleared() {
    let Some(mut f) = fixture_on_path("bash") else {
        return;
    };
    let ran = f.block("true").await;
    let terminal = ran.terminal;
    let clear = |open: Option<&Open>| open.is_some_and(|open| !open.keys.sent && !open.keys.line);
    // A terminal's own answers aren't keys. The shell takes them as text,
    // which the Ctrl-C below clears.
    f.terminals
        .input(1, Caller::App, terminal, b"\x1b[12;40R\x1b[I".to_vec())
        .expect("you may type");
    assert!(clear(f.terminals.lock().get(&terminal)));
    f.terminals
        .input(1, Caller::App, terminal, b"echo half".to_vec())
        .expect("you may type");
    let key = f.run("echo next");
    assert!(matches!(
        f.answer(key).await,
        SessionResult::Fresh(FreshReason::Busy)
    ));
    // Ctrl-C clears the line, and the shell shows its prompt again. Nothing
    // follows it until then: bash 5 keeps the line when more keys come
    // before it took the Ctrl-C in.
    f.terminals
        .input(1, Caller::App, terminal, vec![0x03])
        .expect("you may type");
    f.until(terminal, clear).await;
    let key = f.run("echo next");
    let answer = f.answer(key).await;
    assert!(
        matches!(answer, SessionResult::Wrote { .. }),
        "the cleared prompt didn't take it: {answer:?}"
    );
    f.end().await;
}

#[tokio::test]
async fn sessions_need_shell_integration() {
    let Some(mut f) = fixture_on_path("bash") else {
        return;
    };
    let key = EntityKey::DeviceSettings(f.hub.device().id);
    let Some(Entity::DeviceSettings(settings)) = f.hub.get(&key) else {
        panic!("a new hub has settings");
    };
    f.hub.apply([Change::Upsert {
        value: Entity::DeviceSettings(maxhq_protocol::model::DeviceSettings {
            shell_integration: false,
            read_other_agents: true,
            ..settings
        }),
    }]);
    let key = f.run("echo hi");
    assert!(matches!(
        f.answer(key).await,
        SessionResult::Fresh(FreshReason::IntegrationOff)
    ));
    let key = f.ask(SessionAsk::Cd { path: "sub".into() });
    let SessionResult::Refused(reason) = f.answer(key).await else {
        panic!("a session opened with the integration off");
    };
    assert_eq!(
        reason,
        "Sessions need shell integration, which is off on host."
    );
    f.end().await;
}

#[tokio::test]
async fn a_shell_that_marks_nothing_sends_commands_fresh() {
    let mut f = fixture_with("/bin/bash", Some("/bin/sh"));
    let key = f.run("echo hi");
    assert!(matches!(
        f.answer(key).await,
        SessionResult::Fresh(FreshReason::NoMarks)
    ));
    let key = f.ask(SessionAsk::Cd { path: "sub".into() });
    assert!(matches!(f.answer(key).await, SessionResult::Refused(reason) if reason == NO_MARKS));
    let sessions = (f.terminals.lock().values())
        .filter(|open| matches!(open.note.kind, TerminalKind::Session))
        .count();
    assert_eq!(sessions, 0, "no session opened");
    f.end().await;
}

#[tokio::test]
async fn a_folder_opens_the_session_there_or_moves_it() {
    let Some(mut f) = fixture_on_path("bash") else {
        return;
    };
    let key = f.ask(SessionAsk::Cd { path: "sub".into() });
    let SessionResult::Opened(session) = f.answer(key).await else {
        panic!("no session opened");
    };
    assert_eq!(session.kind, TerminalKind::Session);
    let sub = f.work.join("sub");
    assert_eq!(session.cwd, f.shown(&sub));

    let key = f.ask(SessionAsk::Cd { path: "..".into() });
    let SessionResult::Wrote {
        terminal,
        block,
        command,
        ..
    } = f.answer(key).await
    else {
        panic!("the folder didn't go into the session");
    };
    assert_eq!(terminal, session.id);
    assert_eq!(
        command,
        format!("cd -- {}", quote(&f.work.to_string_lossy(), false))
    );
    let ran = f.ended(terminal, block).await;
    assert_eq!(ran.exit, Some(0), "{ran:?}");
    assert_eq!(ran.cwd_after, Some(f.shown(&f.work)));

    let key = f.ask(SessionAsk::Cd {
        path: "nowhere".into(),
    });
    assert!(matches!(
        f.answer(key).await,
        SessionResult::Refused(reason) if reason == "There's no folder at nowhere."
    ));
    f.end().await;
}

#[test]
fn a_block_that_runs_as_maxd_restarts_ends_in_the_next() {
    let runtime = || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime")
    };
    let Some(bash) = shell("bash") else {
        return;
    };
    let first = runtime();
    let (f, terminal, block) = first.block_on(async {
        let mut f = fixture(&bash);
        let key = f.run("sleep 1; echo done");
        let SessionResult::Wrote {
            terminal, block, ..
        } = f.answer(key).await
        else {
            panic!("the command didn't go into the session");
        };
        f.started(block).await;
        (f, terminal, block)
    });
    // The first maxd stops, its tasks with it, and the holder runs on.
    drop(first);
    let Fixture {
        terminals,
        notes,
        hub,
        agent,
        work,
        dir,
    } = f;
    drop((terminals, notes));

    let second = runtime();
    second.block_on(async {
        let wiring = Terminals::wire(holds(dir.path()), hub.clone());
        let terminals = wiring.terminals.expect("real terminals");
        terminals.restoring().await;
        let mut f = Fixture {
            terminals,
            notes: wiring.notes,
            hub,
            agent,
            work,
            dir,
        };
        let ran = f.ended(terminal, block).await;
        assert_eq!(ran.exit, Some(0), "{ran:?}");
        assert!(ran.output.text.contains("done"), "{ran:?}");
        f.end().await;
    });
}

fn refused(reason: &str) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.to_owned(),
    }
}

#[tokio::test]
async fn force_stop_ends_a_command_that_ignored_stop_and_leaves_the_shell() {
    let Some(mut f) = fixture_on_path("bash") else {
        return;
    };
    let first = f.block("true").await;
    let terminal = first.terminal;
    assert!(!first.stopped, "{first:?}");
    assert_eq!(
        f.terminals.interrupt(Caller::App, terminal, true),
        refused(NOTHING_TO_STOP)
    );
    let key = f.run("trap '' INT; printf 'rea%sy\\n' d; sleep 30");
    let SessionResult::Wrote { block, .. } = f.answer(key).await else {
        panic!("sleep didn't go into the session");
    };
    f.started(block).await;
    f.until(terminal, |open| open.is_some_and(|open| open.entity.busy))
        .await;
    shows(&f.terminals.inner.holds, terminal, "ready").await;
    assert_eq!(
        f.terminals.interrupt(Caller::App, terminal, false),
        ActionOutcome::Done
    );
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(f.terminals.lock()[&terminal].entity.busy, "it ignored Stop");
    assert_eq!(
        f.terminals.interrupt(Caller::App, terminal, true),
        ActionOutcome::Done
    );
    let ran = f.ended(terminal, block).await;
    assert_eq!((ran.exit, ran.stopped), (Some(137), true), "{ran:?}");
    // The shell stayed, and what it runs next ends on its own.
    let next = f.block("trap - INT; echo still here").await;
    assert_eq!(next.terminal, terminal);
    assert_eq!(next.output.text.trim(), "still here", "{next:?}");
    assert!(!next.stopped, "{next:?}");
    f.end().await;
}

#[tokio::test]
async fn a_command_that_ends_well_after_stop_ended_on_its_own() {
    let Some(mut f) = fixture_on_path("bash") else {
        return;
    };
    // A Ctrl-C that lands while sh starts a sleep waits for that sleep to
    // end before the trap runs, so the sleeps are short.
    let key =
        f.run("sh -c 'trap \"exit 0\" INT; printf \"rea%sy\\n\" d; while :; do sleep 1; done'");
    let SessionResult::Wrote {
        terminal, block, ..
    } = f.answer(key).await
    else {
        panic!("sh didn't go into the session");
    };
    f.started(block).await;
    f.until(terminal, |open| open.is_some_and(|open| open.entity.busy))
        .await;
    // Stop only once the trap is set. The echoed line doesn't say "ready".
    shows(&f.terminals.inner.holds, terminal, "ready").await;
    assert_eq!(
        f.terminals.interrupt(Caller::App, terminal, false),
        ActionOutcome::Done
    );
    let ran = f.ended(terminal, block).await;
    assert_eq!((ran.exit, ran.stopped), (Some(0), false), "{ran:?}");
    f.end().await;
}

#[tokio::test]
async fn force_stop_on_a_run_ends_it_and_says_you_stopped_it() {
    let Some(mut f) = fixture_on_path("bash") else {
        return;
    };
    let key = IdempotencyKey::generate();
    Host::open(
        &f.terminals,
        OpenRequest {
            key,
            agent: f.agent,
            kind: TerminalKind::Run {
                command: "trap '' INT HUP; printf 'rea%sy\\n' d; sleep 30".into(),
                private: false,
            },
            cwd: f.work.clone(),
            cols: 80,
            rows: 24,
            opener: Opener::App,
            conn: 1,
        },
    );
    let terminal = loop {
        if let RunNote::Opened {
            key: said, result, ..
        } = f.note().await
            && said == key
        {
            break result.expect("the run opened").id;
        }
    };
    f.until(terminal, |open| open.is_some_and(|open| open.entity.busy))
        .await;
    // Hang up only once the shell ignores it, so the kill is what ends it.
    shows(&f.terminals.inner.holds, terminal, "ready").await;
    assert_eq!(
        f.terminals.interrupt(Caller::App, terminal, true),
        ActionOutcome::Done
    );
    let (exit, stopped) = loop {
        if let RunNote::Ended {
            terminal: ended,
            exit,
            stopped,
            ..
        } = f.note().await
            && ended == terminal
        {
            break (exit, stopped);
        }
    };
    let exit = exit.expect("its holder said how it ended");
    assert_eq!(exit.signal, Some(9), "{exit:?}");
    assert!(stopped);
    f.terminals.release(terminal);
    f.end().await;
}

async fn the_pager_is_off_for_what_you_send_from_the_chat(name: &str) {
    let Some(mut f) = fixture_on_path(name) else {
        return;
    };
    // What the command itself sets stays.
    let first = f.block("export PAGER=less-pager; unset GIT_PAGER").await;
    let terminal = first.terminal;
    let ran = f.block(r#"echo "pager:[$PAGER|${GIT_PAGER-unset}]""#).await;
    assert_eq!(ran.output.text.trim(), "pager:[cat|cat]", "{ran:?}");
    // What you type in the drawer keeps yours.
    f.terminals
        .input(
            1,
            Caller::App,
            terminal,
            br#"echo "typed:[$PAGER|${GIT_PAGER-unset}]""#.to_vec(),
        )
        .expect("you may type");
    f.terminals
        .input(1, Caller::App, terminal, b"\r".to_vec())
        .expect("you may type");
    shows(
        &f.terminals.inner.holds,
        terminal,
        "typed:[less-pager|unset]",
    )
    .await;
    f.end().await;
}

#[tokio::test]
async fn the_pager_is_off_for_what_bash_runs_from_the_chat() {
    the_pager_is_off_for_what_you_send_from_the_chat("bash").await;
}

#[tokio::test]
async fn the_pager_is_off_for_what_zsh_runs_from_the_chat() {
    the_pager_is_off_for_what_you_send_from_the_chat("zsh").await;
}

#[tokio::test]
async fn a_password_counts_and_goes_nowhere() {
    const CANARY: &str = "canary-7d1e4b-not-for-logs";
    let log = Log::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _logging = tracing::subscriber::set_default(subscriber);

    let Some(mut f) = fixture_on_path("bash") else {
        return;
    };
    let key = f.run("read -rs -p 'Password: ' pw; echo; echo got ${#pw}");
    let SessionResult::Wrote {
        terminal, block, ..
    } = f.answer(key).await
    else {
        panic!("the command didn't go into the session");
    };
    let mut notes = Vec::new();
    loop {
        let note = f.note().await;
        let started = matches!(
            &note,
            RunNote::Block {
                change: BlockChange::Started { .. },
                ..
            }
        );
        notes.push(format!("{note:?}"));
        if started {
            break;
        }
    }
    f.until(terminal, |open| {
        open.is_some_and(|open| open.entity.password_prompt)
    })
    .await;
    f.terminals
        .input(1, Caller::App, terminal, format!("{CANARY}\r").into_bytes())
        .expect("you may type");
    let ran = f.ended(terminal, block).await;
    notes.push(format!("{ran:?}"));
    assert_eq!(ran.secret_prompts, 1, "{ran:?}");
    assert!(ran.output.text.contains("Password:"), "{ran:?}");
    assert!(ran.output.text.contains("🔒 sent"), "{ran:?}");
    assert!(
        ran.output.text.contains(&format!("got {}", CANARY.len())),
        "{ran:?}"
    );

    let kept = f
        .terminals
        .inner
        .holds
        .load_note::<serde_json::Value>(terminal);
    notes.push(format!("{kept:?}"));
    for said in &notes {
        assert!(!said.contains(CANARY), "a note has the password: {said}");
    }
    let logged = log.text();
    assert!(!logged.contains(CANARY), "the log has the password");
    for file in files(f.dir.path()) {
        let bytes = fs::read(&file).unwrap_or_default();
        let found = bytes
            .windows(CANARY.len())
            .any(|at| at == CANARY.as_bytes());
        assert!(!found, "{} has the password", file.display());
    }
    f.end().await;
}

/// Waits until the terminal's output shows `text`.
async fn shows(holds: &maxhq_hold::Terminals, terminal: TerminalId, text: &str) {
    let Ok(Reached::Live(link)) = holds.connect(terminal, None).await else {
        panic!("the holder runs");
    };
    let TermLink {
        mut events,
        control,
        ..
    } = link;
    let _control = control;
    let mut tail = Vec::new();
    timeout(WAIT, async {
        while let Some(event) = events.recv().await {
            if let TermEvent::Output { data, .. } = event {
                tail.extend_from_slice(&data);
                if tail.windows(text.len()).any(|at| at == text.as_bytes()) {
                    return;
                }
                tail.drain(..tail.len().saturating_sub(text.len()));
            }
        }
        panic!("the terminal ended before it showed {text}");
    })
    .await
    .expect("the output came in time");
}

#[test]
fn a_session_whose_marks_are_gone_after_a_restart_lives_on() {
    let runtime = || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime")
    };
    let Some(bash) = shell("bash") else {
        return;
    };
    let first = runtime();
    let (f, terminal, block) = first.block_on(async {
        let mut f = fixture(&bash);
        // More output than the holder keeps, so the block's marks go with
        // it, and then something that runs on.
        let key =
            f.run("head -c 2200000 /dev/zero | tr '\\0' x; echo; echo big-$((1+1)); sleep 30");
        let SessionResult::Wrote {
            terminal, block, ..
        } = f.answer(key).await
        else {
            panic!("the command didn't go into the session");
        };
        f.started(block).await;
        shows(&f.terminals.inner.holds, terminal, "big-2").await;
        (f, terminal, block)
    });
    drop(first);
    let Fixture {
        terminals,
        notes,
        hub,
        agent,
        work,
        dir,
    } = f;
    drop((terminals, notes));

    let second = runtime();
    second.block_on(async {
        let wiring = Terminals::wire(holds(dir.path()), hub.clone());
        let terminals = wiring.terminals.expect("real terminals");
        terminals.restoring().await;
        let mut f = Fixture {
            terminals,
            notes: wiring.notes,
            hub,
            agent,
            work,
            dir,
        };
        f.until(terminal, |open| {
            open.is_some_and(|open| open.control.is_some())
        })
        .await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            (f.terminals.lock().get(&terminal)).is_some_and(|open| open.commands.list().is_empty()),
            "the holder still has the block's marks"
        );
        // Long after the shell started, with no marks to go by: the
        // command goes fresh, and the session and what runs in it live on.
        let key = f.run("echo hi");
        assert!(matches!(
            f.answer(key).await,
            SessionResult::Fresh(FreshReason::Busy)
        ));
        {
            let opens = f.terminals.lock();
            let open = opens.get(&terminal).expect("the session is there");
            assert!(open.running() && !open.closing, "the session ended");
            let current = (open.note.session.as_ref()).and_then(|session| session.current.as_ref());
            assert!(
                current.is_some_and(|current| current.id == block && !current.done),
                "the block ended"
            );
        }
        f.end().await;
    });
}

#[tokio::test]
async fn keys_typed_while_a_command_runs_count_at_the_next_prompt() {
    let Some(mut f) = fixture_on_path("bash") else {
        return;
    };
    // Typed with no Enter while something runs: the shell shows it at its
    // next prompt, so a command goes fresh until it's cleared.
    let key = f.run("sleep 1");
    let SessionResult::Wrote {
        terminal, block, ..
    } = f.answer(key).await
    else {
        panic!("sleep didn't go into the session");
    };
    f.started(block).await;
    f.terminals
        .input(1, Caller::App, terminal, b"echo half".to_vec())
        .expect("you may type");
    let ran = f.ended(terminal, block).await;
    assert_eq!(ran.exit, Some(0), "{ran:?}");
    let key = f.run("echo next");
    assert!(matches!(
        f.answer(key).await,
        SessionResult::Fresh(FreshReason::Busy)
    ));
    f.terminals
        .input(1, Caller::App, terminal, vec![0x03])
        .expect("you may type");
    f.until(terminal, |open| {
        open.is_some_and(|open| !open.keys.sent && !open.keys.line)
    })
    .await;
    let ran = f.block("echo next").await;
    assert_eq!(ran.terminal, terminal);
    assert_eq!(ran.output.text.trim(), "next", "{ran:?}");

    // A line sent while something runs: the shell runs it at its next
    // prompt, on its own, and the next command waits for it.
    let key = f.run("sleep 1");
    let SessionResult::Wrote { block, .. } = f.answer(key).await else {
        panic!("sleep didn't go into the session");
    };
    f.started(block).await;
    f.terminals
        .input(1, Caller::App, terminal, b"echo ahead-$((2+3))\r".to_vec())
        .expect("you may type");
    f.until(terminal, |open| {
        open.is_some_and(|open| open.keys.ahead == 1)
    })
    .await;
    let ran = f.ended(terminal, block).await;
    assert!(!ran.output.text.contains("ahead-5"), "{ran:?}");
    let ran = f.block("echo next").await;
    assert_eq!(ran.terminal, terminal);
    assert_eq!(ran.output.text.trim(), "next", "{ran:?}");
    f.end().await;
}

#[tokio::test]
async fn a_line_the_shell_waits_for_more_of_is_cancelled() {
    let Some(mut f) = fixture_on_path("bash") else {
        return;
    };
    let ran = f.block("echo don't").await;
    assert_eq!(ran.exit, None, "{ran:?}");
    assert_eq!(ran.output.text, CANCELLED);
    let terminal = ran.terminal;
    let ran = f.block("echo ok").await;
    assert_eq!(ran.terminal, terminal, "the same session");
    assert_eq!(ran.exit, Some(0), "{ran:?}");
    assert_eq!(ran.output.text.trim(), "ok", "{ran:?}");
    f.end().await;
}

#[tokio::test]
async fn a_block_looks_once_more_as_it_goes_in() {
    let Some(mut f) = fixture_on_path("bash") else {
        return;
    };
    let terminal = f.block("true").await.terminal;
    // You type after the session looked ready, before the line went in.
    f.terminals
        .input(1, Caller::App, terminal, b"echo half".to_vec())
        .expect("you may type");
    f.until(terminal, |open| open.is_some_and(|open| open.keys.line))
        .await;
    let request = SessionRequest {
        key: IdempotencyKey::generate(),
        agent: f.agent,
        ask: SessionAsk::Run {
            command: "echo next".into(),
            private: false,
            fresh: None,
        },
        cwd: f.work.clone(),
        cols: 80,
        rows: 24,
        opener: Opener::App,
        conn: 1,
    };
    let wrote = f
        .terminals
        .write_block(terminal, &request, "echo next", "echo next".into(), false);
    assert!(matches!(wrote, Err(NotNow::Typed)));
    let blocks = f.terminals.with_session(terminal, |session| session.blocks);
    assert_eq!(blocks, Some(1), "no block went in");
    f.terminals
        .input(1, Caller::App, terminal, vec![0x03])
        .expect("you may type");
    f.end().await;
}

/// Every file under `dir`.
fn files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut left = vec![dir.to_owned()];
    while let Some(dir) = left.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => left.push(entry.path()),
                Ok(kind) if kind.is_file() => found.push(entry.path()),
                _ => {}
            }
        }
    }
    found
}

/// Everything written to the log.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl Log {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl io::Write for Log {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn note(after: u64, start: Option<u64>) -> BlockNote {
    BlockNote {
        id: 1,
        command: "echo hi".into(),
        after,
        written_at: Timestamp(1),
        start,
        cwd: "~".into(),
        done: false,
        exit: None,
        typed: false,
    }
}

/// Feeds `cutter` `bytes` from `at`, and returns where they end.
fn feed(cutter: &mut Cutter, at: u64, bytes: &[u8]) -> u64 {
    cutter.output(at, bytes);
    at + bytes.len() as u64
}

#[test]
fn a_cutter_takes_the_output_between_the_commands_marks() {
    let mut cutter = Cutter::new(&note(0, None), 80, 24);
    // The marks come after the output they fall in.
    let input = feed(&mut cutter, 0, b"\x1b]133;A\x07$ \x1b]133;B\x07");
    let start = feed(&mut cutter, input, b"echo hi\r\n");
    let end = feed(&mut cutter, start, b"\x1b]133;C\x07hi\r\n");
    let after = feed(&mut cutter, end, b"\x1b]133;D;0\x07");
    feed(&mut cutter, after, b"\x1b]133;A\x07$ ");
    let at = Timestamp(5);
    assert_eq!(
        cutter.mark(0, at, &Mark::Prompt),
        Cut::None,
        "the prompt it went in at"
    );
    assert_eq!(cutter.mark(input, at, &Mark::Input), Cut::None);
    assert_eq!(
        cutter.mark(
            start,
            at,
            &Mark::Command {
                cwd: Some("/w".into()),
                command: None,
            }
        ),
        Cut::Started {
            start,
            at,
            cwd: Some("/w".into())
        }
    );
    assert_eq!(
        cutter.mark(end, at, &Mark::Done { exit: Some(0) }),
        Cut::Ended { prompt: false }
    );
    assert_eq!(cutter.mark(after, at, &Mark::Prompt), Cut::Prompt);
    let (exit, _, output, secrets) = cutter.finish();
    assert_eq!((exit, secrets), (Some(0), 0));
    assert_eq!(output.text.trim_end(), "hi");
    assert!(!output.truncated);
}

#[test]
fn a_line_with_no_command_ends_with_no_code_and_no_output() {
    let mut cutter = Cutter::new(&note(10, None), 80, 24);
    let input = feed(&mut cutter, 10, b"\x1b]133;A\x07$ \x1b]133;B\x07");
    let prompt = feed(&mut cutter, input, b"# a comment\r\n");
    feed(&mut cutter, prompt, b"\x1b]133;A\x07$ ");
    let at = Timestamp(5);
    assert_eq!(cutter.mark(10, at, &Mark::Prompt), Cut::None);
    assert_eq!(cutter.mark(input, at, &Mark::Input), Cut::None);
    assert_eq!(cutter.mark(prompt, at, &Mark::Prompt), Cut::Grace);
    assert!(cutter.grace_over());
    let (exit, _, output, _) = cutter.finish();
    assert_eq!(exit, None);
    assert_eq!(output.text.trim(), "");
    assert!(!output.truncated);
}

#[test]
fn a_cutter_says_when_it_lost_the_start() {
    let mut cutter = Cutter::new(&note(0, Some(100)), 80, 24);
    // The holder no longer had what came after the start.
    let end = feed(&mut cutter, 4_000, b"tail\r\n");
    assert_eq!(
        cutter.mark(end, Timestamp(9), &Mark::Done { exit: Some(2) }),
        Cut::Ended { prompt: false }
    );
    let (exit, at, output, _) = cutter.finish();
    assert_eq!((exit, at), (Some(2), Timestamp(9)));
    assert_eq!(output.text.trim_end(), "tail");
    assert!(output.truncated);
}

#[test]
fn a_cutter_notes_one_password_per_prompt() {
    let mut cutter = Cutter::new(&note(0, Some(0)), 80, 24);
    let at = feed(&mut cutter, 0, b"\x1b]133;C\x07Password: ");
    cutter.secret(false);
    cutter.secret(false);
    let at = feed(&mut cutter, at, b"\r\nPassword: ");
    cutter.secret(false);
    feed(&mut cutter, at, b"\r\nok\r\n");
    let (_, _, output, secrets) = cutter.finish();
    assert_eq!(secrets, 2);
    assert_eq!(output.text.matches("🔒 sent").count(), 2, "{output:?}");
}

#[test]
fn keys_at_the_prompt_say_whether_a_line_waits() {
    let mut typed = Typed::default();
    typed.keys(
        b"\x1b[3;1R\x1b[I\x1b[O\x1b]11;rgb:0000/0000/0000\x1b\\\x1b[?1;2c\x1bP>|xterm\x1b\\",
        false,
    );
    assert!(
        !typed.sent && !typed.line,
        "a terminal's answers aren't keys"
    );
    typed.keys(b"ls", false);
    assert!(!typed.sent && typed.line);
    typed.keys(b"\r", false);
    assert!(typed.sent && !typed.line);
    typed.prompt();
    assert!(!typed.sent && !typed.line);
    typed.keys(b"make\rgit st", false);
    assert!(typed.sent && typed.line, "what follows the line waits");
    typed.prompt();
    typed.keys(&[0x03], false);
    assert!(typed.sent && !typed.line, "Ctrl-C clears the line");
    typed.prompt();
    typed.keys(b"\x1b[A", false);
    assert!(typed.line, "the up arrow brings a line back");
}

#[test]
fn a_cd_line_quotes_the_folder_for_its_shell() {
    let plain = Path::new("/srv/code/example-app");
    assert_eq!(
        cd_line(plain, Dialect::Posix),
        "cd -- /srv/code/example-app"
    );
    let spaced = Path::new("/srv/it's here");
    assert_eq!(
        cd_line(spaced, Dialect::Posix),
        "cd -- '/srv/it'\\''s here'"
    );
    assert_eq!(cd_line(spaced, Dialect::Fish), "cd -- '/srv/it\\'s here'");
    assert_eq!(quote("a\\b", true), "'a\\\\b'");
    if let Some(home) = home() {
        assert_eq!(cd_line(&home, Dialect::Posix), "cd -- ~");
        assert_eq!(
            cd_line(&home.join("code").join("my app"), Dialect::Posix),
            "cd -- ~/'code/my app'"
        );
    }
}

#[test]
fn powershell_changes_folder_with_the_path_as_it_is() {
    let path = Path::new(r"C:\Users\<user>\it's [here]");
    assert_eq!(
        cd_line(path, Dialect::PowerShell),
        r"Set-Location -LiteralPath 'C:\Users\<user>\it''s [here]'"
    );
    if let Some(home) = home() {
        let line = cd_line(&home, Dialect::PowerShell);
        assert!(
            !line.contains('~'),
            "PowerShell's ~ is its provider's home: {line}"
        );
    }
    let note = |fish, powershell| SessionNote {
        fish,
        powershell,
        ..SessionNote::default()
    };
    assert_eq!(note(false, false).dialect(), Dialect::Posix);
    assert_eq!(note(true, false).dialect(), Dialect::Fish);
    assert_eq!(note(false, true).dialect(), Dialect::PowerShell);
    // A note from before PowerShell keeps reading as it did.
    let old: SessionNote = serde_json::from_str(r#"{"blocks":2,"fish":true}"#).unwrap();
    assert_eq!(old.dialect(), Dialect::Fish);
    let saved = serde_json::to_value(note(false, false)).unwrap();
    assert!(saved.get("powershell").is_none(), "{saved}");
}

#[test]
fn a_folder_resolves_from_where_the_session_is() {
    let dir = TempDir::new();
    let here = fs::canonicalize(dir.path()).expect("the folder is there");
    fs::create_dir(here.join("sub")).expect("a folder");
    assert_eq!(resolve("sub", &here), Ok(here.join("sub")));
    assert_eq!(
        resolve(&here.to_string_lossy(), Path::new("/")),
        Ok(here.clone())
    );
    assert_eq!(resolve("  ", &here), Err(NO_FOLDER_GIVEN.to_owned()));
    assert_eq!(resolve("a\nb", &here), Err(CONTROL_IN_FOLDER.to_owned()));
    assert_eq!(
        resolve("gone", &here),
        Err("There's no folder at gone.".to_owned())
    );
    if let Some(home) = home().filter(|home| home.is_dir()) {
        assert_eq!(resolve("~", &here), Ok(home));
    }
}

#[test]
fn a_busy_refusal_names_the_command_short() {
    let long = "x".repeat(80);
    let reason = NotNow::Busy(Some(long)).reason("host");
    assert_eq!(
        reason,
        format!(
            "The session is busy. Wait for {}… to finish, or stop it.",
            "x".repeat(60)
        )
    );
    assert_eq!(NotNow::Busy(None).reason("host"), BUSY_UNNAMED);
    assert_eq!(NotNow::Typed.fresh(), Ok(FreshReason::Busy));
    assert_eq!(NotNow::Gone.fresh(), Ok(FreshReason::NoMarks));
    assert_eq!(NotNow::Failed("no".into()).fresh(), Err("no".into()));
}

/// A session as a `maxd` that just began following it holds it, with
/// `blocks` run and a prompt shown before when `prompted`.
fn session_open(blocks: u32, prompted: bool) -> Open {
    let note = Note {
        agent: AgentId::generate(),
        kind: TerminalKind::Session,
        cwd: "~".into(),
        // Long ago, as for a session that ran through a restart.
        started_at: Timestamp(1),
        opener: Opener::App,
        moved: false,
        name: None,
        shell: None,
        session: Some(SessionNote {
            blocks,
            prompted,
            ..SessionNote::default()
        }),
    };
    Open::new(note, None, DeviceId::generate(), TerminalId::generate())
}

/// Gives `open` the shell's marks, each at its offset.
fn marks(open: &mut Open, marks: &[(u64, Mark)]) {
    for (offset, mark) in marks {
        open.marked(*offset, Timestamp(2), mark);
    }
}

const COMMAND: Mark = Mark::Command {
    cwd: None,
    command: None,
};
const DONE: Mark = Mark::Done { exit: Some(0) };

#[test]
fn only_a_new_session_waits_for_its_first_prompt() {
    // The wait counts from when this maxd began following it.
    let open = session_open(0, false);
    assert!(matches!(
        open.shell_look(),
        Look::FirstPrompt(followed) if followed.elapsed() < FIRST_PROMPT_WAIT
    ));
    // With no marks to go by, as after a restart until they come again or
    // when the holder dropped them, one that ran a block or showed a
    // prompt is busy, never given up on.
    for (blocks, prompted) in [(1, false), (0, true), (3, true)] {
        let open = session_open(blocks, prompted);
        assert!(
            matches!(open.shell_look(), Look::Busy(NotNow::Busy(None))),
            "{blocks} blocks, prompted {prompted}"
        );
    }
    let mut open = session_open(1, true);
    if let Some(session) = open.note.session.as_mut() {
        session.current = Some(note(0, Some(10)));
    }
    assert!(matches!(
        open.shell_look(),
        Look::Busy(NotNow::Busy(Some(command))) if command == "echo hi"
    ));

    let mut open = session_open(0, false);
    assert!(open.marked(0, Timestamp(2), &Mark::Prompt).1, "kept once");
    assert!(!open.marked(5, Timestamp(2), &Mark::Prompt).1);
    assert!(open
        .note
        .session
        .as_ref()
        .is_some_and(|session| session.prompted));
}

#[test]
fn keys_typed_while_something_runs_count_at_the_next_prompt() {
    let ran = [(0, Mark::Prompt), (2, Mark::Input), (10, COMMAND)];
    let next = [(20, DONE), (22, Mark::Prompt), (24, Mark::Input)];

    // No Enter: the shell shows it at the next prompt.
    let mut open = session_open(1, true);
    marks(&mut open, &ran);
    open.heard_keys(b"rm -rf /tmp/x", false);
    marks(&mut open, &next);
    assert!(matches!(open.shell_look(), Look::Stop(NotNow::Typed)));

    // With Enter: the next prompt runs it, so the session waits for it.
    let mut open = session_open(1, true);
    marks(&mut open, &ran);
    open.heard_keys(b"echo ahead\r", false);
    marks(&mut open, &next);
    assert!(matches!(
        open.shell_look(),
        Look::Ending(NotNow::Busy(None))
    ));
    marks(&mut open, &[(30, COMMAND)]);
    assert!(matches!(open.shell_look(), Look::Busy(NotNow::Busy(None))));
    marks(
        &mut open,
        &[(40, DONE), (42, Mark::Prompt), (44, Mark::Input)],
    );
    assert!(matches!(open.shell_look(), Look::Ready));

    // What ran read it instead: nothing starts, and the wait runs out.
    let mut open = session_open(1, true);
    marks(&mut open, &ran);
    open.heard_keys(b"y\r", false);
    marks(&mut open, &next);
    assert!(matches!(open.shell_look(), Look::Ending(_)));
    open.keys.queued_at = (open.keys.queued_at).and_then(|at| at.checked_sub(AHEAD_WAIT));
    assert!(matches!(open.shell_look(), Look::Ready));
    marks(&mut open, &[(30, Mark::Prompt)]);
    assert_eq!(open.keys.ahead, 0);

    // Ctrl-C while something runs drops what waits.
    let mut open = session_open(1, true);
    marks(&mut open, &ran);
    open.heard_keys(b"ls\rgit st", false);
    open.heard_keys(&[0x03], false);
    marks(&mut open, &next);
    assert!(matches!(open.shell_look(), Look::Ready));

    // A password isn't a line.
    let mut open = session_open(1, true);
    marks(&mut open, &ran);
    open.entity.password_prompt = true;
    open.heard_keys(b"hunter2\r", false);
    assert!(!open.keys.sent && !open.keys.line && open.keys.ahead == 0);
}

#[test]
fn a_line_the_shell_waits_for_more_of_ends_cancelled() {
    let mut cutter = Cutter::new(&note(10, None), 80, 24);
    let input = feed(&mut cutter, 10, b"\x1b]133;A\x07$ \x1b]133;B\x07");
    let more = feed(&mut cutter, input, b"echo don't\r\n> ");
    let at = Timestamp(5);
    assert_eq!(cutter.mark(10, at, &Mark::Prompt), Cut::None);
    assert_eq!(cutter.mark(input, at, &Mark::Input), Cut::None);
    assert!(cutter.waiting_for_more(), "no start and no prompt yet");
    cutter.cancel();
    assert!(!cutter.waiting_for_more(), "cancelled once");
    let prompt = feed(&mut cutter, more, b"^C\r\n");
    feed(&mut cutter, prompt, b"\x1b]133;A\x07$ ");
    assert_eq!(cutter.mark(prompt, at, &Mark::Prompt), Cut::Grace);
    assert!(cutter.grace_over());
    let (exit, _, output, _) = cutter.finish();
    assert_eq!(exit, None);
    assert_eq!(output.text, CANCELLED);

    // A line that starts doesn't wait for more.
    let mut cutter = Cutter::new(&note(10, None), 80, 24);
    let start = feed(&mut cutter, 10, b"\x1b]133;A\x07$ echo hi\r\n");
    assert!(matches!(
        cutter.mark(start, at, &COMMAND),
        Cut::Started { .. }
    ));
    assert!(!cutter.waiting_for_more());
}

mod typed;
