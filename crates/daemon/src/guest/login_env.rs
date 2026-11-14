// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What a login shell sets, for the CLIs a guest starts.
//!
//! A CLI runs its commands in a plain shell that reads neither
//! `/etc/profile.d` nor `~/.profile`, so a toolchain set up there reaches a
//! terminal and not an agent. Each launch asks a login shell for its
//! variables and adds them under MaxHQ's own. They go on the secret pipe,
//! because a profile can hold a key and `launch.json` stays on disk.

use std::env;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use maxhq_hold::RunAs;
use maxhq_protocol::Secret;
use rustix::process::{kill_process_group, Pid, Signal};
use tokio::io::AsyncReadExt;
use tokio::process::{ChildStdout, Command};
use tracing::warn;

use super::worktree;
use crate::session::holders::Launch;

/// How long a login shell gets. After that the CLI starts without its answer.
const TIMEOUT: Duration = Duration::from_secs(5);

/// The most a shell's answer can be, well under the secret pipe's 1 MiB.
const MOST: usize = 512 * 1024;

/// Sits before the answer, so nothing a profile prints can pass for it.
const START: &[u8] = b"\0MAXHQ-LOGIN-ENV\0";

/// The last field of the answer.
const END: &[u8] = b"MAXHQ-LOGIN-ENV-END";

/// What the shell runs. Interactive as well as login, because Debian's
/// `.bashrc` returns early otherwise and tools add themselves below that.
const ASK: &str = r"printf '\0MAXHQ-LOGIN-ENV\0'; command env -0; printf 'MAXHQ-LOGIN-ENV-END\0'";

/// A shell's own variables and the ones the holder sets for the user.
const SHELLS_OWN: &[&str] = &[
    "_", "HOME", "LOGNAME", "MAIL", "OLDPWD", "PWD", "SHELL", "SHLVL", "TERM", "USER",
];

/// Variables that start like these could pick another account or key, and a
/// CLI signs in only the way MaxHQ set it up.
const SIGN_IN_PREFIXES: &[&str] = &[
    "ANTHROPIC",
    "CLAUDE",
    "CODEX",
    "CURSOR",
    "GH_",
    "GITHUB_",
    "MAXHQ",
    "OPENAI",
];

/// Other variables a launch leaves as it found them. `NODE_OPTIONS` can load
/// code into a CLI, so the CLI launches take it out.
const LEFT_TO_THE_LAUNCH: &[&str] = &["ACCESS_TOKEN", "NODE_OPTIONS"];

/// Adds the variables of a login shell to `launch`. A launch with no user is
/// a test's, and one whose shell doesn't answer starts without them.
pub(super) async fn add(launch: &mut Launch) {
    let Some(user) = launch.run_as.clone() else {
        return;
    };
    match read(user, launch.cwd.clone()).await {
        Ok(found) => merge(launch, found, env::var_os("PATH").as_deref()),
        Err(reason) => warn!(
            session = %launch.session,
            "starting the CLI without the login shell's settings: {reason}"
        ),
    }
}

/// Asks the login shell of `user`, started the way a terminal's is.
async fn read(user: RunAs, cwd: PathBuf) -> Result<Vec<(OsString, OsString)>, String> {
    let login = tokio::task::spawn_blocking(move || user.login(&cwd))
        .await
        .map_err(|err| err.to_string())??;
    let shell = login.shell().unwrap_or(Path::new("/bin/bash")).to_owned();
    let mut process = std::process::Command::new(&shell);
    process.args(["-ilc", ASK]).env_clear();
    process.envs(login.env().iter().map(|(key, value)| (key, value)));
    process.env("SHELL", &shell).env("PATH", worktree::PATH);
    login.apply(&mut process)?;
    answer(process, TIMEOUT).await
}

/// Runs `process` and reads its answer, then ends whatever it left running
/// in its group. A background job a profile starts can hold stdout open long
/// after the shell exits, so this stops at the end mark and not at the pipe's
/// end.
async fn answer(
    mut process: std::process::Command,
    timeout: Duration,
) -> Result<Vec<(OsString, OsString)>, String> {
    process
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = Command::from(process)
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| format!("the shell didn't start: {err}"))?;
    let group = child
        .id()
        .and_then(|id| i32::try_from(id).ok())
        .and_then(Pid::from_raw);
    let stdout = child.stdout.take().ok_or("the shell has no stdout")?;
    let found = tokio::time::timeout(timeout, read_answer(stdout)).await;
    if let Some(group) = group {
        // Before the wait, so the group's ID can't have gone to anything else.
        let _ = kill_process_group(group, Signal::KILL);
    }
    let _ = child.wait().await;
    found.map_err(|_| format!("the shell took more than {} seconds", timeout.as_secs()))?
}

