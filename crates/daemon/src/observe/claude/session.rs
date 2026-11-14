// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The `sessions/<pid>.json` file each running Claude Code session writes.
//! The `<pid>.<hash>.key` file beside each one is a secret, and nothing
//! here opens it.

use std::fs;
use std::path::Path;

use maxhq_protocol::attention::{AttentionEvent, AttentionKind};
use maxhq_protocol::ids::{AgentId, AttentionId};
use maxhq_protocol::time::Timestamp;
use serde_json::Value;

/// One session file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    /// The CLI's process ID.
    pub pid: u32,
    /// Claude Code's session ID, a UUID.
    pub session_id: String,
    /// The folder the session runs in.
    pub cwd: String,
    /// When the session started, in milliseconds.
    pub started_at: Option<u64>,
    /// The process's start time as `ps -o lstart=` printed it in UTC, which
    /// tells this process from a later one given the same PID.
    pub proc_start: Option<String>,
    /// The Claude Code version, such as `2.1.303`.
    pub version: Option<String>,
    /// The session's name, as `/rename` or Claude Code itself set it.
    pub name: Option<String>,
    /// `busy`, `idle`, `waiting` or `shell`.
    pub status: Option<String>,
    /// What a waiting session wants, such as "permission prompt".
    pub waiting_for: Option<String>,
    /// When Claude Code last wrote the file, in milliseconds.
    pub updated_at: Option<u64>,
    /// When `status` last changed, in milliseconds.
    pub status_updated_at: Option<u64>,
}

/// What a session's `status` says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Working on a turn.
    Busy,
    /// Running a shell command you typed.
    Shell,
    /// Waiting for a prompt.
    Idle,
    /// Blocked on a permission prompt or a question.
    Waiting,
    /// Missing, or a value this version of `maxd` doesn't know.
    Unknown,
}

impl Session {
    /// Reads one session file's JSON. Returns none when a field that
    /// identifies the session is missing. Other fields are optional, so a
    /// newer Claude Code that drops or retypes one still shows up.
    pub fn parse(json: &[u8]) -> Option<Self> {
        let value: Value = serde_json::from_slice(json).ok()?;
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        let millis = |key: &str| value.get(key).and_then(Value::as_u64);
        let session = Self {
            pid: value.get("pid")?.as_u64()?.try_into().ok()?,
            session_id: text("sessionId")?,
            cwd: text("cwd")?,
            started_at: millis("startedAt"),
            proc_start: text("procStart"),
            version: text("version"),
            name: text("name"),
            status: text("status"),
            waiting_for: text("waitingFor"),
            updated_at: millis("updatedAt"),
            status_updated_at: millis("statusUpdatedAt"),
        };
        // The session ID names the transcript, so anything that could
        // leave the `projects` folder is refused.
        let usable = session.pid > 1
            && !session.session_id.is_empty()
            && session
                .session_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
            && Path::new(&session.cwd).is_absolute();
        usable.then_some(session)
    }

    /// The status, read leniently.
    pub fn status(&self) -> Status {
        match self.status.as_deref() {
            Some("busy") => Status::Busy,
            Some("shell") => Status::Shell,
            Some("idle") => Status::Idle,
            Some("waiting") => Status::Waiting,
            _ => Status::Unknown,
        }
    }

    /// When the status last changed, or the best guess there is.
    pub fn status_changed_at(&self) -> Timestamp {
        Timestamp(
            self.status_updated_at
                .or(self.updated_at)
                .or(self.started_at)
                .unwrap_or(0),
        )
    }

    /// When the file was last written, for picking between two files that
    /// claim the same session.
    pub fn freshness(&self) -> u64 {
        self.updated_at
            .or(self.status_updated_at)
            .or(self.started_at)
            .unwrap_or(0)
    }

    /// The event a waiting session raises, or none when it isn't waiting.
    /// Its ID comes from the agent and when the wait began, so it stays put
    /// while the session waits and the next wait is a new event.
    pub fn attention(&self, agent: AgentId) -> Option<AttentionEvent> {
        if self.status() != Status::Waiting {
            return None;
        }
        let began = self
            .status_updated_at
            .or(self.started_at)
            .unwrap_or_default();
        let summary = self
            .waiting_for
            .clone()
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| "input needed".to_owned());
        let kind = if summary.to_lowercase().contains("permission") {
            AttentionKind::Permission
        } else {
            AttentionKind::Question
        };
        Some(AttentionEvent {
            id: AttentionId::derive(&format!("{agent}:{began}")),
            agent,
            kind,
            summary,
            options: Vec::new(),
            accepts_text: false,
            created_at: self.status_changed_at(),
            expires_at: None,
        })
    }
}

