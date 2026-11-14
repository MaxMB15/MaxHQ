// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The rules for bringing a crashed agent back, from decisions A-11 and
//! A-12. A held agent whose CLI died resumes on its own unless you paused
//! it. The turn that was cut off is never sent again. An agent that was
//! busy gets a message naming the tools that were still running, and an
//! agent that resumes three times in a row without progress pauses.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

use maxhq_protocol::timeline::{ItemBody, ItemStatus, TimelineItem};
use serde::{Deserialize, Serialize};

/// Resumes in a row without progress before the agent pauses.
pub const NO_PROGRESS_LIMIT: u32 = 3;

/// How long a Codex app-server waits before each restart, by how many
/// times it crashed in the window.
pub const BACKOFF: [Duration; 4] = [
    Duration::ZERO,
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];

/// How long a restarted Codex app-server has to say its first line.
pub const HANDSHAKE_LIMIT: Duration = Duration::from_secs(30);

/// The window crashes are counted in.
pub const CRASH_WINDOW: Duration = Duration::from_secs(5 * 60);

/// The crash in the window that ends the restarts.
pub const MAX_CRASHES: usize = 5;

/// An agent's resumes since it last made progress.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resumes {
    /// Resumes in a row with no progress after them.
    pub count: u32,
    /// Whether a tool call or a reply finished since the last resume.
    pub progress: bool,
}

/// What to do with an agent whose CLI died.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// Bring it back.
    Resume,
    /// Too many resumes made no progress, so wait for you.
    Pause,
}

impl Resumes {
    /// Decides what follows a crash, counting the resume if there is one.
    pub fn after_crash(&mut self) -> Next {
        if self.progress {
            self.count = 0;
        }
        if self.count >= NO_PROGRESS_LIMIT {
            return Next::Pause;
        }
        self.count += 1;
        self.progress = false;
        Next::Resume
    }
}

/// The tool items still running, which the crash cut off.
pub fn cut_off<'a>(items: impl Iterator<Item = &'a TimelineItem>) -> Vec<TimelineItem> {
    items
        .filter(|item| {
            item.status == ItemStatus::Running
                && matches!(
                    item.body,
                    ItemBody::Command { .. }
                        | ItemBody::FileChange { .. }
                        | ItemBody::ToolCall { .. }
                )
        })
        .cloned()
        .collect()
}

/// The message a busy agent gets once it's back. It names each cut-off
/// tool and asks the agent to check what each one did before running it
/// again, since a half-run migration or push isn't safe to repeat.
pub fn continue_message(cut: &[TimelineItem]) -> String {
    explain(
        "Your session stopped unexpectedly in the middle of a turn, and MaxHQ restarted it.",
        cut,
    )
}

/// The message an agent gets after a usage limit cut its turn off, which
/// waits until the limit resets.
pub const LIMIT_MESSAGE: &str = "Your last turn stopped at the usage limit, which has reset now. \
                                 Carry on from where you were.";

/// Why an agent's device or container went down before it came back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Downtime {
    /// It restarted with no warning.
    Restarted,
    /// It restarted after a wind-down asked the agent to finish its step.
    Planned,
    /// You stopped the container and started it again.
    Stopped,
}

/// The message an agent gets once it's back from a restart of its device
/// or container. Like [`continue_message`], it names each cut-off tool.
pub fn restart_message(cut: &[TimelineItem], downtime: Downtime) -> String {
    let opening = match downtime {
        Downtime::Restarted => {
            "Your session stopped because its machine restarted, and MaxHQ started it again."
        }
        Downtime::Planned => {
            "Your session paused for a planned restart of its machine, and MaxHQ started it again."
        }
        Downtime::Stopped => {
            "Your session stopped because its machine was shut down, and MaxHQ started it again."
        }
    };
    explain(opening, cut)
}

/// `opening`, then what was cut off and what to do about it.
fn explain(opening: &str, cut: &[TimelineItem]) -> String {
    let mut text = String::from(opening);
    if cut.is_empty() {
        text.push_str(" Nothing was running when it stopped. Carry on from where you were.");
        return text;
    }
    text.push_str(
        " These were still running when it stopped, so each one may have finished, \
         stopped partway or never started:\n",
    );
    for item in cut {
        let _ = writeln!(text, "- {}", describe(&item.body));
    }
    text.push_str(
        "Check what each one already did before you run it again, then carry on from where \
         you were.",
    );
    text
}

fn describe(body: &ItemBody) -> String {
    match body {
        ItemBody::Command { command, .. } => format!("the command `{command}`"),
        ItemBody::FileChange { changes } => {
            let paths: Vec<&str> = changes.iter().map(|edit| edit.path.as_str()).collect();
            if paths.is_empty() {
                "a file change".to_owned()
            } else {
                format!("changes to {}", paths.join(", "))
            }
        }
        ItemBody::ToolCall {
            server,
            name,
            title,
            ..
        } => {
            let tool = match server {
                Some(server) => format!("{server}/{name}"),
                None => name.clone(),
            };
            match title {
                Some(title) => format!("the tool {tool}: {title}"),
                None => format!("the tool {tool}"),
            }
        }
        _ => "a tool call".to_owned(),
    }
}

