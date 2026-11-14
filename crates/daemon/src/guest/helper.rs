// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The helpers a CLI runs: Codex's auth command, git's credential helper
//! and the token for the `gh` wrapper. Each asks the guest over the agent
//! socket and prints the token only to stdout.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use maxhq_protocol::guest::TokenReply;
use maxhq_protocol::secret::Secret;
use maxhq_protocol::time::Timestamp;

use super::agent::AgentWant;
use crate::guestlink::github_repo;

/// How long the guest has to answer, which covers the host's broker.
const ANSWER_WAIT: Duration = Duration::from_secs(70);

/// The longest answer line.
const ANSWER_MAX: u64 = 64 * 1024;

/// The longest credential request git sends.
const INPUT_MAX: u64 = 64 * 1024;

/// Asks the guest at `socket` for a token.
pub(crate) fn ask_at(socket: &Path, want: &AgentWant) -> Result<(Secret, Timestamp), String> {
    let mut stream = UnixStream::connect(socket)
        .map_err(|err| format!("couldn't reach the guest at {}: {err}", socket.display()))?;
    let failed = |err: io::Error| format!("couldn't ask the guest: {err}");
    stream.set_read_timeout(Some(ANSWER_WAIT)).map_err(failed)?;
    stream
        .set_write_timeout(Some(ANSWER_WAIT))
        .map_err(failed)?;
    let mut line = serde_json::to_vec(want).map_err(|err| err.to_string())?;
    line.push(b'\n');
    stream.write_all(&line).map_err(failed)?;
    let mut answer = String::new();
    BufReader::new(stream)
        .take(ANSWER_MAX)
        .read_line(&mut answer)
        .map_err(failed)?;
    match serde_json::from_str(&answer) {
        Ok(TokenReply::Token { value, expires_at }) => Ok((value, expires_at)),
        Ok(TokenReply::Refused { reason }) => Err(reason),
        Err(err) => Err(format!("the guest's answer didn't parse: {err}")),
    }
}

/// Prints a token for `want` on a line of its own.
fn print_token(socket: &Path, want: &AgentWant) -> ExitCode {
    match write_token(socket, want, &mut io::stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("no token: {reason}");
            ExitCode::FAILURE
        }
    }
}

/// Writes a token for `want` to `out` on a line of its own, and nothing
/// when there's none.
fn write_token(socket: &Path, want: &AgentWant, out: &mut impl Write) -> Result<(), String> {
    let (token, _) = ask_at(socket, want)?;
    writeln!(out, "{}", token.expose()).map_err(|err| err.to_string())
}

/// `maxd guest token codex`.
pub(crate) fn token_codex(socket: &Path) -> ExitCode {
    print_token(socket, &AgentWant::Codex)
}

/// `maxd guest github-token`.
pub(crate) fn github_token(socket: &Path) -> ExitCode {
    print_token(socket, &AgentWant::Github { repo: None })
}

/// `maxd guest git-credential <action>`.
pub(crate) fn git_credential(socket: &Path, action: &str) -> ExitCode {
    let mut input = String::new();
    // Git writes the request whatever the action, so it's read in full
    // before answering.
    if io::stdin()
        .lock()
        .take(INPUT_MAX)
        .read_to_string(&mut input)
        .is_err()
    {
        return ExitCode::FAILURE;
    }
    if action != "get" {
        return ExitCode::SUCCESS;
    }
    let mut stdout = io::stdout().lock();
    match credential_get(&input, |want| ask_at(socket, want), &mut stdout) {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("no GitHub credential: {reason}");
            ExitCode::FAILURE
        }
    }
}

