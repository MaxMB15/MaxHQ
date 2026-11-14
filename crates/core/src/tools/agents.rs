// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Follows the behaviour of MIT-licensed code listed in THIRD_PARTY_NOTICES:
// the tools an agent runs other agents with, what each description
// promises, the request id that makes a repeated call safe, the page of 50
// up to 100 threads, and the wait of 10 minutes by default up to 60. The
// original's separate delegate and launch tools are one start_agent here,
// and its environment ids are MaxHQ's devices.

//! The agent tools' arguments: what each takes, and its checks.

use maxhq_protocol::ids::{AccountId, AgentId, DeviceId, WorkspaceId};
use maxhq_protocol::message::{AgentPlace, SendMode};
use maxhq_protocol::model::{PermissionMode, Vendor};
use serde::de::DeserializeOwned;
use serde_json::{json, Map, Value};

use super::{fields, whole, Call, Tool, MAX_MESSAGE};

/// How many agents a list shows when the call doesn't say.
pub const LIST: usize = 50;

/// The most agents one list shows.
pub const MAX_LIST: usize = 100;

/// How long a wait lasts when the call doesn't say, in seconds.
pub const WAIT: u64 = 10 * 60;

/// The longest a wait lasts, in seconds.
pub const MAX_WAIT: u64 = 60 * 60;

/// The most agents one wait names.
pub const MAX_WAITED: usize = 20;

/// The longest a request id may be.
pub(super) const MAX_REQUEST_ID: usize = 200;

/// The longest title a start takes.
pub(super) const MAX_TITLE: usize = 200;

/// What an action's `request_id` says.
pub(super) const REQUEST_ID: &str =
    "A unique id you make up for this request, such as a short random \
     string. Calling again with the same id doesn't do it twice: it gives the first call's \
     answer.";

/// What a `device` argument says.
const DEVICE: &str = "The device's id, as list_targets gives it. Leave it out for the device \
     you're on.";

/// Start an agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Start {
    /// The caller's id for the request.
    pub request_id: String,
    /// The device, or none for the caller's.
    pub device: Option<DeviceId>,
    /// The vendor.
    pub vendor: Vendor,
    /// The account, or none for the first allowed one of the vendor.
    pub account: Option<AccountId>,
    /// Where it runs, or none for the caller's own workspace.
    pub place: Option<AgentPlace>,
    /// The model, or none for the account's default.
    pub model: Option<String>,
    /// The permission mode, or none to take the caller's.
    pub mode: Option<PermissionMode>,
    /// A title.
    pub title: Option<String>,
    /// The task, its first message.
    pub task: String,
    /// An agent whose conversation goes with the task, as a brief.
    pub context_from: Option<AgentId>,
    /// Whether the call waits for the agent's first turn to end.
    pub wait: bool,
    /// How long it waits, in seconds.
    pub timeout: u64,
    /// Whether the caller hears each time the agent finishes a turn.
    pub wake: bool,
}

/// Send an agent a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    /// The caller's id for the request.
    pub request_id: String,
    /// The device, or none for the caller's.
    pub device: Option<DeviceId>,
    /// The agent.
    pub agent: AgentId,
    /// The message.
    pub text: String,
    /// How it goes into a running turn.
    pub mode: SendMode,
}

impl Start {
    /// The arguments [`parse`] reads back into this start, for a call
    /// that goes to another device.
    pub fn arguments(&self) -> Value {
        let mut args = Map::new();
        args.insert("request_id".into(), json!(self.request_id));
        if let Some(device) = self.device {
            args.insert("device".into(), json!(device.to_string()));
        }
        args.insert("vendor".into(), json!(self.vendor));
        if let Some(account) = self.account {
            args.insert("account".into(), json!(account.to_string()));
        }
        match &self.place {
            Some(AgentPlace::Workspace { workspace }) => {
                args.insert("workspace".into(), json!(workspace.to_string()));
            }
            Some(AgentPlace::Folder { path }) => {
                args.insert("folder".into(), json!(path));
            }
            None => {}
        }
        if let Some(model) = &self.model {
            args.insert("model".into(), json!(model));
        }
        if let Some(mode) = self.mode {
            args.insert("mode".into(), json!(mode));
        }
        if let Some(title) = &self.title {
            args.insert("title".into(), json!(title));
        }
        args.insert("task".into(), json!(self.task));
        if let Some(agent) = self.context_from {
            args.insert("context_from".into(), json!(agent.to_string()));
        }
        args.insert("wait".into(), json!(self.wait));
        args.insert("timeout_seconds".into(), json!(self.timeout));
        args.insert("wake".into(), json!(self.wake));
        Value::Object(args)
    }
}

