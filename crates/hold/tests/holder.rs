// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Tests of real holders running a fake agent CLI.
//!
//! This binary plays four parts, picked by its first argument. `__hold` is
//! the holder, `__child` is the fake CLI, `__spawner` is a process that
//! starts a holder and waits to be killed, and anything else runs the
//! tests. Each test has its own folders under the system temp folder, and
//! only ever signals processes it started.
//!
//! The tests that need root, systemd or a running `maxd` are ignored. On
//! Linux, `cargo test -p maxhq-hold --test holder -- --ignored user_scope`
//! runs the user scope one, `-- --ignored host_` the `maxd` one, and the
//! `root_` ones run as root with a runner, such as
//! `CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER='sudo -E' cargo test -p
//! maxhq-hold --test holder -- --ignored root_`.

use std::collections::{HashSet, VecDeque};
use std::ffi::OsString;
use std::fs::{self, File, TryLockError};
use std::future::Future;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use maxhq_hold::{
    unit_name, ChildState, ConnectError, Connection, Detach, Dirs, Ended, Ensured, Event, Events,
    Exit, Found, Framing, Holds, Launch, Limits, Line, RunAs, Secret, SpawnError, WriteError,
    WriteOutcome, DETACH_ENV,
};
use maxhq_protocol::ids::SessionId;
use rustix::process::{Pid, Signal};
use serde_json::{json, Value};

use self::terminal::{
    a_goodbye_before_the_end_changes_nothing,
    a_password_prompt_is_flagged_and_its_answer_kept_nowhere,
    a_reader_past_the_ring_starts_over_at_a_whole_line, a_run_keeps_what_it_showed,
    a_run_shows_nothing_its_shell_printed_before_the_command,
    a_run_whose_startup_asks_for_something_shows_it,
    a_terminal_holder_refuses_a_client_without_its_key, a_terminal_outlives_its_spawner,
    a_terminal_replays_from_an_offset_without_a_gap, a_watch_gets_all_but_the_output,
    an_ended_terminal_leaves_its_exit_when_nobody_says_goodbye,
    interrupt_stops_what_the_shell_runs, kill_escalates_to_sigkill, kill_hangs_up_the_whole_group,
    the_title_and_folder_come_from_the_shell,
};

#[path = "holder/terminal.rs"]
mod terminal;

/// How long any one step may take before the test fails.
const WAIT: Duration = Duration::from_secs(15);

/// How long a whole test may take.
const TEST_WAIT: Duration = Duration::from_secs(90);

/// The fake CLI ends itself after this, so nothing outlives the tests for
/// long.
const CHILD_LIFETIME: Duration = Duration::from_secs(120);

/// How many tests run at once.
const PARALLEL: usize = 6;

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match args.first().and_then(|first| first.to_str()) {
        Some("__hold") => maxhq_hold::run_holder(&args[1..]),
        Some("__child") => fake_child(&args[1..]),
        Some("__spawner") => spawner(&args[1..]),
        _ => run_tests(&args),
    }
}

type TestFn = fn(Ctx) -> Pin<Box<dyn Future<Output = ()> + Send>>;

macro_rules! tests {
    ($($name:ident),* $(,)?) => {
        &[$((stringify!($name), |ctx| Box::pin($name(ctx)))),*]
    };
}

macro_rules! ignored {
    ($($name:ident: $reason:literal),* $(,)?) => {
        &[$((stringify!($name), $reason, |ctx| Box::pin($name(ctx)))),*]
    };
}

const TESTS: &[(&str, TestFn)] = tests![
    lines_are_echoed_numbered_and_recorded,
    a_reconnect_replays_without_a_gap,
    old_lines_come_from_disk_then_live,
    claude_prompts_survive_a_reconnect,
    json_rpc_prompts_survive_a_reconnect,
    the_first_answer_wins,
    a_withdrawn_prompt_is_no_longer_pending,
    the_log_rotates_and_keeps_one_old_file,
    an_exit_is_recorded_with_its_stderr,
    stop_closes_stdin_first,
    stop_escalates_to_sigkill,
    a_killed_holder_ends_with_an_unknown_exit,
    a_holder_outlives_a_killed_spawner,
    a_holder_outlives_a_killed_spawner_group,
    a_running_session_cant_be_forgotten,
    a_session_starts_once,
    a_missing_program_fails_the_spawn,
    a_missing_folder_fails_the_spawn_by_its_name,
    long_lines_from_two_writers_stay_whole,
    a_line_past_the_cap_leaves_a_marker_and_the_rest_comes,
    end_stops_a_running_holder_and_its_cli,
    end_kills_a_stuck_holder_and_the_cli_it_left,
    end_leaves_a_holder_it_cant_confirm,
    a_secret_reaches_the_cli_and_no_file,
    a_failed_spawn_never_shows_a_secret,
    run_as_needs_a_holder_running_as_root,
    ensure_reaches_the_running_holder,
    ensure_clears_a_folder_whose_holder_never_started,
    ensure_leaves_an_ended_session_alone,
    a_holder_refuses_a_client_without_its_key,
    a_client_started_later_finds_the_key,
    a_holder_from_before_keys_takes_any_client,
    the_key_shows_in_no_other_file_environment_or_arguments,
    a_terminal_replays_from_an_offset_without_a_gap,
    a_reader_past_the_ring_starts_over_at_a_whole_line,
    a_password_prompt_is_flagged_and_its_answer_kept_nowhere,
    the_title_and_folder_come_from_the_shell,
    a_watch_gets_all_but_the_output,
    interrupt_stops_what_the_shell_runs,
    kill_hangs_up_the_whole_group,
    kill_escalates_to_sigkill,
    an_ended_terminal_leaves_its_exit_when_nobody_says_goodbye,
    a_goodbye_before_the_end_changes_nothing,
    a_terminal_holder_refuses_a_client_without_its_key,
    a_run_keeps_what_it_showed,
    a_run_shows_nothing_its_shell_printed_before_the_command,
    a_run_whose_startup_asks_for_something_shows_it,
    a_terminal_outlives_its_spawner,
];

/// Tests that need root, systemd or a running `maxd`, each with why. They
/// run with `--ignored` or `--include-ignored`. The ones named `root_` need
/// the test binary to run as root.
const IGNORED: &[(&str, &str, TestFn)] = ignored![
    root_run_as_drops_the_cli_to_the_user:
        "needs root, and a user from MAXHQ_TEST_USER or nobody",
    root_system_scope_holds_the_holder: "needs root and a system systemd",
    root_system_scope_outlives_a_restarted_service: "needs root and a system systemd",
    root_a_stale_scope_is_cleared_before_a_spawn: "needs root and a system systemd",
    user_scope_outlives_a_restarted_service: "needs a user systemd",
    host_holders_outlive_a_maxd_restart:
        "restarts the real maxd user unit, and needs a session running under it",
];

// The test runner.

fn run_tests(args: &[OsString]) -> ExitCode {
    let args: Vec<&str> = args.iter().filter_map(|arg| arg.to_str()).collect();
    let only_ignored = args.contains(&"--ignored");
    let with_ignored = only_ignored || args.contains(&"--include-ignored");
    let exact = args.contains(&"--exact");
    let filters: Vec<&str> = args
        .iter()
        .copied()
        .filter(|arg| !arg.starts_with('-'))
        .collect();
    let wanted = |name: &str| {
        filters.is_empty()
            || filters.iter().any(|filter| {
                if exact {
                    name == *filter
                } else {
                    name.contains(filter)
                }
            })
    };
    let plain = TESTS.iter().map(|&(name, test)| (name, None, test));
    let ignored = IGNORED
        .iter()
        .map(|&(name, reason, test)| (name, Some(reason), test));
    let all: Vec<(&str, Option<&str>, TestFn)> = plain
        .chain(ignored)
        .filter(|&(name, reason, _)| wanted(name) && (!only_ignored || reason.is_some()))
        .collect();
    if args.contains(&"--list") {
        for (name, _, _) in &all {
            println!("{name}: test");
        }
        return ExitCode::SUCCESS;
    }
    let (chosen, skipped): (Vec<_>, Vec<_>) = all
        .into_iter()
        .partition(|&(_, reason, _)| with_ignored || reason.is_none());
    println!("\nrunning {} tests", chosen.len() + skipped.len());
    for (name, reason, _) in &skipped {
        println!("test {name} ... ignored, {}", reason.unwrap_or_default());
    }
    let total = chosen.len();
    let chosen: VecDeque<(&str, TestFn)> = chosen
        .into_iter()
        .map(|(name, _, test)| (name, test))
        .collect();
    let queue = Arc::new(Mutex::new(chosen));
    let failed = Arc::new(Mutex::new(Vec::new()));
    let workers: Vec<_> = (0..PARALLEL)
        .map(|_| {
            let queue = queue.clone();
            let failed = failed.clone();
            thread::spawn(move || loop {
                let next = queue.lock().unwrap().pop_front();
                let Some((name, test)) = next else {
                    return;
                };
                let started = Instant::now();
                let passed = run_one(name, test);
                let took = started.elapsed().as_secs_f32();
                let verdict = if passed { "ok" } else { "FAILED" };
                println!("test {name} ... {verdict} ({took:.1}s)");
                if !passed {
                    failed.lock().unwrap().push(name);
                }
            })
        })
        .collect();
    for worker in workers {
        let _ = worker.join();
    }
    let failed = failed.lock().unwrap();
    let verdict = if failed.is_empty() { "ok" } else { "FAILED" };
    println!(
        "\ntest result: {verdict}. {} passed; {} failed; {} ignored\n",
        total - failed.len(),
        failed.len(),
        skipped.len(),
    );
    if failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        for name in failed.iter() {
            println!("    {name}");
        }
        ExitCode::FAILURE
    }
}

fn run_one(name: &'static str, test: TestFn) -> bool {
    let thread = thread::Builder::new().name(name.into()).spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let ctx = Ctx::new();
        let finished = runtime.block_on(async { tokio::time::timeout(TEST_WAIT, test(ctx)).await });
        runtime.shutdown_timeout(Duration::from_secs(1));
        assert!(finished.is_ok(), "timed out after {TEST_WAIT:?}");
    });
    thread.is_ok_and(|thread| thread.join().is_ok())
}