/// Answers git's `get` for github.com over https, and prints nothing for
/// anything else so git asks its other helpers.
fn credential_get(
    input: &str,
    ask: impl FnOnce(&AgentWant) -> Result<(Secret, Timestamp), String>,
    out: &mut impl Write,
) -> Result<(), String> {
    let mut protocol = None;
    let mut host = None;
    let mut path = None;
    for line in input.lines() {
        if line.is_empty() {
            break;
        }
        match line.split_once('=') {
            Some(("protocol", value)) => protocol = Some(value),
            Some(("host", value)) => host = Some(value),
            Some(("path", value)) => path = Some(value),
            _ => {}
        }
    }
    let github = protocol == Some("https")
        && host.is_some_and(|host| host.eq_ignore_ascii_case("github.com"));
    if !github {
        return Ok(());
    }
    let repo = match path {
        Some(path) => match github_repo(path) {
            Some(repo) => Some(repo),
            None => return Ok(()),
        },
        None => None,
    };
    let (token, expires_at) = ask(&AgentWant::Github { repo })?;
    let expiry = expires_at.0 / 1000;
    write!(
        out,
        "username=x-access-token\npassword={}\npassword_expiry_utc={expiry}\n",
        token.expose()
    )
    .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use std::thread;

    use super::*;
    use crate::testing::TempDir;

    /// A guest at a socket in `temp` that answers one ask with `reply`,
    /// and gives back the line it was asked.
    fn guest(temp: &TempDir, reply: TokenReply) -> (PathBuf, thread::JoinHandle<String>) {
        let socket = temp.path().join("agent.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let answering = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut asked = String::new();
            BufReader::new(&stream).read_line(&mut asked).unwrap();
            let mut line = serde_json::to_vec(&reply).unwrap();
            line.push(b'\n');
            (&stream).write_all(&line).unwrap();
            asked
        });
        (socket, answering)
    }

    #[test]
    fn codexs_auth_command_prints_the_token_alone() {
        let temp = TempDir::new();
        let (value, expires_at) = token();
        let (socket, asked) = guest(&temp, TokenReply::Token { value, expires_at });
        let mut out = Vec::new();
        write_token(&socket, &AgentWant::Codex, &mut out).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "test-not-a-real-token\n");
        let asked: AgentWant = serde_json::from_str(&asked.join().unwrap()).unwrap();
        assert_eq!(asked, AgentWant::Codex);

        let temp = TempDir::new();
        let reason = "this device isn't signed in to Codex".to_owned();
        let (socket, _) = guest(
            &temp,
            TokenReply::Refused {
                reason: reason.clone(),
            },
        );
        let mut out = Vec::new();
        let err = write_token(&socket, &AgentWant::Codex, &mut out).unwrap_err();
        assert_eq!(err, reason);
        assert!(out.is_empty());
    }

    fn token() -> (Secret, Timestamp) {
        (
            Secret::new("test-not-a-real-token".into()),
            Timestamp(1_800_000_000_000),
        )
    }

    #[test]
    fn git_gets_a_token_for_github_over_https() {
        let mut out = Vec::new();
        let mut asked = None;
        let input = "protocol=https\nhost=github.com\npath=example-user/maxhq.git\n\n";
        credential_get(
            input,
            |want| {
                asked = Some(want.clone());
                Ok(token())
            },
            &mut out,
        )
        .unwrap();
        assert_eq!(
            asked,
            Some(AgentWant::Github {
                repo: Some("example-user/maxhq".into())
            })
        );
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "username=x-access-token\npassword=test-not-a-real-token\npassword_expiry_utc=1800000000\n"
        );

        let mut asked = None;
        let mut out = Vec::new();
        credential_get(
            "protocol=https\nhost=GitHub.com\n",
            |want| {
                asked = Some(want.clone());
                Ok(token())
            },
            &mut out,
        )
        .unwrap();
        assert_eq!(asked, Some(AgentWant::Github { repo: None }));
    }

    #[test]
    fn git_gets_nothing_for_other_hosts() {
        for input in [
            "protocol=https\nhost=gitlab.com\n",
            "protocol=http\nhost=github.com\n",
            "protocol=https\nhost=github.com.evil.example\n",
            "host=github.com\n",
        ] {
            let mut out = Vec::new();
            credential_get(input, |_| panic!("asked for {input:?}"), &mut out).unwrap();
            assert!(out.is_empty(), "{input:?}");
        }
    }

    #[test]
    fn a_refusal_prints_nothing() {
        let mut out = Vec::new();
        let err = credential_get(
            "protocol=https\nhost=github.com\n",
            |_| Err("nope".into()),
            &mut out,
        )
        .unwrap_err();
        assert_eq!(err, "nope");
        assert!(out.is_empty());
    }
}
