// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The dispatcher: an agent on the Mac behind the header's text field. It
//! reads what MaxHQ knows, asks what a request leaves open, and submits a
//! plan. Nothing in the plan runs until you confirm it in the app.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::{AgentId, DeviceId, TaskId, WorkspaceId};
use crate::message::AgentSpec;
use crate::tasks::{TaskTarget, Trigger};

/// A device's dispatcher, as its `maxd` publishes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Dispatcher {
    /// The device it runs on.
    pub device: DeviceId,
    /// The workspace of its own folder. Its agents run there, and clients
    /// leave them out of the sidebar.
    pub workspace: WorkspaceId,
    /// The agent of the conversation now. None before the first request,
    /// and after you start a new conversation.
    pub agent: Option<AgentId>,
    /// The plan it submitted, until you confirm or dismiss it.
    pub plan: Option<Plan>,
}

/// What the dispatcher proposes to do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Plan {
    /// Numbers each plan, so a confirm runs the one you saw.
    pub id: u32,
    /// The items, which run in this order.
    pub items: Vec<PlanItem>,
}

/// One item of a plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PlanItem {
    /// Why it's in the plan, in one line.
    pub reason: String,
    /// What it does.
    pub step: PlanStep,
}

/// What one item of a plan does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlanStep {
    /// Starts an agent. One with no first message starts idle.
    StartAgent {
        /// The device it starts on.
        device: DeviceId,
        /// What it should be.
        spec: Box<AgentSpec>,
    },
    /// Sends an agent that runs a message.
    SendMessage {
        /// The device the agent is on.
        device: DeviceId,
        /// The agent.
        agent: AgentId,
        /// The message.
        text: String,
    },
    /// Makes a scheduled task.
    ScheduleTask {
        /// The device that holds and runs it.
        device: DeviceId,
        /// Its name.
        name: String,
        /// What each run sends.
        prompt: String,
        /// What makes it run.
        trigger: Box<Trigger>,
        /// The agent each run reaches.
        target: PlanTarget,
    },
}

/// The agent a planned task's runs reach.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlanTarget {
    /// An agent that runs now, or a new agent for each run.
    Task {
        /// The target, as the task keeps it.
        target: TaskTarget,
    },
    /// The agent an earlier item of the same plan starts.
    Item {
        /// That item's place in the plan, from 0.
        item: u32,
    },
}

/// How one item of a confirmed plan went.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlanOutcome {
    /// The agent started.
    Started {
        /// The new agent.
        agent: AgentId,
    },
    /// The message went to the agent.
    Sent {
        /// The agent.
        agent: AgentId,
    },
    /// The task was saved.
    TaskSaved {
        /// The new task.
        task: TaskId,
    },
    /// The item didn't go, and the items after it didn't run.
    Failed {
        /// Why, for a person to read.
        reason: String,
    },
    /// An outcome from a newer `maxd`. The item went, and how needs a newer
    /// app to show.
    Unknown,
}