/// One test's folders. Dropping it stops any holder the test left running
/// and removes the folders.
struct Ctx {
    root: PathBuf,
    dirs: Dirs,
}

impl Ctx {
    fn new() -> Self {
        // Short, because the socket path has to fit in 104 bytes on macOS.
        let id = SessionId::generate().0.simple().to_string();
        let root = std::env::temp_dir().join(format!("h-{}", &id[24..]));
        fs::create_dir_all(&root).unwrap();
        let dirs = Dirs {
            sessions: root.join("d"),
            sockets: root.join("s"),
        };
        Self { root, dirs }
    }

    fn holds(&self) -> Holds {
        self.holds_with(test_limits())
    }

    fn holds_with(&self, limits: Limits) -> Holds {
        self.holds_detached(Detach::Setsid).with_limits(limits)
    }

    fn holds_detached(&self, detach: Detach) -> Holds {
        Holds::new(self.dirs.clone())
            .with_holder(exe(), vec!["__hold".into()])
            .with_detach(detach)
            .with_limits(test_limits())
    }

    fn launch(&self, framing: Framing) -> Launch {
        fake_launch(SessionId::generate(), framing, &self.root)
    }

    fn session_dir(&self, session: SessionId) -> PathBuf {
        self.dirs.sessions.join(session.to_string())
    }

    /// Starts a fake CLI and connects to its holder.
    async fn start(&self, framing: Framing) -> (SessionId, Connection) {
        self.start_with(self.holds(), self.launch(framing)).await
    }

    async fn start_with(&self, holds: Holds, launch: Launch) -> (SessionId, Connection) {
        holds.spawn(&launch).await.unwrap();
        let connection = holds.connect(launch.session, None).await.unwrap();
        (launch.session, connection)
    }
}

impl Drop for Ctx {
    fn drop(&mut self) {
        // A holder counts as this test's only while it holds the lock in
        // this test's own folder, so nothing else is ever signalled.
        let running = || -> Vec<i32> {
            let Ok(entries) = fs::read_dir(&self.dirs.sessions) else {
                return Vec::new();
            };
            entries
                .filter_map(Result::ok)
                .filter_map(|entry| live_holder(&entry.path()))
                .collect()
        };
        for pid in running() {
            if let Some(pid) = Pid::from_raw(pid) {
                let _ = rustix::process::kill_process(pid, Signal::TERM);
                // A test may have stopped it.
                let _ = rustix::process::kill_process(pid, Signal::CONT);
            }
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while !running().is_empty() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        // A connection the test dropped may still be writing the exit it
        // read from the files. Moving the folder first means it can't
        // leave a file behind.
        let gone = self.root.with_extension("gone");
        let doomed = match fs::rename(&self.root, &gone) {
            Ok(()) => gone,
            Err(_) => self.root.clone(),
        };
        let _ = fs::remove_dir_all(doomed);
    }
}

/// The holder's process ID when one runs for the session in `dir`.
fn live_holder(dir: &Path) -> Option<i32> {
    let lock = File::open(dir.join("holder.lock")).ok()?;
    if !matches!(lock.try_lock(), Err(TryLockError::WouldBlock)) {
        return None;
    }
    let holder: Value = serde_json::from_slice(&fs::read(dir.join("holder.json")).ok()?).ok()?;
    i32::try_from(holder["holder_pid"].as_u64()?).ok()
}

fn pid(pid: u32) -> Pid {
    Pid::from_raw(i32::try_from(pid).unwrap()).unwrap()
}

fn exe() -> PathBuf {
    std::env::current_exe().unwrap()
}

fn test_limits() -> Limits {
    Limits {
        linger: Duration::from_secs(3),
        ..Limits::default()
    }
}

fn fake_launch(session: SessionId, framing: Framing, cwd: &Path) -> Launch {
    Launch {
        session,
        program: exe(),
        args: vec!["__child".into()],
        env: vec![("FAKE_AGENT".into(), "1".into())],
        env_remove: Vec::new(),
        secret_env: Vec::new(),
        run_as: None,
        cwd: cwd.to_owned(),
        framing,
    }
}

// Helpers for reading and writing.

fn print(line: &str) -> String {
    json!({"fake": "print", "line": line}).to_string()
}

fn flood(count: u64, size: usize) -> String {
    json!({"fake": "flood", "count": count, "size": size}).to_string()
}

fn parse(line: &Line) -> Value {
    serde_json::from_str(&line.text).unwrap_or(Value::Null)
}

/// What the fake CLI got, when `line` is its echo.
fn echoed(line: &Line) -> Option<String> {
    let value = parse(line);
    (value["type"] == "echo").then(|| value["got"].as_str().unwrap_or_default().to_owned())
}

async fn next_line(events: &mut Events) -> Line {
    match tokio::time::timeout(WAIT, events.recv()).await {
        Ok(Some(Event::Line(line))) => line,
        Ok(Some(Event::Exited(exit))) => panic!("the agent ended: {exit:?}"),
        Ok(None) => panic!("the events ended"),
        Err(_) => panic!("no line within {WAIT:?}"),
    }
}

async fn lines(events: &mut Events, count: usize) -> Vec<Line> {
    let mut lines: Vec<Line> = Vec::with_capacity(count);
    for _ in 0..count {
        match tokio::time::timeout(WAIT, events.recv()).await {
            Ok(Some(Event::Line(line))) => lines.push(line),
            other => panic!(
                "after {} of {count} lines, the last {:?}, came {other:?}",
                lines.len(),
                lines.last().map(|line| line.seq),
            ),
        }
    }
    lines
}

async fn until_exit(events: &mut Events) -> (Vec<Line>, Exit) {
    let mut lines = Vec::new();
    loop {
        match tokio::time::timeout(WAIT, events.recv()).await {
            Ok(Some(Event::Line(line))) => lines.push(line),
            Ok(Some(Event::Exited(exit))) => return (lines, exit),
            Ok(None) => panic!("the events ended without an exit"),
            Err(_) => panic!("no exit within {WAIT:?}"),
        }
    }
}

async fn write(connection: &Connection, line: &str) -> WriteOutcome {
    connection
        .writer
        .write(line.to_owned(), None)
        .await
        .unwrap()
}

/// Writes `line` and returns the echo.
async fn echo(connection: &mut Connection, line: &str) -> Line {
    assert_eq!(write(connection, line).await, WriteOutcome::Written);
    let echo = next_line(&mut connection.events).await;
    assert_eq!(echoed(&echo).as_deref(), Some(line), "{echo:?}");
    echo
}

fn assert_contiguous(lines: &[Line], from: u64) {
    for (offset, line) in lines.iter().enumerate() {
        assert_eq!(line.seq, from + offset as u64, "a gap before {line:?}");
    }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !check() {
        assert!(Instant::now() < deadline, "{what} didn't happen in time");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn claude_prompt(id: &str) -> String {
    json!({
        "type": "control_request",
        "request_id": id,
        "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "ls"}},
    })
    .to_string()
}

fn claude_answer(id: &str) -> String {
    json!({
        "type": "control_response",
        "response": {"subtype": "success", "request_id": id, "response": {"behavior": "allow"}},
    })
    .to_string()
}

// The tests.

async fn a_line_past_the_cap_leaves_a_marker_and_the_rest_comes(ctx: Ctx) {
    // A byte past the holder's cap of 64 MiB, once the test hears every line.
    let launch = Launch {
        program: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            "read go; echo before; head -c 67108865 /dev/zero | tr '\\000' x; echo; echo after"
                .into(),
        ],
        ..ctx.launch(Framing::ClaudeStreamJson)
    };
    let (_, mut connection) = ctx.start_with(ctx.holds(), launch).await;
    assert_eq!(write(&connection, "go").await, WriteOutcome::Written);
    // The holder reads all of the long line, which takes a while under load.
    let mut lines = Vec::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(80), connection.events.recv());
        match event.await {
            Ok(Some(Event::Line(line))) => lines.push(line),
            Ok(Some(Event::Exited(_))) => break,
            Ok(None) => panic!("the events ended without an exit"),
            Err(_) => panic!("no exit within 80 seconds"),
        }
    }
    assert_contiguous(&lines, 1);
    let texts: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "before",
            "maxhq: left out a line of 67108866 bytes from the agent's output, past the limit of 67108864 bytes",
            "after",
        ]
    );
}

async fn lines_are_echoed_numbered_and_recorded(ctx: Ctx) {
    let (session, mut connection) = ctx.start(Framing::ClaudeStreamJson).await;
    assert_eq!(connection.hello.last_seq, 0);
    assert_eq!(connection.hello.first_seq, 1);
    assert!(matches!(connection.hello.child, ChildState::Running { .. }));
    let first = echo(&mut connection, "hello").await;
    assert_eq!((first.seq, first.replayed), (1, false));
    // A trailing newline is fine, and the fake sees the line without it.
    assert_eq!(write(&connection, "second\n").await, WriteOutcome::Written);
    let second = next_line(&mut connection.events).await;
    assert_eq!(
        (second.seq, echoed(&second).as_deref()),
        (2, Some("second"))
    );
    let bad = connection.writer.write("one\ntwo".into(), None).await;
    assert!(matches!(bad, Err(WriteError::BadLine)), "{bad:?}");

    let log = fs::read_to_string(ctx.session_dir(session).join("events.log")).unwrap();
    let stored: Vec<Value> = log
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0]["seq"], 1);
    assert_eq!(stored[0]["line"], first.text);
    assert_eq!(ctx.holds().list().unwrap(), [session]);
    connection
        .writer
        .stop(Duration::from_secs(5))
        .await
        .unwrap();
}

