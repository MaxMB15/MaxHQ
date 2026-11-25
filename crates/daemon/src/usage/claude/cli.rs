// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Runs `claude` to learn who a config folder's login is and how much of
//! its limits it has used. Each run is a short child process that `maxd`
//! starts, reads and waits for. None of them sends a prompt, and none of
//! them touches a running session.

use std::collections::BTreeMap;
use std::ffi::OsStr;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use maxhq_protocol::model::{Usage, UsageSource};
use maxhq_protocol::time::Timestamp;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::Instant;

use super::windows::windows;
use crate::drive::claude::entrypoint;
use crate::drive::claude::find_claude;
use crate::drive::programs;
use crate::observe::claude::config::ConfigDir;

/// How long a run of `claude` gets before it's stopped.
pub const TIMEOUT: Duration = Duration::from_secs(15);

/// The folder under `maxd`'s data folder that `claude` runs in.
const PROBE_FOLDER: &str = "usage-probe";

/// The probe's flags. Session persistence is off, so the probe leaves no
/// transcript, and hooks and MCP servers stay off, so it starts nothing else.
/// Only the user's own settings apply, since project settings can name a
/// command for `claude` to run or a host to send the login to.
const PROBE_ARGS: [&str; 11] = [
    "--output-format",
    "stream-json",
    "--verbose",
    "--input-format",
    "stream-json",
    "--setting-sources",
    "user",
    "--strict-mcp-config",
    "--no-session-persistence",
    "--settings",
    r#"{"disableAllHooks":true}"#,
];

/// What the probe's environment adds, so `claude` never reaches for an
/// IDE or a claude.ai MCP server. `Cli::command` names the app running it,
/// for every run.
const PROBE_ENV: [(&str, &str); 3] = [
    ("ENABLE_CLAUDEAI_MCP_SERVERS", "false"),
    ("CLAUDE_CODE_AUTO_CONNECT_IDE", "0"),
    ("CLAUDE_CODE_IDE_SKIP_AUTO_INSTALL", "1"),
];

/// The ID of the probe's one request.
const REQUEST_ID: &str = "maxd-usage-1";

/// The one line the probe writes to `claude`. It asks for usage and
/// nothing else.
const REQUEST: &str = concat!(
    r#"{"request_id":"maxd-usage-1","type":"control_request","#,
    r#""request":{"subtype":"get_usage","skip_behaviors":true}}"#,
    "\n"
);

/// The longest failure reason kept, in characters.
const REASON_CHARS: usize = 200;

/// How long the PID of a `claude` that `maxd` ran stays known after it
/// exits. A pass that saw the process just before it ended still leaves it
/// out.
const REMEMBER: Duration = Duration::from_secs(5);

/// The PIDs of the `claude` processes `maxd` runs, each with when it
/// exited, or none while it runs.
static STARTED: Mutex<BTreeMap<u32, Option<std::time::Instant>>> = Mutex::new(BTreeMap::new());

/// Whether `pid` is a `claude` process `maxd` started. The probe registers
/// a session file like any other `claude` process, and the observer uses
/// this to leave it out.
pub fn is_probe(pid: u32) -> bool {
    STARTED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&pid)
        .is_some_and(|exited| remembered(*exited))
}

/// Whether a PID that exited at `exited` is still known.
fn remembered(exited: Option<std::time::Instant>) -> bool {
    exited.is_none_or(|at| at.elapsed() < REMEMBER)
}

/// Holds a PID in [`STARTED`] as running until it's dropped.
struct Registered(u32);

impl Registered {
    /// Adds `pid` to [`STARTED`] and forgets PIDs that exited a while ago.
    fn new(pid: u32) -> Self {
        let mut started = STARTED.lock().unwrap_or_else(PoisonError::into_inner);
        started.retain(|_, exited| remembered(*exited));
        started.insert(pid, None);
        Self(pid)
    }
}

impl Drop for Registered {
    fn drop(&mut self) {
        STARTED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(self.0, Some(std::time::Instant::now()));
    }
}

