// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The dispatcher's tools for running MaxHQ itself, one for each area,
//! each with an `action`, and the risk table that says which changes it
//! makes at once, which it asks about first and which it never makes. Each
//! change is a request the app already sends.

use std::time::Duration;

use maxhq_protocol::dispatch::{AdminAction, PlanStep, ProjectSettingChange};
use maxhq_protocol::ids::{AgentId, DeviceId};
use maxhq_protocol::message::{SendMode, ToolTarget};
use maxhq_protocol::model::{DeviceSettingChange, DeviceSettings, InboxChange, PermissionMode};
use maxhq_protocol::model::{ProjectSettings, Tool as Cli};
use maxhq_protocol::state::ToolPlace;
use maxhq_protocol::time::Timestamp;
use serde_json::{json, Map, Value};

use super::agents::{flag, id, named, number, text, MAX_TITLE};
use super::dispatch::{self, Start, Step};
use super::{fields, tasks, Call, Tool};

/// The most things one change touches, and the most changes one request
/// makes without asking, before the dispatcher asks first.
pub const MAX_AT_ONCE: usize = 5;

/// The longest snooze, in minutes: 30 days.
const MAX_SNOOZE: u64 = 30 * 24 * 60;

/// The deepest a grant lets agents run below the one that holds it.
const MAX_DEPTH: u64 = 3;

/// The tools the dispatcher shares with other agents that change
/// something. Its copies take `asked` too.
pub const SHARED: [Tool; 7] = [
    Tool::StartAgent,
    Tool::SendAgent,
    Tool::InterruptAgent,
    Tool::MoveAgent,
    Tool::ScheduleTask,
    Tool::RunTaskNow,
    Tool::DeleteTask,
];

/// What `asked` reads, in each acting tool the dispatcher has.
const ASKED: &str = "True when the user's request asked for exactly this. False when it's \
                     your own idea, which the user then confirms.";

/// A change, checked: what it does, and on which device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Act {
    /// The device, or none for this one.
    pub device: Option<DeviceId>,
    /// What it does.
    pub action: AdminAction,
}

/// What a dispatcher call that changes something does: what a plan item
/// does, or a change to MaxHQ itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Wants {
    /// Starts, messages or moves an agent, or makes a task.
    Step(Step),
    /// Changes MaxHQ itself.
    Act(Act),
}

/// How the dispatcher goes about a change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Risk {
    /// It makes it at once and says so.
    Direct,
    /// It adds it to a plan the user confirms.
    Confirm,
    /// It never makes it.
    Never,
}

/// Whether the call asked for exactly what it does, as its `asked` says.
/// Left out reads as false, so the user confirms it.
pub fn asked(arguments: &Value) -> bool {
    fields(arguments)
        .ok()
        .and_then(|args| flag(&args, "asked").ok().flatten())
        .unwrap_or(false)
}

/// Whether a tool changes something when the dispatcher calls it.
pub fn acts(tool: Tool) -> bool {
    SHARED.contains(&tool) || tool.administers()
}

/// Adds `asked` to the schema of a tool the dispatcher shares.
pub(super) fn add_asked(schema: &mut Value) {
    schema["properties"]["asked"] = json!({"type": "boolean", "description": ASKED});
    if let Some(required) = schema["required"].as_array_mut() {
        required.push("asked".into());
    }
}

/// The risk class of one change, before the switch and `asked` have their
/// say. `busy` is whether the agent it names works or waits on a prompt.
pub fn risk(step: &PlanStep, busy: bool) -> Risk {
    use AdminAction as A;
    let idle = if busy { Risk::Confirm } else { Risk::Direct };
    let action = match step {
        PlanStep::StartAgent { .. }
        | PlanStep::SendMessage { .. }
        | PlanStep::ScheduleTask { .. } => return Risk::Direct,
        PlanStep::MoveAgent { .. } => return Risk::Confirm,
        PlanStep::Admin { action, .. } => action,
    };
    match action {
        A::RenameAgent { .. }
        | A::SetInbox { .. }
        | A::MarkRead { .. }
        | A::HideProject { .. }
        | A::StartWorkspace { .. }
        | A::SetTask { .. }
        | A::RunTask { .. } => Risk::Direct,
        A::SetMode { mode, .. } if *mode == PermissionMode::FullAccess => Risk::Confirm,
        A::SetMode { .. } | A::SetModel { .. } | A::SetEffort { .. } => idle,
        A::Interrupt { .. } | A::StopAgent { .. } => idle,
        A::SetProject { changes, .. } => {
            let hides = |change: &ProjectSettingChange| {
                matches!(change, ProjectSettingChange::IgnoredPorts { .. })
            };
            if changes.iter().all(hides) {
                Risk::Direct
            } else {
                Risk::Confirm
            }
        }
        A::ChangeSettings { changes } if changes.len() > MAX_AT_ONCE => {
            worst(changes.iter().map(setting).chain([Risk::Confirm]))
        }
        A::ChangeSettings { changes } => worst(changes.iter().map(setting)),
        A::DeleteAgent { .. }
        | A::StopWorkspace { .. }
        | A::DeleteTask { .. }
        | A::UpdateTools { .. }
        | A::GrantPermission { .. }
        | A::RevokePermission { .. } => Risk::Confirm,
    }
}

