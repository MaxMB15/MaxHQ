// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What the tests share on every OS: the runner, each test's folders, the
//! fake CLI, and the tests whose holders act alike everywhere.

use std::collections::{HashSet, VecDeque};
use std::ffi::OsString;
use std::fs::{self, File, TryLockError};
use std::future::Future;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use maxhq_hold::{
    ChildState, ConnectError, Connection, Detach, Dirs, Ended, Ensured, Event, Events, Exit, Found,
    Framing, Holds, Launch, Limits, Line, Secret, SpawnError, WriteError, WriteOutcome,
};
use maxhq_protocol::ids::SessionId;
use serde_json::{json, Value};

/// How long any one step may take before the test fails.
pub(crate) const WAIT: Duration = Duration::from_secs(15);

/// How long a whole test may take.
const TEST_WAIT: Duration = Duration::from_secs(90);

/// The fake CLI ends itself after this, so nothing outlives the tests for
/// long.
const CHILD_LIFETIME: Duration = Duration::from_secs(120);

/// How many tests run at once.
const PARALLEL: usize = 6;

pub(crate) type TestFn = fn(Ctx) -> Pin<Box<dyn Future<Output = ()> + Send>>;

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

// The test runner.

/// Runs the `tests` the arguments pick, and the `ignored` ones when they
/// ask for them, as libtest would.
pub(crate) fn run_tests(
    args: &[OsString],
    tests: &'static [(&'static str, TestFn)],
    ignored: &'static [(&'static str, &'static str, TestFn)],
) -> ExitCode {
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
    let plain = tests.iter().map(|&(name, test)| (name, None, test));
    let ignored = ignored
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
pub(crate) struct Ctx {
    pub(crate) root: PathBuf,
    pub(crate) dirs: Dirs,
}

impl Ctx {
    pub(crate) fn new() -> Self {
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

    pub(crate) fn holds(&self) -> Holds {
        self.holds_with(test_limits())
    }

    pub(crate) fn holds_with(&self, limits: Limits) -> Holds {
        self.holds_detached(Detach::Setsid).with_limits(limits)
    }

    pub(crate) fn holds_detached(&self, detach: Detach) -> Holds {
        Holds::new(self.dirs.clone())
            .with_holder(exe(), vec!["__hold".into()])
            .with_detach(detach)
            .with_limits(test_limits())
    }

    pub(crate) fn launch(&self, framing: Framing) -> Launch {
        fake_launch(SessionId::generate(), framing, &self.root)
    }

    pub(crate) fn session_dir(&self, session: SessionId) -> PathBuf {
        self.dirs.sessions.join(session.to_string())
    }

    /// Starts a fake CLI and connects to its holder.
    pub(crate) async fn start(&self, framing: Framing) -> (SessionId, Connection) {
        self.start_with(self.holds(), self.launch(framing)).await
    }

    pub(crate) async fn start_with(&self, holds: Holds, launch: Launch) -> (SessionId, Connection) {
        holds.spawn(&launch).await.unwrap();
        let connection = holds.connect(launch.session, None).await.unwrap();
        (launch.session, connection)
    }
}

impl Drop for Ctx {
    fn drop(&mut self) {
        // A holder counts as this test's only while it holds the lock in
        // this test's own folder, so nothing else is ever signalled.
        let running = || -> Vec<(PathBuf, i32)> {
            let Ok(entries) = fs::read_dir(&self.dirs.sessions) else {
                return Vec::new();
            };
            entries
                .filter_map(Result::ok)
                .filter_map(|entry| Some((entry.path(), live_holder(&entry.path())?)))
                .collect()
        };
        for (dir, pid) in running() {
            crate::imp::stop_holder(&dir, pid);
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
pub(crate) fn live_holder(dir: &Path) -> Option<i32> {
    let lock = File::open(dir.join("holder.lock")).ok()?;
    if !matches!(lock.try_lock(), Err(TryLockError::WouldBlock)) {
        return None;
    }
    let holder: Value = serde_json::from_slice(&fs::read(dir.join("holder.json")).ok()?).ok()?;
    i32::try_from(holder["holder_pid"].as_u64()?).ok()
}

pub(crate) fn exe() -> PathBuf {
    std::env::current_exe().unwrap()
}

pub(crate) fn test_limits() -> Limits {
    Limits {
        linger: Duration::from_secs(3),
        ..Limits::default()
    }
}

pub(crate) fn fake_launch(session: SessionId, framing: Framing, cwd: &Path) -> Launch {
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

pub(crate) fn print(line: &str) -> String {
    json!({"fake": "print", "line": line}).to_string()
}

pub(crate) fn flood(count: u64, size: usize) -> String {
    json!({"fake": "flood", "count": count, "size": size}).to_string()
}

pub(crate) fn parse(line: &Line) -> Value {
    serde_json::from_str(&line.text).unwrap_or(Value::Null)
}

/// What the fake CLI got, when `line` is its echo.
pub(crate) fn echoed(line: &Line) -> Option<String> {
    let value = parse(line);
    (value["type"] == "echo").then(|| value["got"].as_str().unwrap_or_default().to_owned())
}

pub(crate) async fn next_line(events: &mut Events) -> Line {
    match tokio::time::timeout(WAIT, events.recv()).await {
        Ok(Some(Event::Line(line))) => line,
        Ok(Some(Event::Exited(exit))) => panic!("the agent ended: {exit:?}"),
        Ok(None) => panic!("the events ended"),
        Err(_) => panic!("no line within {WAIT:?}"),
    }
}

pub(crate) async fn lines(events: &mut Events, count: usize) -> Vec<Line> {
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

pub(crate) async fn until_exit(events: &mut Events) -> (Vec<Line>, Exit) {
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

pub(crate) async fn write(connection: &Connection, line: &str) -> WriteOutcome {
    connection
        .writer
        .write(line.to_owned(), None)
        .await
        .unwrap()
}

/// Writes `line` and returns the echo.
pub(crate) async fn echo(connection: &mut Connection, line: &str) -> Line {
    assert_eq!(write(connection, line).await, WriteOutcome::Written);
    let echo = next_line(&mut connection.events).await;
    assert_eq!(echoed(&echo).as_deref(), Some(line), "{echo:?}");
    echo
}

pub(crate) fn assert_contiguous(lines: &[Line], from: u64) {
    for (offset, line) in lines.iter().enumerate() {
        assert_eq!(line.seq, from + offset as u64, "a gap before {line:?}");
    }
}

pub(crate) fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

pub(crate) async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !check() {
        assert!(Instant::now() < deadline, "{what} didn't happen in time");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub(crate) fn claude_prompt(id: &str) -> String {
    json!({
        "type": "control_request",
        "request_id": id,
        "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "ls"}},
    })
    .to_string()
}

pub(crate) fn claude_answer(id: &str) -> String {
    json!({
        "type": "control_response",
        "response": {"subtype": "success", "request_id": id, "response": {"behavior": "allow"}},
    })
    .to_string()
}

pub(crate) async fn lines_are_echoed_numbered_and_recorded(ctx: Ctx) {
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

pub(crate) async fn a_reconnect_replays_without_a_gap(ctx: Ctx) {
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

pub(crate) async fn old_lines_come_from_disk_then_live(ctx: Ctx) {
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

pub(crate) async fn claude_prompts_survive_a_reconnect(ctx: Ctx) {
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

pub(crate) async fn json_rpc_prompts_survive_a_reconnect(ctx: Ctx) {
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

pub(crate) async fn the_first_answer_wins(ctx: Ctx) {
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

pub(crate) async fn a_withdrawn_prompt_is_no_longer_pending(ctx: Ctx) {
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

pub(crate) async fn the_log_rotates_and_keeps_one_old_file(ctx: Ctx) {
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

pub(crate) async fn an_exit_is_recorded_with_its_stderr(ctx: Ctx) {
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

pub(crate) async fn stop_closes_stdin_first(ctx: Ctx) {
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

pub(crate) async fn a_killed_holder_ends_with_an_unknown_exit(ctx: Ctx) {
    let holds = ctx.holds();
    let (session, mut connection) = ctx.start(Framing::ClaudeStreamJson).await;
    echo(&mut connection, "before").await;
    let dir = ctx.session_dir(session);
    let holder = live_holder(&dir).unwrap();
    assert_eq!(holder.unsigned_abs(), connection.hello.holder_pid);
    crate::imp::kill(holder.unsigned_abs());
    let (_, exit) = until_exit(&mut connection.events).await;
    assert_eq!((exit.code, exit.signal), (None, None));
    let again = holds.connect(session, None).await.unwrap();
    assert_eq!(again.hello.child, ChildState::Exited(exit));
    assert_eq!(again.hello.last_seq, 1);
}

pub(crate) async fn a_running_session_cant_be_forgotten(ctx: Ctx) {
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

pub(crate) async fn a_session_starts_once(ctx: Ctx) {
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

pub(crate) async fn a_missing_program_fails_the_spawn(ctx: Ctx) {
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

pub(crate) async fn a_missing_folder_fails_the_spawn_by_its_name(ctx: Ctx) {
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

pub(crate) async fn long_lines_from_two_writers_stay_whole(ctx: Ctx) {
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

pub(crate) async fn end_leaves_a_holder_it_cant_confirm(ctx: Ctx) {
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

// Secrets and finding a holder again.

/// A variable whose value no file may ever hold. The value is new each
/// run, so a match can't come from an earlier one.
pub(crate) fn test_secret() -> (OsString, Secret) {
    let id = SessionId::generate().0.simple().to_string();
    let value = format!("test-not-a-real-token-{id}");
    ("MAXHQ_TEST_SECRET".into(), Secret::new(value))
}

/// Every file under `dir`, read whole.
pub(crate) fn every_file(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
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

pub(crate) fn holds_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

pub(crate) fn assert_no_file_has(dir: &Path, value: &str) {
    for (path, bytes) in every_file(dir) {
        assert!(
            !holds_bytes(&bytes, value.as_bytes()),
            "{} has the secret",
            path.display()
        );
    }
}

pub(crate) async fn a_secret_reaches_the_cli_and_no_file(ctx: Ctx) {
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

pub(crate) async fn a_failed_spawn_never_shows_a_secret(ctx: Ctx) {
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

pub(crate) async fn ensure_reaches_the_running_holder(ctx: Ctx) {
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

pub(crate) async fn ensure_leaves_an_ended_session_alone(ctx: Ctx) {
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

// The fake CLI.

/// Echoes each line as `{"type":"echo","got":...}`, except for commands:
/// `print` prints a line as given, `stderr` writes to stderr, `exit` exits
/// with a code and `flood` prints many lines. `secret` prints the length
/// and a hash of a variable's value, never the value. `whoami` prints the
/// user, groups, variables and folder it runs with, and `orphan` starts a
/// process that sleeps and prints its process ID. It ends at the end of
/// its input, unless `--stubborn`, which also ignores SIGTERM on Unix.
pub(crate) fn fake_child(args: &[OsString]) -> ExitCode {
    let stubborn = args.iter().any(|arg| arg == "--stubborn");
    thread::spawn(|| {
        thread::sleep(CHILD_LIFETIME);
        std::process::exit(70);
    });
    let _term = stubborn.then(crate::imp::ignore_term);
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
                let said = json!({"type": "secret", "len": value.len(), "hash": fnv(value.as_encoded_bytes())});
                writeln!(out, "{said}")
            }
            Some("whoami") => writeln!(out, "{}", crate::imp::whoami()),
            Some("orphan") => {
                let orphan = Command::new(exe())
                    .arg(SLEEP)
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

/// The part that sleeps for 30 seconds, as `sleep 30` would.
pub(crate) const SLEEP: &str = "__sleep";

/// What `__sleep` does.
pub(crate) fn sleep() -> ExitCode {
    thread::sleep(Duration::from_secs(30));
    ExitCode::SUCCESS
}

/// A 64-bit FNV-1a hash, so a test can match a secret without the fake
/// printing it.
pub(crate) fn fnv(bytes: &[u8]) -> String {
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{hash:016x}")
}

// The spawner.

/// `__spawner <sessions> <sockets> <session> <cwd>`: waits for a line or
/// the end of its stdin, starts a holder for a fake CLI, says `spawned`
/// and waits to be killed. The wait lets a test put it in a job first.
///
/// With `--ensure <report>` after those, it plays a service that restarts.
/// It ensures the session has a holder, says what it found and adds that
/// to the end of `report`, as `spawned` or `live <holder pid>`.
pub(crate) fn spawner(args: &[OsString]) -> ExitCode {
    let (args, report) = match args {
        [args @ .., flag, report] if flag == "--ensure" => (args, Some(Path::new(report))),
        args => (args, None),
    };
    let [sessions, sockets, session, cwd] = args else {
        eprintln!("usage: __spawner <sessions> <sockets> <session> <cwd> [--ensure <report>]");
        return ExitCode::from(2);
    };
    let session: SessionId = session.to_str().unwrap().parse().unwrap();
    let _ = io::stdin().lock().read_line(&mut String::new());
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
        println!("{said}");
        let _ = io::stdout().flush();
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
