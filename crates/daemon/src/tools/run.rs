// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the 5 minute
// timeout, the 10,000 byte output cap, progress lines, one update per lock,
// the failure messages and the version check after an update.

//! Running an agent CLI's update command.

use std::collections::HashSet;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt};

use super::defs::{self, Def};
use super::resolve::Command;

/// How long an update may run.
pub const UPDATE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// How much of each stream an update keeps.
pub const OUTPUT_MAX_BYTES: usize = 10_000;
/// The longest progress line.
const PROGRESS_MAX_CHARS: usize = 200;
/// How often progress is passed on.
const PROGRESS_EVERY: Duration = Duration::from_secs(1);
/// How long `--version` may take afterwards.
const VERSION_WAIT: Duration = Duration::from_secs(10);

/// How an update command ended.
#[derive(Debug, Default)]
pub struct Ran {
    /// What it printed to stdout, up to the cap.
    pub stdout: String,
    /// What it printed to stderr, up to the cap.
    pub stderr: String,
    /// Its exit code, or none if a signal ended it or it timed out.
    pub code: Option<i32>,
    /// Whether it ran past the timeout and was killed.
    pub timed_out: bool,
}

impl Ran {
    fn success(&self) -> bool {
        self.code == Some(0)
    }
}

/// The locks updates hold, by lock key.
#[derive(Clone, Default)]
pub struct Locks(Arc<Mutex<HashSet<String>>>);

/// Holds one lock until dropped.
pub struct Held {
    locks: Locks,
    key: String,
}

impl Drop for Held {
    fn drop(&mut self) {
        lock(&self.locks.0).remove(&self.key);
    }
}

impl Locks {
    /// Takes `key`, or none when another update holds it.
    pub fn take(&self, key: &str) -> Option<Held> {
        lock(&self.0).insert(key.to_owned()).then(|| Held {
            locks: self.clone(),
            key: key.to_owned(),
        })
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Runs `command`, passing on its latest output line about once a second.
pub async fn run(
    command: &Command,
    timeout: Duration,
    progress: &(dyn Fn(String) + Send + Sync),
) -> std::io::Result<Ran> {
    // Through HostExec, which won't run anything that switches user.
    let mut child = super::retry_busy(|| async {
        super::HostExec
            .command(&command.program)
            .map_err(std::io::Error::other)?
            .args(&command.args)
            .envs(command.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
    })
    .await?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let latest = Mutex::new((None::<String>, Instant::now() - PROGRESS_EVERY));
    let collect = async {
        let (stdout, stderr, status) = tokio::join!(
            collect_stream(stdout, &latest, progress),
            collect_stream(stderr, &latest, progress),
            child.wait(),
        );
        (stdout, stderr, status)
    };
    let ran = match tokio::time::timeout(timeout, collect).await {
        Ok((stdout, stderr, status)) => Ran {
            stdout,
            stderr,
            code: status?.code(),
            timed_out: false,
        },
        Err(_) => Ran {
            timed_out: true,
            ..Ran::default()
        },
    };
    // A line held back by the once a second limit still counts.
    if let Some(line) = lock(&latest).0.take() {
        progress(line);
    }
    Ok(ran)
}

/// Reads a stream to its end, keeping its first [`OUTPUT_MAX_BYTES`].
async fn collect_stream(
    stream: Option<impl AsyncRead + Unpin>,
    latest: &Mutex<(Option<String>, Instant)>,
    progress: &(dyn Fn(String) + Send + Sync),
) -> String {
    let Some(mut stream) = stream else {
        return String::new();
    };
    let mut kept = Vec::new();
    let mut partial = String::new();
    let mut buffer = [0u8; 4096];
    loop {
        let read = match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        let chunk = &buffer[..read];
        let room = OUTPUT_MAX_BYTES.saturating_sub(kept.len());
        kept.extend_from_slice(&chunk[..chunk.len().min(room)]);
        partial.push_str(&String::from_utf8_lossy(chunk));
        let mut lines: Vec<&str> = partial.split(['\n', '\r']).collect();
        let tail = lines.pop().unwrap_or_default().to_owned();
        let line =
            progress_line(&tail).or_else(|| lines.iter().rev().find_map(|l| progress_line(l)));
        // An installer can write for minutes without a line ending, and
        // only its tail matters.
        partial = tail
            .chars()
            .rev()
            .take(4_096)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if let Some(line) = line {
            let mut latest = lock(latest);
            if latest.1.elapsed() >= PROGRESS_EVERY {
                latest.1 = Instant::now();
                latest.0 = None;
                drop(latest);
                progress(line);
            } else {
                latest.0 = Some(line);
            }
        }
    }
    String::from_utf8_lossy(&kept).into_owned()
}

/// A line cleaned for showing: no escape codes, single spaces, at most
/// [`PROGRESS_MAX_CHARS`].
pub fn progress_line(line: &str) -> Option<String> {
    let text = plain_text(line)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if text.is_empty() {
        return None;
    }
    if text.chars().count() <= PROGRESS_MAX_CHARS {
        return Some(text);
    }
    let mut cut: String = text.chars().take(PROGRESS_MAX_CHARS - 1).collect();
    cut.push('…');
    Some(cut)
}

/// `line` without its escape codes or control characters, tabs aside.
pub fn plain_text(line: &str) -> String {
    let mut plain = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // CSI sequences end at a letter. Others are one more character.
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() || c == '~' {
                        break;
                    }
                }
            } else {
                chars.next();
            }
        } else if !c.is_control() || c == '\t' {
            plain.push(c);
        }
    }
    plain
}

