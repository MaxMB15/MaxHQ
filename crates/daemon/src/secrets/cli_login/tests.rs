// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The CLI sign-in against fake CLIs: shell scripts that print what
//! `claude auth login`, `codex login` and `agent login` print, as measured
//! with claude 2.1.319 and codex-cli 0.167.0 and as read from Cursor CLI
//! 2026.10.22's code, with placeholders in the links.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use maxhq_protocol::ids::{AccountId, DeviceId, SignInId};
use maxhq_protocol::message::{ActionOutcome, CliLoginState};
use maxhq_protocol::model::{LoginKind, Vendor};
use maxhq_protocol::state::{Change, Entity};
use maxhq_protocol::Secret;
use tokio::sync::mpsc;

use super::{
    last_words, plain_lines, push, whole_lines, Flow, Limits, Link, Rounds, GONE, KEEP_OUTPUT,
    TIMED_OUT,
};
use crate::drive::{Framing, Launch};
use crate::hub::{Command, Hub};
use crate::secrets::login_account;
use crate::secrets::testing::hub;
use crate::testing::TempDir;

/// Claude Code's link, with placeholders for its query values.
const CLAUDE_LINK: &str = "https://claude.com/cai/oauth/authorize?code=true&client_id=CLIENT&response_type=code&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback&scope=SCOPES&code_challenge=CHALLENGE&code_challenge_method=S256&state=STATE";

/// What Claude Code says of a code it doesn't take.
const CLAUDE_REFUSAL: &str = "Login failed: That code wasn't accepted. It may be incomplete, expired, already used, or from an earlier sign-in attempt. Sign in again to get a new code.";

/// Codex's link, with placeholders for its query values.
const CODEX_LINK: &str = "https://auth.openai.com/oauth/authorize?response_type=code&client_id=CLIENT&redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback&scope=SCOPES&code_challenge=CHALLENGE&code_challenge_method=S256&state=STATE";

/// What `codex login --device-auth` prints, colours and all.
const CODEX_DEVICE: &str = "\nFollow these steps to sign in with ChatGPT using device code authorization:\n\n1. Open this link in your browser and sign in to your account\n   \x1b[94mhttps://auth.openai.com/codex/device\x1b[0m\n\n2. Enter this one-time code \x1b[90m(expires in 15 minutes)\x1b[0m\n   \x1b[94m927R-RA928\x1b[0m\n\n";

/// Cursor's link, with placeholders for its query values.
const CURSOR_LINK: &str = "https://cursor.com/loginDeepControl?challenge=CHALLENGE&uuid=UUID&mode=login&redirectTarget=cli&supportsSelectedTeamLogin=true";

/// What Cursor's CLI says when its page never signed you in.
const CURSOR_REFUSAL: &str = "Login failed or timed out. Please try again.";

/// The code a fake Claude Code takes.
const GOOD_CODE: &str = "test-code-good#STATE";

/// Limits short enough for a test, and long enough for a busy machine.
const QUICK: Limits = Limits {
    link: Duration::from_secs(10),
    code: Duration::from_secs(10),
    round: Duration::from_secs(60),
    keep: Duration::from_secs(60),
};

/// What `claude auth login` prints with no terminal, up to its prompt,
/// which has no line break.
fn claude_says() -> String {
    format!(
        "Opening browser to sign in…\nIf the browser didn't open, visit: {CLAUDE_LINK}\nPaste code here if prompted > "
    )
}

/// What `codex login` prints.
fn codex_says() -> String {
    format!(
        "Starting local login server on http://localhost:1455.\nIf your browser did not open, navigate to this URL to authenticate:\n\n{CODEX_LINK}\n"
    )
}

/// What `agent login` prints with `NO_OPEN_BROWSER` set, with colours as
/// a CLI that ignores `NO_COLOR` would add them.
fn cursor_says() -> String {
    format!(
        "Starting login process...\nAuthenticating with Cursor...\nWaiting for browser authentication...\n\x1b[1mOpen a browser and navigate to this link:\x1b[22m \x1b[36m{CURSOR_LINK}\x1b[39m\n"
    )
}

