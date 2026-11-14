// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What an agent says it can do: the capabilities and sign-in methods in
//! its `initialize` reply, and the modes and models a session lists. A
//! session lists them as config options, or else in ACP v1's `modes` and
//! unstable `models`, which an agent with no config options still sends.

use maxhq_protocol::acp::{AcpAuthMethod, AcpFeatures};
use maxhq_protocol::model::ModelChoice;
use serde_json::Value;

use super::rpc::text;

/// The mode name that is MaxHQ's Plan.
pub const PLAN: &str = "plan";

/// What an `initialize` reply says the agent can do. Whether it has a plan
/// mode comes from its session, so it's left off here.
pub fn features(result: &Value) -> AcpFeatures {
    let caps = result.get("agentCapabilities");
    let flag = |pointer: &str| {
        caps.and_then(|caps| caps.pointer(pointer))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    AcpFeatures {
        images: flag("/promptCapabilities/image"),
        load: flag("/loadSession"),
        plan: false,
        auth: methods(result)
            .map(|method| AcpAuthMethod {
                id: method.id.to_owned(),
                name: text(method.raw, "name").unwrap_or(method.id).to_owned(),
                description: text(method.raw, "description").map(str::to_owned),
            })
            .collect(),
    }
}

/// The first sign-in method the agent runs itself, which `authenticate`
/// can start. ACP's methods with no `type`, or `agent`, are those. A
/// terminal method or a variable to set is yours to do.
pub fn agent_sign_in(result: &Value) -> Option<String> {
    methods(result)
        .find(|method| matches!(text(method.raw, "type"), None | Some("agent")))
        .map(|method| method.id.to_owned())
}

/// The IDs of the sign-in methods you run in a terminal, as the agent's
/// own command with the arguments it names.
pub fn terminal_methods(result: &Value) -> Vec<String> {
    methods(result)
        .filter(|method| text(method.raw, "type") == Some("terminal"))
        .map(|method| method.id.to_owned())
        .collect()
}

/// One listed sign-in method.
struct Method<'a> {
    id: &'a str,
    raw: &'a Value,
}

/// The sign-in methods a reply lists, with their IDs.
fn methods(result: &Value) -> impl Iterator<Item = Method<'_>> {
    result
        .get("authMethods")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|raw| {
            Some(Method {
                id: text(raw, "id")?,
                raw,
            })
        })
}

/// The names of v1's fields for its modes.
pub const MODES: Legacy = Legacy {
    state: "modes",
    current: "currentModeId",
    list: "availableModes",
    id: "id",
};

/// The names of v1's fields for its models.
pub const MODELS: Legacy = Legacy {
    state: "models",
    current: "currentModelId",
    list: "availableModels",
    id: "modelId",
};

/// Where v1 keeps one kind of choice in a session's result.
pub struct Legacy {
    state: &'static str,
    current: &'static str,
    list: &'static str,
    id: &'static str,
}

/// The choices v1 lists in `result`, with the current one as the default
/// and its ID. None when the result doesn't list them.
pub fn legacy(result: &Value, names: &Legacy) -> Option<(Vec<ModelChoice>, Option<String>)> {
    let state = result.get(names.state)?;
    let current = text(state, names.current);
    let choices = state
        .get(names.list)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let value = text(entry, names.id)?;
            Some(ModelChoice {
                id: value.to_owned(),
                label: text(entry, "name").unwrap_or(value).to_owned(),
                default: Some(value) == current,
                efforts: Vec::new(),
                default_effort: None,
            })
        })
        .collect();
    Some((choices, current.map(str::to_owned)))
}

/// The mode that is MaxHQ's Plan: the one whose ID or name is "plan".
pub fn plan_mode(modes: &[ModelChoice]) -> Option<&str> {
    modes
        .iter()
        .find(|mode| mode.id.eq_ignore_ascii_case(PLAN) || mode.label.eq_ignore_ascii_case(PLAN))
        .map(|mode| mode.id.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn features_come_from_the_capabilities_and_methods() {
        let result = json!({
            "protocolVersion": 1,
            "agentCapabilities": { "loadSession": true, "promptCapabilities": { "image": false } },
            "authMethods": [
                { "id": "key", "name": "API key", "type": "env_var", "vars": [{ "name": "KEY" }] },
                { "id": "browser", "name": "Log in", "description": "Opens a browser" },
            ],
        });
        let found = features(&result);
        assert!(found.load && !found.images && !found.plan);
        assert_eq!(found.auth.len(), 2);
        assert_eq!(
            found.auth[1].description.as_deref(),
            Some("Opens a browser")
        );
        assert_eq!(agent_sign_in(&result).as_deref(), Some("browser"));
        assert_eq!(features(&json!({})), AcpFeatures::default());
    }

    #[test]
    fn terminal_and_variable_methods_arent_the_agents_to_run() {
        let result = json!({ "authMethods": [
            { "id": "t", "name": "Terminal", "type": "terminal" },
            { "id": "v", "name": "Key", "type": "env_var" },
        ] });
        assert_eq!(agent_sign_in(&result), None);
        let typed = json!({ "authMethods": [{ "id": "a", "name": "A", "type": "agent" }] });
        assert_eq!(agent_sign_in(&typed).as_deref(), Some("a"));
        assert_eq!(terminal_methods(&result), ["t"]);
    }

    #[test]
    fn a_plan_mode_is_found_by_id_or_name() {
        let result = json!({ "modes": {
            "currentModeId": "default",
            "availableModes": [
                { "id": "default", "name": "Default" },
                { "id": "architect", "name": "Plan" },
            ],
        } });
        let (modes, current) = legacy(&result, &MODES).unwrap();
        assert!(legacy(&result, &MODELS).is_none());
        assert_eq!(current.as_deref(), Some("default"));
        assert!(modes[0].default);
        assert_eq!(plan_mode(&modes), Some("architect"));
        assert_eq!(plan_mode(&modes[..1]), None);
    }
}
