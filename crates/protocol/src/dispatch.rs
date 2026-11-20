// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The dispatcher: an agent on the Mac behind the header's text field. It
//! reads what MaxHQ knows, asks what a request leaves open, and submits a
//! plan. Nothing in the plan runs until you confirm it in the app.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::agents::MoveTo;
use crate::ids::{AgentId, DeviceId, IdempotencyKey, ProjectId, TaskId, WorkspaceId};
use crate::message::{AgentSpec, ToolTarget};
use crate::model::{DeviceSettingChange, InboxChange, PermissionMode};
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
    /// The changes it made at once that Undo may still put back, by the
    /// key on each one's card. An older `maxd` leaves it out, which reads
    /// as none.
    #[serde(default)]
    pub undo: Vec<IdempotencyKey>,
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
    /// Moves an agent to another device: it stops and settles, and a new
    /// one starts there from a summary of its conversation.
    MoveAgent {
        /// The device the agent is on.
        device: DeviceId,
        /// The agent.
        agent: AgentId,
        /// Where it goes.
        to: MoveTo,
    },
    /// Changes MaxHQ itself: an agent's name, inbox or mode, a project, a
    /// workspace, a task, the device's settings or tools, or a permission.
    /// A `maxd` from before [`crate::message::DISPATCHER_ADMIN_SINCE`]
    /// never plans one.
    Admin {
        /// The device it changes.
        device: DeviceId,
        /// What it does.
        action: AdminAction,
    },
}

/// A change the dispatcher makes to MaxHQ itself, as a plan item or at
/// once. Each one is a request the app already sends.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum AdminAction {
    /// Renames an agent.
    RenameAgent {
        /// The agent.
        agent: AgentId,
        /// Its new title.
        title: String,
    },
    /// Snoozes, wakes, settles or unsettles an agent, or puts its inbox
    /// back as it was.
    SetInbox {
        /// The agent.
        agent: AgentId,
        /// The change.
        change: InboxChange,
    },
    /// Marks what an agent did as read.
    MarkRead {
        /// The agent.
        agent: AgentId,
    },
    /// Sets an agent's permission mode.
    SetMode {
        /// The agent.
        agent: AgentId,
        /// The mode.
        mode: PermissionMode,
    },
    /// Sets an agent's model.
    SetModel {
        /// The agent.
        agent: AgentId,
        /// The model.
        model: String,
    },
    /// Sets an agent's effort level.
    SetEffort {
        /// The agent.
        agent: AgentId,
        /// The level, or none for its model's default.
        effort: Option<String>,
    },
    /// Stops an agent's turn.
    Interrupt {
        /// The agent.
        agent: AgentId,
    },
    /// Stops an agent.
    StopAgent {
        /// The agent.
        agent: AgentId,
    },
    /// Deletes an agent and its timeline.
    DeleteAgent {
        /// The agent.
        agent: AgentId,
    },
    /// Hides a project from the sidebar, or shows it again.
    HideProject {
        /// The project.
        project: ProjectId,
        /// Whether it's hidden.
        hidden: bool,
    },
    /// Changes some of a project's settings on the device.
    SetProject {
        /// The project.
        project: ProjectId,
        /// The settings to change.
        changes: Vec<ProjectSettingChange>,
    },
    /// Starts a workspace.
    StartWorkspace {
        /// The workspace.
        workspace: WorkspaceId,
    },
    /// Stops a workspace.
    StopWorkspace {
        /// The workspace.
        workspace: WorkspaceId,
    },
    /// Turns a scheduled task on or off.
    SetTask {
        /// The task.
        task: TaskId,
        /// Whether it runs.
        enabled: bool,
    },
    /// Runs a scheduled task now.
    RunTask {
        /// The task.
        task: TaskId,
    },
    /// Deletes a scheduled task.
    DeleteTask {
        /// The task.
        task: TaskId,
    },
    /// Changes some of the device's settings.
    ChangeSettings {
        /// The settings to change.
        changes: Vec<DeviceSettingChange>,
    },
    /// Updates agent CLIs on the device and in its workspaces.
    UpdateTools {
        /// The CLIs and where.
        tools: Vec<ToolTarget>,
    },
    /// Lets an agent start, message and stop agents.
    GrantPermission {
        /// The agent.
        agent: AgentId,
        /// The devices it may run agents on.
        devices: Vec<DeviceId>,
        /// How many levels of agents may run below it.
        depth: u8,
    },
    /// Takes back what an agent was allowed to run.
    RevokePermission {
        /// The agent.
        agent: AgentId,
    },
}

/// One of a project's settings on a device, for
/// [`AdminAction::SetProject`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "field", rename_all = "snake_case")]
pub enum ProjectSettingChange {
    /// Whether its agents start again after a restart.
    ResumeOnRestart {
        /// The new value.
        value: bool,
    },
    /// Whether an agent settles once its PR merges.
    SettleOnMerge {
        /// The new value.
        value: bool,
    },
    /// The days after which an idle agent settles, or none for the
    /// device's own.
    AutoSettleDays {
        /// The new value.
        value: Option<u16>,
    },
    /// The ports its workspaces don't offer to forward.
    IgnoredPorts {
        /// The new value.
        value: Vec<u16>,
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
    /// The change was made.
    Done,
    /// The item didn't go, and the items after it didn't run.
    Failed {
        /// Why, for a person to read.
        reason: String,
    },
    /// An outcome from a newer `maxd`. The item went, and how needs a newer
    /// app to show.
    Unknown,
}
