// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The tests on Windows: the shared ones, the ones that end holders, the
//! jobs their CLIs run in and their spawners, and terminals on a
//! pseudoconsole.
//!
//! Windows can't ask a process with no console to end, so where Unix
//! sends SIGTERM these expect the CLI's job ended, which gives exit code
//! 137.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use maxhq_hold::{ChildState, Ended, Framing, Mark, Reached, TermLaunch, WriteError};
use maxhq_os::proc::{self, Stop};
use maxhq_protocol::ids::{SessionId, TerminalId};
use maxhq_protocol::model::WindowsShell;
use serde_json::{json, Value};

use crate::common::*;
use crate::term::*;

pub(crate) use crate::common::spawner;

/// The exit code of a CLI whose job was ended, as 128 plus SIGKILL's 9
/// reads on Unix.
const KILLED: i32 = 137;

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
    stop_kills_a_stubborn_cli_after_its_grace,
    stop_ends_what_the_cli_started,
    a_killed_holder_ends_with_an_unknown_exit,
    a_holder_outlives_a_killed_spawner,
    a_holder_outlives_its_spawners_job,
    a_restarted_spawner_finds_the_holder_it_started,
    a_running_session_cant_be_forgotten,
    a_session_starts_once,
    a_missing_program_fails_the_spawn,
    a_missing_folder_fails_the_spawn_by_its_name,
    long_lines_from_two_writers_stay_whole,
    end_ends_the_cli_tree_then_its_holder,
    end_ends_the_cli_a_killed_holder_left,
    end_leaves_a_holder_it_cant_confirm,
    a_secret_reaches_the_cli_and_no_file,
    a_failed_spawn_never_shows_a_secret,
    ensure_reaches_the_running_holder,
    ensure_leaves_an_ended_session_alone,
    cmd_runs_on_the_pseudoconsole,
    the_shell_choice_falls_back_in_order,
    powershell_marks_its_commands,
];

/// None of the tests on Windows needs more than the runner has.
pub(crate) const IGNORED: &[(&str, &str, TestFn)] = ignored![];

/// Ends a holder a test left running, and its CLI's job first while the
/// holder still keeps the job's name.
pub(crate) fn stop_holder(dir: &Path, holder: i32) {
    if let Ok(record) = std::fs::read(dir.join("holder.json"))
        && let Ok(record) = serde_json::from_slice::<Value>(&record)
        && let Some(cli) = record["child_pid"]
            .as_u64()
            .and_then(|pid| u32::try_from(pid).ok())
        && record["child_started"].as_u64().is_some()
        && proc::started(cli) == record["child_started"].as_u64()
        && proc::stop_group(cli, Stop::Hard).is_err()
    {
        let _ = proc::stop(cli, Stop::Hard);
    }
    let _ = proc::stop(holder.unsigned_abs(), Stop::Hard);
}

/// Kills `pid` at once.
pub(crate) fn kill(pid: u32) {
    proc::stop(pid, Stop::Hard).unwrap();
}

/// Nothing: Windows sends the fake no SIGTERM to ignore.
pub(crate) fn ignore_term() {}

/// Who the fake runs as, for `whoami`.
pub(crate) fn whoami() -> Value {
    json!({
        "type": "whoami",
        "user": std::env::var("USERNAME").unwrap_or_default(),
        "cwd": std::env::current_dir().unwrap_or_default(),
        "pid": std::process::id(),
    })
}

/// The CLI's process ID and when it started, from its holder's hello.
fn running_cli(child: &ChildState) -> (u32, u64) {
    let ChildState::Running { pid } = *child else {
        panic!("the agent ended: {child:?}");
    };
    (pid, proc::started(pid).expect("the CLI runs"))
}

/// Has the fake CLI start a process, and says which and when it started.
async fn start_orphan(connection: &mut maxhq_hold::Connection) -> (u32, u64) {
    write(connection, &json!({"fake": "orphan"}).to_string()).await;
    let said = parse(&next_line(&mut connection.events).await);
    let orphan = (said["pid"].as_u64())
        .and_then(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid > 0)
        .unwrap_or_else(|| panic!("no orphan in {said}"));
    (orphan, proc::started(orphan).expect("the orphan runs"))
}

/// Waits until the process `pid` that started at `started` has gone. One
/// that later has its ID started at another time.
async fn gone(what: &str, (pid, started): (u32, u64)) {
    eventually(what, || proc::started(pid) != Some(started)).await;
}

// The holder and its CLI.

async fn stop_kills_a_stubborn_cli_after_its_grace(ctx: Ctx) {
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
    // One grace for the end of its input, and none for a SIGTERM Windows
    // can't send.
    assert!(started.elapsed() >= Duration::from_millis(300));
    let (_, exit) = until_exit(&mut connection.events).await;
    assert_eq!((exit.code, exit.signal), (Some(KILLED), None));
    let after = connection.writer.write("x".into(), None).await;
    assert!(matches!(after, Err(WriteError::Finished)), "{after:?}");
}

