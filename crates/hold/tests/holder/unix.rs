// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The tests on Unix: the shared ones, the ones that signal holders and
//! groups, keys over a raw socket, users, systemd scopes and terminals.
//!
//! The tests that need root, systemd or a running `maxd` are ignored. On
//! Linux, `cargo test -p maxhq-hold --test holder -- --ignored user_scope`
//! runs the user scope one, `-- --ignored host_` the `maxd` one, and the
//! `root_` ones run as root with a runner, such as
//! `CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER='sudo -E' cargo test -p
//! maxhq-hold --test holder -- --ignored root_`.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use maxhq_hold::{
    unit_name, ChildState, ConnectError, Detach, Dirs, Ended, Ensured, Event, Found, Framing,
    Launch, RunAs, SpawnError, WriteError, WriteOutcome, DETACH_ENV,
};
use maxhq_protocol::ids::SessionId;
use rustix::process::{Pid, Signal};
use serde_json::{json, Value};

use self::terminal::{
    a_goodbye_before_the_end_changes_nothing,
    a_password_prompt_is_flagged_and_its_answer_kept_nowhere,
    a_quiet_question_waits_and_offers_its_answers,
    a_raw_prompt_that_asks_for_a_secret_waits_as_a_password_prompt,
    a_reader_past_the_ring_starts_over_at_a_whole_line, a_run_keeps_what_it_showed,
    a_run_shows_nothing_its_shell_printed_before_the_command,
    a_run_streams_each_piece_as_it_prints, a_run_whose_startup_asks_for_something_shows_it,
    a_shell_mark_needs_its_nonce_and_may_come_in_pieces,
    a_terminal_holder_refuses_a_client_without_its_key, a_terminal_outlives_its_spawner,
    a_terminal_replays_from_an_offset_without_a_gap, a_watch_gets_all_but_the_output,
    an_ended_terminal_leaves_its_exit_when_nobody_says_goodbye,
    bash_on_its_own_keeps_what_its_commands_see, interrupt_stops_what_the_shell_runs,
    kill_escalates_to_sigkill, kill_hangs_up_the_whole_group,
    stop_command_ends_what_the_shell_runs_and_leaves_the_shell,
    the_holder_answers_where_the_cursor_is_with_nobody_looking,
    the_title_and_folder_come_from_the_shell, zsh_bash_and_fish_on_the_path_mark_their_commands,
};

use crate::common::*;

#[path = "terminal.rs"]
mod terminal;

pub(crate) const TESTS: &[(&str, TestFn)] = tests![
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
    stop_command_ends_what_the_shell_runs_and_leaves_the_shell,
    kill_hangs_up_the_whole_group,
    kill_escalates_to_sigkill,
    an_ended_terminal_leaves_its_exit_when_nobody_says_goodbye,
    a_goodbye_before_the_end_changes_nothing,
    a_terminal_holder_refuses_a_client_without_its_key,
    a_run_keeps_what_it_showed,
    a_run_streams_each_piece_as_it_prints,
    a_run_shows_nothing_its_shell_printed_before_the_command,
    a_run_whose_startup_asks_for_something_shows_it,
    the_holder_answers_where_the_cursor_is_with_nobody_looking,
    a_raw_prompt_that_asks_for_a_secret_waits_as_a_password_prompt,
    a_quiet_question_waits_and_offers_its_answers,
    a_terminal_outlives_its_spawner,
    a_shell_mark_needs_its_nonce_and_may_come_in_pieces,
    zsh_bash_and_fish_on_the_path_mark_their_commands,
    bash_on_its_own_keeps_what_its_commands_see,
];

/// Tests that need root, systemd or a running `maxd`, each with why. They
/// run with `--ignored` or `--include-ignored`. The ones named `root_` need
/// the test binary to run as root.
pub(crate) const IGNORED: &[(&str, &str, TestFn)] = ignored![
    root_run_as_drops_the_cli_to_the_user:
        "needs root, and a user from MAXHQ_TEST_USER or nobody",
    root_system_scope_holds_the_holder: "needs root and a system systemd",
    root_system_scope_outlives_a_restarted_service: "needs root and a system systemd",
    root_a_stale_scope_is_cleared_before_a_spawn: "needs root and a system systemd",
    user_scope_outlives_a_restarted_service: "needs a user systemd",
    host_holders_outlive_a_maxd_restart:
        "restarts the real maxd user unit, and needs a session running under it",
];

fn pid(pid: u32) -> Pid {
    Pid::from_raw(i32::try_from(pid).unwrap()).unwrap()
}

/// Asks a holder a test left running to go.
pub(crate) fn stop_holder(_dir: &Path, holder: i32) {
    if let Some(holder) = Pid::from_raw(holder) {
        let _ = rustix::process::kill_process(holder, Signal::TERM);
        // A test may have stopped it.
        let _ = rustix::process::kill_process(holder, Signal::CONT);
    }
}

/// Kills `pid` at once.
pub(crate) fn kill(pid: u32) {
    rustix::process::kill_process(self::pid(pid), Signal::KILL).unwrap();
}

/// Makes SIGTERM leave the process running, while what it returns is kept.
/// Once tokio handles SIGTERM, it no longer ends the process.
pub(crate) fn ignore_term() -> impl Sized {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let term = {
        let _entered = runtime.enter();
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap()
    };
    (term, runtime)
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

// Users and finding a holder again.

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

/// Who the fake runs as, for `whoami`.
pub(crate) fn whoami() -> Value {
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

/// `__spawner`, as the shared one, or with `--terminal <sessions> <sockets>
/// <terminal> <cwd>` for a terminal.
pub(crate) fn spawner(args: &[OsString]) -> ExitCode {
    if let [flag, sessions, sockets, terminal, cwd] = args
        && flag == "--terminal"
    {
        let terminal = terminal.to_str().unwrap();
        return terminal::spawner(sessions.as_ref(), sockets.as_ref(), terminal, cwd.as_ref());
    }
    crate::common::spawner(args)
}