impl Message {
    /// The arguments [`parse`] reads back into this message, for a call
    /// that goes to another device.
    pub fn arguments(&self) -> Value {
        let mut args = json!({
            "request_id": self.request_id,
            "agent": self.agent.to_string(),
            "text": self.text,
            "mode": self.mode,
        });
        if let Some(device) = self.device {
            args["device"] = json!(device.to_string());
        }
        args
    }
}

/// Wait for agents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wait {
    /// The device, or none for the caller's.
    pub device: Option<DeviceId>,
    /// The agents.
    pub agents: Vec<AgentId>,
    /// How long, in seconds.
    pub timeout: u64,
}

/// Stop an agent's turn, or the agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stop {
    /// The caller's id for the request.
    pub request_id: String,
    /// The device, or none for the caller's.
    pub device: Option<DeviceId>,
    /// The agent.
    pub agent: AgentId,
    /// Whether to stop the agent, and those it started, not only its turn.
    pub stop: bool,
}

/// List agents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct List {
    /// Only those on this device.
    pub device: Option<DeviceId>,
    /// Only those the caller started.
    pub mine: bool,
    /// How many to skip, from the cursor the last page gave.
    pub cursor: usize,
    /// How many to show.
    pub limit: usize,
}

/// A short title for each agent tool.
pub(super) fn title(tool: Tool) -> &'static str {
    match tool {
        Tool::ListTargets => "List where agents can start",
        Tool::ListAgents => "List agents",
        Tool::StartAgent => "Start an agent",
        Tool::SendAgent => "Message an agent",
        Tool::WaitAgent => "Wait for agents",
        Tool::InterruptAgent => "Stop an agent",
        _ => unreachable!("not an agent tool"),
    }
}

/// What the agent reads about each agent tool.
pub(super) fn description(tool: Tool) -> &'static str {
    match tool {
        Tool::ListTargets => {
            "Lists what you may start agents with and where: the devices, vendors, accounts, \
             workspaces and models you're allowed, how many levels of agents you may still \
             start, and any limits on you. Only works once the user allows you to run other \
             agents. Call it before start_agent."
        }
        Tool::ListAgents => {
            "Lists the MaxHQ agents on the devices you're allowed, newest activity first, one \
             line each: id, title, vendor, device, state, context used and the last reply. \
             Set `mine` to list only the agents you started. Pages hold 50 by default and 100 \
             at most: pass the `next` cursor a page gives to read on. Only works once the user \
             allows you to run other agents."
        }
        Tool::StartAgent => {
            "Starts a new MaxHQ agent with a task and returns its id. Give a vendor, and a \
             workspace or a folder, as list_targets names them; the account, model, mode and \
             title are optional. Its mode can't allow more than yours. By default it returns \
             once the agent starts, and you get a short message each time the agent finishes \
             a turn; read what it said with read_agent. Set `wake` to false to skip those \
             messages. Set `wait` to true to wait for its first turn to end instead, up to \
             `timeout_seconds`. Add `context_from` with another agent's id to give the new \
             agent a brief of that conversation. Only works once the user allows you to run \
             other agents."
        }
        Tool::SendAgent => {
            "Sends a message to a MaxHQ agent. `queue` waits for its turn to end, `steer` \
             goes into the running turn and `interrupt` stops the turn and sends. All three \
             just send when it's idle. Only works once the user allows you to run other \
             agents."
        }
        Tool::WaitAgent => {
            "Waits until each of the agents you name finishes its turn, needs input or stops, \
             up to `timeout_seconds`, 600 by default and 3600 at most. Returns each one's \
             state and last reply, and whether the wait timed out. Only works once the user \
             allows you to run other agents."
        }
        Tool::InterruptAgent => {
            "Stops a MaxHQ agent's turn. With `stop` set, stops the agent itself, and every \
             agent it started. Only works once the user allows you to run other agents."
        }
        _ => unreachable!("not an agent tool"),
    }
}