/// Who a config folder's login is, as `claude auth status` says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Auth {
    /// Logged in.
    LoggedIn(Identity),
    /// Nobody is logged in.
    LoggedOut,
}

/// The parts of a login an account shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    /// The login's email.
    pub email: Option<String>,
    /// The organization's name.
    pub organization: Option<String>,
    /// The plan, such as "max".
    pub plan: Option<String>,
}

/// The `claude` program and how long each run of it gets.
#[derive(Clone, Debug)]
pub struct Cli {
    /// The program to run.
    program: PathBuf,
    /// How long a run gets before it's stopped.
    timeout: Duration,
    /// The private folder each run starts in.
    folder: PathBuf,
}

impl Cli {
    /// Runs `program` from `folder`, stopping each run after `timeout`.
    pub fn new(program: PathBuf, timeout: Duration, folder: PathBuf) -> Self {
        Self {
            program,
            timeout,
            folder,
        }
    }

    /// The `claude` on this device, if there is one.
    pub fn find() -> Option<Self> {
        let home = maxhq_os::home::home_dir();
        let user = std::env::var_os("USER");
        let path = std::env::var_os("PATH");
        let program = find_claude(path.as_deref(), home.as_deref(), user.as_deref())?;
        let folder = maxhq_core::paths::data_dir()?.join(PROBE_FOLDER);
        Some(Self::new(program, TIMEOUT, folder))
    }

    /// Who the login in `config` is.
    pub async fn auth_status(&self, config: &ConfigDir) -> Result<Auth, String> {
        let mut command = self.command(config);
        private_folder(&self.folder)?;
        command
            .current_dir(&self.folder)
            .args(["auth", "status", "--json"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let child = command
            .spawn()
            .map_err(|error| format!("couldn't start claude: {error}"))?;
        let _running = child.id().map(Registered::new);
        // Dropping the child on a timeout stops it.
        let output = tokio::time::timeout(self.timeout, child.wait_with_output())
            .await
            .map_err(|_| "claude auth status took too long".to_owned())?
            .map_err(|error| format!("claude auth status failed: {error}"))?;
        parse_auth(&output.stdout)
    }

    /// Asks `claude` for the usage of the login in `config`.
    pub async fn probe(&self, config: &ConfigDir) -> Result<Usage, String> {
        let mut command = self.command(config);
        private_folder(&self.folder)?;
        command
            .args(PROBE_ARGS)
            .envs(PROBE_ENV)
            .current_dir(&self.folder)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command
            .spawn()
            .map_err(|error| format!("couldn't start claude: {error}"))?;
        let _running = child.id().map(Registered::new);
        let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err("couldn't talk to claude".to_owned());
        };
        let mut lines = BufReader::new(stdout).lines();
        let deadline = Instant::now() + self.timeout;

        let ask = async {
            stdin
                .write_all(REQUEST.as_bytes())
                .await
                .and(stdin.flush().await)
                .map_err(|error| format!("couldn't write to claude: {error}"))?;
            while let Some(line) = lines
                .next_line()
                .await
                .map_err(|error| format!("couldn't read from claude: {error}"))?
            {
                if let Some(reply) = read_reply(&line, Timestamp::now()) {
                    return reply;
                }
            }
            Err("claude exited without answering the usage request".to_owned())
        };
        let reply = tokio::time::timeout_at(deadline, ask)
            .await
            .unwrap_or_else(|_| Err("claude didn't answer the usage request in time".to_owned()));

        // Closing stdin tells `claude` to exit. Reading on until it does
        // means it never writes into a closed pipe and can clean up its
        // session file.
        drop(stdin);
        let exit = async {
            while let Ok(Some(_)) = lines.next_line().await {}
            child.wait().await
        };
        if tokio::time::timeout_at(deadline, exit).await.is_err() {
            tracing::debug!("the Claude usage probe didn't exit in time, so it was stopped");
            let _ = child.kill().await;
        }
        reply
    }

    /// A command that runs `claude` for `config` with nothing inherited
    /// that could point it at another login or another host, and with
    /// MaxHQ named as the app that runs it. Dropping it stops the process
    /// it started.
    fn command(&self, config: &ConfigDir) -> Command {
        // On Windows an npm shim starts what it names, without cmd.exe.
        let found = maxhq_os::which::launch(&self.program);
        let mut command = Command::new(&found.program);
        command.args(&found.args);
        for (key, _) in std::env::vars_os() {
            if inherited(&key) {
                command.env_remove(&key);
            }
        }
        if let Some(dir) = config.env_value() {
            command.env("CLAUDE_CONFIG_DIR", dir);
        }
        command.env(entrypoint::VAR, entrypoint::label(&config.path));
        // A `claude` that npm installed is a script that needs `node`,
        // which usually sits beside it.
        if let Some(dir) = self.program.parent() {
            let path = std::env::var_os("PATH");
            command.env("PATH", programs::path_with_first(dir, path.as_deref()));
        }
        command.kill_on_drop(true);
        command
    }
}

/// Whether an environment variable `maxd` has should be kept from
/// `claude`. Any `CLAUDE` or `ANTHROPIC` variable could change the login,
/// the API host, the app `claude` says runs it, or how it behaves.
fn inherited(key: &OsStr) -> bool {
    let key = key.to_string_lossy();
    key.starts_with("CLAUDE") || key.starts_with("ANTHROPIC")
}

/// Makes `dir` for `claude` to run in, and checks that only this user can
/// reach it, so nobody else can plant files for `claude` to read there. A
/// link or a folder another user owns is refused, and one that's open to
/// others is closed.
#[cfg(unix)]
fn private_folder(dir: &Path) -> Result<(), String> {
    let failed = |error: std::io::Error| format!("couldn't use {}: {error}", dir.display());
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(failed)?;
    let meta = std::fs::symlink_metadata(dir).map_err(failed)?;
    if !meta.is_dir() {
        return Err(format!("{} isn't a real folder", dir.display()));
    }
    if meta.uid() != rustix::process::geteuid().as_raw() {
        return Err(format!("{} belongs to another user", dir.display()));
    }
    if meta.mode() & 0o777 != 0o700 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(failed)?;
    }
    Ok(())
}