#[test]
fn claude_codes_link_is_read_once_its_line_ends() {
    let lines = whole_lines(claude_says().as_bytes());
    let link = Link {
        url: CLAUDE_LINK.into(),
        user_code: None,
    };
    assert_eq!(Flow::Claude.link(&lines), Some(link));
    // The prompt hasn't ended its line.
    assert!(!lines.iter().any(|line| line.contains("Paste")));
    let says = claude_says();
    let cut = &says[..says.find("&scope").unwrap()];
    assert_eq!(Flow::Claude.link(&whole_lines(cut.as_bytes())), None);
}

#[test]
fn codexs_link_and_device_code_are_read_through_its_colours() {
    let link = Link {
        url: CODEX_LINK.into(),
        user_code: None,
    };
    assert_eq!(
        Flow::Codex.link(&whole_lines(codex_says().as_bytes())),
        Some(link)
    );

    let lines = whole_lines(CODEX_DEVICE.as_bytes());
    let link = Link {
        url: "https://auth.openai.com/codex/device".into(),
        user_code: Some("927R-RA928".into()),
    };
    assert_eq!(Flow::CodexDevice.link(&lines), Some(link));
    // Not before the code came, nor as another sign-in.
    let early = &CODEX_DEVICE[..CODEX_DEVICE.find("2. Enter").unwrap()];
    assert_eq!(Flow::CodexDevice.link(&whole_lines(early.as_bytes())), None);
    assert_eq!(Flow::Claude.link(&lines), None);
    assert_eq!(Flow::Codex.link(&lines), None);
}

#[test]
fn cursors_link_is_read_through_its_colours_once_its_line_ends() {
    let lines = whole_lines(cursor_says().as_bytes());
    let link = Link {
        url: CURSOR_LINK.into(),
        user_code: None,
    };
    assert_eq!(Flow::Cursor.link(&lines), Some(link));
    assert_eq!(Flow::Codex.link(&lines), None);
    assert_eq!(Flow::Claude.link(&lines), None);
    let says = cursor_says();
    let cut = &says[..says.find("&mode").unwrap()];
    assert_eq!(Flow::Cursor.link(&whole_lines(cut.as_bytes())), None);
    assert_eq!(
        last_words(&plain_lines(cursor_says().as_bytes()), 1).as_deref(),
        Some("Waiting for browser authentication...")
    );
}

#[test]
fn what_a_cli_said_last_leaves_out_its_prompt_link_and_code() {
    let failed = format!("{}{CLAUDE_REFUSAL}\n", claude_says());
    assert_eq!(
        last_words(&plain_lines(failed.as_bytes()), 1).as_deref(),
        Some(CLAUDE_REFUSAL)
    );
    assert_eq!(
        last_words(&plain_lines(CODEX_DEVICE.as_bytes()), 2).as_deref(),
        Some(
            "1. Open this link in your browser and sign in to your account 2. Enter this one-time code (expires in 15 minutes)"
        )
    );
    assert_eq!(
        last_words(&plain_lines(claude_says().as_bytes()), 1).as_deref(),
        Some("Opening browser to sign in…")
    );
    assert_eq!(last_words(&plain_lines(b"\n \r\n927R-RA928\n"), 2), None);
    let long = last_words(&["word ".repeat(100)], 1).unwrap();
    assert!(long.ends_with('…') && long.chars().count() == 200);
}

#[test]
fn the_output_kept_is_its_last_bytes_from_a_lines_start() {
    let mut kept = b"first\n".to_vec();
    push(&mut kept, &vec![b'x'; KEEP_OUTPUT]);
    push(&mut kept, b"\nlast line\n");
    assert_eq!(kept, b"last line\n");
    let mut kept = Vec::new();
    push(&mut kept, &vec![b'x'; KEEP_OUTPUT + 1]);
    assert_eq!(kept.len(), KEEP_OUTPUT);
}

/// Rounds on a hub of their own.
fn fresh(limits: Limits) -> (Rounds, Hub) {
    let hub = hub();
    (Rounds::new(hub.clone(), limits), hub)
}

