// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Runs `sdkmanager` and answers its prompts one at a time: yes to its
//! offer to show the licences, yes to a licence only when the record holds
//! its ID and the SHA-256 of the text it shows, and no to anything else.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use maxhq_protocol::hosts::PendingLicence;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::licences::{holds, Accepted};
use crate::tools::install::fetch::hex;
use crate::tools::install::InstallError;

/// How much of the end of its output a failure quotes.
const TAIL: usize = 400;

/// How much output since the last answer is kept to read a question from.
const SINCE: usize = 1 << 20;

/// One run.
pub struct Run<'a> {
    /// `sdkmanager` itself.
    pub program: String,
    /// The SDK's folder.
    pub root: String,
    /// The Java it runs on.
    pub java_home: PathBuf,
    /// What it's asked to do, after `--sdk_root`.
    pub args: Vec<String>,
    /// The licences you accepted.
    pub accepted: Vec<Accepted>,
    /// How long it may take.
    pub timeout: Duration,
    /// Hears how far a download has got, in percent.
    pub progress: &'a (dyn Fn(u8) + Sync),
}

/// How a run went.
#[derive(Debug)]
pub struct Ran {
    /// The licences it showed that the record doesn't hold, which were
    /// answered no.
    pub declined: Vec<PendingLicence>,
    /// Whether it exited 0.
    pub success: bool,
    /// The end of what it printed, for a failure to quote.
    pub tail: String,
}

/// A question `sdkmanager` waits on.
#[derive(Debug, PartialEq, Eq)]
pub enum Prompt {
    /// Whether to show the licences not yet accepted.
    Review,
    /// Whether to accept this licence.
    Licence(PendingLicence),
    /// Anything else, which is answered no.
    Other,
}

