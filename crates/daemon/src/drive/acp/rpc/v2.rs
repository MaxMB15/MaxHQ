// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the negotiating
// `initialize`, and the v2 shapes turned into v1's (the initialize
// response, config options, permission requests, MCP servers and resume
// requests, and the method each call takes on v2), translated to Rust over
// JSON values. Which of them the driver needs is MaxHQ's.

//! ACP v2's shapes, from the schema [`V2_SCHEMA`](super::V2_SCHEMA)
//! names, and the v1 shapes the driver reads them as.

use serde_json::{json, Map, Value};

use super::text;

/// Adds v2's fields to v1's `initialize` parameters: the version, the
/// client's `info`, and its `capabilities`, which carry only the `_meta`.
pub fn initialize(params: &mut Value) {
    let info = params["clientInfo"].clone();
    let meta = params["clientCapabilities"]["_meta"].clone();
    params["protocolVersion"] = json!(2);
    params["info"] = info;
    params["capabilities"] = json!({ "_meta": meta });
}

/// A v2 `initialize` reply in v1's shape: a session capability means the
/// CLI loads sessions, and a sign-in method's `methodId` is its `id`.
pub fn initialized(result: Value) -> Value {
    let session = result
        .pointer("/capabilities/session")
        .filter(|session| !session.is_null());
    let image = session
        .and_then(|session| session.pointer("/prompt/image"))
        .is_some_and(|image| !image.is_null());
    let methods: Vec<Value> = result
        .get("authMethods")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|method| renamed(method, "methodId", "id"))
        .collect();
    json!({
        "protocolVersion": result.get("protocolVersion").cloned().unwrap_or(Value::Null),
        "agentInfo": result.get("info").cloned().unwrap_or(Value::Null),
        "agentCapabilities": {
            "loadSession": session.is_some(),
            "promptCapabilities": { "image": image },
        },
        "authMethods": methods,
    })
}

/// A request the driver gives in v1's method and parameters, in v2's. A
/// sign-in is `auth/login`, a load is `session/resume` from the start,
/// MCP servers name their type, and a config option's value is an ID.
pub fn request(method: &str, mut params: Value) -> (&str, Value) {
    if let Some(servers) = params.get_mut("mcpServers").and_then(Value::as_array_mut) {
        for server in servers {
            if server.get("type").is_none() {
                server["type"] = json!("stdio");
            }
        }
    }
    match method {
        "authenticate" => ("auth/login", params),
        "session/load" => {
            params["replayFrom"] = json!({ "type": "start" });
            ("session/resume", params)
        }
        "session/set_config_option" => {
            params["type"] = json!("id");
            (method, params)
        }
        _ => (method, params),
    }
}

/// A result in v1's shape: its config options name their `id`.
pub fn result(mut result: Value) -> Value {
    options(&mut result);
    result
}

/// A `session/update`'s parameters in v1's shape. A list of plan items is
/// v1's `plan`, and config options name their `id`. The rest is the same
/// in both.
pub fn update(mut params: Value) -> Value {
    let Some(update) = params.get_mut("update") else {
        return params;
    };
    match text(update, "sessionUpdate") {
        Some("config_option_update") => options(update),
        Some("plan_update") if update.pointer("/plan/type") == Some(&json!("items")) => {
            let entries = update["plan"]["entries"].take();
            *update = json!({ "sessionUpdate": "plan", "entries": entries });
        }
        _ => {}
    }
    params
}

/// A v2 `session/request_permission`'s parameters in v1's shape, with the
/// `toolCall` its subject names. A command's subject names its call's ID
/// at most, so the request's ID stands in when it doesn't.
pub fn permission(mut params: Value, id: &Value) -> Value {
    if params.get("toolCall").is_some() {
        return params;
    }
    let subject = params.get("subject").cloned().unwrap_or(Value::Null);
    let tool = match text(&subject, "type") {
        Some("tool_call") if subject.get("toolCall").is_some() => subject["toolCall"].clone(),
        kind => {
            let call = text(&subject, "toolCallId")
                .filter(|_| kind == Some("command"))
                .map_or_else(|| super::key(id).unwrap_or_default(), str::to_owned);
            let kind = if kind == Some("command") {
                "execute"
            } else {
                "other"
            };
            let mut tool = json!({ "toolCallId": call, "kind": kind });
            if let Some(title) = params.get("title") {
                tool["title"] = title.clone();
            }
            if let Some(command) = subject.get("command") {
                tool["rawInput"] = json!({ "command": command });
            }
            tool
        }
    };
    params["toolCall"] = tool;
    params
}

/// Gives each of `value`'s config options its `configId` as `id`.
fn options(value: &mut Value) {
    if let Some(options) = value.get_mut("configOptions").and_then(Value::as_array_mut) {
        for option in options {
            if let Some(renamed) = renamed(option, "configId", "id") {
                *option = renamed;
            }
        }
    }
}

/// `value` with its field `from` named `to`, or none when it has no
/// `from`.
fn renamed(value: &Value, from: &str, to: &str) -> Option<Value> {
    let mut fields: Map<String, Value> = value.as_object()?.clone();
    let moved = fields.remove(from)?;
    fields.insert(to.to_owned(), moved);
    Some(Value::Object(fields))
}