/// Every readable session file in a `sessions` folder. Only `.json` files
/// are opened, so the `.key` files beside them never are.
pub fn list(dir: &Path) -> Vec<Session> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut sessions = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        match fs::read(&path).ok().and_then(|json| Session::parse(&json)) {
            Some(session) => sessions.push(session),
            None => tracing::debug!(path = %path.display(), "skipping an unreadable session file"),
        }
    }
    sessions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::claude::tests::{fixture, scratch};

    fn session(file: &str) -> Session {
        let path = fixture("config/sessions").join(file);
        Session::parse(&fs::read(path).unwrap()).expect("a valid session file")
    }

    #[test]
    fn reads_a_session_file() {
        let busy = session("41001.json");
        assert_eq!(busy.pid, 41001);
        assert_eq!(busy.session_id, "6f1d2c3b-0a4e-4b8f-9c11-2d3e4f5a6b7c");
        assert_eq!(busy.cwd, "/work/maxhq");
        assert_eq!(busy.started_at, Some(1_792_880_000_000));
        assert_eq!(busy.proc_start.as_deref(), Some("Sat Oct 24 22:13:20 2026"));
        assert_eq!(busy.name.as_deref(), Some("observer refactor"));
        assert_eq!(busy.version.as_deref(), Some("2.1.310"));
        assert_eq!(busy.status(), Status::Busy);
        assert_eq!(busy.waiting_for, None);
    }

    #[test]
    fn maps_every_status() {
        let mut session = session("41001.json");
        for (status, want) in [
            (Some("busy"), Status::Busy),
            (Some("shell"), Status::Shell),
            (Some("idle"), Status::Idle),
            (Some("waiting"), Status::Waiting),
            (Some("thinking-hard"), Status::Unknown),
            (None, Status::Unknown),
        ] {
            session.status = status.map(str::to_owned);
            assert_eq!(session.status(), want, "{status:?}");
        }
    }

    #[test]
    fn a_waiting_session_raises_one_event_per_wait() {
        let waiting = session("41002.json");
        let agent = AgentId::derive("lead");
        let event = waiting.attention(agent).expect("an event");
        assert_eq!(event.kind, AttentionKind::Permission);
        assert_eq!(event.summary, "permission prompt");
        assert_eq!(
            event.id,
            AttentionId::derive(&format!("{agent}:1792889000000"))
        );
        assert!(event.options.is_empty());
        assert!(!event.accepts_text);
        assert_eq!(event.created_at, Timestamp(1_792_889_000_000));

        let mut asking = waiting.clone();
        asking.waiting_for = Some("input needed".into());
        asking.status_updated_at = Some(1_792_889_100_000);
        let question = asking.attention(agent).unwrap();
        assert_eq!(question.kind, AttentionKind::Question);
        assert_ne!(question.id, event.id);

        assert_eq!(session("41001.json").attention(agent), None);
    }

    #[test]
    fn skips_key_files_and_broken_ones() {
        let dir = scratch();
        let sessions = dir.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::copy(
            fixture("config/sessions/41001.json"),
            sessions.join("41001.json"),
        )
        .unwrap();
        // Shaped like a session file, so reading it would show up here.
        fs::copy(
            fixture("config/sessions/41002.json"),
            sessions.join("41002.0badc0de.key"),
        )
        .unwrap();
        fs::write(sessions.join("41003.json"), "{\"pid\": 41003").unwrap();
        fs::write(
            sessions.join("41004.json"),
            "{\"pid\": 41004, \"cwd\": \"/w\"}",
        )
        .unwrap();
        fs::write(
            sessions.join("41005.json"),
            "{\"pid\": 41005, \"sessionId\": \"../../x\", \"cwd\": \"/w\"}",
        )
        .unwrap();

        let found = list(&sessions);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].pid, 41001);
        assert!(list(&dir.join("missing")).is_empty());
    }
}