async fn a_reconnect_replays_without_a_gap(ctx: Ctx) {
    let holds = ctx.holds();
    let (session, mut first) = ctx.start(Framing::ClaudeStreamJson).await;
    assert_eq!(
        write(&first, &flood(3000, 200)).await,
        WriteOutcome::Written
    );
    let seen = lines(&mut first.events, 50).await;
    assert_contiguous(&seen, 1);
    // The CLI keeps printing while nobody is connected.
    drop(first);
    let mut second = holds.connect(session, Some(50)).await.unwrap();
    let replay_upto = second.hello.last_seq;
    assert!(replay_upto >= 50);
    let rest = lines(&mut second.events, 2950).await;
    assert_contiguous(&rest, 51);
    for line in &rest {
        assert_eq!(line.replayed, line.seq <= replay_upto, "{}", line.seq);
    }
    assert_eq!(parse(rest.last().unwrap())["n"], 2999);
    let live = echo(&mut second, "after").await;
    assert_eq!((live.seq, live.replayed), (3001, false));
    second.writer.stop(Duration::from_secs(5)).await.unwrap();
}

async fn old_lines_come_from_disk_then_live(ctx: Ctx) {
    let limits = Limits {
        memory_tail: 2048,
        ..test_limits()
    };
    let holds = ctx.holds_with(limits);
    let launch = ctx.launch(Framing::JsonRpc);
    let (session, mut first) = ctx.start_with(holds.clone(), launch).await;
    write(&first, &flood(300, 100)).await;
    let all = lines(&mut first.events, 300).await;
    assert_contiguous(&all, 1);
    let mut second = holds.connect(session, Some(10)).await.unwrap();
    assert_eq!(second.hello.first_seq, 1);
    assert_eq!(second.hello.last_seq, 300);
    // More output while the second client is still reading the disk.
    write(&first, &flood(300, 100)).await;
    let caught_up = lines(&mut second.events, 590).await;
    assert_contiguous(&caught_up, 11);
    for (got, want) in caught_up.iter().zip(&all[10..]) {
        assert_eq!(got.text, want.text);
        assert!(got.replayed);
    }
    assert_eq!(parse(&caught_up[589])["n"], 299);
    second.writer.stop(Duration::from_secs(5)).await.unwrap();
}

async fn claude_prompts_survive_a_reconnect(ctx: Ctx) {
    let holds = ctx.holds();
    let (session, mut first) = ctx.start(Framing::ClaudeStreamJson).await;
    let prompt = claude_prompt("r1");
    write(&first, &print(&prompt)).await;
    // An initialize request is for the host, so it isn't pending.
    let setup = json!({"type": "control_request", "request_id": "i1", "request": {"subtype": "initialize"}});
    write(&first, &print(&setup.to_string())).await;
    let asked = next_line(&mut first.events).await;
    assert_eq!(asked.text, prompt);
    next_line(&mut first.events).await;
    let on_disk = read_json(&ctx.session_dir(session).join("pending.json"));
    assert_eq!(on_disk[0]["key"], "r1");
    assert_eq!(on_disk.as_array().unwrap().len(), 1);
    drop(first);

    let mut second = holds.connect(session, Some(2)).await.unwrap();
    let pending = &second.hello.pending;
    assert_eq!(pending.len(), 1);
    assert_eq!((pending[0].seq, pending[0].replayed), (asked.seq, true));
    assert_eq!(pending[0].text, prompt);

    let answer = claude_answer("r1");
    echo(&mut second, &answer).await;
    let on_disk = read_json(&ctx.session_dir(session).join("pending.json"));
    assert_eq!(on_disk, json!([]));
    // A second answer never reaches the CLI.
    assert_eq!(write(&second, &answer).await, WriteOutcome::AlreadyAnswered);
    echo(&mut second, "marker").await;
    // Answers to requests that aren't prompts always go through.
    let reply = json!({
        "type": "control_response",
        "response": {"subtype": "success", "request_id": "i1"},
    })
    .to_string();
    echo(&mut second, &reply).await;
    echo(&mut second, &reply).await;
    let third = holds.connect(session, None).await.unwrap();
    assert!(third.hello.pending.is_empty());
    second.writer.stop(Duration::from_secs(5)).await.unwrap();
}

async fn json_rpc_prompts_survive_a_reconnect(ctx: Ctx) {
    let holds = ctx.holds();
    let (session, mut first) = ctx.start(Framing::JsonRpc).await;
    let request = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "item/commandExecution/requestApproval",
        "params": {"threadId": "t", "itemId": "i"},
    })
    .to_string();
    let notice = json!({"jsonrpc": "2.0", "method": "turn/started", "params": {}}).to_string();
    write(&first, &print(&request)).await;
    write(&first, &print(&notice)).await;
    lines(&mut first.events, 2).await;
    drop(first);

    let mut second = holds.connect(session, Some(0)).await.unwrap();
    assert_eq!(second.hello.pending.len(), 1);
    assert_eq!(second.hello.pending[0].text, request);
    let replay = lines(&mut second.events, 2).await;
    assert!(replay.iter().all(|line| line.replayed));
    let answer = json!({"jsonrpc": "2.0", "id": 7, "result": {"decision": "accept"}}).to_string();
    let outcome = second
        .writer
        .write(answer.clone(), Some("7".into()))
        .await
        .unwrap();
    assert_eq!(outcome, WriteOutcome::Written);
    assert_eq!(
        echoed(&next_line(&mut second.events).await),
        Some(answer.clone())
    );
    assert_eq!(write(&second, &answer).await, WriteOutcome::AlreadyAnswered);
    second.writer.stop(Duration::from_secs(5)).await.unwrap();
}

async fn the_first_answer_wins(ctx: Ctx) {
    let holds = ctx.holds();
    let (session, mut first) = ctx.start(Framing::ClaudeStreamJson).await;
    let mut second = holds.connect(session, None).await.unwrap();
    write(&first, &print(&claude_prompt("r9"))).await;
    next_line(&mut first.events).await;
    next_line(&mut second.events).await;

    let (a, b) = tokio::join!(
        first.writer.write(claude_answer("r9"), None),
        second.writer.write(claude_answer("r9"), Some("r9".into())),
    );
    let mut outcomes = [a.unwrap(), b.unwrap()];
    outcomes.sort_by_key(|outcome| *outcome == WriteOutcome::Written);
    assert_eq!(
        outcomes,
        [WriteOutcome::AlreadyAnswered, WriteOutcome::Written]
    );
    // Naming a prompt that was never asked is also too late.
    let stray = first
        .writer
        .write("anything".into(), Some("never".into()))
        .await
        .unwrap();
    assert_eq!(stray, WriteOutcome::AlreadyAnswered);
    write(&first, "marker").await;
    // Both clients see the one answer that got through, then the marker.
    for events in [&mut first.events, &mut second.events] {
        let got = lines(events, 2).await;
        assert_eq!(echoed(&got[0]), Some(claude_answer("r9")));
        assert_eq!(echoed(&got[1]).as_deref(), Some("marker"));
    }
    first.writer.stop(Duration::from_secs(5)).await.unwrap();
}

async fn a_withdrawn_prompt_is_no_longer_pending(ctx: Ctx) {
    let holds = ctx.holds();
    let (claude, mut connection) = ctx.start(Framing::ClaudeStreamJson).await;
    write(&connection, &print(&claude_prompt("r2"))).await;
    let cancel = json!({"type": "control_cancel_request", "request_id": "r2"}).to_string();
    write(&connection, &print(&cancel)).await;
    lines(&mut connection.events, 2).await;
    let again = holds.connect(claude, None).await.unwrap();
    assert!(again.hello.pending.is_empty());
    let late = again.writer.write(claude_answer("r2"), None).await.unwrap();
    assert_eq!(late, WriteOutcome::AlreadyAnswered);
    assert_eq!(
        read_json(&ctx.session_dir(claude).join("pending.json")),
        json!([])
    );
    connection
        .writer
        .stop(Duration::from_secs(5))
        .await
        .unwrap();

    let (codex, mut connection) = ctx.start(Framing::JsonRpc).await;
    let request = json!({"jsonrpc": "2.0", "id": 9, "method": "item/fileChange/requestApproval", "params": {}});
    let resolved = json!({"jsonrpc": "2.0", "method": "serverRequest/resolved", "params": {"threadId": "t", "requestId": 9}});
    write(&connection, &print(&request.to_string())).await;
    write(&connection, &print(&resolved.to_string())).await;
    lines(&mut connection.events, 2).await;
    let again = holds.connect(codex, None).await.unwrap();
    assert!(again.hello.pending.is_empty());
    let late = json!({"jsonrpc": "2.0", "id": 9, "result": {}}).to_string();
    assert_eq!(write(&again, &late).await, WriteOutcome::AlreadyAnswered);
    connection
        .writer
        .stop(Duration::from_secs(5))
        .await
        .unwrap();
}