/// How the dispatcher goes about a change of class `risk`: at once only
/// when the user asked for it, the switch lets it act, and the request
/// hasn't made [`MAX_AT_ONCE`] changes already.
pub fn decide(risk: Risk, asked: bool, acts: bool, made: usize) -> Risk {
    match risk {
        Risk::Never => Risk::Never,
        Risk::Direct if asked && acts && made < MAX_AT_ONCE => Risk::Direct,
        _ => Risk::Confirm,
    }
}

/// The worst of some classes, or direct for none.
fn worst(risks: impl Iterator<Item = Risk>) -> Risk {
    risks.fold(Risk::Direct, |worst, risk| match (worst, risk) {
        (Risk::Never, _) | (_, Risk::Never) => Risk::Never,
        (Risk::Confirm, _) | (_, Risk::Confirm) => Risk::Confirm,
        _ => Risk::Direct,
    })
}

/// The class of one device setting. Those that reach credentials, sign-ins
/// or the network, and the dispatcher's own switch, it never changes.
fn setting(change: &DeviceSettingChange) -> Risk {
    use DeviceSettingChange as S;
    match change {
        S::AutoSettleDays { .. }
        | S::SnoozeAtLimit { .. }
        | S::ContinueClaudeAtLimit { .. }
        | S::ContinueCodexAtLimit { .. }
        | S::ClaudeEffort { .. }
        | S::CodexEffort { .. }
        | S::Placement { .. }
        | S::KeepAwake { .. }
        | S::IdleReleaseMinutes { .. }
        | S::MemoryItems { .. }
        | S::HistoryMib { .. } => Risk::Direct,
        S::DefaultPermission { .. }
        | S::WorkspaceNetwork { .. }
        | S::WorkspacePool { .. }
        | S::ToolUpdates { .. }
        | S::TestHosts { .. }
        | S::SkillLibrary { .. }
        | S::ShellIntegration { .. } => Risk::Confirm,
        S::CursorUsage { .. }
        | S::Network { .. }
        | S::ClaudeSetupToken { .. }
        | S::DispatcherActs { .. } => Risk::Never,
    }
}

