// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Agents an automation keeps active. Auto-settle leaves be an agent an
//! enabled scheduled task sends its runs to, whatever makes the task
//! run: you keep the agent for the task. You can still settle it, and
//! the next run brings it back.
//!
//! A task lives on its home device, but the agent's own device decides
//! whether it settles. This device's own tasks are read where they're
//! kept. Each other device tells this one which agents here its enabled
//! tasks reach, every time that changes and again every hour
//! ([`crate::tasks::targets`]). What it said is stored, so it holds
//! across a restart, until it says otherwise or goes quiet for
//! [`LAPSE`].

use std::collections::HashMap;
use std::time::Duration;

use maxhq_core::inbox;
use maxhq_protocol::ids::{AgentId, DeviceId};
use maxhq_protocol::time::Timestamp;
use serde::{Deserialize, Serialize};
use tracing::warn;

use super::super::{stopping, Input, Manager, Sessions};

/// How long what another device said holds once it stops saying it. It
/// says it every hour it reaches this device, so this only drops what a
/// device that's gone said, or one that's been off for a week, whose
/// tasks don't run either.
pub(in crate::session) const LAPSE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// What each other device last said its enabled tasks reach here.
pub(in crate::session) type Heard = HashMap<DeviceId, Said>;

/// What one device said, as stored.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::session) struct Said {
    /// The device the tasks live on.
    home: DeviceId,
    /// The agents here they reach.
    agents: Vec<AgentId>,
    /// When it said so.
    at: Timestamp,
}

impl Manager {
    /// Puts back what the other devices said before `maxd` stopped, and
    /// forgets what has lapsed since.
    pub(super) fn load_task_targets(&mut self) {
        let stored: Vec<Said> = self.db.task_targets().unwrap_or_else(|err| {
            warn!(%err, "couldn't read what other devices' tasks reach here");
            Vec::new()
        });
        let now = self.clock.wall();
        for said in stored {
            if lapsed(&said, now) {
                self.forget_task_targets(said.home);
            } else {
                self.inbox.heard.insert(said.home, said);
            }
        }
    }

    /// Whether an enabled task reaches the agent: one on this device, or
    /// one another device said reaches it, within [`LAPSE`].
    pub(in crate::session) fn automated(&self, agent: AgentId) -> bool {
        let tasks = self.tasks.watch();
        let here = inbox::automated(tasks.borrow().iter())
            .any(|(device, id)| device == self.device && id == agent);
        let now = self.clock.wall();
        here || self
            .inbox
            .heard
            .values()
            .any(|said| said.agents.contains(&agent) && !lapsed(said, now))
    }

    /// Keeps what `home` said: the agents here its enabled tasks reach.
    /// None forgets what it said before. This device's own tasks are read
    /// where they're kept, so it doesn't count.
    pub(in crate::session) fn heard_task_targets(&mut self, home: DeviceId, agents: Vec<AgentId>) {
        if home == self.device {
            return;
        }
        if agents.is_empty() {
            self.forget_task_targets(home);
            return;
        }
        let said = Said {
            home,
            agents,
            at: self.clock.wall(),
        };
        if let Err(err) = self.db.put_task_targets(home, &said) {
            warn!(%err, %home, "couldn't store what the device's tasks reach here");
        }
        self.inbox.heard.insert(home, said);
    }

    fn forget_task_targets(&mut self, home: DeviceId) {
        self.inbox.heard.remove(&home);
        if let Err(err) = self.db.remove_task_targets(home) {
            warn!(%err, %home, "couldn't forget what the device's tasks reach here");
        }
    }
}

/// Whether what a device said no longer holds at `now`.
fn lapsed(said: &Said, now: Timestamp) -> bool {
    said.at.after(LAPSE) <= now
}

impl Sessions {
    /// Keeps what another device's `maxd` said: the agents here its
    /// enabled tasks send runs to, so auto-settle leaves them be. Device
    /// links call it.
    pub fn task_targets(&self, from: DeviceId, agents: Vec<AgentId>) -> Result<(), String> {
        self.tx
            .send(Input::TaskTargets { from, agents })
            .map_err(|_| stopping().text)
    }
}