/// A fake CLI that runs `body` in `dir`, with `says` in a file there for
/// it to print, and the launch that starts it under a config folder.
fn cli(dir: &Path, says: &str, body: &str) -> Launch {
    fs::write(dir.join("says"), says).unwrap();
    fs::write(dir.join("refusal"), format!("{CLAUDE_REFUSAL}\n")).unwrap();
    let program = dir.join("cli");
    fs::write(&program, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    Launch {
        program,
        args: vec!["auth".into(), "login".into()],
        env: vec![("CLAUDE_CONFIG_DIR".into(), dir.join("config").into())],
        env_remove: Vec::new(),
        cwd: dir.to_owned(),
        framing: Framing::ClaudeStreamJson,
        secret_env: Vec::new(),
        run_as: None,
    }
}

/// A fake `claude auth login`: it notes where it ran, prints its link
/// and takes [`GOOD_CODE`] alone.
fn claude(dir: &Path) -> Launch {
    let body = format!(
        "echo \"$CLAUDE_CONFIG_DIR $*\" > ran\necho $$ >> pids\ncat says\nread code\n\
         [ \"$code\" = '{GOOD_CODE}' ] && exit 0\ncat refusal >&2\nexit 1"
    );
    cli(dir, &claude_says(), &body)
}

/// A fake `codex login --device-auth`, which signs in once `go` is there.
fn codex_device(dir: &Path) -> Launch {
    cli(
        dir,
        CODEX_DEVICE,
        "cat says\nwhile [ ! -e go ]; do sleep 0.05; done\nexit 0",
    )
}

/// A fake `agent login`: it prints its link, then signs in once `go` is
/// there, or fails once `no` is.
fn cursor(dir: &Path) -> Launch {
    let body = format!(
        "echo $$ >> pids\ncat says\n\
         while [ ! -e go ] && [ ! -e no ]; do sleep 0.05; done\n\
         if [ -e go ]; then\n\
         printf '✓ Logged in as someone@example.com\\nAuthentication tokens stored securely.\\n'\n\
         exit 0\nfi\n\
         echo '{CURSOR_REFUSAL}' >&2\nexit 1"
    );
    cli(dir, &cursor_says(), &body)
}

fn begun(outcome: ActionOutcome) -> (SignInId, String, Option<String>, bool) {
    match outcome {
        ActionOutcome::CliLogin {
            sign_in,
            url,
            user_code,
            paste,
        } => (sign_in, url, user_code, paste),
        other => panic!("no sign-in began: {other:?}"),
    }
}

fn stands(state: CliLoginState) -> ActionOutcome {
    ActionOutcome::CliLoginState { state }
}

fn refusal(reason: &str) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.into(),
    }
}

/// Waits until `check` holds, for at most 10 seconds.
async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    for _ in 0..500 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{what} never happened");
}

/// Whether anything of the process group a fake CLI wrote first in its
/// `pids` file still runs.
fn group_runs(dir: &Path, nth: usize) -> bool {
    let pids = fs::read_to_string(dir.join("pids")).unwrap_or_default();
    let Some(pid) = pids
        .lines()
        .nth(nth)
        .and_then(|pid| pid.parse::<i32>().ok())
    else {
        return true;
    };
    let pid = rustix::process::Pid::from_raw(pid).unwrap();
    rustix::process::test_kill_process_group(pid).is_ok()
}

#[tokio::test]
async fn claude_code_takes_the_pasted_code_and_signs_in() {
    let dir = TempDir::new();
    let (rounds, hub) = fresh(QUICK);
    let mut commands = hub.commands();
    let (tx, mut signed_in) = mpsc::unbounded_channel();
    rounds.on_signed_in(Box::new(move |account| tx.send(account).unwrap()));
    let account = AccountId::derive("claude");

    let launch = claude(dir.path());
    let begin = rounds.start(account, &launch, Flow::Claude);
    let (sign_in, url, user_code, paste) = begun(begin.await);
    assert_eq!((url.as_str(), user_code, paste), (CLAUDE_LINK, None, true));
    assert_eq!(rounds.check(sign_in), stands(CliLoginState::Waiting));

    // Not a code at all, then the code with the spaces a paste brings.
    let blank = rounds.finish(sign_in, Secret::new(" \n".into())).await;
    assert!(matches!(blank, ActionOutcome::Refused { .. }));
    assert_eq!(rounds.check(sign_in), stands(CliLoginState::Waiting));
    let pasted = Secret::new(format!(" {GOOD_CODE}\n"));
    assert_eq!(
        rounds.finish(sign_in, pasted).await,
        stands(CliLoginState::SignedIn)
    );
    let ran = fs::read_to_string(dir.path().join("ran")).unwrap();
    let config = dir.path().join("config");
    assert_eq!(ran, format!("{} auth login\n", config.display()));

    // Its usage is checked again, and its idle CLIs let go.
    let refresh = tokio::time::timeout(Duration::from_secs(10), commands.recv());
    let refresh = refresh.await.unwrap().unwrap();
    assert_eq!(refresh, Command::RefreshUsage(Some(account)));
    let hooked = tokio::time::timeout(Duration::from_secs(10), signed_in.recv());
    assert_eq!(hooked.await.unwrap(), Some(account));
    assert_eq!(rounds.check(sign_in), stands(CliLoginState::SignedIn));
}