/// What a dispatcher call that changes something does, or why it can't.
pub fn wants(call: Call) -> Result<Wants, String> {
    Ok(match call {
        Call::Admin(act) => Wants::Act(act),
        Call::StartAgent(start) => {
            if start.context_from.is_some() {
                return Err("Copy the [Agent: title] marker into `task` instead of \
                            `context_from`."
                    .into());
            }
            let agent = tasks::NewAgent {
                device: start.device,
                vendor: start.vendor,
                account: start.account,
                place: start.place,
                model: start.model,
                mode: start.mode,
            };
            if agent.place.is_none() {
                return Err("Give a `workspace` or a `folder`.".into());
            }
            Wants::Step(Step::Start(Start {
                agent,
                title: start.title,
                first: Some(start.task),
            }))
        }
        Call::SendAgent(message) if message.mode == SendMode::Interrupt => {
            return Err(
                "Messages wait for the turn to end. Stop the turn first with \
                        interrupt_agent."
                    .into(),
            )
        }
        Call::SendAgent(message) => Wants::Step(Step::Send {
            device: message.device,
            agent: message.agent,
            text: message.text,
        }),
        Call::MoveAgent(moved) => Wants::Step(Step::Move {
            device: moved.device,
            agent: moved.agent,
            to: moved.to,
        }),
        Call::ScheduleTask(schedule) => {
            if schedule.task.is_some() {
                return Err(
                    "You can make tasks but not change them. Turn one on or off \
                            with manage_task."
                        .into(),
                );
            }
            let target = match schedule.target {
                Some(tasks::Aim::Agent { device, agent }) => dispatch::Aim::Agent { device, agent },
                Some(tasks::Aim::New(new)) if new.place.is_some() => dispatch::Aim::New(new),
                Some(tasks::Aim::New(_)) => {
                    return Err("Give the new agent a `workspace` or a `folder`.".into())
                }
                _ => return Err("Give a `target`: an agent that runs, or new_agent.".into()),
            };
            let prompt = schedule.prompt.unwrap_or_default();
            Wants::Step(Step::Schedule {
                device: None,
                name: schedule.name.unwrap_or_else(|| {
                    let line = prompt.lines().next().unwrap_or_default();
                    line.chars().take(MAX_TITLE).collect()
                }),
                schedule: schedule.schedule.unwrap_or_default(),
                prompt,
                target,
            })
        }
        Call::InterruptAgent(stop) => Wants::Act(Act {
            device: stop.device,
            action: if stop.stop {
                AdminAction::StopAgent { agent: stop.agent }
            } else {
                AdminAction::Interrupt { agent: stop.agent }
            },
        }),
        Call::RunTaskNow(named) => Wants::Act(Act {
            device: None,
            action: AdminAction::RunTask { task: named.task },
        }),
        Call::DeleteTask(named) => Wants::Act(Act {
            device: None,
            action: AdminAction::DeleteTask { task: named.task },
        }),
        call => return Err(format!("{} doesn't change anything.", call.tool().name())),
    })
}

/// The tool that makes a change, for [`Call::tool`].
pub fn tool(action: &AdminAction) -> Tool {
    use AdminAction as A;
    match action {
        A::RenameAgent { .. }
        | A::SetInbox { .. }
        | A::MarkRead { .. }
        | A::SetMode { .. }
        | A::SetModel { .. }
        | A::SetEffort { .. }
        | A::Interrupt { .. }
        | A::StopAgent { .. }
        | A::DeleteAgent { .. } => Tool::ManageAgent,
        A::HideProject { .. } | A::SetProject { .. } => Tool::ManageProject,
        A::StartWorkspace { .. } | A::StopWorkspace { .. } => Tool::ManageWorkspace,
        A::SetTask { .. } | A::RunTask { .. } | A::DeleteTask { .. } => Tool::ManageTask,
        A::ChangeSettings { .. } | A::UpdateTools { .. } => Tool::ManageDevice,
        A::GrantPermission { .. } | A::RevokePermission { .. } => Tool::ManagePermission,
    }
}

/// Changes a project's settings.
pub fn apply_project(settings: &mut ProjectSettings, changes: &[ProjectSettingChange]) {
    for change in changes {
        match change.clone() {
            ProjectSettingChange::ResumeOnRestart { value } => settings.resume_on_restart = value,
            ProjectSettingChange::SettleOnMerge { value } => settings.settle_on_merge = value,
            ProjectSettingChange::AutoSettleDays { value } => settings.auto_settle_days = value,
            ProjectSettingChange::IgnoredPorts { value } => settings.ignored_ports = value,
        }
    }
}

/// The values a project has now of the settings `changes` names, which
/// put them back.
pub fn project_now(
    settings: &ProjectSettings,
    changes: &[ProjectSettingChange],
) -> Vec<ProjectSettingChange> {
    changes
        .iter()
        .map(|change| match change {
            ProjectSettingChange::ResumeOnRestart { .. } => ProjectSettingChange::ResumeOnRestart {
                value: settings.resume_on_restart,
            },
            ProjectSettingChange::SettleOnMerge { .. } => ProjectSettingChange::SettleOnMerge {
                value: settings.settle_on_merge,
            },
            ProjectSettingChange::AutoSettleDays { .. } => ProjectSettingChange::AutoSettleDays {
                value: settings.auto_settle_days,
            },
            ProjectSettingChange::IgnoredPorts { .. } => ProjectSettingChange::IgnoredPorts {
                value: settings.ignored_ports.clone(),
            },
        })
        .collect()
}

/// The values a device has now of the settings `changes` names, which put
/// them back.
pub fn settings_now(
    settings: &DeviceSettings,
    changes: &[DeviceSettingChange],
) -> Vec<DeviceSettingChange> {
    let all = settings.changes_from(None);
    changes
        .iter()
        .filter_map(|change| {
            let field = std::mem::discriminant(change);
            all.iter()
                .find(|now| std::mem::discriminant(*now) == field)
                .cloned()
        })
        .collect()
}

