// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Whether an idle CLI may stop to free its memory. `--resume` brings the
//! conversation back, but nothing the process alone holds: a turn, a
//! prompt, a sub-agent, a background command or monitor, a wakeup or cron
//! job it set for itself, or a message still on its way.

use maxhq_protocol::processes::AgentWait;
use maxhq_protocol::time::Timestamp;

use super::ClaudeSession;
use crate::drive::claude::convo::ToolState;

/// How long a cron job lasts at most before the CLI drops it.
const CRON_LIFETIME_MS: u64 = 7 * 24 * 60 * 60 * 1000;

impl ClaudeSession {
    /// Whether the CLI can stop now and resume later with nothing lost.
    pub(super) fn releasable(&self) -> bool {
        self.initialized
            // A transcript still being read holds what came meanwhile.
            && self.seeding.is_none()
            && !self.exited
            && !self.stopping
            && !self.busy
            && !self.background
            // A reply only reads the CLI's state back, which the next start
            // reads again, unless an action of yours waits on it.
            && self.ours.values().all(|ours| ours.key().is_none())
            && self.prompts.is_empty()
            && self.asked.is_empty()
            && self.calls.is_empty()
            && self.subs.is_empty()
            && self.tasks.is_empty()
            && self.held.is_empty()
            && self.wait.is_none()
            && self.sends.values().all(|sent| sent.started)
            && self
                .convo
                .tools
                .values()
                .all(|tool| tool.state == ToolState::Ended)
            && !self.scheduled()
    }

    /// Whether a wakeup or a cron job the model set is still to fire.
    /// Only the running CLI fires them. A cron job the model never deleted
    /// counts until a week after the last one it made, when the CLI drops
    /// it anyway.
    fn scheduled(&self) -> bool {
        let now = self.now().0;
        let schedule = &self.convo.schedule;
        schedule.wakeups.iter().any(|&due| due > now)
            || (schedule.crons > 0 && schedule.last_cron + CRON_LIFETIME_MS > now)
    }

    /// What the idle agent waits on that wakes it: a background command or
    /// monitor first, since it may end any moment, then its soonest wakeup.
    pub(super) fn waits_on(&self) -> Option<AgentWait> {
        if self.working() || self.exited {
            return None;
        }
        let task = self
            .tasks
            .values()
            .filter(|task| task.sub.is_none())
            .map(|task| task.description.clone().unwrap_or_default())
            .min();
        if let Some(name) = task {
            return Some(AgentWait::Task { name });
        }
        let now = self.now().0;
        let due = self
            .convo
            .schedule
            .wakeups
            .iter()
            .copied()
            .filter(|&due| due > now)
            .min()?;
        Some(AgentWait::Wakeup { at: Timestamp(due) })
    }
}