#[tokio::test]
async fn a_code_claude_code_refuses_fails_with_what_it_said() {
    let dir = TempDir::new();
    let (rounds, hub) = fresh(QUICK);
    let mut commands = hub.commands();
    let account = AccountId::derive("claude");
    let launch = claude(dir.path());
    let begin = rounds.start(account, &launch, Flow::Claude);
    let (sign_in, ..) = begun(begin.await);

    let wrong = Secret::new("test-code-wrong".into());
    let failed = stands(CliLoginState::Failed {
        message: CLAUDE_REFUSAL.into(),
    });
    assert_eq!(rounds.finish(sign_in, wrong.clone()).await, failed);
    // The CLI ended, so another code goes nowhere.
    assert_eq!(rounds.finish(sign_in, wrong).await, failed);
    assert!(commands.try_recv().is_err(), "no usage check");
}

#[tokio::test]
async fn codexs_device_code_signs_in_once_you_typed_it_at_its_page() {
    let dir = TempDir::new();
    let (rounds, _hub) = fresh(QUICK);
    let account = AccountId::derive("codex");
    let launch = codex_device(dir.path());
    let begin = rounds.start(account, &launch, Flow::CodexDevice);
    let (sign_in, url, user_code, paste) = begun(begin.await);
    assert_eq!(url, "https://auth.openai.com/codex/device");
    assert_eq!(user_code.as_deref(), Some("927R-RA928"));
    assert!(!paste);

    let code = Secret::new(GOOD_CODE.into());
    let finished = rounds.finish(sign_in, code).await;
    assert!(matches!(finished, ActionOutcome::Refused { .. }));
    assert_eq!(rounds.check(sign_in), stands(CliLoginState::Waiting));
    fs::write(dir.path().join("go"), "").unwrap();
    let done = stands(CliLoginState::SignedIn);
    eventually("the sign-in", || rounds.check(sign_in) == done).await;
}

#[tokio::test]
async fn cursor_signs_in_once_its_page_did() {
    let dir = TempDir::new();
    let (rounds, hub) = fresh(QUICK);
    let mut commands = hub.commands();
    let (tx, mut signed_in) = mpsc::unbounded_channel();
    rounds.on_signed_in(Box::new(move |account| tx.send(account).unwrap()));
    let account = AccountId::derive("cursor");

    let launch = cursor(dir.path());
    let begin = rounds.start(account, &launch, Flow::Cursor);
    let (sign_in, url, user_code, paste) = begun(begin.await);
    assert_eq!((url.as_str(), user_code, paste), (CURSOR_LINK, None, false));
    assert_eq!(rounds.check(sign_in), stands(CliLoginState::Waiting));

    // It ends in the browser, with no code to paste.
    let code = Secret::new(GOOD_CODE.into());
    let finished = rounds.finish(sign_in, code).await;
    assert!(matches!(finished, ActionOutcome::Refused { .. }));
    assert_eq!(rounds.check(sign_in), stands(CliLoginState::Waiting));

    fs::write(dir.path().join("go"), "").unwrap();
    let done = stands(CliLoginState::SignedIn);
    eventually("the sign-in", || rounds.check(sign_in) == done).await;
    let refresh = tokio::time::timeout(Duration::from_secs(10), commands.recv());
    let refresh = refresh.await.unwrap().unwrap();
    assert_eq!(refresh, Command::RefreshUsage(Some(account)));
    let hooked = tokio::time::timeout(Duration::from_secs(10), signed_in.recv());
    assert_eq!(hooked.await.unwrap(), Some(account));
}

#[tokio::test]
async fn a_cursor_sign_in_that_failed_says_what_cursor_said() {
    let dir = TempDir::new();
    let (rounds, hub) = fresh(QUICK);
    let mut commands = hub.commands();
    let launch = cursor(dir.path());
    let begin = rounds.start(AccountId::derive("cursor"), &launch, Flow::Cursor);
    let (sign_in, ..) = begun(begin.await);
    fs::write(dir.path().join("no"), "").unwrap();
    let failed = stands(CliLoginState::Failed {
        message: CURSOR_REFUSAL.into(),
    });
    eventually("the failure", || rounds.check(sign_in) == failed).await;
    assert!(commands.try_recv().is_err(), "no usage check");
}

