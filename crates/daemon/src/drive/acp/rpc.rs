// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The JSON-RPC 2.0 lines an ACP CLI speaks on stdio, one per line, and
//! the Agent Client Protocol requests the driver sends. Request IDs come
//! from the Codex driver's [`Ids`], so a reply meant for an earlier `maxd`
//! is never taken for a new one.
//!
//! The driver speaks ACP v1 and v2 through a [`Wire`]. `initialize` asks
//! in both, and the reply's shape says which the CLI speaks. On v2 the
//! wire turns each request the driver writes into v2's, and each line the
//! CLI sends back into v1's, so the driver knows one shape only.

use std::collections::HashSet;

use maxhq_core::tools::SERVER;
use maxhq_protocol::agents::McpCommand;
use serde_json::{json, Value};

pub use crate::drive::codex::rpc::{key, Ids, METHOD_NOT_FOUND};

mod v2;

/// The ACP v1 protocol version.
pub const PROTOCOL_VERSION: u64 = 1;

/// The ACP v2 schema the v2 shapes come from. It's a preview, so when it
/// moves only this module and its tests change.
pub const V2_SCHEMA: &str = "2.0.0-alpha.3";

/// Which ACP a CLI speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Generation {
    /// ACP v1.
    V1,
    /// ACP v2.
    V2,
}

/// Which ACP an `initialize` reply is in, from its shape rather than its
/// number: an `info` field is v2's, and v1's names its `agentCapabilities`
/// or `agentInfo`, or says version 1 or none. Some v1 agents say 2.
/// None for a reply in neither shape.
pub fn generation(result: &Value) -> Option<Generation> {
    if result.get("info").is_some() {
        return Some(Generation::V2);
    }
    let v1 = result.get("agentCapabilities").is_some()
        || result.get("agentInfo").is_some()
        || result
            .get("protocolVersion")
            .is_none_or(|version| version.as_u64() == Some(PROTOCOL_VERSION));
    v1.then_some(Generation::V1)
}

/// Why `name` can't run, when its `initialize` reply is in neither shape.
pub fn unspoken(name: &str, result: &Value) -> String {
    match result.get("protocolVersion") {
        Some(Value::Number(version)) => {
            format!("{name} speaks ACP version {version}, which MaxHQ doesn't")
        }
        _ => format!("{name} speaks an ACP version MaxHQ doesn't know"),
    }
}

/// One CLI's connection: which ACP it speaks, once its `initialize`
/// reply said, and on v2 the prompt whose turn is running.
#[derive(Debug, Default)]
pub struct Wire {
    generation: Option<Generation>,
    /// The `initialize` request's ID.
    initialize: Option<String>,
    /// v2: the `session/prompt` whose turn runs, which an idle
    /// `state_update` ends.
    prompt: Option<String>,
    /// v2: the prompts whose acceptance hasn't come yet. It says nothing
    /// the driver needs, so it's dropped.
    accepts: HashSet<String>,
    /// v2: set while the CLI says a turn runs, so a turn read back after a
    /// restart still ends.
    running: bool,
}

impl Wire {
    /// Which ACP the CLI speaks, once its `initialize` reply said. None
    /// before then, and for a reply in neither shape.
    pub fn generation(&self) -> Option<Generation> {
        self.generation
    }

    /// A request line, in the CLI's ACP. The driver gives v1's method and
    /// parameters.
    pub fn request(&mut self, id: &str, method: &str, params: Value) -> String {
        if method == "initialize" {
            self.initialize = Some(id.to_owned());
        }
        if self.generation != Some(Generation::V2) {
            return request(id, method, params);
        }
        if method == "session/prompt" {
            self.prompt = Some(id.to_owned());
            self.accepts.insert(id.to_owned());
        }
        let (method, params) = v2::request(method, params);
        request(id, method, params)
    }