/// The last line an updater printed, from stderr first.
fn last_output_line(ran: &Ran) -> Option<String> {
    [&ran.stderr, &ran.stdout]
        .into_iter()
        .find_map(|output| output.lines().rev().find_map(progress_line))
}

/// Why an update failed, for a person to read.
pub fn failure_message(ran: &Ran) -> String {
    if ran.timed_out {
        return "Update timed out.".to_owned();
    }
    let summary = match ran.code {
        Some(code) if code != 0 => format!("Update command exited with code {code}"),
        _ => "Update command failed".to_owned(),
    };
    match last_output_line(ran) {
        Some(reason) => format!("{summary}: {reason}"),
        None => format!("{summary}."),
    }
}

/// Runs `binary --version` and reads it.
pub async fn read_version(def: &Def, binary: &Path) -> Option<String> {
    let output = tokio::process::Command::new(binary)
        .args(def.version_args)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(VERSION_WAIT, output)
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    defs::version_text(&String::from_utf8_lossy(&output.stdout))
}

/// Updates one CLI: takes the command's lock, runs it, then checks the
/// version `binary` reports. Gives the new version, or why it failed.
pub async fn update(
    def: &Def,
    command: &Command,
    binary: &Path,
    latest: Option<&str>,
    locks: &Locks,
    timeout: Duration,
    progress: &(dyn Fn(String) + Send + Sync),
) -> Result<String, String> {
    let Some(_held) = locks.take(&command.lock) else {
        return Err("An update is already running.".to_owned());
    };
    let ran = run(command, timeout, progress).await.map_err(|err| {
        format!(
            "Couldn't run the update command {}: {err}",
            command.display()
        )
    })?;
    if !ran.success() {
        return Err(failure_message(&ran));
    }
    progress("Checking the installed version".to_owned());
    let Some(version) = read_version(def, binary).await else {
        return Err("The update finished, but MaxHQ couldn't read the new version.".to_owned());
    };
    let behind = latest
        .and_then(crate::recipe::steps::Version::parse)
        .zip(crate::recipe::steps::Version::parse(&version))
        .is_some_and(|(latest, now)| !now.at_least(latest));
    if behind {
        return Err(format!(
            "The update finished, but {} is still at version {version}.",
            def.binary
        ));
    }
    Ok(version)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::super::defs::CLAUDE;
    use super::*;
    use crate::testing::TempDir;

    /// A fake CLI: `--version` prints the version in `version`, and
    /// `update` runs `update_body`.
    fn fake_cli(dir: &TempDir, update_body: &str) -> PathBuf {
        let version = dir.path().join("version");
        fs::write(&version, "2.1.0\n").unwrap();
        let path = dir.path().join("bin/claude");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let script = format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then printf '%s (Claude Code)\\n' \"$(cat {version})\"; exit 0; fi\n{update_body}\n",
            version = version.display()
        );
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn update_command(binary: &Path) -> Command {
        Command {
            program: binary.display().to_string(),
            args: vec!["update".to_owned()],
            env: Vec::new(),
            lock: "claude-native".to_owned(),
        }
    }

    fn quiet(_: String) {}

    #[tokio::test]
    async fn an_update_that_runs_too_long_times_out() {
        let dir = TempDir::new();
        let binary = fake_cli(&dir, "sleep 5");
        let result = update(
            &CLAUDE,
            &update_command(&binary),
            &binary,
            None,
            &Locks::default(),
            Duration::from_millis(200),
            &quiet,
        )
        .await;
        assert_eq!(result, Err("Update timed out.".to_owned()));
    }

    #[tokio::test]
    async fn output_is_capped() {
        let dir = TempDir::new();
        let binary = fake_cli(
            &dir,
            "head -c 50000 /dev/zero | tr '\\0' x; echo; echo done >&2",
        );
        let ran = run(&update_command(&binary), UPDATE_TIMEOUT, &quiet)
            .await
            .unwrap();
        assert_eq!(ran.stdout.len(), OUTPUT_MAX_BYTES);
        assert_eq!(ran.stderr, "done\n");
        assert_eq!(ran.code, Some(0));
    }

    #[tokio::test]
    async fn a_failed_update_says_why() {
        let dir = TempDir::new();
        let binary = fake_cli(&dir, "echo working; echo 'no space left' >&2; exit 3");
        let result = update(
            &CLAUDE,
            &update_command(&binary),
            &binary,
            None,
            &Locks::default(),
            UPDATE_TIMEOUT,
            &quiet,
        )
        .await;
        assert_eq!(
            result,
            Err("Update command exited with code 3: no space left".to_owned())
        );
    }

    #[tokio::test]
    async fn an_update_that_leaves_the_old_version_fails_its_check() {
        let dir = TempDir::new();
        let binary = fake_cli(&dir, "echo updated");
        let result = update(
            &CLAUDE,
            &update_command(&binary),
            &binary,
            Some("2.2.0"),
            &Locks::default(),
            UPDATE_TIMEOUT,
            &quiet,
        )
        .await;
        assert_eq!(
            result,
            Err("The update finished, but claude is still at version 2.1.0.".to_owned())
        );
    }

    #[tokio::test]
    async fn an_update_that_moves_the_version_passes() {
        let dir = TempDir::new();
        let version = dir.path().join("version");
        let binary = fake_cli(
            &dir,
            &format!(
                "echo 'Installing 2.2.0'; echo 2.2.0 > {}",
                version.display()
            ),
        );
        let lines = Mutex::new(Vec::new());
        let progress = |line: String| lock(&lines).push(line);
        let result = update(
            &CLAUDE,
            &update_command(&binary),
            &binary,
            Some("2.2.0"),
            &Locks::default(),
            UPDATE_TIMEOUT,
            &progress,
        )
        .await;
        assert_eq!(result, Ok("2.2.0".to_owned()));
        let lines = lock(&lines);
        assert!(lines.contains(&"Installing 2.2.0".to_owned()), "{lines:?}");
    }

    #[test]
    fn one_update_runs_per_lock() {
        let locks = Locks::default();
        let held = locks.take("npm-global:/usr/local").unwrap();
        assert!(locks.take("npm-global:/usr/local").is_none());
        assert!(locks.take("claude-native").is_some());
        drop(held);
        assert!(locks.take("npm-global:/usr/local").is_some());
    }

    #[test]
    fn progress_lines_are_cleaned_and_cut() {
        assert_eq!(
            progress_line("\u{1b}[32m  Downloading\t 50%  \u{1b}[0m").as_deref(),
            Some("Downloading 50%")
        );
        assert_eq!(progress_line("   "), None);
        let long = progress_line(&"x".repeat(300)).unwrap();
        assert_eq!(long.chars().count(), PROGRESS_MAX_CHARS);
        assert!(long.ends_with('…'));
    }
}
