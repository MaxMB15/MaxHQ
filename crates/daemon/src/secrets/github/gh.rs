// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The `#` lookup's way in without the App: read-only `gh api` GETs, signed
//! in as whoever `gh` is signed in as. `maxd` never sees that sign-in. It
//! runs `gh` with no token in its environment and lets `gh` read its own,
//! so nothing here holds or logs one.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::process::Command;
use tokio::sync::Semaphore;

use super::api::{message, Fetched, API_VERSION};
use super::GithubError;
use crate::service::find_program;

/// How long one run may take before it's stopped.
const TIMEOUT: Duration = Duration::from_secs(10);
/// The most runs at once.
const RUNS: usize = 4;
/// What `gh` exits with when it has no sign-in.
const SIGNED_OUT: i32 = 4;
/// Variables that would sign `gh` in as someone else, point it at another
/// host, or make it print more than its reply.
const UNSET: [&str; 9] = [
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
    "GH_HOST",
    "GH_DEBUG",
    "DEBUG",
    "GH_FORCE_TTY",
    "CLICOLOR_FORCE",
];
/// Variables that keep `gh` from asking anything or printing more.
const SET: [(&str, &str); 3] = [
    ("GH_PROMPT_DISABLED", "1"),
    ("GH_NO_UPDATE_NOTIFIER", "1"),
    ("NO_COLOR", "1"),
];
/// How tokens start, so one in `gh`'s words is never passed on.
pub const TOKEN_STARTS: [&str; 6] = ["gho_", "ghp_", "ghu_", "ghs_", "ghr_", "github_pat_"];

/// `gh` on this device, run a few at a time.
pub struct Gh {
    runner: Runner,
    runs: Semaphore,
}

/// What runs `gh`.
enum Runner {
    /// The program itself, found again on each run so one installed later
    /// is used. A test names its own.
    Program {
        program: Option<PathBuf>,
        timeout: Duration,
    },
    /// A test's stand-in, which answers on the spot.
    #[cfg(test)]
    Fake(Answer),
}

/// How a test's stand-in answers a run's arguments.
#[cfg(test)]
type Answer = Box<dyn Fn(&[String]) -> Result<Ran, RunError> + Send + Sync>;

/// What a run printed and how it ended.
pub struct Ran {
    /// Its exit code, or none when a signal stopped it.
    pub code: Option<i32>,
    /// What it printed.
    pub stdout: Vec<u8>,
    /// What it printed as errors.
    pub stderr: Vec<u8>,
}

/// Why a run didn't finish.
pub enum RunError {
    /// There's no `gh` to run.
    Missing,
    /// It ran past the timeout and was stopped.
    TimedOut,
    /// It wouldn't start.
    Failed(String),
}

impl Gh {
    /// The `gh` installed on this device, found on `PATH` or in a usual
    /// install folder.
    pub fn installed() -> Self {
        Self::with(Runner::Program {
            program: None,
            timeout: TIMEOUT,
        })
    }

    /// The `gh` at `program`, stopped after `timeout`.
    #[cfg(test)]
    pub fn at(program: &Path, timeout: Duration) -> Self {
        Self::with(Runner::Program {
            program: Some(program.to_owned()),
            timeout,
        })
    }

    /// A stand-in that answers each run's arguments with `answer`.
    #[cfg(test)]
    pub fn fake(
        answer: impl Fn(&[String]) -> Result<Ran, RunError> + Send + Sync + 'static,
    ) -> Self {
        Self::with(Runner::Fake(Box::new(answer)))
    }

    fn with(runner: Runner) -> Self {
        Self {
            runner,
            runs: Semaphore::new(RUNS),
        }
    }

    /// `GET path` on github.com, asking only for a change since `etag`.
    pub async fn get_if_changed(
        &self,
        path: &str,
        etag: Option<&str>,
    ) -> Result<Fetched<Value>, GithubError> {
        let ran = self
            .run(&arguments(path, etag))
            .await
            .map_err(|err| match err {
                RunError::Missing => GithubError::NoGh,
                RunError::TimedOut => GithubError::GhFailed(format!(
                    "it didn't answer within {} seconds",
                    TIMEOUT.as_secs()
                )),
                RunError::Failed(reason) => GithubError::GhFailed(reason),
            })?;
        read(&ran)
    }