async fn the_log_rotates_and_keeps_one_old_file(ctx: Ctx) {
    let limits = Limits {
        rotate_at: 8 * 1024,
        memory_tail: 2048,
        ..test_limits()
    };
    let holds = ctx.holds_with(limits);
    let launch = ctx.launch(Framing::ClaudeStreamJson);
    let (session, first) = ctx.start_with(holds.clone(), launch).await;
    write(&first, &flood(400, 50)).await;
    // Nobody reads while the CLI prints, so every replay comes from disk.
    drop(first);
    let deadline = Instant::now() + WAIT;
    while holds.connect(session, None).await.unwrap().hello.last_seq < 400 {
        assert!(Instant::now() < deadline, "the flood didn't land in time");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let dir = ctx.session_dir(session);
    assert!(dir.join("events.log.1").exists());
    assert!(!dir.join("events.log.2").exists());
    let on_disk = fs::metadata(dir.join("events.log")).unwrap().len()
        + fs::metadata(dir.join("events.log.1")).unwrap().len();
    assert!(on_disk <= 16 * 1024 + 200, "{on_disk} bytes");

    let mut second = holds.connect(session, Some(0)).await.unwrap();
    let first_seq = second.hello.first_seq;
    assert!(first_seq > 1, "{first_seq}");
    let count = usize::try_from(400 - first_seq + 1).unwrap();
    let kept = lines(&mut second.events, count).await;
    assert_contiguous(&kept, first_seq);
    assert_eq!(kept.last().unwrap().seq, 400);
    second.writer.stop(Duration::from_secs(5)).await.unwrap();
}

async fn an_exit_is_recorded_with_its_stderr(ctx: Ctx) {
    let holds = ctx.holds();
    let (session, mut connection) = ctx.start(Framing::ClaudeStreamJson).await;
    echo(&mut connection, "one").await;
    let say = json!({"fake": "stderr", "text": "something broke\n"}).to_string();
    write(&connection, &say).await;
    // Let the stderr line land before the exit, as a real CLI's would.
    tokio::time::sleep(Duration::from_millis(100)).await;
    write(&connection, &json!({"fake": "exit", "code": 3}).to_string()).await;
    assert_eq!(holds.list().unwrap(), [session]);
    let (_, exit) = until_exit(&mut connection.events).await;
    assert_eq!((exit.code, exit.signal), (Some(3), None));
    assert!(exit.stderr_tail.contains("something broke"), "{exit:?}");
    let dir = ctx.session_dir(session);
    let recorded = read_json(&dir.join("exit.json"));
    assert_eq!(recorded["code"], 3);
    assert!(fs::read_to_string(dir.join("stderr.log"))
        .unwrap()
        .contains("something broke"));
    // Once a client has seen the exit, the holder goes and the session
    // isn't listed.
    eventually("the holder leaving", || live_holder(&dir).is_none()).await;
    assert!(holds.list().unwrap().is_empty());

    let mut again = holds.connect(session, Some(0)).await.unwrap();
    assert_eq!(again.hello.child, ChildState::Exited(exit.clone()));
    assert_eq!(again.hello.last_seq, 1);
    let (replayed, exit_again) = until_exit(&mut again.events).await;
    assert_eq!(replayed.len(), 1);
    assert!(replayed[0].replayed);
    assert_eq!(exit_again, exit);
    let late = again.writer.write("too late".into(), None).await;
    assert!(matches!(late, Err(WriteError::Finished)), "{late:?}");
    again.writer.stop(Duration::from_secs(1)).await.unwrap();
    again.writer.forget().await.unwrap();
    assert!(!dir.exists());
    let gone = holds.connect(session, None).await;
    assert!(
        matches!(gone, Err(ConnectError::NotFound { .. })),
        "{gone:?}"
    );
}

async fn stop_closes_stdin_first(ctx: Ctx) {
    let (_, mut connection) = ctx.start(Framing::JsonRpc).await;
    echo(&mut connection, "x").await;
    connection
        .writer
        .stop(Duration::from_secs(5))
        .await
        .unwrap();
    let (_, exit) = until_exit(&mut connection.events).await;
    // The fake ends by itself at the end of its input.
    assert_eq!((exit.code, exit.signal), (Some(0), None));
}

async fn stop_escalates_to_sigkill(ctx: Ctx) {
    let mut launch = ctx.launch(Framing::ClaudeStreamJson);
    launch.args.push("--stubborn".into());
    let (_, mut connection) = ctx.start_with(ctx.holds(), launch).await;
    echo(&mut connection, "x").await;
    let started = Instant::now();
    connection
        .writer
        .stop(Duration::from_millis(300))
        .await
        .unwrap();
    assert!(started.elapsed() >= Duration::from_millis(600));
    let (_, exit) = until_exit(&mut connection.events).await;
    assert_eq!((exit.code, exit.signal), (None, Some(9)));
    let after = connection.writer.write("x".into(), None).await;
    assert!(matches!(after, Err(WriteError::Finished)), "{after:?}");
}

async fn a_killed_holder_ends_with_an_unknown_exit(ctx: Ctx) {
    let holds = ctx.holds();
    let (session, mut connection) = ctx.start(Framing::ClaudeStreamJson).await;
    echo(&mut connection, "before").await;
    let dir = ctx.session_dir(session);
    let holder = live_holder(&dir).unwrap();
    assert_eq!(holder.unsigned_abs(), connection.hello.holder_pid);
    rustix::process::kill_process(Pid::from_raw(holder).unwrap(), Signal::KILL).unwrap();
    let (_, exit) = until_exit(&mut connection.events).await;
    assert_eq!((exit.code, exit.signal), (None, None));
    let again = holds.connect(session, None).await.unwrap();
    assert_eq!(again.hello.child, ChildState::Exited(exit));
    assert_eq!(again.hello.last_seq, 1);
}

async fn a_holder_outlives_a_killed_spawner(ctx: Ctx) {
    outlives_its_spawner(ctx, false).await;
}

async fn a_holder_outlives_a_killed_spawner_group(ctx: Ctx) {
    outlives_its_spawner(ctx, true).await;
}

/// Starts a holder from a separate process, kills that process or its
/// whole process group, and checks the holder and its CLI carry on.
async fn outlives_its_spawner(ctx: Ctx, whole_group: bool) {
    let session = SessionId::generate();
    let mut spawner = start_spawner(&ctx, session, whole_group).await;
    let spawner_pid = i32::try_from(spawner.id()).unwrap();
    if whole_group {
        // The spawner isn't reaped yet, so its group ID is still its own.
        rustix::process::kill_process_group(Pid::from_raw(spawner_pid).unwrap(), Signal::KILL)
            .unwrap();
    } else {
        spawner.kill().unwrap();
    }
    spawner.wait().unwrap();

    let mut connection = ctx.holds().connect(session, None).await.unwrap();
    let ChildState::Running { pid: child } = connection.hello.child else {
        panic!("the agent ended: {:?}", connection.hello.child);
    };
    let holder = Pid::from_raw(i32::try_from(connection.hello.holder_pid).unwrap()).unwrap();
    let child = Pid::from_raw(i32::try_from(child).unwrap()).unwrap();
    assert_ne!(holder.as_raw_nonzero().get(), spawner_pid);
    // The holder leads a session of its own, and the CLI a process group.
    if let Ok(sid) = rustix::process::getsid(Some(holder)) {
        assert_eq!(sid, holder);
    }
    if let Ok(group) = rustix::process::getpgid(Some(holder)) {
        assert_eq!(group, holder);
    }
    if let Ok(group) = rustix::process::getpgid(Some(child)) {
        assert_eq!(group, child);
    }
    echo(&mut connection, "still here").await;
    connection
        .writer
        .stop(Duration::from_secs(5))
        .await
        .unwrap();
    let (_, exit) = until_exit(&mut connection.events).await;
    assert_eq!(exit.code, Some(0));
}

/// Runs `__spawner` for `session`, in a process group of its own when
/// `own_group`, and waits until it says the holder has started.
async fn start_spawner(ctx: &Ctx, session: SessionId, own_group: bool) -> std::process::Child {
    let mut command = Command::new(exe());
    command
        .arg("__spawner")
        .arg(&ctx.dirs.sessions)
        .arg(&ctx.dirs.sockets)
        .arg(session.to_string())
        .arg(&ctx.root)
        .env(DETACH_ENV, "setsid")
        .stdin(Stdio::null())
        .stdout(Stdio::piped());
    if own_group {
        command.process_group(0);
    }
    let mut spawner = command.spawn().unwrap();
    let stdout = spawner.stdout.take().unwrap();
    let said = tokio::task::spawn_blocking(move || {
        let mut line = String::new();
        BufReader::new(stdout).read_line(&mut line).map(|_| line)
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(said.trim(), "spawned");
    spawner
}

async fn a_running_session_cant_be_forgotten(ctx: Ctx) {
    let holds = ctx.holds();
    let (one, first) = ctx.start(Framing::ClaudeStreamJson).await;
    let (two, mut second) = ctx.start(Framing::JsonRpc).await;
    let mut both = vec![one, two];
    both.sort();
    assert_eq!(holds.list().unwrap(), both);
    let early = first.writer.forget().await;
    assert!(matches!(early, Err(WriteError::StillRunning)), "{early:?}");
    second.writer.stop(Duration::from_secs(5)).await.unwrap();
    until_exit(&mut second.events).await;
    second.writer.forget().await.unwrap();
    assert!(!ctx.session_dir(two).exists());
    assert_eq!(holds.list().unwrap(), [one]);
    first.writer.stop(Duration::from_secs(5)).await.unwrap();
}

async fn a_session_starts_once(ctx: Ctx) {
    let holds = ctx.holds();
    let launch = ctx.launch(Framing::ClaudeStreamJson);
    holds.spawn(&launch).await.unwrap();
    let again = holds.spawn(&launch).await;
    assert!(matches!(again, Err(SpawnError::Exists { .. })), "{again:?}");
    let connection = holds.connect(launch.session, None).await.unwrap();
    connection
        .writer
        .stop(Duration::from_secs(5))
        .await
        .unwrap();
}

async fn a_missing_program_fails_the_spawn(ctx: Ctx) {
    let holds = ctx.holds();
    let mut launch = ctx.launch(Framing::ClaudeStreamJson);
    launch.program = ctx.root.join("no-such-agent");
    let failed = holds.spawn(&launch).await;
    match failed {
        Err(SpawnError::Holder { message }) => {
            assert!(message.contains("no-such-agent"), "{message}");
        }
        other => panic!("expected the holder to fail, got {other:?}"),
    }
    assert!(!ctx.session_dir(launch.session).exists());
    let missing = holds.connect(launch.session, None).await;
    assert!(
        matches!(missing, Err(ConnectError::NotFound { .. })),
        "{missing:?}"
    );
}

async fn a_missing_folder_fails_the_spawn_by_its_name(ctx: Ctx) {
    let holds = ctx.holds();
    let mut launch = ctx.launch(Framing::ClaudeStreamJson);
    launch.cwd = ctx.root.join("wiped");
    let err = holds.spawn(&launch).await.unwrap_err();
    // It names the folder rather than the program, which is there, in the
    // holder's own words.
    assert!(matches!(err, SpawnError::Holder { .. }), "{err:?}");
    assert_eq!(
        err.to_string(),
        format!("the folder {} is gone", launch.cwd.display())
    );
    assert!(!ctx.session_dir(launch.session).exists());
}

async fn long_lines_from_two_writers_stay_whole(ctx: Ctx) {
    let (_, mut connection) = ctx.start(Framing::JsonRpc).await;
    let line = |who: char, n: usize| format!("{who}{n:03}{}", who.to_string().repeat(100_000));
    let send = |who: char| {
        let writer = connection.writer.clone();
        async move {
            for n in 0..30 {
                let outcome = writer.write(line(who, n), None).await.unwrap();
                assert_eq!(outcome, WriteOutcome::Written);
            }
        }
    };
    tokio::join!(send('a'), send('b'));
    let mut expected: HashSet<String> =
        (0..30).flat_map(|n| [line('a', n), line('b', n)]).collect();
    for got in lines(&mut connection.events, 60).await {
        let got = echoed(&got).expect("an echo");
        assert!(expected.remove(&got), "a line came back changed");
    }
    assert!(expected.is_empty());
    connection
        .writer
        .stop(Duration::from_secs(5))
        .await
        .unwrap();
}

async fn end_stops_a_running_holder_and_its_cli(ctx: Ctx) {
    let holds = ctx.holds();
    let (session, mut connection) = ctx.start(Framing::ClaudeStreamJson).await;
    echo(&mut connection, "x").await;
    let dir = ctx.session_dir(session);
    let holder = connection.hello.holder_pid;
    let ended = holds.end(session, Some(holder), WAIT).await;
    assert_eq!(ended, Ended::Gone);
    assert_eq!(live_holder(&dir), None);
    // The holder stopped the CLI and recorded how it ended.
    let exit = read_json(&dir.join("exit.json"));
    assert_eq!(exit["code"], 0, "{exit}");
    // Ending it again finds nothing to end.
    assert_eq!(holds.end(session, None, WAIT).await, Ended::Gone);
}

async fn end_kills_a_stuck_holder_and_the_cli_it_left(ctx: Ctx) {
    let holds = ctx.holds();
    let mut launch = ctx.launch(Framing::ClaudeStreamJson);
    launch.args.push("--stubborn".into());
    let (session, mut connection) = ctx.start_with(holds.clone(), launch).await;
    echo(&mut connection, "x").await;
    let ChildState::Running { pid: cli } = connection.hello.child else {
        panic!("the agent ended: {:?}", connection.hello.child);
    };
    let holder = connection.hello.holder_pid;
    // A stopped holder can't act on SIGTERM, and the stubborn CLI ignores
    // it, so both need SIGKILL.
    rustix::process::kill_process(pid(holder), Signal::STOP).unwrap();
    let grace = Duration::from_millis(300);
    let started = Instant::now();
    let ended = holds.end(session, Some(holder), grace).await;
    assert_eq!(ended, Ended::Gone);
    assert!(started.elapsed() >= grace * 2);
    let dir = ctx.session_dir(session);
    assert_eq!(live_holder(&dir), None);
    // SIGKILL has landed, but the CLI counts as alive until init reaps it.
    eventually("the stuck CLI to go", || {
        rustix::process::test_kill_process(pid(cli)).is_err()
    })
    .await;
}

async fn end_leaves_a_holder_it_cant_confirm(ctx: Ctx) {
    let holds = ctx.holds();
    let (session, mut connection) = ctx.start(Framing::JsonRpc).await;
    let holder = connection.hello.holder_pid;
    let ended = holds.end(session, Some(holder + 1), WAIT).await;
    assert!(
        matches!(&ended, Ended::Unconfirmed { pid: Some(pid), .. } if *pid == holder),
        "{ended:?}"
    );
    // Nothing was signalled.
    echo(&mut connection, "still here").await;
    connection
        .writer
        .stop(Duration::from_secs(5))
        .await
        .unwrap();
    let (_, exit) = until_exit(&mut connection.events).await;
    assert_eq!(exit.code, Some(0));
}

// Secrets, users and finding a holder again.

/// A variable whose value no file may ever hold. The value is new each
/// run, so a match can't come from an earlier one.
fn test_secret() -> (OsString, Secret) {
    let id = SessionId::generate().0.simple().to_string();
    let value = format!("test-not-a-real-token-{id}");
    ("MAXHQ_TEST_SECRET".into(), Secret::new(value))
}

/// Every file under `dir`, read whole.
fn every_file(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let mut folders = vec![dir.to_owned()];
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                folders.push(entry.path());
            } else if kind.is_file() {
                files.push((entry.path(), fs::read(entry.path()).unwrap()));
            }
        }
    }
    files
}