    /// Sorts one line, in v1's shape, or gives `None` for a line that
    /// isn't JSON-RPC or that the driver has no use for.
    pub fn read(&mut self, text: &str) -> Option<Incoming> {
        match parse(text)? {
            Incoming::Response {
                id,
                outcome: Ok(result),
            } if self.initialize.is_some() && key(&id) == self.initialize
                || result.get("protocolVersion").is_some() =>
            {
                self.generation = generation(&result);
                self.prompt = None;
                self.accepts.clear();
                self.running = false;
                let result = match self.generation {
                    Some(Generation::V2) => v2::initialized(result),
                    _ => result,
                };
                Some(Incoming::Response {
                    id,
                    outcome: Ok(result),
                })
            }
            incoming if self.generation != Some(Generation::V2) => Some(incoming),
            Incoming::Response { id, outcome } => self.response(id, outcome),
            Incoming::Request { id, method, params } => {
                let params = match method.as_str() {
                    "session/request_permission" => v2::permission(params, &id),
                    _ => params,
                };
                Some(Incoming::Request { id, method, params })
            }
            Incoming::Notification { method, params } if method == "session/update" => {
                self.update(params)
            }
            notification => Some(notification),
        }
    }

    /// A v2 answer. A prompt's acceptance is dropped, since its turn ends
    /// with a `state_update`, unless it's an error, which ends the turn.
    fn response(&mut self, id: Value, outcome: Result<Value, RpcError>) -> Option<Incoming> {
        let accepted = key(&id).is_some_and(|key| self.accepts.remove(&key));
        let outcome = match outcome {
            Ok(_) if accepted => return None,
            Ok(result) => Ok(v2::result(result)),
            Err(error) => {
                if accepted && key(&id) == self.prompt {
                    self.prompt = None;
                    self.running = false;
                }
                Err(error)
            }
        };
        Some(Incoming::Response { id, outcome })
    }

    /// A v2 `session/update`. An idle `state_update` is the turn's end,
    /// which v1 sends as the prompt's result.
    fn update(&mut self, params: Value) -> Option<Incoming> {
        let update = params.get("update")?;
        if text(update, "sessionUpdate") != Some("state_update") {
            return Some(Incoming::Notification {
                method: "session/update".to_owned(),
                params: v2::update(params),
            });
        }
        match text(update, "state") {
            Some("running") => {
                self.running = true;
                None
            }
            Some("idle") if self.prompt.is_some() || self.running => {
                self.running = false;
                let stop = text(update, "stopReason").unwrap_or("end_turn");
                // A turn read back after a restart has no prompt of this
                // `maxd`'s, and the driver reads it by its shape.
                let id = self.prompt.take().map_or(Value::Null, Value::String);
                Some(Incoming::Response {
                    id,
                    outcome: Ok(json!({ "stopReason": stop })),
                })
            }
            _ => None,
        }
    }
}

/// One line from the CLI, sorted by what it is.
#[derive(Debug)]
pub enum Incoming {
    /// The answer to a request.
    Response {
        /// The request's ID, as it was sent.
        id: Value,
        /// The result, or the error.
        outcome: Result<Value, RpcError>,
    },
    /// A request the driver must answer.
    Request {
        /// The request's ID, to echo in the answer.
        id: Value,
        /// The method, such as `session/request_permission`.
        method: String,
        /// Its parameters.
        params: Value,
    },
    /// A notification, which needs no answer.
    Notification {
        /// The method, such as `session/update`.
        method: String,
        /// Its parameters.
        params: Value,
    },
}

/// An error the CLI answered with.
#[derive(Clone, Debug, PartialEq)]
pub struct RpcError {
    /// The JSON-RPC code, such as -32603.
    pub code: i64,
    /// The message, for a person to read.
    pub message: String,
    /// Whatever else the CLI put in `data`.
    pub data: Value,
}

/// Sorts one line, or gives `None` for a line that isn't JSON-RPC.
pub fn parse(text: &str) -> Option<Incoming> {
    let mut value: Value = serde_json::from_str(text).ok()?;
    let message = value.as_object_mut()?;
    let id = message.remove("id").filter(|id| !id.is_null());
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let params = message.remove("params").unwrap_or(Value::Null);
    match (id, method) {
        (Some(id), Some(method)) => Some(Incoming::Request { id, method, params }),
        (None, Some(method)) => Some(Incoming::Notification { method, params }),
        (Some(id), None) => {
            let outcome = match message.remove("error") {
                Some(mut error) => Err(RpcError {
                    code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                    message: error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    data: error
                        .as_object_mut()
                        .and_then(|error| error.remove("data"))
                        .unwrap_or(Value::Null),
                }),
                None => Ok(message.remove("result").unwrap_or(Value::Null)),
            };
            Some(Incoming::Response { id, outcome })
        }
        (None, None) => None,
    }
}

/// A request line.
pub fn request(id: &str, method: &str, params: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string()
}

/// A notification line.
pub fn notification(method: &str, params: Value) -> String {
    json!({ "jsonrpc": "2.0", "method": method, "params": params }).to_string()
}

