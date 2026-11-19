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
                    ..
                } => {
                    return Ran {
                        terminal,
                        start,
                        exit,
                        cwd_after,
                        output,
                        secret_prompts,
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
}

/// Whether this machine has zsh, which the zsh tests need.
fn has_zsh() -> bool {
    Path::new("/bin/zsh").exists()
}

async fn cd_and_export_carry(shell: &str) {
    let mut f = fixture(shell);
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
    cd_and_export_carry("/bin/bash").await;
}

#[tokio::test]
async fn commands_share_one_zsh_so_cd_and_export_carry() {
    if has_zsh() {
        cd_and_export_carry("/bin/zsh").await;
    }
}

/// Checks that a block's start is where hold says the command started, and
/// that what clients get from there starts with the command's mark.
async fn start_is_the_shells_mark(shell: &str) {
    let mut f = fixture(shell);
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
    start_is_the_shells_mark("/bin/bash").await;
}

#[tokio::test]
async fn a_zsh_block_starts_where_hold_says_its_command_started() {
    if has_zsh() {
        start_is_the_shells_mark("/bin/zsh").await;
    }
}

#[tokio::test]
async fn a_busy_session_sends_a_command_fresh_and_asks_before_it_ends() {
    let mut f = fixture("/bin/bash");
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
    let mut f = fixture("/bin/bash");
    let ran = f.block("true").await;
    let terminal = ran.terminal;
    f.terminals
        .input(1, Caller::App, terminal, b"echo half".to_vec())
        .expect("you may type");
    let key = f.run("echo next");
    assert!(matches!(
        f.answer(key).await,
        SessionResult::Fresh(FreshReason::Busy)
    ));
    // Ctrl-C clears the line, and the shell shows its prompt again.
    f.terminals
        .input(1, Caller::App, terminal, vec![0x03])
        .expect("you may type");
    // A terminal's own answers aren't keys.
    f.terminals
        .input(1, Caller::App, terminal, b"\x1b[12;40R\x1b[I".to_vec())
        .expect("you may type");
    f.until(terminal, |open| {
        open.is_some_and(|open| !open.keys.sent && !open.keys.line)
    })
    .await;
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
    let mut f = fixture("/bin/bash");
    let key = EntityKey::DeviceSettings(f.hub.device().id);
    let Some(Entity::DeviceSettings(settings)) = f.hub.get(&key) else {
        panic!("a new hub has settings");
    };
    f.hub.apply([Change::Upsert {
        value: Entity::DeviceSettings(maxhq_protocol::model::DeviceSettings {
            shell_integration: false,
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
    let mut f = fixture("/bin/bash");
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
    let first = runtime();
    let (f, terminal, block) = first.block_on(async {
        let mut f = fixture("/bin/bash");
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

    let mut f = fixture("/bin/bash");
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
                cwd: Some("/w".into())
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
    cutter.secret();
    cutter.secret();
    let at = feed(&mut cutter, at, b"\r\nPassword: ");
    cutter.secret();
    feed(&mut cutter, at, b"\r\nok\r\n");
    let (_, _, output, secrets) = cutter.finish();
    assert_eq!(secrets, 2);
    assert_eq!(output.text.matches("🔒 sent").count(), 2, "{output:?}");
}

#[test]
fn keys_at_the_prompt_say_whether_a_line_waits() {
    let mut typed = Typed::default();
    typed
        .keys(b"\x1b[3;1R\x1b[I\x1b[O\x1b]11;rgb:0000/0000/0000\x1b\\\x1b[?1;2c\x1bP>|xterm\x1b\\");
    assert!(
        !typed.sent && !typed.line,
        "a terminal's answers aren't keys"
    );
    typed.keys(b"ls");
    assert!(!typed.sent && typed.line);
    typed.keys(b"\r");
    assert!(typed.sent && !typed.line);
    typed.prompt();
    assert!(!typed.sent && !typed.line);
    typed.keys(b"make\rgit st");
    assert!(typed.sent && typed.line, "what follows the line waits");
    typed.prompt();
    typed.keys(&[0x03]);
    assert!(typed.sent && !typed.line, "Ctrl-C clears the line");
    typed.prompt();
    typed.keys(b"\x1b[A");
    assert!(typed.line, "the up arrow brings a line back");
}

#[test]
fn a_cd_line_quotes_the_folder_for_its_shell() {
    let plain = Path::new("/srv/code/example-app");
    assert_eq!(cd_line(plain, false), "cd -- /srv/code/example-app");
    let spaced = Path::new("/srv/it's here");
    assert_eq!(cd_line(spaced, false), "cd -- '/srv/it'\\''s here'");
    assert_eq!(cd_line(spaced, true), "cd -- '/srv/it\\'s here'");
    assert_eq!(quote("a\\b", true), "'a\\\\b'");
    if let Some(home) = home() {
        assert_eq!(cd_line(&home, false), "cd -- ~");
        assert_eq!(
            cd_line(&home.join("code/my app"), false),
            "cd -- ~/'code/my app'"
        );
    }
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
