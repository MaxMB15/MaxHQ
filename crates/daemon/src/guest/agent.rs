// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The agent socket, where the token and credential helpers ask for
//! tokens. Anyone in the container can connect, so the guest finds the
//! caller by its peer credentials and walks up its parents to a holder it
//! started. That holder names the session the token is for. A caller
//! outside every holder's tree gets nothing.
//!
//! Each connection carries one request line and one answer line. Neither
//! side shuts its write half to end one.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use maxhq_protocol::guest::{TokenReply, TokenWant};
use maxhq_protocol::ids::SessionId;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

use super::link::{refused, Guest};
use crate::guestlink::github_repo;

/// The longest request line.
const REQUEST_MAX: usize = 4096;

/// How long a caller has to send its request.
const REQUEST_WAIT: Duration = Duration::from_secs(10);

/// How far up its parents a caller is followed.
const DEPTH: usize = 64;

/// What a helper asks the guest for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum AgentWant {
    /// A token for Codex.
    Codex,
    /// A GitHub token for `repo`, as "owner/name" or a URL, or for the
    /// workspace's own repository when unset. Only that one gets a token.
    Github {
        /// The repository.
        #[serde(default)]
        repo: Option<String>,
    },
}

/// Serves the agent socket for good.
pub(crate) async fn accept(
    guest: Arc<Guest>,
    listener: UnixListener,
    proc_root: std::path::PathBuf,
) {
    let proc_root: Arc<Path> = proc_root.into();
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(serve(guest.clone(), stream, proc_root.clone()));
            }
            Err(err) => {
                tracing::warn!("couldn't accept on the agent socket: {err}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

/// Answers one helper.
async fn serve(guest: Arc<Guest>, mut stream: UnixStream, proc_root: Arc<Path>) {
    let reply = match tokio::time::timeout(REQUEST_WAIT, read_request(&mut stream)).await {
        Ok(Ok(want)) => answer(&guest, &stream, &proc_root, want).await,
        Ok(Err(reason)) => refused(&reason),
        Err(_) => refused("the request didn't come in time"),
    };
    if let TokenReply::Refused { reason } = &reply {
        tracing::info!("refused a helper a token: {reason}");
    }
    let Ok(mut line) = serde_json::to_vec(&reply) else {
        return;
    };
    line.push(b'\n');
    let _ = stream.write_all(&line).await;
}

/// Reads the one request line.
async fn read_request(stream: &mut UnixStream) -> Result<AgentWant, String> {
    let mut buf = Vec::new();
    let mut chunk = [0; 512];
    loop {
        if let Some(end) = buf.iter().position(|byte| *byte == b'\n') {
            return serde_json::from_slice(&buf[..end])
                .map_err(|err| format!("the request didn't parse: {err}"));
        }
        if buf.len() > REQUEST_MAX {
            return Err("the request is too long".to_owned());
        }
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|err| format!("couldn't read the request: {err}"))?;
        if read == 0 {
            return Err("the request ended before its newline".to_owned());
        }
        buf.extend_from_slice(&chunk[..read]);
    }
}

/// Finds the caller's session, checks the request, and asks the host.
async fn answer(
    guest: &Guest,
    stream: &UnixStream,
    proc_root: &Path,
    want: AgentWant,
) -> TokenReply {
    let pid = match stream.peer_cred() {
        Ok(cred) => match cred.pid().and_then(|pid| u32::try_from(pid).ok()) {
            Some(pid) if pid > 0 => pid,
            _ => return refused("couldn't tell which process is asking"),
        },
        Err(err) => return refused(&format!("couldn't tell which process is asking: {err}")),
    };
    let holders = guest.pids.holder_pids().await;
    let Some(session) = session_of(proc_root, pid, &holders) else {
        return refused("only a process an agent runs can ask for a token");
    };
    let Some(current) = guest.current() else {
        return refused("the host isn't linked to this workspace right now");
    };
    let want = match want {
        AgentWant::Codex => TokenWant::Codex,
        AgentWant::Github { repo } => {
            let Some(own) = current.repo.as_deref().and_then(github_repo) else {
                return refused("this workspace has no GitHub repository");
            };
            if repo.is_some_and(|asked| github_repo(&asked).as_deref() != Some(own.as_str())) {
                return refused("only the workspace's own repository gets a token");
            }
            TokenWant::Github { repo: own }
        }
    };
    // Over the limit the guest refuses on its own, since the host drops a
    // link that asks too often.
    if !guest.allow_token(session) {
        return refused("this agent asked for too many tokens in the last minute");
    }
    current.token(session, want).await
}

/// The session whose holder `pid` runs under, walking up `PPid` in
/// `<proc_root>/<pid>/status`. None for a process outside every holder's
/// tree, the guest itself among them.
pub(crate) fn session_of(
    proc_root: &Path,
    pid: u32,
    holders: &HashMap<u32, SessionId>,
) -> Option<SessionId> {
    let me = std::process::id();
    let mut pid = pid;
    for _ in 0..DEPTH {
        if let Some(session) = holders.get(&pid) {
            return Some(*session);
        }
        if pid <= 1 || pid == me {
            return None;
        }
        pid = parent(proc_root, pid)?;
    }
    None
}

/// A process's parent, from its status file.
fn parent(proc_root: &Path, pid: u32) -> Option<u32> {
    let status = std::fs::read_to_string(proc_root.join(pid.to_string()).join("status")).ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("PPid:"))
        .and_then(|ppid| ppid.trim().parse().ok())
}
