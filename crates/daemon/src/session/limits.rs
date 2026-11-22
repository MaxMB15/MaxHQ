// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Carrying on after a usage limit. A Claude Code or Codex agent whose turn
//! the limit cut off gets a message to carry on once the limit resets, when
//! its own switch says so, or when its device's default for its vendor does
//! and it has no switch of its own. Deciding at the reset rather than at the
//! cut means a switch you turn while the agent waits still counts. A wait
//! that ends before its reset time, as one a turn the CLI starts by itself
//! can end, is checked again a minute after the reset, so an agent it left
//! idle still carries on. Cursor and other ACP agents don't say when a
//! limit resets, so they're left alone. A banked reset you use on a Codex
//! account brings its agents' reset time forward to now.

use std::time::{Duration, Instant};

use maxhq_protocol::ids::{AccountId, AgentId, SessionId};
use maxhq_protocol::model::{Activity, Vendor};

use super::held::is_busy;
use super::recovery::LIMIT_MESSAGE;
use super::Manager;
use crate::drive::{self, Effects};
use crate::hub::Moved;

/// How long after a reset the check for an agent left idle runs, so the
/// end of the wait the driver sees at the reset comes first.
const CHECK_AFTER: Duration = Duration::from_secs(60);

/// Whether an agent is waiting for a limit to reset.
fn waits(activity: &Activity) -> bool {
    matches!(activity, Activity::WaitingForLimitReset { .. })
}

/// Whether an agent sits idle.
fn idle(activity: &Activity) -> bool {
    matches!(activity, Activity::Idle | Activity::IdleCacheWarm { .. })
}

impl Manager {
    /// Whether a held lead carries on once a limit that cut its turn off
    /// resets: its own switch, or else its device's default for its vendor.
    pub(super) fn continues_at_limit(&self, lead: AgentId) -> bool {
        let Some(held) = self.held.get(&lead) else {
            return false;
        };
        let vendor = held.vendor();
        if !matches!(vendor, Vendor::Claude | Vendor::Codex) {
            return false;
        }
        self.hub
            .inbox(lead)
            .continue_at_limit
            .unwrap_or_else(|| self.device_settings().continues_at_limit(vendor))
    }

    /// Notes a held agent's turn stopping at a usage limit, and sends it a
    /// message to carry on once the limit resets with the agent idle. A wait
    /// that ends before its reset time keeps the note until the check after
    /// it. A wait you end yourself, by a message that goes when the limit
    /// resets or one still queued, gets none, and neither does an agent you
    /// paused.
    pub(super) fn continue_at_limit(&mut self, moved: &Moved) {
        let lead = moved.agent;
        let now = self.clock.wall();
        let Some(held) = self.held.get_mut(&lead) else {
            return;
        };
        if let Activity::WaitingForLimitReset { resets_at } = &moved.to {
            if moved.from.as_ref().is_some_and(is_busy) {
                held.cut_at_limit = true;
            }
            if held.cut_at_limit {
                held.limit_resets_at = resets_at.or(held.limit_resets_at);
                self.mark_dirty(lead);
            }
            return;
        }
        if !moved.from.as_ref().is_some_and(waits) {
            return;
        }
        // A restart of the agent's machine starts it again mid-wait, and
        // the restart takes the mark over from there.
        if matches!(moved.to, Activity::Starting) {
            return;
        }
        if held.cut_at_limit && held.limit_resets_at.is_some_and(|at| at > now) {
            return;
        }
        held.limit_resets_at = None;
        let cut = std::mem::take(&mut held.cut_at_limit);
        self.mark_dirty(lead);
        if cut && idle(&moved.to) {
            self.carry_on(lead);
        }
    }

    /// When the next check for an agent whose wait ended before its reset
    /// is due.
    pub(super) fn carry_on_deadline(&self) -> Option<Instant> {
        let wall = self.clock.wall();
        let after = CHECK_AFTER.as_millis() as u64;
        self.held
            .values()
            .filter(|held| held.cut_at_limit)
            .filter_map(|held| held.limit_resets_at)
            .min()
            .map(|at| {
                self.clock.now() + Duration::from_millis((at.0 + after).saturating_sub(wall.0))
            })
    }