/// A short title for each admin tool.
pub(super) fn title(tool: Tool) -> &'static str {
    match tool {
        Tool::ManageAgent => "Manage an agent",
        Tool::ManageProject => "Manage a project",
        Tool::ManageWorkspace => "Manage a workspace",
        Tool::ManageTask => "Manage a scheduled task",
        Tool::ManageDevice => "Manage a device",
        Tool::ManagePermission => "Manage what an agent may run",
        _ => unreachable!("not an admin tool"),
    }
}

/// What the dispatcher reads about each admin tool.
pub(super) fn description(tool: Tool) -> &'static str {
    match tool {
        Tool::ManageAgent => {
            "Changes an agent, by the id list_agents gives: rename it, snooze it for some \
             minutes, wake, settle or unsettle it, mark it read, set its permission mode, \
             model or effort, or stop or delete it."
        }
        Tool::ManageProject => {
            "Hides a project from the sidebar or shows it, or changes its settings on a \
             device: whether its agents resume after a restart, whether an agent settles \
             once its PR merges, its auto-settle days, and the ports it doesn't offer to \
             forward."
        }
        Tool::ManageWorkspace => "Starts or stops a container workspace.",
        Tool::ManageTask => "Turns a scheduled task on or off.",
        Tool::ManageDevice => {
            "Changes a device's settings, as Settings shows them, or updates its agent CLIs \
             on the device and in its workspaces. Sign-ins, network access, Cursor usage and \
             your own switch stay with the user."
        }
        Tool::ManagePermission => {
            "Lets an agent start, message and stop other agents on its own device, or takes \
             that back."
        }
        _ => unreachable!("not an admin tool"),
    }
}

/// The JSON schema for each admin tool's arguments.
pub(super) fn input_schema(tool: Tool) -> Value {
    let id = |what: &str| json!({"type": "string", "minLength": 1, "description": what});
    let (required, actions, mut properties) = match tool {
        Tool::ManageAgent => (
            "agent",
            json!([
                "rename",
                "snooze",
                "wake",
                "settle",
                "unsettle",
                "mark_read",
                "set_mode",
                "set_model",
                "set_effort",
                "stop",
                "delete"
            ]),
            json!({
                "agent": id("The agent's id, as list_agents gives it."),
                "title": {"type": "string", "minLength": 1, "maxLength": MAX_TITLE, "description": "rename: the new title."},
                "minutes": {"type": "integer", "minimum": 1, "maximum": MAX_SNOOZE, "description": "snooze: for how long."},
                "mode": {"type": "string", "enum": ["ask", "accept_edits", "auto", "plan", "full_access"], "description": "set_mode: the permission mode."},
                "model": {"type": "string", "minLength": 1, "description": "set_model: the model, as list_targets names it."},
                "effort": {"type": ["string", "null"], "description": "set_effort: the level, as list_targets names it, or null for the model's default."},
            }),
        ),
        Tool::ManageProject => (
            "project",
            json!(["hide", "show", "set"]),
            json!({
                "project": id("The project's id, as list_targets gives it."),
                "resume_on_restart": {"type": "boolean", "description": "set: whether its agents start again after a restart."},
                "settle_on_merge": {"type": "boolean", "description": "set: whether an agent settles once its PR merges."},
                "auto_settle_days": {"type": ["integer", "null"], "minimum": 0, "maximum": 90, "description": "set: idle days before an agent settles, 0 for never, null for the device's."},
                "ignored_ports": {"type": "array", "items": {"type": "integer", "minimum": 1, "maximum": 65535}, "description": "set: the ports it doesn't offer to forward."},
            }),
        ),
        Tool::ManageWorkspace => (
            "workspace",
            json!(["start", "stop"]),
            json!({"workspace": id("The workspace's id, as list_targets gives it.")}),
        ),
        Tool::ManageTask => (
            "task",
            json!(["enable", "disable"]),
            json!({"task": id("The task's id, as list_tasks gives it.")}),
        ),
        Tool::ManageDevice => (
            "action",
            json!(["settings", "update_tools"]),
            json!({
                "changes": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": {
                            "field": {"type": "string", "enum": ["default_permission", "workspace_network", "workspace_pool", "auto_settle_days", "snooze_at_limit", "continue_claude_at_limit", "continue_codex_at_limit", "claude_effort", "codex_effort", "placement", "tool_updates", "keep_awake", "idle_release_minutes", "memory_items", "history_mib", "shell_integration", "skill_library"]},
                            "value": {"description": "The new value: true or false for a switch, a whole number for days, minutes, items or MiB, a mode for default_permission, a level or null for an effort, prefer or normal for placement, charger, always, working or off for keep_awake, and a list of claude, codex or cursor for tool_updates."},
                        },
                        "required": ["field", "value"],
                    },
                    "description": "settings: the settings to change.",
                },
                "tools": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": {
                            "tool": {"type": "string", "enum": ["claude", "codex", "cursor"]},
                            "workspace": {"type": "string", "description": "A container workspace's id. Leave it out for the device itself."},
                        },
                        "required": ["tool"],
                    },
                    "description": "update_tools: the CLIs to update, and where.",
                },
            }),
        ),
        Tool::ManagePermission => (
            "agent",
            json!(["grant", "revoke"]),
            json!({
                "agent": id("The agent's id, as list_agents gives it."),
                "depth": {"type": "integer", "minimum": 1, "maximum": MAX_DEPTH, "description": format!("grant: how many levels of agents may run below it. {MAX_DEPTH} when left out.")},
            }),
        ),
        _ => unreachable!("not an admin tool"),
    };
    properties["action"] = json!({"type": "string", "enum": actions});
    properties["device"] = json!({"type": "string", "description": "The device's id, as list_targets gives it. Leave it out for this Mac."});
    properties["asked"] = json!({"type": "boolean", "description": ASKED});
    let required = if required == "action" {
        json!(["action", "asked"])
    } else {
        json!(["action", required, "asked"])
    };
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