#[tokio::test]
async fn a_cursor_sign_in_past_its_time_or_cancelled_stops_its_cli() {
    let dir = TempDir::new();
    // Long enough for macOS to check a new script under load first.
    let quick = Limits {
        round: Duration::from_secs(5),
        ..QUICK
    };
    let (rounds, _hub) = fresh(quick);
    let launch = cursor(dir.path());
    let begin = rounds.start(AccountId::derive("cursor"), &launch, Flow::Cursor);
    let (sign_in, ..) = begun(begin.await);
    let timed_out = stands(CliLoginState::Failed {
        message: TIMED_OUT.into(),
    });
    eventually("the time limit", || rounds.check(sign_in) == timed_out).await;
    eventually("the first CLI's stop", || !group_runs(dir.path(), 0)).await;

    let (rounds, _hub) = fresh(QUICK);
    let begin = rounds.start(AccountId::derive("cursor"), &launch, Flow::Cursor);
    let (sign_in, ..) = begun(begin.await);
    assert_eq!(rounds.cancel(sign_in), ActionOutcome::Done);
    assert_eq!(rounds.check(sign_in), refusal(GONE));
    eventually("the second CLI's stop", || !group_runs(dir.path(), 1)).await;
}

#[tokio::test]
async fn a_cli_that_gives_no_link_is_refused_with_what_it_said() {
    let dir = TempDir::new();
    let (rounds, _hub) = fresh(QUICK);
    let account = AccountId::derive("codex");
    let said =
        "printf 'Error: device code sign-in is off.\\nAsk an admin to turn it on.\\n' >&2\nexit 1";
    let launch = cli(dir.path(), "", said);
    assert_eq!(
        rounds.start(account, &launch, Flow::CodexDevice).await,
        refusal(
            "Codex didn't give a sign-in link: Error: device code sign-in is off. Ask an admin to turn it on."
        )
    );

    // One that says nothing in time stops.
    let quick = Limits {
        link: Duration::from_millis(300),
        ..QUICK
    };
    let (rounds, _hub) = fresh(quick);
    let launch = cli(dir.path(), "", "echo $$ >> pids\nsleep 30");
    assert_eq!(
        rounds.start(account, &launch, Flow::Claude).await,
        refusal("Claude Code didn't give a sign-in link.")
    );
    eventually("the CLI's stop", || !group_runs(dir.path(), 0)).await;

    // And one that isn't there.
    let mut missing = launch.clone();
    missing.program = PathBuf::from("/nonexistent/claude");
    let ActionOutcome::Refused { reason } = rounds.start(account, &missing, Flow::Claude).await
    else {
        panic!("a missing CLI started");
    };
    assert!(reason.starts_with("This device couldn't start Claude Code to sign in: "));
}

#[tokio::test]
async fn a_round_past_its_time_stops_its_cli() {
    let dir = TempDir::new();
    // Long enough for macOS to check a new script under load first.
    let quick = Limits {
        round: Duration::from_secs(5),
        ..QUICK
    };
    let (rounds, _hub) = fresh(quick);
    let launch = claude(dir.path());
    let begin = rounds.start(AccountId::derive("claude"), &launch, Flow::Claude);
    let (sign_in, ..) = begun(begin.await);
    let timed_out = stands(CliLoginState::Failed {
        message: TIMED_OUT.into(),
    });
    eventually("the time limit", || rounds.check(sign_in) == timed_out).await;
    eventually("the CLI's stop", || !group_runs(dir.path(), 0)).await;
}