/// The JSON schema for each agent tool's arguments.
pub(super) fn input_schema(tool: Tool) -> Value {
    let request_id = json!({"type": "string", "minLength": 1, "maxLength": MAX_REQUEST_ID, "description": REQUEST_ID});
    let device = json!({"type": "string", "description": DEVICE});
    let agent = json!({"type": "string", "minLength": 1, "description": "The agent's id."});
    let timeout = |default: u64| {
        json!({
            "type": "integer",
            "minimum": 1,
            "maximum": MAX_WAIT,
            "description": format!("How long to wait, in seconds. {default} when left out."),
        })
    };
    let (properties, required) = match tool {
        Tool::ListTargets => (json!({}), json!([])),
        Tool::ListAgents => (
            json!({
                "device": {"type": "string", "description": "Only the agents on this device, by its id."},
                "mine": {"type": "boolean", "description": "Only the agents you started."},
                "cursor": {"type": "string", "description": "The `next` cursor the last page gave."},
                "limit": {"type": "integer", "minimum": 1, "maximum": MAX_LIST, "description": format!("How many to list, {LIST} when left out.")},
            }),
            json!([]),
        ),
        Tool::StartAgent => (
            json!({
                "request_id": request_id,
                "task": {"type": "string", "minLength": 1, "maxLength": MAX_MESSAGE, "description": "What the new agent should do. It's the agent's first message."},
                "vendor": {"type": "string", "minLength": 1, "description": "The vendor, as list_targets names it."},
                "device": device,
                "account": {"type": "string", "description": "The account's id, as list_targets gives it. Leave it out for the first one you're allowed for the vendor."},
                "workspace": {"type": "string", "description": "The workspace's id, as list_targets gives it. Give this or `folder`. Leave both out for your own workspace."},
                "folder": {"type": "string", "description": "An absolute folder to run in. Give this or `workspace`."},
                "model": {"type": "string", "description": "The model, as list_targets names it. Leave it out for the account's default."},
                "mode": {"type": "string", "description": "The permission mode, as list_targets names it. Leave it out for your own, or the nearest the vendor has below it."},
                "title": {"type": "string", "maxLength": MAX_TITLE, "description": "A title for the agent."},
                "context_from": {"type": "string", "description": "Another agent's id, whose conversation goes with the task as a brief."},
                "wait": {"type": "boolean", "description": "Wait for the agent's first turn to end. False when left out."},
                "timeout_seconds": timeout(WAIT),
                "wake": {"type": "boolean", "description": "Get a message each time the agent finishes a turn. True when left out, unless `wait` is set."},
            }),
            json!(["request_id", "task", "vendor"]),
        ),
        Tool::SendAgent => (
            json!({
                "request_id": request_id,
                "agent": agent,
                "device": device,
                "text": {"type": "string", "minLength": 1, "maxLength": MAX_MESSAGE, "description": "The message."},
                "mode": {"type": "string", "enum": ["queue", "steer", "interrupt"], "description": "How it goes into a running turn. queue when left out."},
            }),
            json!(["request_id", "agent", "text"]),
        ),
        Tool::WaitAgent => (
            json!({
                "agents": {"type": "array", "items": {"type": "string", "minLength": 1}, "minItems": 1, "maxItems": MAX_WAITED, "description": "The agents' ids."},
                "device": device,
                "timeout_seconds": timeout(WAIT),
            }),
            json!(["agents"]),
        ),
        Tool::InterruptAgent => (
            json!({
                "request_id": request_id,
                "agent": agent,
                "device": device,
                "stop": {"type": "boolean", "description": "Stop the agent, and the agents it started, not only its turn."},
            }),
            json!(["request_id", "agent"]),
        ),
        _ => unreachable!("not an agent tool"),
    };
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

/// Checks an agent tool's arguments.
pub(super) fn parse(tool: Tool, arguments: &Value) -> Result<Call, String> {
    let args = fields(arguments)?;
    Ok(match tool {
        Tool::ListTargets => Call::ListTargets,
        Tool::ListAgents => Call::ListAgents(List {
            device: id(&args, "device")?,
            mine: flag(&args, "mine")?.unwrap_or(false),
            cursor: match args.get("cursor") {
                None | Some(Value::Null) => 0,
                Some(value) => whole(value)
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or("`cursor` must be the `next` a page gave.")?,
            },
            limit: number(&args, "limit", 1, MAX_LIST as u64)?.map_or(LIST, |n| n as usize),
        }),
        Tool::StartAgent => {
            let wait = flag(&args, "wait")?.unwrap_or(false);
            Call::StartAgent(Start {
                request_id: request_id(&args)?,
                device: id(&args, "device")?,
                vendor: named(&args, "vendor")?.ok_or("Give a `vendor`.")?,
                account: id(&args, "account")?,
                place: place(&args)?,
                model: text(&args, "model")?,
                mode: named(&args, "mode")?,
                title: text(&args, "title")?.map(|title| title.chars().take(MAX_TITLE).collect()),
                task: message(&args, "task")?,
                context_from: id(&args, "context_from")?,
                wait,
                timeout: timeout(&args)?,
                wake: flag(&args, "wake")?.unwrap_or(!wait),
            })
        }
        Tool::SendAgent => Call::SendAgent(Message {
            request_id: request_id(&args)?,
            device: id(&args, "device")?,
            agent: id(&args, "agent")?.ok_or("Give an `agent`.")?,
            text: message(&args, "text")?,
            mode: named(&args, "mode")?.unwrap_or(SendMode::Queue),
        }),
        Tool::WaitAgent => {
            let Some(Value::Array(list)) = args.get("agents") else {
                return Err("`agents` must be a list of agent ids.".into());
            };
            if list.is_empty() || list.len() > MAX_WAITED {
                return Err(format!("Name 1 to {MAX_WAITED} agents."));
            }
            let mut agents = Vec::with_capacity(list.len());
            for value in list {
                let agent = value
                    .as_str()
                    .and_then(|text| text.trim().parse().ok())
                    .ok_or("Each of `agents` must be an agent id.")?;
                if !agents.contains(&agent) {
                    agents.push(agent);
                }
            }
            Call::WaitAgent(Wait {
                device: id(&args, "device")?,
                agents,
                timeout: timeout(&args)?,
            })
        }
        Tool::InterruptAgent => Call::InterruptAgent(Stop {
            request_id: request_id(&args)?,
            device: id(&args, "device")?,
            agent: id(&args, "agent")?.ok_or("Give an `agent`.")?,
            stop: flag(&args, "stop")?.unwrap_or(false),
        }),
        _ => unreachable!("not an agent tool"),
    })
}

/// A text argument, trimmed, or none when left out or empty.
pub(super) fn text(args: &Map<String, Value>, name: &str) -> Result<Option<String>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => {
            let text = text.trim();
            Ok((!text.is_empty()).then(|| text.to_owned()))
        }
        Some(_) => Err(format!("`{name}` must be text.")),
    }
}