/// Checks an admin tool's arguments.
pub(super) fn parse(tool: Tool, arguments: &Value) -> Result<Call, String> {
    let args = fields(arguments)?;
    let action = text(&args, "action")?.ok_or("Give an `action`.")?;
    let wrong = || Err(format!("`action` can't be {action:?}."));
    let required = |name: &str| format!("Give `{name}`.");
    let action = match tool {
        Tool::ManageAgent => {
            let agent: AgentId = id(&args, "agent")?.ok_or_else(|| required("agent"))?;
            let inbox = |change| AdminAction::SetInbox { agent, change };
            match action.as_str() {
                "rename" => AdminAction::RenameAgent {
                    agent,
                    title: text(&args, "title")?.ok_or_else(|| required("title"))?,
                },
                "snooze" => {
                    let minutes = number(&args, "minutes", 1, MAX_SNOOZE)?
                        .ok_or_else(|| required("minutes"))?;
                    inbox(InboxChange::Snooze {
                        until: Timestamp::now().after(Duration::from_secs(minutes * 60)),
                    })
                }
                "wake" => inbox(InboxChange::Wake),
                "settle" => inbox(InboxChange::Settle),
                "unsettle" => inbox(InboxChange::Unsettle),
                "mark_read" => AdminAction::MarkRead { agent },
                "set_mode" => AdminAction::SetMode {
                    agent,
                    mode: named(&args, "mode")?.ok_or_else(|| required("mode"))?,
                },
                "set_model" => AdminAction::SetModel {
                    agent,
                    model: text(&args, "model")?.ok_or_else(|| required("model"))?,
                },
                "set_effort" => AdminAction::SetEffort {
                    agent,
                    effort: text(&args, "effort")?,
                },
                "stop" => AdminAction::StopAgent { agent },
                "delete" => AdminAction::DeleteAgent { agent },
                _ => return wrong(),
            }
        }
        Tool::ManageProject => {
            let project = id(&args, "project")?.ok_or_else(|| required("project"))?;
            match action.as_str() {
                "hide" | "show" => AdminAction::HideProject {
                    project,
                    hidden: action == "hide",
                },
                "set" => AdminAction::SetProject {
                    project,
                    changes: project_changes(&args)?,
                },
                _ => return wrong(),
            }
        }
        Tool::ManageWorkspace => {
            let workspace = id(&args, "workspace")?.ok_or_else(|| required("workspace"))?;
            match action.as_str() {
                "start" => AdminAction::StartWorkspace { workspace },
                "stop" => AdminAction::StopWorkspace { workspace },
                _ => return wrong(),
            }
        }
        Tool::ManageTask => {
            let task = id(&args, "task")?.ok_or_else(|| required("task"))?;
            match action.as_str() {
                "enable" | "disable" => AdminAction::SetTask {
                    task,
                    enabled: action == "enable",
                },
                _ => return wrong(),
            }
        }
        Tool::ManageDevice => match action.as_str() {
            "settings" => AdminAction::ChangeSettings {
                changes: list(&args, "changes", |item| {
                    serde_json::from_value(Value::Object(item.clone())).map_err(|_| {
                        "Each change needs a `field` Settings has and a `value` it takes."
                            .to_owned()
                    })
                })?,
            },
            "update_tools" => AdminAction::UpdateTools {
                tools: list(&args, "tools", |item| {
                    Ok(ToolTarget {
                        tool: named::<Cli>(item, "tool")?.ok_or("Give each a `tool`.")?,
                        place: match id(item, "workspace")? {
                            Some(workspace) => ToolPlace::Workspace { workspace },
                            None => ToolPlace::Host,
                        },
                    })
                })?,
            },
            _ => return wrong(),
        },
        Tool::ManagePermission => {
            let agent = id(&args, "agent")?.ok_or_else(|| required("agent"))?;
            match action.as_str() {
                "grant" => AdminAction::GrantPermission {
                    agent,
                    devices: Vec::new(),
                    depth: number(&args, "depth", 1, MAX_DEPTH)?.unwrap_or(MAX_DEPTH) as u8,
                },
                "revoke" => AdminAction::RevokePermission { agent },
                _ => return wrong(),
            }
        }
        _ => unreachable!("not an admin tool"),
    };
    Ok(Call::Admin(Act {
        device: id(&args, "device")?,
        action,
    }))
}