fn holds_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn assert_no_file_has(dir: &Path, value: &str) {
    for (path, bytes) in every_file(dir) {
        assert!(
            !holds_bytes(&bytes, value.as_bytes()),
            "{} has the secret",
            path.display()
        );
    }
}

async fn a_secret_reaches_the_cli_and_no_file(ctx: Ctx) {
    let (name, secret) = test_secret();
    let value = secret.expose().to_owned();
    let mut launch = ctx.launch(Framing::ClaudeStreamJson);
    launch.secret_env.push((name, secret));
    let shown = format!("{launch:?}");
    assert!(
        !shown.contains(&value) && shown.contains("<secret>"),
        "{shown}"
    );
    let (session, mut connection) = ctx.start_with(ctx.holds(), launch).await;
    let ask = json!({"fake": "secret", "name": "MAXHQ_TEST_SECRET"}).to_string();
    write(&connection, &ask).await;
    let said = parse(&next_line(&mut connection.events).await);
    assert_eq!(said["type"], "secret", "{said}");
    assert_eq!(said["len"], value.len());
    assert_eq!(said["hash"], fnv(value.as_bytes()));
    let dir = ctx.session_dir(session);
    let launched = read_json(&dir.join("launch.json"));
    assert_eq!(launched["secret_env"], json!(["MAXHQ_TEST_SECRET"]));
    if cfg!(target_os = "linux") {
        // The holder keeps the value out of its own environment.
        let holder = connection.hello.holder_pid;
        for part in ["environ", "cmdline"] {
            let read = fs::read(format!("/proc/{holder}/{part}")).unwrap();
            assert!(
                !holds_bytes(&read, value.as_bytes()),
                "the holder's {part} has the secret"
            );
        }
    }
    connection
        .writer
        .stop(Duration::from_secs(5))
        .await
        .unwrap();
    let (_, exit) = until_exit(&mut connection.events).await;
    assert_eq!(exit.code, Some(0));
    eventually("the holder leaving", || live_holder(&dir).is_none()).await;
    assert!(dir.join("launch.json").exists());
    assert_no_file_has(&ctx.root, &value);
}

async fn a_failed_spawn_never_shows_a_secret(ctx: Ctx) {
    let (name, secret) = test_secret();
    let value = secret.expose().to_owned();
    let mut launch = ctx.launch(Framing::ClaudeStreamJson);
    launch.program = ctx.root.join("no-such-agent");
    launch.secret_env.push((name, secret));
    let failed = ctx.holds().spawn(&launch).await;
    let Err(err) = failed else {
        panic!("the spawn worked");
    };
    assert!(matches!(err, SpawnError::Holder { .. }), "{err:?}");
    for shown in [err.to_string(), format!("{err:?}")] {
        assert!(!shown.contains(&value), "{shown}");
    }
    assert!(!ctx.session_dir(launch.session).exists());
    assert_no_file_has(&ctx.root, &value);
}

async fn run_as_needs_a_holder_running_as_root(ctx: Ctx) {
    let root = RunAs::lookup("root").unwrap();
    assert_eq!((root.uid, root.gid), (0, 0));
    let missing = RunAs::lookup("maxhq-no-such-user");
    assert_eq!(missing.unwrap_err().kind(), io::ErrorKind::NotFound);
    // As root, the root_ test checks what happens instead.
    if rustix::process::geteuid().is_root() {
        return;
    }
    let mut launch = ctx.launch(Framing::JsonRpc);
    launch.run_as = Some(RunAs {
        user: "nobody".into(),
        uid: 65_534,
        gid: 65_534,
        home: "/nonexistent".into(),
    });
    match ctx.holds().spawn(&launch).await {
        Err(SpawnError::Holder { message }) => assert!(message.contains("root"), "{message}"),
        other => panic!("expected the holder to refuse, got {other:?}"),
    }
    assert!(!ctx.session_dir(launch.session).exists());
}

async fn ensure_reaches_the_running_holder(ctx: Ctx) {
    let holds = ctx.holds();
    let launch = ctx.launch(Framing::ClaudeStreamJson);
    assert_eq!(holds.find(launch.session).await.unwrap(), Found::Nothing);
    assert_eq!(holds.ensure(&launch).await.unwrap(), Ensured::Spawned);
    let mut connection = holds.connect(launch.session, None).await.unwrap();
    let holder_pid = connection.hello.holder_pid;
    let live = Found::Live { holder_pid };
    assert_eq!(holds.find(launch.session).await.unwrap(), live);
    // A caller that restarted asks again and reaches the same holder.
    let again = ctx.holds().ensure(&launch).await.unwrap();
    assert_eq!(again, Ensured::Existing(live));
    echo(&mut connection, "still the one").await;

    // Two at once start one holder between them.
    let other = ctx.launch(Framing::JsonRpc);
    let (one, two) = tokio::join!(holds.ensure(&other), holds.ensure(&other));
    let mut both = [one.unwrap(), two.unwrap()];
    both.sort_by_key(|ensured| *ensured != Ensured::Spawned);
    assert_eq!(both[0], Ensured::Spawned, "{both:?}");
    assert!(
        matches!(both[1], Ensured::Existing(Found::Live { .. })),
        "{both:?}"
    );
    let second = holds.connect(other.session, None).await.unwrap();
    for writer in [&connection.writer, &second.writer] {
        writer.stop(Duration::from_secs(5)).await.unwrap();
    }
}