/// A message argument: required, and no longer than [`MAX_MESSAGE`].
pub(super) fn message(args: &Map<String, Value>, name: &str) -> Result<String, String> {
    let text = text(args, name)?.ok_or_else(|| format!("Give a `{name}`."))?;
    if text.chars().count() > MAX_MESSAGE {
        return Err(format!("`{name}` is longer than {MAX_MESSAGE} characters."));
    }
    Ok(text)
}

/// The request id, required on every action.
pub(super) fn request_id(args: &Map<String, Value>) -> Result<String, String> {
    let id = text(args, "request_id")?.ok_or("Give a `request_id`.")?;
    if id.chars().count() > MAX_REQUEST_ID {
        return Err(format!(
            "`request_id` is longer than {MAX_REQUEST_ID} characters."
        ));
    }
    Ok(id)
}

/// An id argument.
pub(super) fn id<T: std::str::FromStr>(
    args: &Map<String, Value>,
    name: &str,
) -> Result<Option<T>, String> {
    text(args, name)?
        .map(|text| {
            text.parse()
                .map_err(|_| format!("`{name}` isn't a valid id."))
        })
        .transpose()
}

/// Where a new agent runs: a `workspace` or a `folder`, or none for the
/// caller's own workspace.
pub(super) fn place(args: &Map<String, Value>) -> Result<Option<AgentPlace>, String> {
    Ok(
        match (id::<WorkspaceId>(args, "workspace")?, text(args, "folder")?) {
            (Some(_), Some(_)) => return Err("Give a workspace or a folder, not both.".into()),
            (Some(workspace), None) => Some(AgentPlace::Workspace { workspace }),
            (None, Some(path)) if path.starts_with('/') => Some(AgentPlace::Folder { path }),
            (None, Some(_)) => return Err("`folder` must be an absolute path.".into()),
            (None, None) => None,
        },
    )
}

/// An argument that names one of a type's values, as the protocol spells
/// it, such as a vendor or a mode. Spaces and dashes read as underscores.
pub(super) fn named<T: DeserializeOwned>(
    args: &Map<String, Value>,
    name: &str,
) -> Result<Option<T>, String> {
    text(args, name)?
        .map(|text| {
            let spelled = text.to_lowercase().replace([' ', '-'], "_");
            serde_json::from_value(Value::String(spelled))
                .map_err(|_| format!("`{name}` can't be {text:?}. list_targets names each one."))
        })
        .transpose()
}