/// A successful answer to a request.
pub fn response(id: &Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

/// An error answer to a request.
pub fn error(id: &Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

/// The `initialize` parameters, with the flavor's `meta`, which ask in v1
/// and v2 at once. The driver offers no file system and no terminal, so
/// the CLI uses its own.
pub fn initialize(meta: Value) -> Value {
    let info = json!({ "name": "maxhq", "title": "MaxHQ", "version": env!("CARGO_PKG_VERSION") });
    let mut params = json!({
        "protocolVersion": PROTOCOL_VERSION,
        "clientCapabilities": {
            "fs": { "readTextFile": false, "writeTextFile": false },
            "terminal": false,
            "_meta": meta,
        },
        "clientInfo": info,
    });
    v2::initialize(&mut params);
    params
}

/// The `session/new` parameters for a session in `cwd`, with the agent
/// tools when `tools` runs them.
pub fn new_session(cwd: &str, tools: Option<&McpCommand>) -> Value {
    json!({ "cwd": cwd, "mcpServers": mcp_servers(tools) })
}

/// The `session/load` parameters to load `session` in `cwd`. ACP's schema
/// has a load name its MCP servers again, as a new session does.
pub fn load_session(session: &str, cwd: &str, tools: Option<&McpCommand>) -> Value {
    json!({ "sessionId": session, "cwd": cwd, "mcpServers": mcp_servers(tools) })
}

/// The MCP servers a session request names: `maxd mcp` as the stdio
/// server [`SERVER`], or none. Cursor offers no MCP over ACP itself, as
/// its `mcpCapabilities` say, but it runs a stdio server it's given.
fn mcp_servers(tools: Option<&McpCommand>) -> Value {
    match tools {
        Some(tools) => json!([{
            "name": SERVER,
            "command": tools.command,
            "args": tools.args,
            "env": [],
        }]),
        None => json!([]),
    }
}

/// The `session/prompt` parameters.
pub fn prompt(session: &str, blocks: Vec<Value>) -> Value {
    json!({ "sessionId": session, "prompt": blocks })
}

/// The `session/cancel` line, a notification. The recording never
/// cancelled a turn, so this and the `cancelled` stop reason follow ACP's
/// schema.
pub fn cancel(session: &str) -> String {
    notification("session/cancel", json!({ "sessionId": session }))
}

/// The `session/set_config_option` parameters, from ACP's schema: the
/// recording never sent one.
pub fn set_config(session: &str, config: &str, value: &str) -> Value {
    json!({ "sessionId": session, "configId": config, "value": value })
}

/// v1's `session/set_mode` parameters, for a CLI with no mode option.
pub fn set_mode(session: &str, mode: &str) -> Value {
    json!({ "sessionId": session, "modeId": mode })
}

/// v1's unstable `session/set_model` parameters, for a CLI with no model
/// option.
pub fn set_model(session: &str, model: &str) -> Value {
    json!({ "sessionId": session, "modelId": model })
}

/// The `authenticate` parameters, to sign in with `method`.
pub fn authenticate(method: &str) -> Value {
    json!({ "methodId": method })
}

/// A `session/request_permission` answer picking `option`, as the
/// recording answered one.
pub fn selected(option: &str) -> Value {
    json!({ "outcome": { "outcome": "selected", "optionId": option } })
}

/// The answer for a prompt the turn no longer needs, for permission and
/// the CLI's own requests alike.
pub fn cancelled() -> Value {
    json!({ "outcome": { "outcome": "cancelled" } })
}

/// A string field of `value`.
pub fn text<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
    value.get(name).and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_sort_into_requests_notifications_and_responses() {
        let request =
            parse(r#"{"jsonrpc":"2.0","id":0,"method":"session/request_permission","params":{}}"#);
        assert!(matches!(request, Some(Incoming::Request { ref method, .. })
            if method == "session/request_permission"));
        let note = parse(r#"{"jsonrpc":"2.0","method":"session/update","params":{}}"#);
        assert!(matches!(note, Some(Incoming::Notification { .. })));
        let failed = parse(
            r#"{"jsonrpc":"2.0","id":"a-1","error":{"code":-32603,"message":"no","data":{"code":"rate_limited"}}}"#,
        );
        let Some(Incoming::Response { outcome, .. }) = failed else {
            panic!("not a response");
        };
        let error = outcome.unwrap_err();
        assert_eq!(error.code, -32603);
        assert_eq!(error.data, json!({ "code": "rate_limited" }));
        assert!(parse("not json").is_none());
        assert!(parse("{}").is_none());
    }

    #[test]
    fn the_generation_comes_from_the_replys_shape() {
        let v2 = json!({ "protocolVersion": 2, "info": {}, "capabilities": {} });
        assert_eq!(generation(&v2), Some(Generation::V2));
        let says_2 = json!({ "protocolVersion": 2, "agentCapabilities": {} });
        assert_eq!(generation(&says_2), Some(Generation::V1));
        assert_eq!(
            generation(&json!({ "protocolVersion": 1 })),
            Some(Generation::V1)
        );
        assert_eq!(generation(&json!({})), Some(Generation::V1));
        let neither = json!({ "protocolVersion": 3 });
        assert_eq!(generation(&neither), None);
        assert_eq!(
            unspoken("Example", &neither),
            "Example speaks ACP version 3, which MaxHQ doesn't"
        );
        assert_eq!(
            unspoken("Example", &json!({ "protocolVersion": "next" })),
            "Example speaks an ACP version MaxHQ doesn't know"
        );
    }

    /// A wire past a v2 `initialize`.
    fn v2_wire() -> Wire {
        let mut wire = Wire::default();
        wire.request("i", "initialize", initialize(json!({})));
        wire.read(r#"{"jsonrpc":"2.0","id":"i","result":{"protocolVersion":2,"info":{}}}"#);
        assert_eq!(wire.generation(), Some(Generation::V2));
        wire
    }

    fn idle(stop: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"s","update":{{"sessionUpdate":"state_update","state":"idle","stopReason":"{stop}"}}}}}}"#
        )
    }

    #[test]
    fn a_v2_turn_ends_as_its_prompts_result() {
        let mut wire = v2_wire();
        wire.request("p", "session/prompt", prompt("s", Vec::new()));
        assert!(wire
            .read(r#"{"jsonrpc":"2.0","id":"p","result":{}}"#)
            .is_none());
        let Some(Incoming::Response { id, outcome }) = wire.read(&idle("max_tokens")) else {
            panic!("no result");
        };
        assert_eq!(id, "p");
        assert_eq!(outcome.unwrap(), json!({ "stopReason": "max_tokens" }));
        // An idle state with no turn says nothing.
        assert!(wire.read(&idle("end_turn")).is_none());
    }

    #[test]
    fn a_v2_turn_read_back_after_a_restart_ends_by_its_shape() {
        let mut wire = v2_wire();
        let running = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"state_update","state":"running"}}}"#;
        assert!(wire.read(running).is_none());
        let Some(Incoming::Response { id, outcome }) = wire.read(&idle("end_turn")) else {
            panic!("no result");
        };
        assert_eq!(id, Value::Null);
        assert_eq!(outcome.unwrap()["stopReason"], "end_turn");
    }

    #[test]
    fn a_v1_wire_passes_lines_through() {
        let mut wire = Wire::default();
        wire.request("i", "initialize", initialize(json!({})));
        wire.read(
            r#"{"jsonrpc":"2.0","id":"i","result":{"protocolVersion":1,"agentCapabilities":{}}}"#,
        );
        assert_eq!(wire.generation(), Some(Generation::V1));
        let line = wire.request("l", "session/load", load_session("s", "/w", None));
        let line: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(line["method"], "session/load");
        assert!(line["params"].get("replayFrom").is_none());
        let Some(Incoming::Response { outcome, .. }) =
            wire.read(r#"{"jsonrpc":"2.0","id":"p","result":{}}"#)
        else {
            panic!("dropped");
        };
        assert_eq!(outcome.unwrap(), json!({}));
    }

    #[test]
    fn v2_mcp_servers_name_their_type() {
        let mut wire = v2_wire();
        let tools = McpCommand {
            command: "maxd".into(),
            args: Vec::new(),
        };
        let line = wire.request("n", "session/new", new_session("/w", Some(&tools)));
        let line: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(line["params"]["mcpServers"][0]["type"], "stdio");
    }

    #[test]
    fn every_line_names_its_version() {
        let line: Value = serde_json::from_str(&cancel("s")).unwrap();
        assert_eq!(line["jsonrpc"], "2.0");
        assert_eq!(line["params"]["sessionId"], "s");
        assert!(line.get("id").is_none());
    }
}