    async fn run(&self, args: &[String]) -> Result<Ran, RunError> {
        let _turn = self
            .runs
            .acquire()
            .await
            .map_err(|_| RunError::Failed("it stopped taking runs".to_owned()))?;
        match &self.runner {
            Runner::Program { program, timeout } => {
                let program = match program {
                    Some(program) => program.clone(),
                    None => find_program("gh").ok_or(RunError::Missing)?,
                };
                run(&program, args, *timeout).await
            }
            #[cfg(test)]
            Runner::Fake(answer) => answer(args),
        }
    }
}

/// The arguments for `GET path`.
fn arguments(path: &str, etag: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = [
        "api",
        "--hostname",
        "github.com",
        "--method",
        "GET",
        "--include",
        "-H",
        "Accept: application/vnd.github+json",
        "-H",
    ]
    .map(str::to_owned)
    .into();
    args.push(format!("X-GitHub-Api-Version: {API_VERSION}"));
    if let Some(etag) = etag {
        args.push("-H".to_owned());
        args.push(format!("If-None-Match: {etag}"));
    }
    args.push(path.to_owned());
    args
}

/// `program` with `args`, its environment cleared of tokens, and nothing
/// to read.
fn command(program: &Path, args: &[String]) -> Command {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for name in UNSET {
        command.env_remove(name);
    }
    for (name, value) in SET {
        command.env(name, value);
    }
    command
}

/// Runs `program` and waits for it, up to `timeout`.
async fn run(program: &Path, args: &[String], timeout: Duration) -> Result<Ran, RunError> {
    let child = command(program, args)
        .spawn()
        .map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => RunError::Missing,
            _ => RunError::Failed(format!("it wouldn't start: {err}")),
        })?;
    // Dropping the child on a timeout stops it.
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| RunError::TimedOut)?
        .map_err(|err| RunError::Failed(format!("its output couldn't be read: {err}")))?;
    Ok(Ran {
        code: output.status.code(),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

/// What a run says GitHub answered. `--include` puts the status and
/// headers first, and `gh` exits 1 for any status past 299 but still
/// prints them.
fn read(ran: &Ran) -> Result<Fetched<Value>, GithubError> {
    if ran.code == Some(SIGNED_OUT) {
        return Err(GithubError::GhSignedOut);
    }
    if let Some(reply) = Reply::split(&ran.stdout) {
        return match reply.status {
            304 => Ok(Fetched::Unchanged),
            200..=299 => serde_json::from_slice(reply.body)
                .map(|value| Fetched::Fresh {
                    value,
                    etag: reply.etag,
                })
                .map_err(|_| GithubError::BadReply),
            // The sign-in `gh` holds stopped working.
            401 => Err(GithubError::GhSignedOut),
            status => Err(GithubError::Refused {
                status,
                message: message(reply.body, Some(reply.reason)),
            }),
        };
    }
    if ran.code == Some(0) {
        return Err(GithubError::BadReply);
    }
    let said = String::from_utf8_lossy(&ran.stderr);
    if said.contains("gh auth login") {
        return Err(GithubError::GhSignedOut);
    }
    let said = said.lines().map(str::trim).find(|line| !line.is_empty());
    Err(GithubError::GhFailed(match said {
        Some(line) => hidden(line.strip_prefix("gh: ").unwrap_or(line)),
        None => match ran.code {
            Some(code) => format!("it exited with {code}"),
            None => "it was stopped".to_owned(),
        },
    }))
}

/// A reply as `--include` prints it.
struct Reply<'a> {
    status: u16,
    reason: &'a str,
    etag: Option<String>,
    body: &'a [u8],
}

impl<'a> Reply<'a> {
    /// Splits `out` into its status line, headers and body, or none when
    /// it doesn't start with a status line.
    fn split(out: &'a [u8]) -> Option<Self> {
        let (first, mut rest) = line(out)?;
        let mut words = first.strip_prefix("HTTP/")?.splitn(3, ' ');
        let status = words.nth(1)?.parse().ok()?;
        let reason = words.next().unwrap_or_default();
        let mut etag = None;
        loop {
            let (header, after) = line(rest)?;
            rest = after;
            if header.is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':')
                && name.trim().eq_ignore_ascii_case("etag")
            {
                etag = Some(value.trim().to_owned());
            }
        }
        Some(Self {
            status,
            reason,
            etag,
            body: rest,
        })
    }
}