async fn read_answer(mut stdout: ChildStdout) -> Result<Vec<(OsString, OsString)>, String> {
    let mut seen = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let read = stdout
            .read(&mut chunk)
            .await
            .map_err(|err| format!("couldn't read the shell's answer: {err}"))?;
        if read == 0 {
            return Err("the shell ended without answering".to_owned());
        }
        seen.extend_from_slice(&chunk[..read]);
        if let Some(found) = parse(&seen) {
            return Ok(found);
        }
        if seen.len() > MOST {
            return Err("the shell's answer is too long".to_owned());
        }
    }
}

/// The names and values in a shell's answer. None until the end has arrived.
fn parse(answer: &[u8]) -> Option<Vec<(OsString, OsString)>> {
    let start = answer.windows(START.len()).position(|part| part == START)? + START.len();
    let mut found = Vec::new();
    for field in answer[start..].split(|byte| *byte == 0) {
        if field == END {
            return Some(found);
        }
        let Some(equals) = field.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        if equals > 0 {
            found.push((
                OsString::from_vec(field[..equals].to_vec()),
                OsString::from_vec(field[equals + 1..].to_vec()),
            ));
        }
    }
    None
}

/// Whether `name` stays out of a launch whatever the shell says.
fn left_out(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return true;
    };
    SHELLS_OWN.contains(&name)
        || LEFT_TO_THE_LAUNCH.contains(&name)
        || SIGN_IN_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
}

/// Adds `found` under what `launch` sets and takes out. `PATH` is the one
/// variable both have a say in, and `inherited` is what the CLI would get
/// when the launch doesn't set it.
fn merge(launch: &mut Launch, found: Vec<(OsString, OsString)>, inherited: Option<&OsStr>) {
    for (name, value) in found {
        if name == "PATH" {
            merge_path(launch, &value, inherited);
            continue;
        }
        let set_here = launch.env.iter().any(|(set, _)| *set == name)
            || launch.secret_env.iter().any(|(set, _)| *set == name)
            || launch.env_remove.contains(&name);
        if set_here || left_out(&name) {
            continue;
        }
        // The pipe carries text. A value that isn't is left out by name only.
        match value.into_string() {
            Ok(value) => launch.secret_env.push((name, Secret::new(value))),
            Err(_) => warn!(?name, "a login shell's variable isn't text"),
        }
    }
}