/// Runs `run` to its end.
pub async fn run(run: Run<'_>) -> Result<Ran, InstallError> {
    let failed = |err: String| InstallError::Failed(format!("sdkmanager didn't run: {err}"));
    let mut command = crate::tools::HostExec
        .command(&run.program)
        .map_err(|err| failed(err.to_string()))?;
    command
        .arg(format!("--sdk_root={}", run.root))
        .args(&run.args)
        .env("JAVA_HOME", &run.java_home)
        .env("ANDROID_HOME", &run.root)
        .env_remove("ANDROID_SDK_ROOT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = crate::tools::retry_busy(|| std::future::ready(command.spawn()))
        .await
        .map_err(|err| failed(err.to_string()))?;
    let (Some(mut stdin), Some(mut stdout), Some(mut stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return Err(failed("its pipes didn't open".to_owned()));
    };
    let errors = tokio::spawn(async move {
        let mut text = Vec::new();
        let _ = stderr.read_to_end(&mut text).await;
        String::from_utf8_lossy(&text).into_owned()
    });

    let mut declined = Vec::new();
    let mut tail = String::new();
    let talk = async {
        // What it printed since the last answer.
        let mut since = Vec::new();
        let mut chunk = [0u8; 8192];
        let mut last_percent = None;
        loop {
            let read = stdout.read(&mut chunk).await?;
            if read == 0 {
                break;
            }
            let fresh = String::from_utf8_lossy(&chunk[..read]).into_owned();
            if let Some(percent) = percent(&fresh)
                && last_percent != Some(percent)
            {
                last_percent = Some(percent);
                (run.progress)(percent);
            }
            keep_tail(&mut tail, &fresh);
            // Downloads print a lot between questions; a licence is far
            // shorter than what's kept.
            if since.len() > SINCE {
                since.drain(..since.len() - SINCE / 2);
            }
            since.extend_from_slice(&chunk[..read]);
            let text = String::from_utf8_lossy(&since);
            let Some(prompt) = prompt(&text) else {
                continue;
            };
            let yes = match prompt {
                Prompt::Review => true,
                Prompt::Licence(licence) => {
                    let yes = holds(&run.accepted, &licence.id, &licence.text_sha256);
                    if !yes && !declined.contains(&licence) {
                        declined.push(licence);
                    }
                    yes
                }
                Prompt::Other => false,
            };
            stdin.write_all(if yes { b"y\n" } else { b"n\n" }).await?;
            stdin.flush().await?;
            since.clear();
        }
        child.wait().await
    };
    let status = tokio::time::timeout(run.timeout, talk)
        .await
        .map_err(|_| failed("it took too long".to_owned()))?
        .map_err(|err| failed(err.to_string()))?;
    let errors = errors.await.unwrap_or_default();
    keep_tail(&mut tail, &errors);
    Ok(Ran {
        declined,
        success: status.success(),
        tail: tail.split_whitespace().collect::<Vec<_>>().join(" "),
    })
}

/// Adds `text` to the end of `tail`, keeping its last [`TAIL`] bytes.
fn keep_tail(tail: &mut String, text: &str) {
    tail.push_str(text);
    if tail.len() > TAIL {
        let mut cut = tail.len() - TAIL;
        while !tail.is_char_boundary(cut) {
            cut += 1;
        }
        tail.drain(..cut);
    }
}

/// The last download percentage in `text`, as `[===   ] 45% ...` shows.
fn percent(text: &str) -> Option<u8> {
    let end = text.rfind('%')?;
    let start = text[..end]
        .rfind(|c: char| !c.is_ascii_digit())
        .map_or(0, |at| at + 1);
    text[start..end]
        .parse()
        .ok()
        .filter(|percent| *percent <= 100)
}

/// The question `text` ends on, if it ends on one.
pub fn prompt(text: &str) -> Option<Prompt> {
    let end = text.trim_end();
    if !(end.ends_with("(y/N):") || end.ends_with("(y/N)?")) {
        return None;
    }
    let last = end.rsplit(['\n', '\r']).next().unwrap_or(end);
    if last.contains("Review licenses") {
        return Some(Prompt::Review);
    }
    Some(licence(text).map_or(Prompt::Other, Prompt::Licence))
}

/// The licence `text` shows last: its `License <id>:` line, then its text
/// between two lines of dashes.
fn licence(text: &str) -> Option<PendingLicence> {
    let lines: Vec<&str> = text
        .split('\n')
        .map(|line| line.trim_end_matches('\r'))
        .collect();
    let (at, id) = lines
        .iter()
        .enumerate()
        .rev()
        .find_map(|(at, line)| header(line).map(|id| (at, id)))?;
    let dashes = |line: &&str| line.len() >= 3 && line.chars().all(|c| c == '-');
    let mut rest = lines[at + 1..].iter();
    rest.position(&dashes)?;
    let body: Vec<&str> = rest
        .by_ref()
        .take_while(|line| !dashes(line))
        .copied()
        .collect();
    let text = body.join("\n");
    let text_sha256 = hex(ring::digest::digest(&ring::digest::SHA256, text.as_bytes()).as_ref());
    Some(PendingLicence {
        id: id.to_owned(),
        text,
        text_sha256,
    })
}

/// The ID a `License <id>:` line names, which `--licenses` numbers as
/// `1/2: License <id>:`.
fn header(line: &str) -> Option<&str> {
    let line = line.trim();
    let (before, after) = line.split_once("License ")?;
    let counted = before.strip_suffix(": ").is_some_and(|count| {
        !count.is_empty() && count.chars().all(|c| c.is_ascii_digit() || c == '/')
    });
    if !before.is_empty() && !counted {
        return None;
    }
    let id = after.strip_suffix(':')?;
    (!id.is_empty() && !id.contains(char::is_whitespace)).then_some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOWN: &str = "2 of 2 SDK package licenses not accepted.\n\
        Review licenses that have not been accepted (y/N)? y\n\n\
        1/2: License android-sdk-license:\n\
        ---------------------------------------\n\
        Terms and Conditions\r\n\
        \r\n\
        This is the licence.\n\
        ---------------------------------------\n\
        Accept? (y/N): ";

    #[test]
    fn a_licence_prompt_reads_its_id_and_text() {
        let Some(Prompt::Licence(licence)) = prompt(SHOWN) else {
            panic!("no licence in {SHOWN}");
        };
        assert_eq!(licence.id, "android-sdk-license");
        assert_eq!(licence.text, "Terms and Conditions\n\nThis is the licence.");
        assert_eq!(
            licence.text_sha256,
            hex(ring::digest::digest(&ring::digest::SHA256, licence.text.as_bytes()).as_ref())
        );
    }

    #[test]
    fn other_prompts_read_plainly() {
        assert_eq!(
            prompt("7 of 7 SDK package licenses not accepted.\nReview licenses that have not been accepted (y/N)? "),
            Some(Prompt::Review)
        );
        assert_eq!(prompt("Delete everything? (y/N): "), Some(Prompt::Other));
        assert_eq!(prompt("[===    ] 45% Downloading"), None);
        assert_eq!(prompt("Accept? (y/N"), None);
        assert_eq!(
            header("License android-sdk-license:"),
            Some("android-sdk-license")
        );
        assert_eq!(header("Some License text:"), None);
    }

    #[test]
    fn progress_and_tails_read_plainly() {
        assert_eq!(percent("[====      ] 45% Downloading x.zip"), Some(45));
        assert_eq!(percent("no progress"), None);
        let mut tail = String::new();
        keep_tail(&mut tail, &"x".repeat(TAIL));
        keep_tail(&mut tail, "end");
        assert_eq!(tail.len(), TAIL);
        assert!(tail.ends_with("end"));
    }
}