/// A list argument of objects, each checked by `item`.
fn list<T>(
    args: &Map<String, Value>,
    name: &str,
    item: impl Fn(&Map<String, Value>) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    let Some(Value::Array(values)) = args.get(name) else {
        return Err(format!("Give `{name}` as a list."));
    };
    if values.is_empty() {
        return Err(format!("Give `{name}` at least one entry."));
    }
    values
        .iter()
        .map(|value| match value {
            Value::Object(entry) => item(entry),
            _ => Err(format!("Each of `{name}` must be an object.")),
        })
        .collect()
}

/// The settings a `set` on a project changes.
fn project_changes(args: &Map<String, Value>) -> Result<Vec<ProjectSettingChange>, String> {
    let mut changes = Vec::new();
    if let Some(value) = flag(args, "resume_on_restart")? {
        changes.push(ProjectSettingChange::ResumeOnRestart { value });
    }
    if let Some(value) = flag(args, "settle_on_merge")? {
        changes.push(ProjectSettingChange::SettleOnMerge { value });
    }
    if args.contains_key("auto_settle_days") {
        let value = number(args, "auto_settle_days", 0, 90)?.map(|days| days as u16);
        changes.push(ProjectSettingChange::AutoSettleDays { value });
    }
    if let Some(ports) = args.get("ignored_ports") {
        let value = ports
            .as_array()
            .and_then(|ports| {
                ports
                    .iter()
                    .map(|port| super::whole(port).and_then(|port| u16::try_from(port).ok()))
                    .collect::<Option<Vec<u16>>>()
            })
            .ok_or("`ignored_ports` must be a list of port numbers.")?;
        changes.push(ProjectSettingChange::IgnoredPorts { value });
    }
    if changes.is_empty() {
        return Err("Give at least one setting to change.".into());
    }
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;

    use maxhq_protocol::ids::{ProjectId, TaskId, WorkspaceId};
    use maxhq_protocol::model::KeepAwake;

    fn admin(action: AdminAction) -> PlanStep {
        PlanStep::Admin {
            device: DeviceId::derive("mac"),
            action,
        }
    }

    fn parsed(tool: Tool, arguments: Value) -> AdminAction {
        match tool.parse(&arguments) {
            Ok(Call::Admin(act)) => act.action,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn each_admin_tool_reads_its_actions() {
        let agent = AgentId::derive("a");
        let task = TaskId::derive("t");
        let workspace = WorkspaceId::derive("w");
        let project = ProjectId::derive("p");
        assert_eq!(
            parsed(
                Tool::ManageAgent,
                json!({"action": "rename", "agent": agent.to_string(), "title": "Auth fix", "asked": true})
            ),
            AdminAction::RenameAgent {
                agent,
                title: "Auth fix".into()
            }
        );
        assert_eq!(
            parsed(
                Tool::ManageAgent,
                json!({"action": "set_effort", "agent": agent.to_string(), "effort": null})
            ),
            AdminAction::SetEffort {
                agent,
                effort: None
            }
        );
        let AdminAction::SetInbox {
            change: InboxChange::Snooze { until },
            ..
        } = parsed(
            Tool::ManageAgent,
            json!({"action": "snooze", "agent": agent.to_string(), "minutes": 60}),
        )
        else {
            panic!("a snooze");
        };
        assert!(until > Timestamp::now());
        assert_eq!(
            parsed(
                Tool::ManageProject,
                json!({"action": "set", "project": project.to_string(), "auto_settle_days": null, "ignored_ports": [3000]})
            ),
            AdminAction::SetProject {
                project,
                changes: vec![
                    ProjectSettingChange::AutoSettleDays { value: None },
                    ProjectSettingChange::IgnoredPorts { value: vec![3000] },
                ],
            }
        );
        assert_eq!(
            parsed(
                Tool::ManageWorkspace,
                json!({"action": "stop", "workspace": workspace.to_string()})
            ),
            AdminAction::StopWorkspace { workspace }
        );
        assert_eq!(
            parsed(
                Tool::ManageTask,
                json!({"action": "disable", "task": task.to_string()})
            ),
            AdminAction::SetTask {
                task,
                enabled: false
            }
        );
        assert_eq!(
            parsed(
                Tool::ManageDevice,
                json!({"action": "settings", "changes": [{"field": "keep_awake", "value": "always"}]})
            ),
            AdminAction::ChangeSettings {
                changes: vec![DeviceSettingChange::KeepAwake {
                    value: KeepAwake::Always
                }]
            }
        );
        assert_eq!(
            parsed(
                Tool::ManageDevice,
                json!({"action": "update_tools", "tools": [{"tool": "codex"}, {"tool": "claude", "workspace": workspace.to_string()}]})
            ),
            AdminAction::UpdateTools {
                tools: vec![
                    ToolTarget {
                        tool: Cli::Codex,
                        place: ToolPlace::Host
                    },
                    ToolTarget {
                        tool: Cli::Claude,
                        place: ToolPlace::Workspace { workspace }
                    },
                ]
            }
        );
        assert_eq!(
            parsed(
                Tool::ManagePermission,
                json!({"action": "grant", "agent": agent.to_string()})
            ),
            AdminAction::GrantPermission {
                agent,
                devices: Vec::new(),
                depth: 3
            }
        );
    }

    #[test]
    fn bad_admin_calls_say_what_is_wrong() {
        for (tool, arguments, says) in [
            (Tool::ManageAgent, json!({"action": "rename"}), "`agent`"),
            (
                Tool::ManageAgent,
                json!({"action": "fly", "agent": AgentId::derive("a").to_string()}),
                "\"fly\"",
            ),
            (
                Tool::ManageProject,
                json!({"action": "set", "project": ProjectId::derive("p").to_string()}),
                "at least one",
            ),
            (
                Tool::ManageDevice,
                json!({"action": "settings", "changes": [{"field": "theme", "value": "dark"}]}),
                "`field`",
            ),
        ] {
            let refused = tool.parse(&arguments).expect_err("refused");
            assert!(refused.contains(says), "{refused}");
        }
    }

    #[test]
    fn the_risk_table_sorts_each_change() {
        let agent = AgentId::derive("a");
        let mode = |mode| AdminAction::SetMode { agent, mode };
        for (action, idle, busy) in [
            (
                AdminAction::RenameAgent {
                    agent,
                    title: "t".into(),
                },
                Risk::Direct,
                Risk::Direct,
            ),
            (mode(PermissionMode::Auto), Risk::Direct, Risk::Confirm),
            (
                mode(PermissionMode::FullAccess),
                Risk::Confirm,
                Risk::Confirm,
            ),
            (
                AdminAction::StopAgent { agent },
                Risk::Direct,
                Risk::Confirm,
            ),
            (
                AdminAction::DeleteAgent { agent },
                Risk::Confirm,
                Risk::Confirm,
            ),
            (
                AdminAction::ChangeSettings {
                    changes: vec![DeviceSettingChange::MemoryItems { value: 512 }],
                },
                Risk::Direct,
                Risk::Direct,
            ),
            (
                AdminAction::ChangeSettings {
                    changes: vec![
                        DeviceSettingChange::MemoryItems { value: 512 },
                        DeviceSettingChange::DefaultPermission {
                            value: PermissionMode::Auto,
                        },
                    ],
                },
                Risk::Confirm,
                Risk::Confirm,
            ),
            (
                AdminAction::ChangeSettings {
                    changes: vec![DeviceSettingChange::ClaudeSetupToken { value: true }],
                },
                Risk::Never,
                Risk::Never,
            ),
            (
                AdminAction::ChangeSettings {
                    changes: vec![DeviceSettingChange::DispatcherActs { value: false }],
                },
                Risk::Never,
                Risk::Never,
            ),
            (
                AdminAction::ChangeSettings {
                    changes: vec![DeviceSettingChange::SnoozeAtLimit { value: true }; 6],
                },
                Risk::Confirm,
                Risk::Confirm,
            ),
            (
                AdminAction::SetProject {
                    project: ProjectId::derive("p"),
                    changes: vec![ProjectSettingChange::IgnoredPorts { value: vec![3000] }],
                },
                Risk::Direct,
                Risk::Direct,
            ),
            (
                AdminAction::SetProject {
                    project: ProjectId::derive("p"),
                    changes: vec![ProjectSettingChange::ResumeOnRestart { value: false }],
                },
                Risk::Confirm,
                Risk::Confirm,
            ),
            (
                AdminAction::GrantPermission {
                    agent,
                    devices: Vec::new(),
                    depth: 3,
                },
                Risk::Confirm,
                Risk::Confirm,
            ),
        ] {
            let step = admin(action);
            assert_eq!(
                (risk(&step, false), risk(&step, true)),
                (idle, busy),
                "{step:?}"
            );
        }
    }

    #[test]
    fn only_an_asked_direct_change_runs_at_once() {
        assert_eq!(decide(Risk::Direct, true, true, 0), Risk::Direct);
        assert_eq!(decide(Risk::Direct, false, true, 0), Risk::Confirm);
        assert_eq!(decide(Risk::Direct, true, false, 0), Risk::Confirm);
        assert_eq!(decide(Risk::Direct, true, true, MAX_AT_ONCE), Risk::Confirm);
        assert_eq!(decide(Risk::Confirm, true, true, 0), Risk::Confirm);
        assert_eq!(decide(Risk::Never, true, true, 0), Risk::Never);
        assert!(asked(&json!({"asked": true})));
        assert!(!asked(&json!({"asked": false})));
        assert!(!asked(&json!({})));
    }

    #[test]
    fn the_current_values_put_a_change_back() {
        let settings: DeviceSettings = serde_json::from_value(json!({
            "device": DeviceId::derive("mac"),
            "default_permission": "ask",
            "workspace_network": null,
            "workspace_pool": null,
        }))
        .expect("settings");
        let changes = [
            DeviceSettingChange::MemoryItems { value: 1 },
            DeviceSettingChange::KeepAwake {
                value: KeepAwake::Off,
            },
        ];
        let now = settings_now(&settings, &changes);
        let mut changed = settings.clone();
        for change in changes {
            changed.change(change);
        }
        for change in now {
            changed.change(change);
        }
        assert_eq!(changed, settings);
    }

    #[test]
    fn shared_calls_become_what_a_plan_does() {
        let agent = AgentId::derive("a");
        let Ok(Wants::Step(Step::Start(start))) = wants(
            Tool::StartAgent
                .parse(&json!({"request_id": "r", "vendor": "claude", "folder": "/work", "task": "fix it"}))
                .expect("parses"),
        ) else {
            panic!("a start");
        };
        assert_eq!(start.first.as_deref(), Some("fix it"));
        assert_eq!(
            wants(
                Tool::InterruptAgent
                    .parse(&json!({"request_id": "r", "agent": agent.to_string(), "stop": true}))
                    .expect("parses")
            ),
            Ok(Wants::Act(Act {
                device: None,
                action: AdminAction::StopAgent { agent }
            }))
        );
        let refused = wants(
            Tool::ScheduleTask
                .parse(&json!({"request_id": "r", "prompt": "p", "schedule": {"type": "webhook"}, "bind_to_current_agent": true}))
                .expect("parses"),
        )
        .expect_err("refused");
        assert!(refused.contains("`target`"), "{refused}");
    }
}