/// A true or false argument.
pub(super) fn flag(args: &Map<String, Value>, name: &str) -> Result<Option<bool>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(flag)) => Ok(Some(*flag)),
        Some(Value::String(text)) if text == "true" || text == "false" => Ok(Some(text == "true")),
        Some(_) => Err(format!("`{name}` must be true or false.")),
    }
}

/// A whole number argument from `low` to `high`.
pub(super) fn number(
    args: &Map<String, Value>,
    name: &str,
    low: u64,
    high: u64,
) -> Result<Option<u64>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => whole(value)
            .filter(|n| (low..=high).contains(n))
            .map(Some)
            .ok_or_else(|| format!("`{name}` must be a whole number from {low} to {high}.")),
    }
}

/// A wait's timeout, in seconds.
fn timeout(args: &Map<String, Value>) -> Result<u64, String> {
    Ok(number(args, "timeout_seconds", 1, MAX_WAIT)?.unwrap_or(WAIT))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_and_a_message_read_back_from_their_arguments() {
        let start = Start {
            request_id: "task:run".into(),
            device: Some(DeviceId::derive("devbox")),
            vendor: Vendor::Codex,
            account: Some(AccountId::derive("account")),
            place: Some(AgentPlace::Folder {
                path: "/work/app".into(),
            }),
            model: Some("gpt-5".into()),
            mode: Some(PermissionMode::AcceptEdits),
            title: Some("Nightly".into()),
            task: "check the build".into(),
            context_from: None,
            wait: false,
            timeout: WAIT,
            wake: false,
        };
        assert_eq!(
            Tool::StartAgent.parse(&start.arguments()),
            Ok(Call::StartAgent(start))
        );
        let message = Message {
            request_id: "task:run".into(),
            device: Some(DeviceId::derive("devbox")),
            agent: AgentId::derive("agent"),
            text: "check the build".into(),
            mode: SendMode::Queue,
        };
        assert_eq!(
            Tool::SendAgent.parse(&message.arguments()),
            Ok(Call::SendAgent(message))
        );
    }

    #[test]
    fn a_start_reads_names_generically_and_defaults_to_waking() {
        let call = Tool::StartAgent
            .parse(&json!({
                "request_id": "r1",
                "task": "fix the flaky test",
                "vendor": "Codex",
                "folder": "/work/app",
                "mode": "accept edits",
            }))
            .expect("the start parses");
        let Call::StartAgent(start) = call else {
            panic!("not a start");
        };
        assert_eq!(start.vendor, Vendor::Codex);
        assert_eq!(start.mode, Some(PermissionMode::AcceptEdits));
        assert!(start.wake && !start.wait);
        assert_eq!(start.timeout, WAIT);
        assert_eq!(
            start.place,
            Some(AgentPlace::Folder {
                path: "/work/app".into()
            })
        );
    }

    #[test]
    fn a_waiting_start_does_not_wake_unless_asked() {
        let Ok(Call::StartAgent(start)) = Tool::StartAgent.parse(&json!({
            "request_id": "r1", "task": "t", "vendor": "claude", "wait": true,
        })) else {
            panic!("the start parses");
        };
        assert!(start.wait && !start.wake);
    }

    #[test]
    fn bad_arguments_say_what_is_wrong() {
        let refused = |tool: Tool, args: Value| tool.parse(&args).expect_err("refused");
        assert!(
            refused(Tool::StartAgent, json!({"task": "t", "vendor": "codex"}))
                .contains("request_id")
        );
        assert!(refused(
            Tool::StartAgent,
            json!({"request_id": "r", "task": "t", "vendor": "nobody"})
        )
        .contains("vendor"));
        assert!(refused(
            Tool::StartAgent,
            json!({"request_id": "r", "task": "t", "vendor": "codex", "folder": "relative"})
        )
        .contains("absolute"));
        assert!(refused(Tool::WaitAgent, json!({"agents": []})).contains("1 to"));
        assert!(refused(
            Tool::WaitAgent,
            json!({"agents": [AgentId::derive("x").to_string()], "timeout_seconds": 3601})
        )
        .contains("3600"));
        assert!(refused(Tool::ListAgents, json!({"limit": 101})).contains("100"));
    }

    #[test]
    fn a_list_pages_by_cursor() {
        let Ok(Call::ListAgents(list)) = Tool::ListAgents.parse(&json!({"cursor": "50"})) else {
            panic!("the list parses");
        };
        assert_eq!((list.cursor, list.limit, list.mine), (50, LIST, false));
    }
}