/// The first line of `bytes` without its line end, and what follows it.
fn line(bytes: &[u8]) -> Option<(&str, &[u8])> {
    let end = bytes.iter().position(|&byte| byte == b'\n')?;
    let text = std::str::from_utf8(&bytes[..end]).ok()?;
    Some((text.strip_suffix('\r').unwrap_or(text), &bytes[end + 1..]))
}

/// `text` kept short, with any word that looks like a token taken out.
fn hidden(text: &str) -> String {
    let words: Vec<&str> = text
        .split(' ')
        .map(|word| {
            let bare = word.trim_start_matches(|c: char| !c.is_ascii_alphanumeric());
            if TOKEN_STARTS.iter().any(|start| bare.starts_with(start)) {
                "[token]"
            } else {
                word
            }
        })
        .collect();
    words.join(" ").chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::ffi::OsStr;
    use std::fs;
    use std::time::Instant;

    use super::*;
    use crate::testing::TempDir;

    fn ran(code: i32, stdout: &str, stderr: &str) -> Ran {
        Ran {
            code: Some(code),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    /// A `gh` that runs `body` as a shell script.
    fn script(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let program = dir.join("gh");
        fs::write(&program, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        program
    }

    #[test]
    fn a_reply_reads_with_its_etag() {
        let out = "HTTP/2.0 200 OK\nContent-Type: application/json\r\nEtag: W/\"abc\"\r\n\r\n[1,2]";
        match read(&ran(0, out, "")) {
            Ok(Fetched::Fresh { value, etag }) => {
                assert_eq!(value, serde_json::json!([1, 2]));
                assert_eq!(etag.as_deref(), Some("W/\"abc\""));
            }
            _ => panic!("expected a fresh reply"),
        }
    }

    #[test]
    fn a_status_past_299_reads_as_github_said() {
        let unchanged = "HTTP/2.0 304 Not Modified\nEtag: \"abc\"\r\n\r\n";
        assert!(matches!(
            read(&ran(1, unchanged, "gh: HTTP 304\n")),
            Ok(Fetched::Unchanged)
        ));
        let missing = "HTTP/2.0 404 Not Found\nContent-Type: application/json\r\n\r\n{\"message\":\"Not Found\"}";
        assert!(matches!(
            read(&ran(1, missing, "gh: Not Found (HTTP 404)\n")),
            Err(GithubError::Refused { status: 404, message }) if message == "Not Found"
        ));
        let bad = "HTTP/2.0 502 Bad Gateway\n\r\n<html>";
        assert!(matches!(
            read(&ran(1, bad, "")),
            Err(GithubError::Refused { status: 502, message }) if message == "Bad Gateway"
        ));
        let revoked = "HTTP/2.0 401 Unauthorized\n\r\n{\"message\":\"Bad credentials\"}";
        assert_eq!(
            read(&ran(1, revoked, "")).err(),
            Some(GithubError::GhSignedOut)
        );
    }

    #[test]
    fn no_sign_in_reads_as_signed_out() {
        let said = "To get started with GitHub CLI, please run:  gh auth login\n";
        assert_eq!(
            read(&ran(4, "", said)).err(),
            Some(GithubError::GhSignedOut)
        );
        assert_eq!(
            read(&ran(1, "", said)).err(),
            Some(GithubError::GhSignedOut)
        );
    }

    #[test]
    fn a_failure_says_gh_s_first_line_without_a_token() {
        let said = "error connecting to api.github.com\ncheck your internet connection\n";
        assert_eq!(
            read(&ran(1, "", said)).err(),
            Some(GithubError::GhFailed(
                "error connecting to api.github.com".to_owned()
            ))
        );
        let leaky = "gh: bad token 'ghp_abc123' for host\n";
        assert_eq!(
            read(&ran(1, "", leaky)).err(),
            Some(GithubError::GhFailed(
                "bad token [token] for host".to_owned()
            ))
        );
        assert_eq!(
            read(&ran(1, "", "")).err(),
            Some(GithubError::GhFailed("it exited with 1".to_owned()))
        );
        assert_eq!(read(&ran(0, "[]", "")).err(), Some(GithubError::BadReply));
    }

    #[test]
    fn a_run_carries_no_token_and_asks_nothing() {
        let command = command(Path::new("gh"), &arguments("/x", Some("\"e\"")));
        let envs: HashMap<&OsStr, Option<&OsStr>> = command.as_std().get_envs().collect();
        for name in ["GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN", "GH_HOST"] {
            assert_eq!(envs.get(OsStr::new(name)), Some(&None), "{name}");
        }
        for name in ["GH_PROMPT_DISABLED", "GH_NO_UPDATE_NOTIFIER"] {
            assert_eq!(
                envs.get(OsStr::new(name)),
                Some(&Some(OsStr::new("1"))),
                "{name}"
            );
        }
        let args: Vec<&OsStr> = command.as_std().get_args().collect();
        assert_eq!(args.last(), Some(&OsStr::new("/x")));
        assert!(args.contains(&OsStr::new("If-None-Match: \"e\"")));
        assert!(args.contains(&OsStr::new("--include")));
    }

    #[tokio::test]
    async fn the_program_runs_with_its_arguments_and_settings() {
        let temp = TempDir::new();
        let program = script(
            temp.path(),
            r#"printf 'HTTP/2.0 200 OK\nEtag: "e1"\r\n\r\n'
printf '{"args":"%s","token":"%s","prompt":"%s","notifier":"%s"}' "$*" "${GH_TOKEN-none}" "$GH_PROMPT_DISABLED" "$GH_NO_UPDATE_NOTIFIER""#,
        );
        let gh = Gh::at(&program, Duration::from_secs(5));
        let Ok(Fetched::Fresh { value, etag }) = gh.get_if_changed("/repos/o/r/pulls", None).await
        else {
            panic!("expected a fresh reply");
        };
        assert_eq!(etag.as_deref(), Some("\"e1\""));
        assert_eq!(
            value["args"],
            format!(
                "api --hostname github.com --method GET --include -H Accept: application/vnd.github+json -H X-GitHub-Api-Version: {API_VERSION} /repos/o/r/pulls"
            )
        );
        assert_eq!(value["token"], "none");
        assert_eq!(value["prompt"], "1");
        assert_eq!(value["notifier"], "1");
    }

    #[tokio::test]
    async fn a_run_that_hangs_is_stopped() {
        let temp = TempDir::new();
        let marker = temp.path().join("finished");
        let program = script(
            temp.path(),
            &format!("sleep 1\ntouch '{}'", marker.display()),
        );
        let gh = Gh::at(&program, Duration::from_millis(100));
        let started = Instant::now();
        assert!(matches!(
            gh.get_if_changed("/x", None).await,
            Err(GithubError::GhFailed(reason)) if reason.contains("didn't answer")
        ));
        assert!(started.elapsed() < Duration::from_millis(900));
        tokio::time::sleep(Duration::from_millis(1_300)).await;
        assert!(!marker.exists(), "the stopped run went on");
    }

    #[tokio::test]
    async fn no_program_reads_as_missing() {
        let temp = TempDir::new();
        let gh = Gh::at(&temp.path().join("gh"), Duration::from_secs(1));
        assert_eq!(
            gh.get_if_changed("/x", None).await.err(),
            Some(GithubError::NoGh)
        );
    }
}