async fn ensure_clears_a_folder_whose_holder_never_started(ctx: Ctx) {
    let holds = ctx.holds();
    let launch = ctx.launch(Framing::ClaudeStreamJson);
    let dir = ctx.session_dir(launch.session);
    fs::create_dir_all(&dir).unwrap();
    // While it's new, a holder may yet start in it.
    assert_eq!(holds.find(launch.session).await.unwrap(), Found::Starting);
    let long_ago = SystemTime::now() - Duration::from_secs(10 * 60);
    File::open(&dir).unwrap().set_modified(long_ago).unwrap();
    assert_eq!(holds.find(launch.session).await.unwrap(), Found::NotStarted);
    assert_eq!(holds.ensure(&launch).await.unwrap(), Ensured::Spawned);
    let mut connection = holds.connect(launch.session, None).await.unwrap();
    echo(&mut connection, "started").await;
    connection
        .writer
        .stop(Duration::from_secs(5))
        .await
        .unwrap();
}

async fn ensure_leaves_an_ended_session_alone(ctx: Ctx) {
    let holds = ctx.holds();
    let launch = ctx.launch(Framing::JsonRpc);
    let (session, mut connection) = ctx.start_with(holds.clone(), launch.clone()).await;
    connection
        .writer
        .stop(Duration::from_secs(5))
        .await
        .unwrap();
    let (_, exit) = until_exit(&mut connection.events).await;
    let dir = ctx.session_dir(session);
    eventually("the holder leaving", || live_holder(&dir).is_none()).await;
    assert_eq!(holds.find(session).await.unwrap(), Found::Ended);
    let ensured = holds.ensure(&launch).await.unwrap();
    assert_eq!(ensured, Ensured::Existing(Found::Ended));
    // Nothing new started, and the exit is still there to read.
    assert_eq!(live_holder(&dir), None);
    let again = holds.connect(session, None).await.unwrap();
    assert_eq!(again.hello.child, ChildState::Exited(exit));
}

// The holder's key.

type RawRead = tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>;
type RawWrite = tokio::net::unix::OwnedWriteHalf;

/// The socket the holder in `dir` listens on.
fn socket_of(dir: &Path) -> PathBuf {
    let holder = read_json(&dir.join("holder.json"));
    PathBuf::from(holder["socket"].as_str().unwrap())
}

/// Sends `message` as one line, as a client would.
async fn send_raw(write: &mut RawWrite, message: &Value) -> io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut line = message.to_string();
    line.push('\n');
    write.write_all(line.as_bytes()).await
}

/// Connects to `socket` by hand, says `hello` and returns the answer.
async fn say_hello(socket: &Path, hello: &Value) -> (Value, RawRead, RawWrite) {
    use tokio::io::AsyncBufReadExt;
    let stream = tokio::net::UnixStream::connect(socket).await.unwrap();
    let (read, mut write) = stream.into_split();
    let mut read = tokio::io::BufReader::new(read);
    send_raw(&mut write, hello).await.unwrap();
    let mut answer = String::new();
    tokio::time::timeout(WAIT, read.read_line(&mut answer))
        .await
        .expect("the holder didn't answer the hello")
        .unwrap();
    (serde_json::from_str(&answer).unwrap(), read, write)
}

/// Whether the holder closes the connection without saying anything more.
async fn hung_up(read: &mut RawRead) -> bool {
    use tokio::io::AsyncBufReadExt;
    let mut rest = String::new();
    matches!(
        tokio::time::timeout(WAIT, read.read_line(&mut rest)).await,
        Ok(Ok(0) | Err(_))
    )
}

fn hello_for(session: SessionId, key: Option<&str>) -> Value {
    let mut hello = json!({"type": "hello", "session": session, "after": null});
    if let Some(key) = key {
        hello["key"] = json!(key);
    }
    hello
}

/// What the system shows of process `pid`'s arguments and environment.
fn args_and_env(pid: u32) -> Vec<u8> {
    if cfg!(target_os = "linux") {
        let mut seen = fs::read(format!("/proc/{pid}/cmdline")).unwrap();
        seen.extend(fs::read(format!("/proc/{pid}/environ")).unwrap());
        seen
    } else {
        // `ps -E` shows the environment after the arguments.
        let pid = pid.to_string();
        let out = Command::new("ps")
            .args(["-wwE", "-o", "command=", "-p", &pid])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success(), "ps failed for {pid}");
        out.stdout
    }
}