/// A Codex app-server's recent crashes.
#[derive(Debug, Default)]
pub struct Restarts {
    crashes: VecDeque<Instant>,
}

impl Restarts {
    /// Records a crash at `now`. Returns when to restart, or none once the
    /// window holds [`MAX_CRASHES`].
    pub fn crashed(&mut self, now: Instant) -> Option<Instant> {
        while self
            .crashes
            .front()
            .is_some_and(|at| now.duration_since(*at) >= CRASH_WINDOW)
        {
            self.crashes.pop_front();
        }
        self.crashes.push_back(now);
        if self.crashes.len() >= MAX_CRASHES {
            return None;
        }
        let wait = BACKOFF[(self.crashes.len() - 1).min(BACKOFF.len() - 1)];
        Some(now + wait)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maxhq_protocol::ids::AgentId;
    use maxhq_protocol::timeline::{FileEdit, FileOp, ItemId};

    fn item(id: &str, status: ItemStatus, body: ItemBody) -> TimelineItem {
        TimelineItem {
            id: ItemId(id.into()),
            agent: AgentId::derive("a"),
            turn: None,
            parent: None,
            ordinal: 0,
            status,
            started_at: None,
            completed_at: None,
            body,
        }
    }

    fn command(text: &str) -> ItemBody {
        ItemBody::Command {
            command: text.into(),
            cwd: None,
            exit_code: None,
            output: String::new(),
            output_truncated: false,
            background: false,
        }
    }

    #[test]
    fn three_resumes_without_progress_pause_the_agent() {
        let mut resumes = Resumes::default();
        for _ in 0..3 {
            assert_eq!(resumes.after_crash(), Next::Resume);
        }
        assert_eq!(resumes.after_crash(), Next::Pause);
        assert_eq!(resumes.count, 3);
    }

    #[test]
    fn progress_starts_the_count_again() {
        let mut resumes = Resumes::default();
        resumes.after_crash();
        resumes.after_crash();
        resumes.progress = true;
        assert_eq!(resumes.after_crash(), Next::Resume);
        assert_eq!(resumes.count, 1);
        assert!(!resumes.progress);
    }

    #[test]
    fn only_running_tools_are_cut_off_and_each_is_named() {
        let edit = ItemBody::FileChange {
            changes: vec![FileEdit {
                path: "src/db.rs".into(),
                old_path: None,
                op: FileOp::Modify,
                additions: 1,
                deletions: 0,
                patch: None,
            }],
        };
        let tool = ItemBody::ToolCall {
            server: Some("github".into()),
            name: "push".into(),
            title: None,
            input: None,
            output: None,
            images: Vec::new(),
        };
        let items = [
            item("1", ItemStatus::Running, command("make migrate")),
            item("2", ItemStatus::Completed, command("ls")),
            item("3", ItemStatus::Running, edit),
            item("4", ItemStatus::Running, tool),
            item(
                "5",
                ItemStatus::Running,
                ItemBody::Reasoning {
                    text: String::new(),
                    streaming: true,
                },
            ),
        ];
        let cut = cut_off(items.iter());
        assert_eq!(cut.len(), 3);
        let text = continue_message(&cut);
        assert!(text.contains("`make migrate`"), "{text}");
        assert!(text.contains("changes to src/db.rs"), "{text}");
        assert!(text.contains("github/push"), "{text}");
        assert!(text.contains("Check what each one already did"), "{text}");
        assert!(!text.contains("`ls`"));
    }

    #[test]
    fn a_restart_names_the_cut_off_tools_too() {
        let cut = [item("1", ItemStatus::Running, command("cargo test"))];
        let text = restart_message(&cut, Downtime::Restarted);
        assert!(text.starts_with("Your session stopped because its machine restarted"));
        assert!(text.contains("`cargo test`"), "{text}");
        let text = restart_message(&[], Downtime::Planned);
        assert!(text.contains("planned restart"), "{text}");
        assert!(text.contains("Nothing was running"), "{text}");
        let text = restart_message(&cut, Downtime::Stopped);
        assert!(text.starts_with("Your session stopped because its machine was shut down"));
        assert!(text.contains("`cargo test`"), "{text}");
    }

    #[test]
    fn codex_restarts_back_off_and_stop_at_the_fifth_crash() {
        let start = Instant::now();
        let mut restarts = Restarts::default();
        let waits: Vec<Option<Duration>> = (0..5)
            .map(|n| {
                let now = start + Duration::from_secs(n * 10);
                restarts.crashed(now).map(|at| at - now)
            })
            .collect();
        assert_eq!(
            waits,
            [
                Some(Duration::ZERO),
                Some(Duration::from_secs(1)),
                Some(Duration::from_secs(2)),
                Some(Duration::from_secs(4)),
                None,
            ]
        );
    }

    #[test]
    fn crashes_leave_the_window_after_five_minutes() {
        let start = Instant::now();
        let mut restarts = Restarts::default();
        for n in 0..4 {
            restarts.crashed(start + Duration::from_secs(n));
        }
        let later = start + CRASH_WINDOW + Duration::from_secs(2);
        assert_eq!(
            restarts.crashed(later),
            Some(later + Duration::from_secs(1))
        );
    }
}
