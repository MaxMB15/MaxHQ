// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Restarts of the device or a container, per decision A-11. Before a
//! planned restart, `maxd host quiesce` asks for a wind-down: each busy
//! agent is asked to finish the step it's on, and when the window closes
//! each one that was working gets a one-shot flag saying whether it wound
//! down or was cut off. After a restart, every agent that was busy comes
//! back with a continue message, and its queue with it. Idle agents come
//! back with nothing sent. Your pause survives, and a project with resume
//! on restart turned off leaves its agents paused. A limit wait resumes
//! once its limit resets. Stopping a container workspace flags each agent
//! at work in it the same way, so once you start it again the agent hears
//! what the stop cut off.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use maxhq_core::fs::{write_atomic, Atomic};
use maxhq_protocol::ids::{AgentId, BootId, IdempotencyKey, WorkspaceId};
use maxhq_protocol::message::{ActionOutcome, SendMode};
use maxhq_protocol::model::{PauseReason, Vendor};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::TimelineItem;
use serde::{Deserialize, Serialize};
use tracing::warn;

use super::held::{internal_key, is_busy, Internal, Target};
use super::idem::Outcome;
use super::recovery::{restart_message, Downtime, Next};
use super::revive::Cut;
use super::Manager;
use crate::drive::Action;

/// How long a flag counts after it's written.
const FLAG_LIFE: Duration = Duration::from_secs(12 * 60 * 60);

/// What an agent that was working when its machine restarted is told, in
/// its timeline.
const RESTARTED: &str = "This agent's machine restarted while it was working, so MaxHQ \
                         started it again.";

/// How an agent stood when a wind-down's window closed, or when you
/// stopped its container.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlagKind {
    /// It finished the step it was on.
    WoundDown,
    /// It was still working.
    CutOff,
    /// Its machine went down with no wind-down. MaxHQ can't write this one
    /// as it happens, so it only reads it.
    Crashed,
    /// It was working when you stopped its container.
    Stopped,
}

/// A one-shot flag for an agent, at `<data>/resume/<agent>.json`. The
/// next restart that brings the agent back reads it and deletes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Flag {
    /// The agent it's for.
    pub agent: AgentId,
    /// How the agent stood.
    pub kind: FlagKind,
    /// The tool items that were still running, for a cut-off or stopped
    /// agent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cut_off_items: Vec<TimelineItem>,
    /// When it was written.
    pub written_at: Timestamp,
    /// When it stops counting.
    pub expires_at: Timestamp,
    /// The boot of the machine the agent ran on when it was written. A
    /// flag from the boot that's still running means no restart happened.
    pub boot_id: Option<BootId>,
}

/// A wind-down in progress.
pub(super) struct Winding {
    /// When its window closes.
    pub until: Instant,
    /// The requests waiting for it to finish.
    pub keys: Vec<IdempotencyKey>,
    /// The agents asked to finish their step.
    pub steered: HashSet<AgentId>,
}

/// What a busy agent is told when a wind-down starts.
fn steer_message(window: Duration) -> String {
    let minutes = window.as_secs().div_ceil(60).max(1);
    let unit = if minutes == 1 { "minute" } else { "minutes" };
    format!(
        "The machine you run on restarts in about {minutes} {unit}. Finish the step you're on \
         and don't start any long-running command. MaxHQ starts you again after the restart, \
         and you carry on from there."
    )
}

impl Manager {
    /// Decides how an agent comes back after its machine restarted, and
    /// sets what it owes. Returns whether it starts again now. It doesn't
    /// when you paused or stopped it, when its project has resume on
    /// restart turned off, when it made no progress too many times, or
    /// when it waits for a limit to reset.
    pub(super) fn after_restart(&mut self, cut: &Cut, stopping: bool) -> bool {
        let lead = cut.lead;
        let Some(guest) = self.held.get(&lead).map(|held| held.guest) else {
            return false;
        };
        let boot = self.boot_at(guest);
        let flag = self.take_flag(lead, boot.as_ref());
        let resume_on = self.resume_on_restart(lead);
        let carries_on = self.continues_at_limit(lead);
        let now = self.clock.wall();
        let Some(held) = self.held.get_mut(&lead) else {
            return false;
        };
        held.down_since = Some(now);
        let cut_at_limit = std::mem::take(&mut held.cut_at_limit);
        held.limit_resets_at = None;
        self.limit_waits.remove(&lead);
        if stopping || held.paused.is_some() {
            // Your pause survives the restart, and so does its queue.
            held.paused.get_or_insert(PauseReason::ByYou);
            held.resuming = false;
            return false;
        }
        let wait = held.resume_after.filter(|at| *at > now);
        let flagged = flag.is_some();
        let downtime = match flag.as_ref().map(|flag| flag.kind) {
            None => Downtime::Restarted,
            Some(FlagKind::Stopped) => Downtime::Stopped,
            Some(_) => Downtime::Planned,
        };
        // The CLI may have ended the tools a stop cut off as it died, so
        // the flag knows them when the timeline no longer does.
        let items = match flag {
            Some(flag) if cut.items.is_empty() => flag.cut_off_items,
            _ => cut.items.clone(),
        };
        // A wound-down agent was mid-task, and so was one a limit cut off
        // that carries on once the limit resets.
        let limited = cut_at_limit && held.resume_after.is_some() && carries_on;
        if cut.busy || flagged || limited {
            held.owed = Some(restart_message(&items, downtime));
        }
        if !resume_on {
            held.paused = Some(PauseReason::ResumeOff);
            held.resuming = false;
            return false;
        }
        let owes = held.owed.is_some();
        if owes && held.resumes.after_crash() == Next::Pause {
            self.pause_no_progress(lead);
            return false;
        }
        if let Some(at) = wait {
            held.resuming = false;
            let left = Duration::from_millis(at.0.saturating_sub(now.0));
            self.limit_waits.insert(lead, self.clock.now() + left);
            return false;
        }
        if owes {
            self.notice(lead, RESTARTED);
        }
        true
    }