async fn a_holder_refuses_a_client_without_its_key(ctx: Ctx) {
    let (session, mut connection) = ctx.start(Framing::ClaudeStreamJson).await;
    let dir = ctx.session_dir(session);
    let key_path = dir.join("holder.key");
    let key = fs::read_to_string(&key_path).unwrap();
    assert_eq!(key.len(), 64, "{} isn't a key", key_path.display());
    let mode = fs::metadata(&key_path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    let socket = socket_of(&dir);
    let wrong = "0".repeat(64);
    let tries = [
        (None, "show its key"),
        (Some(wrong.as_str()), "isn't this holder's"),
        (Some(&key[..63]), "isn't this holder's"),
        (Some(""), "isn't this holder's"),
    ];
    for (shown, why) in tries {
        let (answer, mut read, mut write) = say_hello(&socket, &hello_for(session, shown)).await;
        assert_eq!(answer["type"], "refused", "{answer}");
        let message = answer["message"].as_str().unwrap();
        assert!(message.contains(why), "{message}");
        assert!(!message.contains(&key[..16]), "{message}");
        // A write after the refusal goes nowhere, and the holder hangs up.
        let sneak = json!({"type": "write", "id": 1, "line": print("sneaked in"), "answers": null});
        let _ = send_raw(&mut write, &sneak).await;
        assert!(hung_up(&mut read).await, "the holder kept a refused client");
    }
    // The key with another session is still turned away, as before keys.
    let other = SessionId::generate();
    let (answer, _, _) = say_hello(&socket, &hello_for(other, Some(&key))).await;
    assert_eq!(answer["type"], "refused", "{answer}");
    assert!(answer["message"].as_str().unwrap().contains("runs session"));
    // The right key gets in.
    let (answer, _, _) = say_hello(&socket, &hello_for(session, Some(&key))).await;
    assert_eq!(answer["type"], "hello", "{answer}");
    assert_eq!(answer["holder_pid"], connection.hello.holder_pid);
    echo(&mut connection, "after the refusals").await;
    let events = fs::read(dir.join("events.log")).unwrap();
    assert!(
        !holds_bytes(&events, b"sneaked in"),
        "a refused write reached the agent"
    );
    // The crate's own client turns a refusal into an error after its wait.
    fs::write(&key_path, &wrong).unwrap();
    let err = ctx.holds().connect(session, None).await.unwrap_err();
    assert!(matches!(err, ConnectError::Refused { .. }), "{err:?}");
    assert!(err.to_string().contains("isn't this holder's"), "{err}");
    fs::write(&key_path, &key).unwrap();
    connection
        .writer
        .stop(Duration::from_secs(5))
        .await
        .unwrap();
}

async fn a_client_started_later_finds_the_key(ctx: Ctx) {
    // The holder starts in another process, which then dies, as maxd does
    // in a restart. This process knows nothing of the key but its file.
    let session = SessionId::generate();
    let mut spawner = start_spawner(&ctx, session, false).await;
    spawner.kill().unwrap();
    spawner.wait().unwrap();
    assert!(ctx.session_dir(session).join("holder.key").exists());
    let holds = ctx.holds();
    let found = holds.find(session).await.unwrap();
    assert!(matches!(found, Found::Live { .. }), "{found:?}");
    let launch = fake_launch(session, Framing::ClaudeStreamJson, &ctx.root);
    assert_eq!(
        holds.ensure(&launch).await.unwrap(),
        Ensured::Existing(found)
    );
    let mut connection = holds.connect(session, None).await.unwrap();
    echo(&mut connection, "after the restart").await;
    connection
        .writer
        .stop(Duration::from_secs(5))
        .await
        .unwrap();
    let (_, exit) = until_exit(&mut connection.events).await;
    assert_eq!(exit.code, Some(0));
}

async fn a_holder_from_before_keys_takes_any_client(ctx: Ctx) {
    // A holder started the old way: `launch.json` without `keyed`, no
    // `holder.key` and nothing on stdin. The file comes from a new spawn,
    // so it's otherwise the same.
    let (template, mut first) = ctx.start(Framing::ClaudeStreamJson).await;
    let mut launched = read_json(&ctx.session_dir(template).join("launch.json"));
    assert_eq!(launched["keyed"], true);
    let session = SessionId::generate();
    let launched = launched.as_object_mut().unwrap();
    launched.remove("keyed");
    launched.insert("session".into(), json!(session));
    let dir = ctx.session_dir(session);
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        dir.join("launch.json"),
        Value::from(launched.clone()).to_string(),
    )
    .unwrap();
    let mut holder = Command::new(exe())
        .arg("__hold")
        .arg(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = holder.stdout.take().unwrap();
    let said = tokio::task::spawn_blocking(move || {
        let mut line = String::new();
        BufReader::new(stdout).read_line(&mut line).map(|_| line)
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(said.trim(), "ready");
    holder.wait().unwrap();
    assert!(!dir.join("holder.key").exists());

    // The client sends the hello it sent before keys, and any key is let
    // through, since this holder has none to check.
    let holds = ctx.holds();
    let mut connection = holds.connect(session, None).await.unwrap();
    echo(&mut connection, "no key needed").await;
    assert!(matches!(
        holds.find(session).await.unwrap(),
        Found::Live { .. }
    ));
    let socket = socket_of(&dir);
    for shown in [None, Some("0".repeat(64))] {
        let (answer, _, _) = say_hello(&socket, &hello_for(session, shown.as_deref())).await;
        assert_eq!(answer["type"], "hello", "{answer}");
    }
    for writer in [&connection.writer, &first.writer] {
        writer.stop(Duration::from_secs(5)).await.unwrap();
    }
    let (_, exit) = until_exit(&mut connection.events).await;
    assert_eq!(exit.code, Some(0));
    until_exit(&mut first.events).await;
}

async fn the_key_shows_in_no_other_file_environment_or_arguments(ctx: Ctx) {
    // With a secret variable too, both share the pipe.
    let (name, secret) = test_secret();
    let value = secret.expose().to_owned();
    let mut launch = ctx.launch(Framing::ClaudeStreamJson);
    launch.secret_env.push((name, secret));
    let (session, mut connection) = ctx.start_with(ctx.holds(), launch).await;
    let ask = json!({"fake": "secret", "name": "MAXHQ_TEST_SECRET"}).to_string();
    write(&connection, &ask).await;
    let said = parse(&next_line(&mut connection.events).await);
    assert_eq!(said["hash"], fnv(value.as_bytes()), "{said}");
    let dir = ctx.session_dir(session);
    let key = fs::read_to_string(dir.join("holder.key")).unwrap();
    let ChildState::Running { pid: child } = connection.hello.child else {
        panic!("the agent ended: {:?}", connection.hello.child);
    };
    for (who, pid, marker) in [
        ("holder", connection.hello.holder_pid, &b"__hold"[..]),
        ("agent", child, &b"FAKE_AGENT=1"[..]),
    ] {
        let seen = args_and_env(pid);
        // The markers show the reading worked.
        assert!(
            holds_bytes(&seen, marker),
            "couldn't read the {who}'s arguments"
        );
        assert!(
            holds_bytes(&seen, b"PATH="),
            "couldn't read the {who}'s environment"
        );
        assert!(
            !holds_bytes(&seen, key.as_bytes()),
            "the {who}'s arguments or environment have the key"
        );
    }
    connection
        .writer
        .stop(Duration::from_secs(5))
        .await
        .unwrap();
    let (_, exit) = until_exit(&mut connection.events).await;
    assert_eq!(exit.code, Some(0));
    eventually("the holder leaving", || live_holder(&dir).is_none()).await;
    assert!(dir.join("holder.log").exists());
    for (path, bytes) in every_file(&ctx.root) {
        if path == dir.join("holder.key") {
            continue;
        }
        assert!(
            !holds_bytes(&bytes, key.as_bytes()),
            "{} has the key",
            path.display()
        );
    }
}

// The ignored tests, and what they need.

/// Runs a command the test needs and returns what it printed, failing the
/// test when the command fails.
fn run(program: &str, args: &[&str]) -> String {
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|err| panic!("couldn't run {program}: {err}"));
    assert!(
        out.status.success(),
        "{program} {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn require_root() {
    assert!(
        rustix::process::geteuid().is_root(),
        "this test has to run as root"
    );
}

/// Fails the test before it looks at anything, unless systemd runs here.
fn require_systemd() {
    assert!(
        Path::new("/run/systemd/system").is_dir(),
        "this test needs Linux running systemd"
    );
}

/// A unit's load state and active state, as `systemctl show` gives them.
fn unit_state(user: bool, unit: &str) -> (String, String) {
    let mut args = vec!["show", "--property=LoadState", "--property=ActiveState"];
    if user {
        args.insert(0, "--user");
    }
    args.extend(["--", unit]);
    let shown = run("systemctl", &args);
    let field = |key: &str| {
        shown
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .unwrap_or_default()
            .to_owned()
    };
    (field("LoadState="), field("ActiveState="))
}

/// The parent of process `pid`, from `/proc`.
fn parent_of(pid: i32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, after) = stat.rsplit_once(')')?;
    after.split_whitespace().nth(1)?.parse().ok()
}

/// A process the test started, killed when dropped.
struct Started(std::process::Child);

impl Drop for Started {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A transient service that plays the guest service, running
/// `__spawner --ensure`. Dropping it stops it.
struct Service {
    user: bool,
    unit: String,
}

impl Service {
    fn start(ctx: &Ctx, user: bool, detach: &str, session: SessionId, report: &Path) -> Self {
        let id = SessionId::generate().0.simple().to_string();
        let unit = format!("maxhq-test-guest-{}.service", &id[20..]);
        let mut command = Command::new("systemd-run");
        if user {
            command.arg("--user");
        }
        command
            .arg(format!("--unit={unit}"))
            .args(["--collect", "--quiet"])
            .arg(format!("--setenv={DETACH_ENV}={detach}"))
            .arg("--")
            .arg(exe())
            .arg("__spawner")
            .arg(&ctx.dirs.sessions)
            .arg(&ctx.dirs.sockets)
            .arg(session.to_string())
            .arg(&ctx.root)
            .arg("--ensure")
            .arg(report)
            .stdin(Stdio::null());
        assert!(command.status().unwrap().success(), "systemd-run failed");
        Self { user, unit }
    }

    fn systemctl(&self, verb: &str) -> bool {
        let mut command = Command::new("systemctl");
        if self.user {
            command.arg("--user");
        }
        command
            .args([verb, "--", &self.unit])
            .stdin(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.systemctl("stop");
    }
}

async fn root_run_as_drops_the_cli_to_the_user(ctx: Ctx) {
    require_root();
    let user = std::env::var("MAXHQ_TEST_USER").unwrap_or_else(|_| "nobody".into());
    let run_as = RunAs::lookup(&user).unwrap();
    // The user has to reach the CLI and the folder it starts in, which a
    // home folder may not let it, so both go in a folder of its own.
    fs::set_permissions(&ctx.root, fs::Permissions::from_mode(0o755)).unwrap();
    let work = ctx.root.join("work");
    fs::create_dir(&work).unwrap();
    std::os::unix::fs::chown(&work, Some(run_as.uid), Some(run_as.gid)).unwrap();
    let program = work.join("fake-agent");
    fs::copy(exe(), &program).unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    let mut launch = ctx.launch(Framing::ClaudeStreamJson);
    launch.program = program;
    launch.cwd = work.clone();
    launch.run_as = Some(run_as.clone());
    let holds = ctx.holds();
    holds.spawn(&launch).await.unwrap_or_else(|err| {
        panic!(
            "{err}, and {user} has to be able to reach {}",
            work.display()
        )
    });
    let mut connection = holds.connect(launch.session, None).await.unwrap();
    let holder = connection.hello.holder_pid;
    write(&connection, &json!({"fake": "whoami"}).to_string()).await;
    let me = parse(&next_line(&mut connection.events).await);
    assert_eq!(me["type"], "whoami", "{me}");
    assert_eq!(
        (me["uid"].clone(), me["gid"].clone()),
        (json!(run_as.uid), json!(run_as.gid))
    );
    let groups = group_set(&run("id", &["-G", &user]));
    assert_eq!(me["groups"], json!(groups), "{me}");
    assert_eq!(me["home"], json!(run_as.home), "{me}");
    assert_eq!((&me["user"], &me["logname"]), (&json!(user), &json!(user)));
    let shell = me["shell"].as_str().unwrap_or_default();
    assert!(shell.starts_with('/'), "{me}");
    assert_eq!(me["cwd"], json!(fs::canonicalize(&work).unwrap()), "{me}");
    // The CLI leads a process group of its own, under the holder.
    assert_eq!(me["pgid"], me["pid"], "{me}");
    assert_eq!(me["ppid"], holder, "{me}");
    if cfg!(target_os = "linux") {
        // A process the CLI leaves behind goes to the holder, which reaps
        // it rather than init.
        write(&connection, &json!({"fake": "orphan"}).to_string()).await;
        let orphan = parse(&next_line(&mut connection.events).await)["pid"]
            .as_i64()
            .and_then(|pid| i32::try_from(pid).ok())
            .filter(|pid| *pid > 0)
            .expect("the orphan's process ID");
        let exit = json!({"fake": "exit", "code": 0}).to_string();
        write(&connection, &exit).await;
        eventually("the orphan going to the holder", || {
            parent_of(orphan) == Some(holder)
        })
        .await;
        let _ = rustix::process::kill_process(Pid::from_raw(orphan).unwrap(), Signal::KILL);
    } else {
        connection
            .writer
            .stop(Duration::from_secs(5))
            .await
            .unwrap();
    }
    let (_, exit) = until_exit(&mut connection.events).await;
    assert_eq!(exit.code, Some(0));

    // The user enters its folder after the fork, and one that's gone is
    // still named rather than the program.
    let mut gone = ctx.launch(Framing::ClaudeStreamJson);
    gone.program = launch.program;
    gone.cwd = ctx.root.join("wiped");
    gone.run_as = Some(run_as);
    match holds.spawn(&gone).await {
        Err(SpawnError::Holder { message }) => assert_eq!(
            message,
            format!("the folder {} is gone", gone.cwd.display())
        ),
        other => panic!("expected the holder to fail, got {other:?}"),
    }
}

async fn root_system_scope_holds_the_holder(ctx: Ctx) {
    require_systemd();
    require_root();
    let holds = ctx.holds_detached(Detach::SystemScope);
    let launch = ctx.launch(Framing::ClaudeStreamJson);
    let (session, mut connection) = ctx.start_with(holds, launch).await;
    let unit = unit_name(session);
    assert_eq!(
        unit_state(false, &unit),
        ("loaded".to_owned(), "active".to_owned())
    );
    let holder = connection.hello.holder_pid;
    let cgroup = fs::read_to_string(format!("/proc/{holder}/cgroup")).unwrap();
    assert!(cgroup.contains(&unit), "{cgroup}");
    echo(&mut connection, "in a scope").await;
    connection
        .writer
        .stop(Duration::from_secs(5))
        .await
        .unwrap();
    until_exit(&mut connection.events).await;
    // Once the holder goes, the scope is empty and systemd drops it.
    eventually("the scope going", || {
        unit_state(false, &unit).0 == "not-found"
    })
    .await;
}

async fn root_system_scope_outlives_a_restarted_service(ctx: Ctx) {
    require_systemd();
    require_root();
    outlives_a_restarted_service(ctx, false).await;
}

async fn user_scope_outlives_a_restarted_service(ctx: Ctx) {
    require_systemd();
    outlives_a_restarted_service(ctx, true).await;
}

/// Starts a holder from a service, restarts the service and checks the
/// restarted one finds the same holder rather than starting another.
async fn outlives_a_restarted_service(ctx: Ctx, user: bool) {
    let session = SessionId::generate();
    let report = ctx.root.join("report");
    let reported = || -> Vec<String> {
        let text = fs::read_to_string(&report).unwrap_or_default();
        text.lines().map(str::to_owned).collect()
    };
    let detach = if user { "user" } else { "system" };
    let service = Service::start(&ctx, user, detach, session, &report);
    eventually("the service spawning", || !reported().is_empty()).await;
    assert_eq!(reported(), ["spawned"]);
    let holds = ctx.holds();
    let Found::Live { holder_pid } = holds.find(session).await.unwrap() else {
        panic!("no live holder");
    };
    let unit = unit_name(session);
    assert_eq!(unit_state(user, &unit).1, "active");
    assert!(service.systemctl("restart"), "the restart failed");
    eventually("the restarted service looking", || reported().len() > 1).await;
    assert_eq!(
        reported(),
        ["spawned".to_owned(), format!("live {holder_pid}")]
    );
    drop(service);
    let mut connection = holds.connect(session, None).await.unwrap();
    assert_eq!(connection.hello.holder_pid, holder_pid);
    echo(&mut connection, "still here").await;
    connection
        .writer
        .stop(Duration::from_secs(5))
        .await
        .unwrap();
    until_exit(&mut connection.events).await;
}

async fn root_a_stale_scope_is_cleared_before_a_spawn(ctx: Ctx) {
    require_systemd();
    require_root();
    let launch = ctx.launch(Framing::ClaudeStreamJson);
    let unit = unit_name(launch.session);
    // A scope with the holder's name and no holder in it, as a process the
    // CLI left behind would keep.
    let base = unit.trim_end_matches(".scope");
    let mut stale = Started(
        Command::new("systemd-run")
            .args(["--scope", "--collect", "--quiet"])
            .arg(format!("--unit={base}"))
            .args(["sleep", "600"])
            .stdin(Stdio::null())
            .spawn()
            .unwrap(),
    );
    eventually("the stale scope starting", || {
        unit_state(false, &unit).1 == "active"
    })
    .await;
    let holds = ctx.holds_detached(Detach::SystemScope);
    holds.spawn(&launch).await.unwrap();
    // Stopping the stale scope ended its sleep.
    eventually("the stale sleep ending", || {
        stale.0.try_wait().is_ok_and(|status| status.is_some())
    })
    .await;
    let mut connection = holds.connect(launch.session, None).await.unwrap();
    let holder = connection.hello.holder_pid;
    let cgroup = fs::read_to_string(format!("/proc/{holder}/cgroup")).unwrap();
    assert!(cgroup.contains(&unit), "{cgroup}");
    echo(&mut connection, "in a new scope").await;
    connection
        .writer
        .stop(Duration::from_secs(5))
        .await
        .unwrap();
    until_exit(&mut connection.events).await;
}

/// The holders running under `sessions`, by folder, read from their files
/// alone.
fn live_holders(sessions: &Path) -> Vec<(PathBuf, i32)> {
    let Ok(entries) = fs::read_dir(sessions) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| Some((entry.path(), live_holder(&entry.path())?)))
        .collect()
}

async fn host_holders_outlive_a_maxd_restart(_ctx: Ctx) {
    require_systemd();
    // It reads the real holders' files and never connects to one or
    // signals it.
    let dirs = Dirs::from_env().expect("maxd's folders");
    let before = live_holders(&dirs.sessions);
    assert!(!before.is_empty(), "start a session under maxd first");
    run("systemctl", &["--user", "restart", "maxd.service"]);
    eventually("maxd running again", || {
        unit_state(true, "maxd.service").1 == "active"
    })
    .await;
    let after = live_holders(&dirs.sessions);
    for holder in &before {
        assert!(after.contains(holder), "{holder:?} ended with maxd");
    }
}

// The fake CLI.

/// Echoes each line as `{"type":"echo","got":...}`, except for commands:
/// `print` prints a line as given, `stderr` writes to stderr, `exit` exits
/// with a code and `flood` prints many lines. `secret` prints the length
/// and a hash of a variable's value, never the value. `whoami` prints the
/// user, groups, variables and folder it runs with, and `orphan` starts a
/// `sleep` and prints its process ID. It ends at the end of its input,
/// unless `--stubborn`, which also ignores SIGTERM.
fn fake_child(args: &[OsString]) -> ExitCode {
    let stubborn = args.iter().any(|arg| arg == "--stubborn");
    thread::spawn(|| {
        thread::sleep(CHILD_LIFETIME);
        std::process::exit(70);
    });
    // Once tokio handles SIGTERM, it no longer ends the process.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _term = stubborn.then(|| {
        let _entered = runtime.enter();
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap()
    });
    let mut out = io::stdout().lock();
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else {
            break;
        };
        let command: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
        let written = match command["fake"].as_str() {
            Some("print") => writeln!(out, "{}", command["line"].as_str().unwrap_or_default()),
            Some("stderr") => {
                eprint!("{}", command["text"].as_str().unwrap_or_default());
                Ok(())
            }
            Some("exit") => {
                let _ = out.flush();
                let code = command["code"].as_i64().unwrap_or(0);
                std::process::exit(i32::try_from(code).unwrap_or(1));
            }
            Some("flood") => {
                let size = usize::try_from(command["size"].as_u64().unwrap_or(10)).unwrap();
                let pad = "x".repeat(size);
                (0..command["count"].as_u64().unwrap_or(0)).try_for_each(|n| {
                    writeln!(out, "{}", json!({"type": "flood", "n": n, "pad": pad}))
                })
            }
            Some("secret") => {
                let name = command["name"].as_str().unwrap_or_default();
                let value = std::env::var_os(name).unwrap_or_default();
                let said =
                    json!({"type": "secret", "len": value.len(), "hash": fnv(value.as_bytes())});
                writeln!(out, "{said}")
            }
            Some("whoami") => writeln!(out, "{}", whoami()),
            Some("orphan") => {
                let orphan = Command::new("sleep")
                    .arg("30")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
                let pid = orphan.map(|orphan| orphan.id()).unwrap_or_default();
                writeln!(out, "{}", json!({"type": "orphan", "pid": pid}))
            }
            _ => writeln!(out, "{}", json!({"type": "echo", "got": line})),
        };
        if written.and_then(|()| out.flush()).is_err() {
            return ExitCode::FAILURE;
        }
    }
    if stubborn {
        loop {
            thread::sleep(Duration::from_secs(1));
        }
    }
    ExitCode::SUCCESS
}