/// Sets `PATH` to the shell's folders in its order, as in a terminal, then any
/// others the CLI would have had, so nothing it could find before goes
/// missing.
fn merge_path(launch: &mut Launch, login: &OsStr, inherited: Option<&OsStr>) {
    let set = launch.env.iter().position(|(name, _)| name == "PATH");
    let had = set.map_or(inherited, |at| Some(launch.env[at].1.as_os_str()));
    let mut folders: Vec<PathBuf> = Vec::new();
    for folder in env::split_paths(login).chain(had.into_iter().flat_map(env::split_paths)) {
        if !folder.as_os_str().is_empty() && !folders.contains(&folder) {
            folders.push(folder);
        }
    }
    // Folders from a split can't hold the separator, so this can't fail.
    let Ok(path) = env::join_paths(folders) else {
        return;
    };
    match set {
        Some(at) => launch.env[at].1 = path,
        None => launch.env.push(("PATH".into(), path)),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use maxhq_protocol::ids::SessionId;

    use super::*;
    use crate::drive::{claude, codex, cursor};
    use crate::testing::TempDir;

    fn launch() -> Launch {
        Launch {
            session: SessionId::generate(),
            program: PathBuf::from("/usr/local/bin/claude"),
            args: Vec::new(),
            env: vec![("CLAUDE_CONFIG_DIR".into(), "/home/agent/.claude-1".into())],
            env_remove: vec!["XDG_RUNTIME_DIR".into()],
            secret_env: Vec::new(),
            run_as: None,
            cwd: PathBuf::from("/home/agent/work"),
            framing: maxhq_hold::Framing::ClaudeStreamJson,
        }
    }

    fn found(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).into(), (*value).into()))
            .collect()
    }

    fn secret<'a>(launch: &'a Launch, name: &str) -> Option<&'a str> {
        launch
            .secret_env
            .iter()
            .find(|(set, _)| set == name)
            .map(|(_, value)| value.expose())
    }

    fn set<'a>(launch: &'a Launch, name: &str) -> Option<&'a OsStr> {
        launch
            .env
            .iter()
            .find(|(set, _)| set == name)
            .map(|(_, value)| value.as_os_str())
    }

    /// `script` run as a shell's stand-in, with only the variables it sets.
    /// The tests' own PATH, since NixOS has no `sleep` on the fixed one, and
    /// a script that can't find it ends at once instead of hanging.
    fn sh(script: &str) -> std::process::Command {
        let mut process = std::process::Command::new("sh");
        process
            .args(["-c", script])
            .env_clear()
            .env("PATH", env::var_os("PATH").unwrap());
        process
    }

    #[test]
    fn a_profile_can_print_before_the_answer() {
        let answer = b"Welcome!\nA=not this\n\0MAXHQ-LOGIN-ENV\0JAVA_HOME=/opt/jdk\0TWO=line\nline\0EMPTY=\0MAXHQ-LOGIN-ENV-END\0";
        assert_eq!(
            parse(answer),
            Some(found(&[
                ("JAVA_HOME", "/opt/jdk"),
                ("TWO", "line\nline"),
                ("EMPTY", ""),
            ])),
        );
    }

    #[test]
    fn an_answer_counts_once_its_end_has_arrived() {
        assert_eq!(parse(b"Welcome!"), None);
        assert_eq!(parse(b"\0MAXHQ-LOGIN-ENV\0A=1\0"), None);
    }

    #[test]
    fn the_settings_go_on_the_secret_pipe_and_stay_off_disk() {
        let mut launch = launch();
        merge(
            &mut launch,
            found(&[("JAVA_HOME", "/opt/jdk"), ("SOME_KEY", "value")]),
            None,
        );
        assert_eq!(secret(&launch, "JAVA_HOME"), Some("/opt/jdk"));
        assert_eq!(secret(&launch, "SOME_KEY"), Some("value"));
        assert_eq!(set(&launch, "JAVA_HOME"), None);
        assert_eq!(set(&launch, "SOME_KEY"), None);
    }

    #[test]
    fn what_a_launch_sets_or_takes_out_stands() {
        let mut launch = launch();
        launch
            .secret_env
            .push(("HELD".into(), Secret::new("held".to_owned())));
        merge(
            &mut launch,
            found(&[
                ("CLAUDE_CONFIG_DIR", "/home/agent/.claude"),
                ("HELD", "other"),
                ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ]),
            None,
        );
        assert_eq!(
            set(&launch, "CLAUDE_CONFIG_DIR"),
            Some(OsStr::new("/home/agent/.claude-1")),
        );
        assert_eq!(secret(&launch, "HELD"), Some("held"));
        assert_eq!(secret(&launch, "XDG_RUNTIME_DIR"), None);
    }

    #[test]
    fn no_sign_in_comes_from_a_profile() {
        let mut names: Vec<String> = vec![
            claude::OAUTH_TOKEN.to_owned(),
            cursor::API_KEY.to_owned(),
            "GH_TOKEN".to_owned(),
            "GITHUB_TOKEN".to_owned(),
            "MAXHQ_AGENT".to_owned(),
            "NODE_OPTIONS".to_owned(),
        ];
        names.extend(codex::OTHER_AUTH.map(str::to_owned));
        names.extend(
            claude::STRIPPED_PREFIXES
                .iter()
                .map(|prefix| format!("{prefix}_API_KEY")),
        );
        let pairs: Vec<(OsString, OsString)> = names
            .iter()
            .map(|name| (name.into(), "from a profile".into()))
            .collect();
        let mut launch = launch();
        merge(&mut launch, pairs, None);
        assert!(launch.secret_env.is_empty(), "{names:?}");
    }

    #[test]
    fn a_shells_own_variables_stay_out() {
        let mut launch = launch();
        merge(
            &mut launch,
            found(&[
                ("HOME", "/root"),
                ("PWD", "/"),
                ("SHLVL", "1"),
                ("_", "env"),
            ]),
            None,
        );
        assert!(launch.secret_env.is_empty());
    }

    #[test]
    fn a_value_that_isnt_text_stays_out() {
        let mut launch = launch();
        let value = OsString::from_vec(vec![0xff, 0xfe]);
        merge(&mut launch, vec![("ODD".into(), value)], None);
        assert!(launch.secret_env.is_empty());
    }

    #[test]
    fn the_shells_folders_come_first_and_none_go_missing() {
        let mut launch = launch();
        launch.env.push((
            "PATH".into(),
            "/home/agent/.local/bin:/usr/local/bin:/usr/bin".into(),
        ));
        merge(
            &mut launch,
            found(&[("PATH", "/home/agent/.cargo/bin:/usr/bin::/bin")]),
            Some(OsStr::new("/inherited")),
        );
        assert_eq!(
            set(&launch, "PATH"),
            Some(OsStr::new(
                "/home/agent/.cargo/bin:/usr/bin:/bin:/home/agent/.local/bin:/usr/local/bin"
            )),
        );
    }

    #[test]
    fn a_launch_that_sets_no_path_keeps_the_inherited_folders() {
        let mut launch = launch();
        merge(
            &mut launch,
            found(&[("PATH", "/home/agent/.cargo/bin:/usr/bin")]),
            Some(OsStr::new("/usr/bin:/opt/inherited")),
        );
        assert_eq!(
            set(&launch, "PATH"),
            Some(OsStr::new("/home/agent/.cargo/bin:/usr/bin:/opt/inherited")),
        );
    }

    #[tokio::test]
    async fn a_launch_with_no_user_is_left_alone() {
        let before = launch();
        let mut changed = before.clone();
        add(&mut changed).await;
        assert_eq!(changed, before);
    }

    /// The setup Debian's files give: `.profile` runs `.bashrc`, which stops
    /// early unless the shell is interactive.
    #[tokio::test]
    async fn a_login_shell_gets_past_the_early_return_in_bashrc() {
        let home = TempDir::new();
        fs::write(
            home.path().join(".profile"),
            "export FROM_PROFILE=profile\n. \"$HOME/.bashrc\"\necho printed on stdout\n",
        )
        .unwrap();
        fs::write(
            home.path().join(".bashrc"),
            "case $- in *i*) ;; *) return ;; esac\nexport FROM_BASHRC=bashrc\nexport PATH=\"$HOME/.cargo/bin:$PATH\"\n",
        )
        .unwrap();
        let mut process = std::process::Command::new("bash");
        process
            .args(["-ilc", ASK])
            .env_clear()
            .env("HOME", home.path())
            .env("PATH", env::var_os("PATH").unwrap());

        let found = answer(process, Duration::from_secs(30)).await.unwrap();

        let value = |name: &str| {
            found
                .iter()
                .find(|(set, _)| set == name)
                .map(|(_, value)| value.clone())
        };
        assert_eq!(value("FROM_PROFILE"), Some("profile".into()));
        assert_eq!(value("FROM_BASHRC"), Some("bashrc".into()));
        let cargo = home.path().join(".cargo/bin");
        assert!(env::split_paths(&value("PATH").unwrap()).any(|folder| folder == cargo));
    }

    #[tokio::test]
    async fn a_job_left_holding_stdout_doesnt_hold_the_launch() {
        let script = format!("sleep 60 & {ASK}");
        let found = answer(sh(&script), Duration::from_secs(30)).await.unwrap();
        assert!(found.iter().any(|(name, _)| name == "PATH"));
    }

    #[tokio::test]
    async fn a_shell_that_hangs_runs_out_of_time() {
        let error = answer(sh("sleep 60"), Duration::from_millis(100))
            .await
            .unwrap_err();
        assert!(error.contains("took more than"), "{error}");
    }

    #[tokio::test]
    async fn a_shell_that_ends_without_answering_is_a_failure() {
        let error = answer(sh("echo no"), Duration::from_secs(30))
            .await
            .unwrap_err();
        assert!(error.contains("without answering"), "{error}");
    }

    #[tokio::test]
    async fn a_shell_that_wont_start_is_a_failure() {
        let process = std::process::Command::new("/nonexistent/shell");
        let error = answer(process, Duration::from_secs(30)).await.unwrap_err();
        assert!(error.contains("didn't start"), "{error}");
    }

    #[tokio::test]
    async fn an_answer_too_long_is_a_failure() {
        let script = "printf '\\0MAXHQ-LOGIN-ENV\\0'; while :; do printf 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'; done";
        let error = answer(sh(script), Duration::from_secs(30))
            .await
            .unwrap_err();
        assert!(error.contains("too long"), "{error}");
    }
}