    /// A Claude or Cursor CLI that died with its machine. It comes back as
    /// A-11 says.
    pub(super) fn agent_restarted(&mut self, cut: Cut, stopping: bool) {
        let lead = cut.lead;
        if let Some(held) = self.held.get_mut(&lead) {
            held.session = None;
        }
        if self.after_restart(&cut, stopping) {
            self.resume_agent(lead);
        } else {
            self.save(lead);
            self.publish(lead);
        }
    }

    /// Resumes the agents whose limit has reset since a restart.
    pub(super) fn limits_reset(&mut self) {
        let now = self.clock.now();
        let due: Vec<AgentId> = self
            .limit_waits
            .iter()
            .filter(|(_, at)| **at <= now)
            .map(|(lead, _)| *lead)
            .collect();
        for lead in due {
            self.limit_waits.remove(&lead);
            let Some(held) = self.held.get_mut(&lead) else {
                continue;
            };
            held.resume_after = None;
            if held.paused.is_some() || held.fault.is_some() {
                continue;
            }
            match held.vendor() {
                Vendor::Codex => {
                    held.resuming = true;
                    self.mark_dirty(lead);
                    self.publish(lead);
                    self.follow_up(lead);
                }
                _ if held.session.is_none() => self.resume_agent(lead),
                _ => self.follow_up(lead),
            }
        }
    }

    /// Whether a lead or any of its sub-agents is working.
    pub(super) fn working(&self, lead: AgentId) -> bool {
        self.held
            .get(&lead)
            .is_some_and(|held| is_busy(&held.agent.activity))
            || self
                .subs_of(lead)
                .iter()
                .filter_map(|sub| self.subs.get(sub))
                .any(|sub| is_busy(&sub.activity))
    }

    /// The held leads that run and are working.
    fn busy_leads(&self) -> Vec<AgentId> {
        self.held
            .values()
            .filter(|held| held.paused.is_none() && self.session_of(held).is_some())
            .map(|held| held.agent.id)
            .filter(|lead| self.working(*lead))
            .collect()
    }

    /// Starts a wind-down for `maxd host quiesce`. Each busy agent is asked
    /// to finish its step, and the request is answered once they all have
    /// or the window closes. A second request joins the first.
    pub(super) fn wind_down(
        &mut self,
        key: IdempotencyKey,
        window_secs: u32,
    ) -> Option<ActionOutcome> {
        if let Some(winding) = &mut self.winding {
            winding.keys.push(key);
            return None;
        }
        let window = Duration::from_secs(window_secs.into());
        let until = Timestamp(
            self.clock
                .wall()
                .0
                .saturating_add(u64::try_from(window.as_millis()).unwrap_or(u64::MAX)),
        );
        let mut device = match self.hub.get(&EntityKey::Device(self.device)) {
            Some(Entity::Device(device)) => device,
            _ => self.hub.device().clone(),
        };
        device.winding_down_until = Some(until);
        self.hub.apply([Change::Upsert {
            value: Entity::Device(device),
        }]);
        let busy = self.busy_leads();
        let text = steer_message(window);
        for lead in &busy {
            let key = internal_key();
            self.internal.insert(key, Internal::Plain);
            self.work.push_back((
                Target::Agent(*lead),
                Action::Send {
                    key,
                    agent: *lead,
                    text: text.clone(),
                    refs: Vec::new(),
                    attachments: Vec::new(),
                    mode: SendMode::Steer,
                },
            ));
        }
        self.winding = Some(Winding {
            until: self.clock.now() + window,
            keys: vec![key],
            steered: busy.into_iter().collect(),
        });
        None
    }