    /// Checks the agents whose limit reset a minute ago. One left idle
    /// carries on, one still shown waiting is left to the end of its wait,
    /// and one doing anything else went on without it.
    pub(super) fn carry_ons_due(&mut self) {
        let due = self
            .clock
            .wall()
            .0
            .saturating_sub(CHECK_AFTER.as_millis() as u64);
        let leads: Vec<AgentId> = self
            .held
            .iter()
            .filter(|(_, held)| {
                held.cut_at_limit && held.limit_resets_at.is_some_and(|at| at.0 <= due)
            })
            .map(|(lead, _)| *lead)
            .collect();
        for lead in leads {
            self.reset_came(lead);
        }
    }

    /// Carries on the Codex agents a usage limit held on `account`, after
    /// a banked reset you used there landed, as their reset time would
    /// have: one left idle carries on now, and one still shown waiting
    /// once its wait ends, which its app-server is asked to check now.
    /// Only an agent whose switch, or else its device's default, says to
    /// carry on counts. The others keep their reset time, as do other
    /// accounts' agents.
    pub(super) async fn reset_used(&mut self, account: AccountId) {
        let leads: Vec<AgentId> = self
            .held
            .iter()
            .filter(|(_, held)| {
                held.account == account && held.vendor() == Vendor::Codex && held.cut_at_limit
            })
            .map(|(lead, _)| *lead)
            .collect();
        let mut servers: Vec<SessionId> = Vec::new();
        for lead in leads {
            if !self.continues_at_limit(lead) {
                continue;
            }
            let Some(held) = self.held.get(&lead) else {
                continue;
            };
            let carries_on = held.paused.is_none() && held.fault.is_none();
            if carries_on
                && waits(&held.agent.activity)
                && let Some(session) = self.session_of(held)
                && !servers.contains(&session)
            {
                servers.push(session);
            }
            self.reset_came(lead);
        }
        for session in servers {
            let Some(hold) = self.holds.get_mut(&session) else {
                continue;
            };
            let mut fx = Effects::default();
            hold.driver.limits_lifted(&mut fx);
            self.apply_fx(session, fx).await;
        }
    }

    /// Takes a held lead's limit as reset. One left idle carries on, one
    /// still shown waiting is left to the end of its wait, and one doing
    /// anything else went on without it.
    fn reset_came(&mut self, lead: AgentId) {
        let Some(held) = self.held.get_mut(&lead) else {
            return;
        };
        held.limit_resets_at = None;
        let activity = held.agent.activity.clone();
        if !waits(&activity) {
            held.cut_at_limit = false;
        }
        self.mark_dirty(lead);
        if idle(&activity) {
            self.carry_on(lead);
        }
    }

    /// Forgets a carry-on the agent was owed, as a message you send ends it.
    pub(super) fn drop_carry_on(&mut self, lead: AgentId) {
        let Some(held) = self.held.get_mut(&lead) else {
            return;
        };
        if held.cut_at_limit || held.limit_resets_at.is_some() {
            held.cut_at_limit = false;
            held.limit_resets_at = None;
            self.mark_dirty(lead);
        }
    }

    /// Sends the carry-on to a lead whose limit reset, unless you paused
    /// it, it has a fault, its switch is off or a message of yours is
    /// queued on it.
    fn carry_on(&mut self, lead: AgentId) {
        let Some(held) = self.held.get(&lead) else {
            return;
        };
        if held.paused.is_some() || held.fault.is_some() {
            return;
        }
        if !self.continues_at_limit(lead) || self.has_queued(lead) {
            return;
        }
        self.send_own(
            lead,
            drive::Message {
                text: LIMIT_MESSAGE.to_owned(),
                refs: Vec::new(),
                attachments: Vec::new(),
            },
        );
    }
}