async fn stop_ends_what_the_cli_started(ctx: Ctx) {
    let mut launch = ctx.launch(Framing::ClaudeStreamJson);
    launch.args.push("--stubborn".into());
    let (_, mut connection) = ctx.start_with(ctx.holds(), launch).await;
    let orphan = start_orphan(&mut connection).await;
    connection
        .writer
        .stop(Duration::from_millis(300))
        .await
        .unwrap();
    let (_, exit) = until_exit(&mut connection.events).await;
    assert_eq!(exit.code, Some(KILLED));
    // It was in the CLI's job, which ended with it.
    gone("the orphan to go with the job", orphan).await;
}

async fn a_holder_outlives_a_killed_spawner(ctx: Ctx) {
    let session = SessionId::generate();
    let mut spawner = start_spawner(&ctx, session, None);
    assert_eq!(go(&mut spawner).await, "spawned");
    spawner.kill().unwrap();
    spawner.wait().unwrap();
    still_runs(&ctx, session, spawner.id()).await;
}

/// The spawner plays a `maxd` whose service manager runs it in a job and
/// ends the job to stop it, as a Windows service host or a scheduled task
/// can. A holder that broke away from the job outlives it.
async fn a_holder_outlives_its_spawners_job(ctx: Ctx) {
    let session = SessionId::generate();
    let mut spawner = start_spawner(&ctx, session, None);
    // It waits for a line before it starts anything, so all it starts is
    // in the job.
    let _group = proc::adopt(spawner.id()).unwrap();
    assert_eq!(go(&mut spawner).await, "spawned");
    let record = read_json(&ctx.session_dir(session).join("holder.json"));
    proc::stop_group(spawner.id(), Stop::Hard).unwrap();
    spawner.wait().unwrap();
    if record["in_spawner_job"] == true {
        // The job the runner put the tests in doesn't let the holder
        // break away, so it went with the spawner's.
        eprintln!(
            "note: the runner's job keeps holders from breaking away, so a holder can't outlive its spawner's job here"
        );
        return;
    }
    still_runs(&ctx, session, spawner.id()).await;
}

/// The spawner plays a `maxd` that restarts: the second one finds the
/// holder the first started and starts none.
async fn a_restarted_spawner_finds_the_holder_it_started(ctx: Ctx) {
    let session = SessionId::generate();
    let report = ctx.root.join("report");
    let mut first = start_spawner(&ctx, session, Some(&report));
    assert_eq!(go(&mut first).await, "spawned");
    first.kill().unwrap();
    first.wait().unwrap();
    let mut second = start_spawner(&ctx, session, Some(&report));
    let said = go(&mut second).await;
    second.kill().unwrap();
    second.wait().unwrap();
    let mut connection = ctx.holds().connect(session, None).await.unwrap();
    assert_eq!(said, format!("live {}", connection.hello.holder_pid));
    let reported = std::fs::read_to_string(&report).unwrap();
    assert_eq!(reported, format!("spawned\n{said}\n"));
    echo(&mut connection, "still the one").await;
    connection
        .writer
        .stop(Duration::from_secs(5))
        .await
        .unwrap();
}