#[tokio::test]
async fn a_newer_round_or_a_cancel_stops_the_one_before() {
    let dir = TempDir::new();
    let (rounds, _hub) = fresh(QUICK);
    let account = AccountId::derive("claude");
    let launch = claude(dir.path());
    let (first, ..) = begun(rounds.start(account, &launch, Flow::Claude).await);
    let (second, ..) = begun(rounds.start(account, &launch, Flow::Claude).await);
    assert_eq!(rounds.check(first), refusal(GONE));
    assert_eq!(rounds.check(second), stands(CliLoginState::Waiting));
    eventually("the first CLI's stop", || !group_runs(dir.path(), 0)).await;
    let code = Secret::new(GOOD_CODE.into());
    assert_eq!(rounds.finish(first, code).await, refusal(GONE));

    // Another account's round runs alongside.
    let other = TempDir::new();
    let codex = AccountId::derive("codex");
    let (third, ..) = begun(
        rounds
            .start(codex, &claude(other.path()), Flow::Claude)
            .await,
    );
    assert_eq!(rounds.cancel(second), ActionOutcome::Done);
    assert_eq!(rounds.check(second), refusal(GONE));
    eventually("the second CLI's stop", || !group_runs(dir.path(), 1)).await;
    assert_eq!(rounds.check(third), stands(CliLoginState::Waiting));
    // Cancelling one that ended is done too.
    assert_eq!(rounds.cancel(second), ActionOutcome::Done);
}

#[tokio::test]
async fn an_ended_round_goes_a_while_after() {
    let dir = TempDir::new();
    let quick = Limits {
        keep: Duration::from_millis(300),
        ..QUICK
    };
    let (rounds, _hub) = fresh(quick);
    let launch = codex_device(dir.path());
    fs::write(dir.path().join("go"), "").unwrap();
    let begin = rounds.start(AccountId::derive("codex"), &launch, Flow::CodexDevice);
    let (sign_in, ..) = begun(begin.await);
    eventually("the round's end", || rounds.check(sign_in) == refusal(GONE)).await;
}

/// An account on `device`.
fn account(id: AccountId, device: DeviceId, vendor: Vendor, login: LoginKind) -> Entity {
    let dir = "/home/me/.config-placeholder".to_owned();
    Entity::Account(login_account(id, device, vendor, "Work".into(), login, dir))
}

#[tokio::test]
async fn only_a_cli_login_in_its_folder_here_signs_in_again() {
    let (rounds, hub) = fresh(QUICK);
    let here = hub.device().id;
    let cli_only =
        "This account doesn't sign in through its CLI. Change its login in Settings, Logins.";
    let cases = [
        ("unknown", None, "This device doesn't hold that account."),
        (
            "elsewhere",
            Some(account(
                AccountId::derive("elsewhere"),
                DeviceId::derive("devbox"),
                Vendor::Claude,
                LoginKind::ConfigDir,
            )),
            "That account is on another device. Sign in to it again from there.",
        ),
        (
            "token",
            Some(account(
                AccountId::derive("token"),
                here,
                Vendor::Claude,
                LoginKind::SetupToken,
            )),
            cli_only,
        ),
        (
            "chatgpt",
            Some(account(
                AccountId::derive("chatgpt"),
                here,
                Vendor::Codex,
                LoginKind::ChatGptSignIn,
            )),
            cli_only,
        ),
        (
            "cursor-key",
            Some(account(
                AccountId::derive("cursor-key"),
                here,
                Vendor::Cursor,
                LoginKind::CursorApiKey,
            )),
            cli_only,
        ),
        (
            "acp",
            Some(account(
                AccountId::derive("acp"),
                here,
                Vendor::Acp,
                LoginKind::ConfigDir,
            )),
            "Signing in again works for Claude Code, Codex and Cursor logins only.",
        ),
    ];
    for (name, entity, reason) in cases {
        if let Some(value) = entity {
            hub.apply([Change::Upsert { value }]);
        }
        assert_eq!(
            rounds.begin(AccountId::derive(name), true).await,
            refusal(reason),
            "{name}"
        );
    }
}

#[test]
fn a_cursor_cli_login_here_runs_agent_login_without_a_browser() {
    let (rounds, hub) = fresh(QUICK);
    let id = AccountId::derive("cursor");
    let value = account(id, hub.device().id, Vendor::Cursor, LoginKind::ConfigDir);
    hub.apply([Change::Upsert { value }]);
    // Only planned, never run: the real CLI may be on this machine.
    let Ok((launch, flow)) = rounds.sign_in(id, true) else {
        panic!("a Cursor CLI login was refused");
    };
    assert_eq!(flow, Flow::Cursor, "a device code is for Codex alone");
    assert_eq!(launch.args, ["login"]);
    assert!(launch.env.contains(&("NO_OPEN_BROWSER".into(), "1".into())));
    assert!(launch.env_remove.contains(&"CURSOR_API_KEY".into()));
}