/// Makes `dir` for `claude` to run in. Windows keeps a folder in the
/// user's profile to that user, so only a link in its place is refused.
#[cfg(not(unix))]
fn private_folder(dir: &Path) -> Result<(), String> {
    let failed = |error: std::io::Error| format!("couldn't use {}: {error}", dir.display());
    maxhq_os::fsmode::private_dir_all(dir).map_err(failed)?;
    let meta = std::fs::symlink_metadata(dir).map_err(failed)?;
    if !meta.is_dir() {
        return Err(format!("{} isn't a real folder", dir.display()));
    }
    Ok(())
}

/// Reads what `claude auth status --json` prints. It exits 1 when logged
/// out, so only the JSON says how things stand.
pub fn parse_auth(stdout: &[u8]) -> Result<Auth, String> {
    let start = stdout.iter().position(|&byte| byte == b'{');
    let status: Value = start
        .and_then(|start| serde_json::from_slice(&stdout[start..]).ok())
        .ok_or_else(|| "claude auth status printed no JSON".to_owned())?;
    let text = |key: &str| {
        status
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    match status.get("loggedIn").and_then(Value::as_bool) {
        Some(true) => Ok(Auth::LoggedIn(Identity {
            email: text("email"),
            organization: text("orgName"),
            plan: text("subscriptionType"),
        })),
        Some(false) => Ok(Auth::LoggedOut),
        None => Err("claude auth status didn't say whether anyone is logged in".to_owned()),
    }
}

/// What one line of the probe's output says: nothing when it isn't the
/// answer to the probe's request, and the usage or why there's none when it
/// is. `now` becomes the reading's time.
pub fn read_reply(line: &str, now: Timestamp) -> Option<Result<Usage, String>> {
    let message: Value = serde_json::from_str(line).ok()?;
    if message.get("type")?.as_str()? != "control_response" {
        return None;
    }
    let response = message.get("response")?;
    if response.get("request_id")?.as_str()? != REQUEST_ID {
        return None;
    }
    if response.get("subtype").and_then(Value::as_str) != Some("success") {
        let error = response
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("no reason given");
        return Some(Err(shorten(&format!(
            "claude refused the usage request: {error}"
        ))));
    }
    let Some(usage) = response.get("response") else {
        return Some(Err("claude's usage reply was empty".to_owned()));
    };
    if usage.get("rate_limits_available").and_then(Value::as_bool) == Some(false) {
        return Some(Err("this login has no rate limits to read".to_owned()));
    }
    let windows = windows(usage.get("rate_limits").unwrap_or(&Value::Null));
    if windows.is_empty() {
        return Some(Err("claude's usage reply had no limits".to_owned()));
    }
    Some(Ok(Usage {
        checked_at: now,
        source: UsageSource::ClaudeProbe,
        windows,
        reset_credits: None,
    }))
}

/// `reason` cut to [`REASON_CHARS`] characters.
fn shorten(reason: &str) -> String {
    match reason.char_indices().nth(REASON_CHARS) {
        Some((end, _)) => format!("{}...", &reason[..end]),
        None => reason.to_owned(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::ffi::OsString;
    use std::fs;

    use super::*;
    use crate::observe::claude::tests::{fixture, scratch, Scratch};

    /// The probe's reply in the fixture, which came from a real one.
    fn reply() -> String {
        fs::read_to_string(fixture("cli/probe-reply.json")).unwrap()
    }

    /// A `claude` in the folder `bin` that prints the fixture file `auth`
    /// for `auth status` and runs `probe` as a shell script for anything
    /// else. The script sees `$REPLY_FILE`, the fixture's probe reply.
    pub(crate) fn fake_claude(bin: &Path, auth: &str, probe: &str) -> Cli {
        fs::create_dir_all(bin).unwrap();
        let program = bin.join("claude");
        // The real one exits 1 when logged out.
        let status = if auth.contains("logged-out") { 1 } else { 0 };
        let script = format!(
            "#!/bin/sh\n\
             REPLY_FILE='{reply}'\n\
             if [ \"$1\" = auth ]; then\n\
             cat '{auth}'\n\
             exit {status}\n\
             fi\n\
             {probe}\n",
            reply = fixture("cli/probe-reply.json").display(),
            auth = fixture(auth).display(),
        );
        fs::write(&program, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        }
        Cli::new(program, Duration::from_secs(5), bin.join("probe-folder"))
    }

    /// A probe script that answers like `claude` does, after checking it
    /// was run the way the probe runs it.
    pub(crate) const ANSWERS: &str = r#"
[ "$CLAUDE_CODE_ENTRYPOINT" = maxhq ] || exit 3
[ -n "$CLAUDE_CONFIG_DIR" ] || exit 4
case " $* " in *" --no-session-persistence "*) ;; *) exit 5 ;; esac
case " $* " in *" --setting-sources user "*) ;; *) exit 7 ;; esac
[ "$(basename "$PWD")" = probe-folder ] || exit 8
read -r request
case "$request" in *'"subtype":"get_usage"'*) ;; *) exit 6 ;; esac
echo '{"type":"system","subtype":"status","status":null}'
echo '{"type":"control_response","response":{"subtype":"success","request_id":"someone-else","response":{}}}'
cat "$REPLY_FILE"
cat > /dev/null
"#;

    /// A probe script that dies before it answers.
    pub(crate) const FAILS: &str = "read -r request\nexit 1";

    fn config(dir: &Scratch) -> ConfigDir {
        ConfigDir::custom(dir.join("claude"))
    }

    #[test]
    fn reads_the_usage_reply() {
        let now = Timestamp(1_792_890_000_000);
        let usage = read_reply(reply().trim(), now).unwrap().unwrap();
        assert_eq!(usage.checked_at, now);
        assert_eq!(usage.source, UsageSource::ClaudeProbe);
        let summary: Vec<(Option<&str>, u32, f64)> = usage
            .windows
            .iter()
            .map(|w| (w.scope.as_deref(), w.window_minutes, w.used_percent))
            .collect();
        assert_eq!(summary, [(None, 300, 22.0), (None, 10_080, 45.0)]);
    }

    #[test]
    fn other_lines_are_not_the_reply() {
        let now = Timestamp(1);
        assert_eq!(read_reply("", now), None);
        assert_eq!(read_reply("not json", now), None);
        assert_eq!(
            read_reply(r#"{"type":"system","subtype":"init"}"#, now),
            None
        );
        let other = reply().replace(REQUEST_ID, "another-request");
        assert_eq!(read_reply(&other, now), None);
    }

    #[test]
    fn a_reply_without_limits_is_a_failure() {
        let now = Timestamp(1);
        let refused = r#"{"type":"control_response","response":{"subtype":"error","request_id":"maxd-usage-1","error":"Unknown request"}}"#;
        assert_eq!(
            read_reply(refused, now),
            Some(Err(
                "claude refused the usage request: Unknown request".to_owned()
            ))
        );
        let unavailable = reply().replace(
            "\"rate_limits_available\":true",
            "\"rate_limits_available\":false",
        );
        assert_eq!(
            read_reply(&unavailable, now),
            Some(Err("this login has no rate limits to read".to_owned()))
        );
        let empty = r#"{"type":"control_response","response":{"subtype":"success","request_id":"maxd-usage-1","response":{"rate_limits":{}}}}"#;
        assert!(matches!(read_reply(empty, now), Some(Err(_))));
    }

    #[test]
    fn reads_the_login() {
        let status = fs::read(fixture("cli/auth-status.json")).unwrap();
        assert_eq!(
            parse_auth(&status),
            Ok(Auth::LoggedIn(Identity {
                email: Some("max@example.com".into()),
                organization: Some("Example Org".into()),
                plan: Some("max".into()),
            }))
        );
        let logged_out = fs::read(fixture("cli/auth-logged-out.json")).unwrap();
        assert_eq!(parse_auth(&logged_out), Ok(Auth::LoggedOut));
        assert!(parse_auth(b"").is_err());
        assert!(parse_auth(br#"{"email":"max@example.com"}"#).is_err());
    }

    #[test]
    fn keeps_claude_and_anthropic_settings_from_claude() {
        assert!(inherited(OsStr::new("CLAUDE_CONFIG_DIR")));
        assert!(inherited(OsStr::new("CLAUDECODE")));
        assert!(inherited(OsStr::new("ANTHROPIC_BASE_URL")));
        assert!(!inherited(OsStr::new("HOME")));
        assert!(!inherited(OsStr::new("PATH")));
    }

    #[test]
    fn the_default_folder_needs_no_config_dir() {
        let cli = Cli::new(
            PathBuf::from("/opt/claude/bin/claude"),
            TIMEOUT,
            PathBuf::from("/nowhere"),
        );
        let env = |config: &ConfigDir| -> Vec<(OsString, Option<OsString>)> {
            cli.command(config)
                .as_std()
                .get_envs()
                .map(|(key, value)| (key.to_owned(), value.map(OsStr::to_owned)))
                .collect()
        };
        let custom = env(&ConfigDir::custom(PathBuf::from("/srv/claude-work")));
        assert!(custom.contains(&("CLAUDE_CONFIG_DIR".into(), Some("/srv/claude-work".into()))));
        let default = env(&ConfigDir::default_in(Path::new("/home/dev")));
        assert!(default
            .iter()
            .all(|(key, value)| key != "CLAUDE_CONFIG_DIR" || value.is_none()));
        let path = default
            .iter()
            .find(|(key, _)| key == "PATH")
            .and_then(|(_, value)| value.clone())
            .unwrap();
        assert!(path.to_string_lossy().starts_with("/opt/claude/bin"));
    }

    #[test]
    fn every_run_names_maxhq_or_the_logins_choice_as_the_entrypoint() {
        let dir = scratch();
        let cli = Cli::new(
            PathBuf::from("/opt/claude/bin/claude"),
            TIMEOUT,
            PathBuf::from("/nowhere"),
        );
        // What a run sets for the variable. Set after the inherited ones
        // are removed, so another app's name never reaches `claude`.
        let entrypoint = |config: &ConfigDir| {
            cli.command(config)
                .as_std()
                .get_envs()
                .filter(|(key, _)| *key == "CLAUDE_CODE_ENTRYPOINT")
                .map(|(_, value)| value.map(OsStr::to_owned))
                .collect::<Vec<_>>()
        };
        assert_eq!(entrypoint(&config(&dir)), [Some("maxhq".into())]);

        fs::create_dir_all(dir.join("claude")).unwrap();
        fs::write(
            dir.join("claude/settings.json"),
            r#"{"env": {"CLAUDE_CODE_ENTRYPOINT": "custom", "OTHER_KEY": "placeholder"}}"#,
        )
        .unwrap();
        assert_eq!(entrypoint(&config(&dir)), [Some("custom".into())]);
        assert!(PROBE_ENV
            .iter()
            .all(|(key, _)| *key != "CLAUDE_CODE_ENTRYPOINT"));
    }

    #[test]
    fn finds_claude_off_the_path() {
        let dir = scratch();
        let bin = dir.join("bin");
        let home = dir.join("home");
        fs::create_dir_all(&bin).unwrap();
        // Not runnable, so it's passed over.
        fs::write(bin.join("claude"), "").unwrap();
        let installed = fake_claude(&home.join(".local/bin"), "cli/auth-status.json", FAILS);
        let path = std::env::join_paths([dir.join("empty"), bin]).unwrap();
        assert_eq!(
            find_claude(Some(&path), Some(&home), Some(OsStr::new("max"))),
            Some(installed.program.clone())
        );
        let first = std::env::join_paths([home.join(".local/bin")]).unwrap();
        assert_eq!(
            find_claude(Some(&first), None, None),
            Some(installed.program)
        );
    }

    #[test]
    fn shortens_long_reasons() {
        assert_eq!(shorten("short"), "short");
        let long = "é".repeat(REASON_CHARS + 10);
        assert_eq!(shorten(&long).chars().count(), REASON_CHARS + 3);
    }

    #[test]
    fn a_probe_pid_is_known_while_it_runs_and_a_moment_after() {
        assert!(!is_probe(4_200_001));
        let running = Registered::new(4_200_001);
        assert!(is_probe(4_200_001));
        drop(running);
        assert!(is_probe(4_200_001), "it just exited");

        let long_ago = std::time::Instant::now().checked_sub(REMEMBER).unwrap();
        STARTED.lock().unwrap().insert(4_200_001, Some(long_ago));
        assert!(!is_probe(4_200_001));
        let _other = Registered::new(4_200_002);
        assert!(!STARTED.lock().unwrap().contains_key(&4_200_001));
    }

    #[tokio::test]
    async fn the_probe_asks_and_reads_the_answer() {
        let dir = scratch();
        let cli = fake_claude(&dir.join("bin"), "cli/auth-status.json", ANSWERS);
        let before = Timestamp::now();
        let usage = cli.probe(&config(&dir)).await.unwrap();
        assert!(usage.checked_at >= before);
        assert_eq!(usage.source, UsageSource::ClaudeProbe);
        assert_eq!(usage.windows.len(), 2);
        assert_eq!(
            cli.auth_status(&config(&dir)).await.unwrap(),
            parse_auth(&fs::read(fixture("cli/auth-status.json")).unwrap()).unwrap()
        );
    }

    #[tokio::test]
    async fn a_probe_that_dies_or_hangs_fails() {
        let dir = scratch();
        let dies = fake_claude(&dir.join("bin"), "cli/auth-status.json", FAILS);
        assert_eq!(
            dies.probe(&config(&dir)).await,
            Err("claude exited without answering the usage request".to_owned())
        );

        let mut hangs = fake_claude(
            &dir.join("hangs"),
            "cli/auth-status.json",
            "read -r request\nexec sleep 30",
        );
        hangs.timeout = Duration::from_millis(500);
        let started = std::time::Instant::now();
        assert_eq!(
            hangs.probe(&config(&dir)).await,
            Err("claude didn't answer the usage request in time".to_owned())
        );
        assert!(started.elapsed() < Duration::from_secs(5));

        let missing = Cli::new(dir.join("nothing-here"), TIMEOUT, dir.join("probe-folder"));
        assert!(missing.probe(&config(&dir)).await.is_err());
    }

    /// A `claude` that leaves a mark in `dir` as soon as it runs, for
    /// `auth status` or the probe alike.
    #[cfg(unix)]
    fn marking_claude(dir: &Path) -> (Cli, PathBuf) {
        fs::create_dir_all(dir).unwrap();
        let mark = dir.join("ran");
        let program = dir.join("claude");
        fs::write(
            &program,
            format!("#!/bin/sh\ntouch '{}'\nexit 1\n", mark.display()),
        )
        .unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        let cli = Cli::new(program, Duration::from_secs(5), dir.join("probe-folder"));
        (cli, mark)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn claude_never_runs_from_a_folder_someone_planted() {
        let dir = scratch();
        let (mut cli, mark) = marking_claude(&dir.join("bin"));

        // A link to a folder holding someone's settings.
        let planted = dir.join("planted");
        fs::create_dir_all(planted.join(".claude")).unwrap();
        fs::write(
            planted.join(".claude/settings.json"),
            r#"{"apiKeyHelper": "touch ran"}"#,
        )
        .unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&planted, &link).unwrap();
        cli.folder = link.clone();
        let refused = cli.probe(&config(&dir)).await.unwrap_err();
        assert!(refused.contains("isn't a real folder"), "{refused}");
        assert!(cli.auth_status(&config(&dir)).await.is_err());

        // A link to nothing is no better.
        let dangling = dir.join("dangling");
        std::os::unix::fs::symlink(dir.join("not-there"), &dangling).unwrap();
        cli.folder = dangling;
        assert!(cli.probe(&config(&dir)).await.is_err());

        // A file in the folder's place.
        let file = dir.join("file");
        fs::write(&file, "").unwrap();
        cli.folder = file;
        assert!(cli.probe(&config(&dir)).await.is_err());

        // A folder another user owns. Root owns `/`, unless root runs this.
        if !rustix::process::geteuid().is_root() {
            cli.folder = PathBuf::from("/");
            let refused = cli.probe(&config(&dir)).await.unwrap_err();
            assert!(refused.contains("another user"), "{refused}");
        }

        assert!(!mark.exists(), "claude ran from a folder it shouldn't have");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_folder_claude_runs_in_is_private() {
        let dir = scratch();
        let (mut cli, mark) = marking_claude(&dir.join("bin"));
        let mode = |path: &Path| fs::symlink_metadata(path).unwrap().mode() & 0o777;

        // Made for the first run, inside a parent that doesn't exist yet.
        cli.folder = dir.join("data/usage-probe");
        let _ = cli.probe(&config(&dir)).await;
        assert_eq!(mode(&cli.folder), 0o700);
        assert!(mark.exists());

        // Closed again if something opened it.
        fs::set_permissions(&cli.folder, fs::Permissions::from_mode(0o777)).unwrap();
        let _ = cli.auth_status(&config(&dir)).await;
        assert_eq!(mode(&cli.folder), 0o700);
    }
}