/// Runs `__spawner` for `session`, and with `report` as a service that
/// restarts. It starts nothing until [`go`].
fn start_spawner(ctx: &Ctx, session: SessionId, report: Option<&Path>) -> Child {
    let mut command = Command::new(exe());
    command
        .arg("__spawner")
        .arg(&ctx.dirs.sessions)
        .arg(&ctx.dirs.sockets)
        .arg(session.to_string())
        .arg(&ctx.root);
    if let Some(report) = report {
        command.arg("--ensure").arg(report);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap()
}

/// Lets the spawner start and returns what it said once it had.
async fn go(spawner: &mut Child) -> String {
    let mut stdin = spawner.stdin.take().unwrap();
    writeln!(stdin, "go").unwrap();
    let stdout = spawner.stdout.take().unwrap();
    let said = tokio::task::spawn_blocking(move || {
        let mut line = String::new();
        BufReader::new(stdout).read_line(&mut line).map(|_| line)
    })
    .await
    .unwrap()
    .unwrap();
    said.trim().to_owned()
}

/// Checks the holder for `session` and its CLI carry on after the spawner
/// `spawner` went, and stops them.
async fn still_runs(ctx: &Ctx, session: SessionId, spawner: u32) {
    let mut connection = ctx.holds().connect(session, None).await.unwrap();
    running_cli(&connection.hello.child);
    assert_ne!(connection.hello.holder_pid, spawner);
    echo(&mut connection, "still here").await;
    connection
        .writer
        .stop(Duration::from_secs(5))
        .await
        .unwrap();
    let (_, exit) = until_exit(&mut connection.events).await;
    assert_eq!(exit.code, Some(0));
}

async fn end_ends_the_cli_tree_then_its_holder(ctx: Ctx) {
    let holds = ctx.holds();
    let (session, mut connection) = ctx.start(Framing::ClaudeStreamJson).await;
    let orphan = start_orphan(&mut connection).await;
    let dir = ctx.session_dir(session);
    let holder = connection.hello.holder_pid;
    let ended = holds.end(session, Some(holder), WAIT).await;
    assert_eq!(ended, Ended::Gone);
    assert_eq!(live_holder(&dir), None);
    // The holder recorded how its CLI's job ended before it went.
    let exit = read_json(&dir.join("exit.json"));
    assert_eq!(exit["code"], KILLED, "{exit}");
    gone("the orphan to go with the job", orphan).await;
    // Ending it again finds nothing to end.
    assert_eq!(holds.end(session, None, WAIT).await, Ended::Gone);
}

async fn end_ends_the_cli_a_killed_holder_left(ctx: Ctx) {
    let holds = ctx.holds();
    let mut launch = ctx.launch(Framing::ClaudeStreamJson);
    launch.args.push("--stubborn".into());
    let (session, mut connection) = ctx.start_with(holds.clone(), launch).await;
    echo(&mut connection, "x").await;
    let cli = running_cli(&connection.hello.child);
    let holder = connection.hello.holder_pid;
    kill(holder);
    until_exit(&mut connection.events).await;
    // The job has no kill-on-close, so the stubborn CLI runs on.
    assert_eq!(proc::started(cli.0), Some(cli.1));
    let ended = holds
        .end(session, Some(holder), Duration::from_millis(300))
        .await;
    assert_eq!(ended, Ended::Gone);
    gone("the CLI to go", cli).await;
}

// Terminals.

/// A terminal with `program` and `args`, in the test's folder.
fn console(ctx: &Ctx, program: PathBuf, args: &[&str]) -> TermLaunch {
    TermLaunch {
        terminal: TerminalId::generate(),
        program,
        args: args.iter().map(OsString::from).collect(),
        env: Vec::new(),
        cwd: ctx.root.clone(),
        cols: 120,
        rows: 30,
        run: false,
        nonce: None,
    }
}

/// What a line typed at the pseudoconsole runs gets back, and how the
/// program's exit code comes through.
async fn cmd_runs_on_the_pseudoconsole(ctx: Ctx) {
    let cmd = std::env::var_os("ComSpec").map_or_else(|| "cmd.exe".into(), PathBuf::from);
    let launch = console(&ctx, cmd, &["/d", "/q"]);
    let mut link = start(&ctx, &launch).await;
    let mut seen = Seen::from(&link);
    // The typed line shows `%OS%`, and only its output what it expands to.
    assert!(link.control.input(b"echo %OS%-round-trip\r".to_vec()));
    seen.until_text(&mut link, "Windows_NT-round-trip").await;
    assert!(link.control.input(b"exit 5\r".to_vec()));
    let exit = seen.until_exit(&mut link).await;
    assert_eq!(exit.code, Some(5), "{seen:?}");
}

/// Each choice that's installed is what starts, and one that isn't falls
/// back to the first of Git Bash, PowerShell 7 and Windows PowerShell
/// that is, which Windows always has the last of.
async fn the_shell_choice_falls_back_in_order(_ctx: Ctx) {
    let pick = |choice| TermLaunch::pick_shell(choice);
    let name = |path: &Path| path.file_name().unwrap().to_string_lossy().to_lowercase();
    let (git_bash, git_bash_missing) = pick(WindowsShell::GitBash);
    let (pwsh, pwsh_missing) = pick(WindowsShell::Pwsh);
    let (powershell, powershell_missing) = pick(WindowsShell::PowerShell);
    let (auto, auto_fell_back) = pick(WindowsShell::Auto);
    assert!(!powershell_missing, "no Windows PowerShell");
    assert!(!auto_fell_back);
    assert_eq!(name(&powershell), "powershell.exe");
    let first = [
        (git_bash_missing, &git_bash),
        (pwsh_missing, &pwsh),
        (false, &powershell),
    ]
    .into_iter()
    .find(|(missing, _)| !missing)
    .map(|(_, path)| path.clone());
    assert_eq!(Some(&auto), first.as_ref());
    if !git_bash_missing {
        assert_eq!(name(&git_bash), "bash.exe");
        // Not WSL's, which System32 has.
        assert!(
            !git_bash
                .to_string_lossy()
                .to_lowercase()
                .contains("system32"),
            "{}",
            git_bash.display()
        );
    }
    if !pwsh_missing {
        assert_eq!(name(&pwsh), "pwsh.exe");
    }
    for (missing, path, what) in [
        (git_bash_missing, &git_bash, "Git Bash"),
        (pwsh_missing, &pwsh, "PowerShell 7"),
    ] {
        if missing {
            eprintln!("note: no {what}, so {} starts in its place", path.display());
        }
    }
}

/// PowerShell 7 and Windows PowerShell, each that's installed, mark their
/// prompts and commands through the pseudoconsole, with each command's
/// exit and folder.
async fn powershell_marks_its_commands(ctx: Ctx) {
    for choice in [WindowsShell::Pwsh, WindowsShell::PowerShell] {
        let (program, missing) = TermLaunch::pick_shell(choice);
        if missing {
            eprintln!("note: no {choice:?}, so its shell integration goes untested");
            continue;
        }
        a_powershell_marks_its_commands(&ctx, program).await;
    }
}

async fn a_powershell_marks_its_commands(ctx: &Ctx, program: PathBuf) {
    let shown = program.display().to_string();
    let terminals = terminals(ctx);
    let folder = ctx.root.join("a b'c");
    std::fs::create_dir_all(&folder).unwrap();
    // Wide, so no line the test types wraps.
    let mut launch =
        TermLaunch::shell_in(TerminalId::generate(), program, ctx.root.clone(), 200, 30);
    assert!(launch.powershell(), "{shown}");
    assert!(terminals.integrate(&mut launch).unwrap(), "{shown}");
    let nonce = launch.nonce.clone().unwrap();
    let mut link = start(ctx, &launch).await;
    let Reached::Live(mut watch) = terminals.watch(launch.terminal).await.unwrap() else {
        panic!("no watch for {shown}");
    };
    let mut watched = Seen::from(&watch);
    let cd = format!(
        "Set-Location -LiteralPath {}",
        maxhq_core::shell::powershell::literal(&folder.to_string_lossy())
    );
    let typed = [
        "Write-Output \"nonce:$($null -eq $env:MAXHQ_SHELL_NONCE)\"",
        cd.as_str(),
        "cmd /c exit 3",
    ];
    for (done, line) in typed.iter().enumerate() {
        ready(&mut watch, &mut watched, done).await;
        assert!(link.control.input(format!("{line}\r").into_bytes()));
    }
    ready(&mut watch, &mut watched, typed.len()).await;
    assert!(link.control.input(b"exit 0\r".to_vec()));
    let mut seen = Seen::from(&link);
    let exit = seen.until_exit(&mut link).await;
    assert_eq!(exit.code, Some(0), "{shown}: {seen:?}");
    watched.until_exit(&mut watch).await;

    // Whether the pseudoconsole moved or dropped a mark: each is where
    // the holder says, in the order the shell wrote them.
    assert_marks_in_place(&seen.bytes, &watched.marks);
    assert!(!holds_bytes(&seen.bytes, nonce.as_bytes()), "{shown}");
    let order: Vec<&str> = (watched.marks.iter())
        .map(|(_, mark)| match mark {
            Mark::Prompt => "A",
            Mark::Input => "B",
            Mark::Command { .. } => "C",
            Mark::Done { .. } => "D",
        })
        .collect();
    let expected: Vec<&str> = (0..typed.len())
        .flat_map(|_| ["A", "B", "C", "D"])
        .chain(["A", "B"])
        .collect();
    assert_eq!(order[..expected.len()], expected[..], "{shown}: {order:?}");
    let commands = commands_of(&watched.marks);
    let list: Vec<_> = commands.list().iter().take(typed.len()).collect();
    let exits: Vec<_> = list.iter().map(|command| command.exit).collect();
    assert_eq!(exits, [Some(0), Some(0), Some(3)], "{shown}: {watched:?}");
    let text = |from: Option<u64>, to: Option<u64>| {
        let range = usize::try_from(from.unwrap()).unwrap()..usize::try_from(to.unwrap()).unwrap();
        String::from_utf8_lossy(&seen.bytes[range]).into_owned()
    };
    // The nonce was gone from the environment by the first command.
    let output = text(list[0].start, list[0].end);
    assert!(output.contains("nonce:True"), "{shown}: {output:?}");
    let folder_of =
        |command: &maxhq_hold::ShellCommand| PathBuf::from(command.cwd.clone().unwrap());
    let root = ctx.root.file_name().unwrap();
    assert!(folder_of(list[0]).ends_with(root), "{shown}: {list:?}");
    assert!(
        folder_of(list[2]).ends_with(folder.file_name().unwrap()),
        "{shown}: {list:?}"
    );
}