/// A 64-bit FNV-1a hash, so a test can match a secret without the fake
/// printing it.
fn fnv(bytes: &[u8]) -> String {
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{hash:016x}")
}

/// Who the fake runs as, for `whoami`.
fn whoami() -> Value {
    let var = |name: &str| std::env::var(name).unwrap_or_default();
    let groups = Command::new("id")
        .arg("-G")
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default();
    json!({
        "type": "whoami",
        "uid": rustix::process::getuid().as_raw(),
        "gid": rustix::process::getgid().as_raw(),
        "groups": group_set(&groups),
        "home": var("HOME"),
        "user": var("USER"),
        "logname": var("LOGNAME"),
        "shell": var("SHELL"),
        "cwd": std::env::current_dir().unwrap_or_default(),
        "pid": std::process::id(),
        "pgid": rustix::process::getpgrp().as_raw_nonzero().get(),
        "ppid": rustix::process::getppid().map_or(0, |ppid| ppid.as_raw_nonzero().get()),
    })
}

/// The group IDs in what `id -G` printed, sorted.
fn group_set(printed: &str) -> Vec<u32> {
    let mut groups: Vec<u32> = printed
        .split_whitespace()
        .filter_map(|group| group.parse().ok())
        .collect();
    groups.sort_unstable();
    groups.dedup();
    groups
}

// The spawner.

/// `__spawner <sessions> <sockets> <session> <cwd>`: starts a holder for a
/// fake CLI, says `spawned` and waits to be killed.
///
/// With `--ensure <report>` after those, it plays a service that restarts.
/// It ensures the session has a holder and adds what it found to the end
/// of `report`, as `spawned` or `live <holder pid>`.
fn spawner(args: &[OsString]) -> ExitCode {
    let (args, report) = match args {
        [args @ .., flag, report] if flag == "--ensure" => (args, Some(Path::new(report))),
        args => (args, None),
    };
    if let [flag, sessions, sockets, terminal, cwd] = args
        && flag == "--terminal"
    {
        let terminal = terminal.to_str().unwrap();
        return terminal::spawner(sessions.as_ref(), sockets.as_ref(), terminal, cwd.as_ref());
    }
    let [sessions, sockets, session, cwd] = args else {
        eprintln!("usage: __spawner <sessions> <sockets> <session> <cwd> [--ensure <report>]");
        return ExitCode::from(2);
    };
    let session: SessionId = session.to_str().unwrap().parse().unwrap();
    let holds = Holds::new(Dirs {
        sessions: sessions.into(),
        sockets: sockets.into(),
    })
    .with_holder(exe(), vec!["__hold".into()])
    .with_limits(test_limits());
    let launch = fake_launch(session, Framing::ClaudeStreamJson, Path::new(cwd));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    // Detach::Auto here, so MAXHQ_HOLD_DETACH picks the way.
    if let Some(report) = report {
        let said = match runtime.block_on(holds.ensure(&launch)) {
            Ok(Ensured::Spawned) => "spawned".to_owned(),
            Ok(Ensured::Existing(Found::Live { holder_pid })) => format!("live {holder_pid}"),
            Ok(Ensured::Existing(found)) => format!("found {found:?}"),
            Err(err) => format!("failed {err}"),
        };
        let written = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(report)
            .and_then(|mut file| writeln!(file, "{said}"));
        if written.is_err() {
            return ExitCode::FAILURE;
        }
        thread::sleep(Duration::from_secs(60));
        return ExitCode::SUCCESS;
    }
    if let Err(err) = runtime.block_on(holds.spawn(&launch)) {
        eprintln!("spawn failed: {err}");
        return ExitCode::FAILURE;
    }
    println!("spawned");
    let _ = io::stdout().flush();
    thread::sleep(Duration::from_secs(60));
    ExitCode::SUCCESS
}