    /// Finishes a wind-down once no agent works or its window closes. Each
    /// agent that worked during it gets a flag. Agents never asked to
    /// finish a step and idle now get none, nor do paused ones.
    pub(super) fn check_winding(&mut self) {
        let Some(winding) = &self.winding else {
            return;
        };
        let busy = self.busy_leads();
        if self.clock.now() < winding.until && !busy.is_empty() {
            return;
        }
        let Some(winding) = self.winding.take() else {
            return;
        };
        let leads: Vec<AgentId> = self.held.keys().copied().collect();
        for lead in leads {
            let Some(held) = self.held.get(&lead) else {
                continue;
            };
            if held.paused.is_some() {
                continue;
            }
            let kind = if busy.contains(&lead) {
                FlagKind::CutOff
            } else if winding.steered.contains(&lead) {
                FlagKind::WoundDown
            } else {
                continue;
            };
            let cut_off_items = match kind {
                FlagKind::CutOff => self.cut(lead).items,
                _ => Vec::new(),
            };
            self.put_flag(lead, kind, cut_off_items, self.boot_at(held.guest));
        }
        for key in winding.keys {
            self.finish(key, Outcome::Action(ActionOutcome::Done));
        }
    }

    /// Flags each agent at work in a container you're stopping, with the
    /// tools it has running. Its CLI may end them as it dies, so once you
    /// start the container again, the flag is what says the stop cut them
    /// off. `boot` is the container's boot before the stop. A stop that
    /// fails leaves the boot as it was, so the flags are dropped when the
    /// link comes back.
    pub(super) fn flag_stopped(&self, workspace: WorkspaceId, boot: Option<BootId>) {
        let leads: Vec<AgentId> = self
            .busy_leads()
            .into_iter()
            .filter(|lead| {
                self.held
                    .get(lead)
                    .is_some_and(|held| held.guest == Some(workspace))
            })
            .collect();
        for lead in leads {
            let items = self.cut(lead).items;
            self.put_flag(lead, FlagKind::Stopped, items, boot.clone());
        }
    }

    /// Writes an agent a flag that counts for [`FLAG_LIFE`] from now.
    fn put_flag(
        &self,
        lead: AgentId,
        kind: FlagKind,
        cut_off_items: Vec<TimelineItem>,
        boot_id: Option<BootId>,
    ) {
        let written_at = self.clock.wall();
        let expires_at = Timestamp(
            written_at
                .0
                .saturating_add(u64::try_from(FLAG_LIFE.as_millis()).unwrap_or(u64::MAX)),
        );
        let flag = Flag {
            agent: lead,
            kind,
            cut_off_items,
            written_at,
            expires_at,
            boot_id,
        };
        if let Err(err) = self.write_flag(&flag) {
            warn!(%err, agent = %lead, "couldn't write the agent's restart flag");
        }
    }

    /// When the next wind-down check is due.
    pub(super) fn winding_until(&self) -> Option<Instant> {
        self.winding.as_ref().map(|winding| winding.until)
    }

    /// Whether the project an agent works in resumes its agents after a
    /// restart. It does unless you turned it off.
    fn resume_on_restart(&self, lead: AgentId) -> bool {
        let Some(held) = self.held.get(&lead) else {
            return true;
        };
        let project = match self.hub.get(&EntityKey::Workspace(held.agent.workspace)) {
            Some(Entity::Workspace(workspace)) => workspace.project,
            _ => None,
        };
        let Some(project) = project else {
            return true;
        };
        let key = EntityKey::ProjectSettings {
            project,
            device: self.device,
        };
        match self.hub.get(&key) {
            Some(Entity::ProjectSettings(settings)) => settings.resume_on_restart,
            _ => true,
        }
    }

    /// Where an agent's flag is kept.
    fn flag_path(&self, lead: AgentId) -> PathBuf {
        self.resume_dir.join(format!("{lead}.json"))
    }

    /// Writes a flag, replacing any the agent had.
    fn write_flag(&self, flag: &Flag) -> io::Result<()> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.resume_dir)?;
        let path = self.flag_path(flag.agent);
        let text = serde_json::to_vec(flag).map_err(io::Error::other)?;
        write_atomic(&path, &text, Atomic::PLAIN)
    }

    /// Reads an agent's flag and deletes it. A flag that expired, or that
    /// was written in the boot of the machine that runs now, counts as
    /// none.
    pub(super) fn take_flag(&mut self, lead: AgentId, boot: Option<&BootId>) -> Option<Flag> {
        let path = self.flag_path(lead);
        let text = match fs::read(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return None,
            Err(err) => {
                warn!(%err, agent = %lead, "couldn't read the agent's restart flag");
                return None;
            }
        };
        self.drop_flag(lead);
        let flag: Flag = match serde_json::from_slice(&text) {
            Ok(flag) => flag,
            Err(err) => {
                warn!(%err, agent = %lead, "the agent's restart flag doesn't parse");
                return None;
            }
        };
        let fresh = flag.expires_at > self.clock.wall();
        let restarted = boot.is_none() || flag.boot_id.as_ref() != boot;
        (flag.agent == lead && fresh && restarted).then_some(flag)
    }

    /// Deletes an agent's flag, if it has one.
    pub(super) fn drop_flag(&self, lead: AgentId) {
        match fs::remove_file(self.flag_path(lead)) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => warn!(%err, agent = %lead, "couldn't delete the agent's restart flag"),
        }
    }
}
