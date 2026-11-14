// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What `maxd` keeps for one workspace move, in its `moves` table. The
//! record is the only thing that opens the guard on a `ws-` container.

use std::collections::BTreeMap;

use maxhq_protocol::hosts::{Move, MovePlan, MoveState, MoveStep};
use maxhq_protocol::ids::{DeviceId, MoveId, WorkspaceId};
use maxhq_protocol::model::Vendor;
use maxhq_protocol::time::Timestamp;
use serde::{Deserialize, Serialize};

/// What you approved, which a guarded write checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Approval {
    /// A step, from `RunMoveStep`, or the retire from `RetireMove`.
    Step(MoveStep),
    /// The move back, from `MoveBack`.
    Back,
}

/// What the look found that later steps use.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Found {
    /// The agent CLI it runs.
    pub vendor: Option<Vendor>,
    /// The agent's session or thread.
    pub session: Option<String>,
    /// The folder the agent runs in, where the repo is.
    pub cwd: Option<String>,
    /// The repo's `origin` remote, or the repo the old scripts named on the
    /// container when the agent's folder has no git config.
    pub repo: Option<String>,
    /// The project's name.
    pub title: Option<String>,
    /// The pool its root disk is on.
    pub pool: Option<String>,
    /// Its `boot.autostart`, empty when unset, so a move back sets it back.
    pub autostart: String,
}

/// One move, as `maxd` keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// Stable for the life of the move.
    pub id: MoveId,
    /// This device.
    pub device: DeviceId,
    /// The original, such as `ws-alpha`.
    pub source: String,
    /// The copy, such as `mhq-alpha`.
    pub target: String,
    /// The copy's workspace, once it's copied.
    pub workspace: Option<WorkspaceId>,
    /// The step it's at.
    pub step: MoveStep,
    /// How that step stands.
    pub state: MoveState,
    /// What the look found and what each step changes.
    pub plan: Option<MovePlan>,
    /// Whether the original changed since the snapshot.
    #[serde(default)]
    pub source_changed: bool,
    /// When the keep ends.
    pub kept_until: Option<Timestamp>,
    /// What a move back or a retire found that wasn't saved.
    #[serde(default)]
    pub unsaved: Vec<String>,
    /// What's left to do by hand once the last original is retired.
    #[serde(default)]
    pub checklist: Vec<String>,
    /// The step you approved that runs now. Cleared once it ends.
    #[serde(default)]
    pub approved: Option<Approval>,
    /// What the look found.
    #[serde(default)]
    pub found: Found,
    /// The original's files' versions at the snapshot, by path, None for
    /// one that wasn't there.
    #[serde(default)]
    pub marks: BTreeMap<String, Option<String>>,
    /// The original's unsaved work, measured in the copy at the switch,
    /// when the two are the same.
    #[serde(default)]
    pub original_unsaved: Vec<String>,
    /// The warning a copy or a retire showed last, which the next approval
    /// of the same step goes past.
    #[serde(default)]
    pub warned: Option<String>,
    /// Whether the copy ran the agent since it was made, so it may hold
    /// work of its own.
    #[serde(default)]
    pub copy_used: bool,
}

impl Record {
    /// A move at its look, waiting for you.
    pub fn new(id: MoveId, device: DeviceId, source: String, target: String) -> Self {
        Self {
            id,
            device,
            source,
            target,
            workspace: None,
            step: MoveStep::Look,
            state: MoveState::Ready,
            plan: None,
            source_changed: false,
            kept_until: None,
            unsaved: Vec::new(),
            checklist: Vec::new(),
            approved: None,
            found: Found::default(),
            marks: BTreeMap::new(),
            original_unsaved: Vec::new(),
            warned: None,
            copy_used: false,
        }
    }

    /// Whether it went back or retired its original.
    pub fn is_over(&self) -> bool {
        matches!(self.state, MoveState::MovedBack | MoveState::Retired)
    }

    /// The move as clients see it.
    pub fn entity(&self) -> Move {
        Move {
            id: self.id,
            device: self.device,
            source: self.source.clone(),
            target: self.target.clone(),
            workspace: self.workspace,
            step: self.step,
            state: self.state.clone(),
            plan: self.plan.clone(),
            source_changed: self.source_changed,
            kept_until: self.kept_until,
            unsaved: self.unsaved.clone(),
            checklist: self.checklist.clone(),
        }
    }
}
