// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Agents MaxHQ doesn't know by name, run as a local command that speaks
//! the Agent Client Protocol. A definition belongs to the device it runs
//! on, and shows up as one account with [`Vendor::Acp`].
//!
//! [`Vendor::Acp`]: crate::model::Vendor::Acp

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::{AcpAgentId, DeviceId};
use crate::Secret;

/// A local ACP command on one device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct AcpAgent {
    /// Stable across changes to the definition.
    pub id: AcpAgentId,
    /// The device that runs it.
    pub device: DeviceId,
    /// What the app calls it, such as "Gemini".
    pub name: String,
    /// A name looked up on `PATH`, or an absolute path.
    pub program: String,
    /// One literal argument each, passed without a shell.
    #[serde(default)]
    pub args: Vec<String>,
    /// The names of the variables set for it. The device keeps their
    /// values as secrets and never sends them back.
    #[serde(default)]
    pub env: Vec<String>,
    /// Whether agents of it may also run in the device's containers.
    #[serde(default)]
    pub containers: bool,
    /// What went wrong the last time the device tried it, such as a program
    /// it couldn't find.
    #[serde(default)]
    pub problem: Option<String>,
    /// Whether it reads the project's skills. A definition from before it
    /// leaves this out, and reads as [`AcpSkills::Agents`].
    #[serde(default)]
    pub skills: AcpSkills,
}

/// Which skills an [`AcpAgent`] reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AcpSkills {
    /// It reads `.agents/skills`, as most ACP agents do.
    #[default]
    Agents,
    /// It reads none.
    None,
}

/// One variable's value for an [`AcpAgent`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct AcpEnvValue {
    /// The variable, one of [`AcpAgent::env`].
    pub name: String,
    /// Its value.
    pub value: Secret,
}

/// What an ACP agent said it can do in its `initialize` reply.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct AcpFeatures {
    /// It takes images inline. Without this they go as paths in the text.
    #[serde(default)]
    pub images: bool,
    /// It loads an earlier session, so a restart keeps the history.
    #[serde(default)]
    pub load: bool,
    /// It has a mode whose ID or name is "plan", which MaxHQ's Plan uses.
    #[serde(default)]
    pub plan: bool,
    /// The ways it signs in, as it lists them.
    #[serde(default)]
    pub auth: Vec<AcpAuthMethod>,
}

/// One way an ACP agent signs in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct AcpAuthMethod {
    /// The agent's ID for it, which `authenticate` takes.
    pub id: String,
    /// What it's called.
    pub name: String,
    /// What it does, if the agent says.
    #[serde(default)]
    pub description: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn agent() -> AcpAgent {
        AcpAgent {
            id: "0199a7c2-0000-7000-8000-0000000000e1".parse().unwrap(),
            device: "0199a7c2-0000-7000-8000-000000000001".parse().unwrap(),
            name: "Gemini".into(),
            program: "gemini".into(),
            args: vec!["--experimental-acp".into()],
            env: vec!["GEMINI_API_KEY".into()],
            containers: false,
            problem: None,
            skills: AcpSkills::None,
        }
    }

    #[test]
    fn agents_survive_a_round_trip() {
        let agent = agent();
        let value = serde_json::to_value(&agent).unwrap();
        assert_eq!(value["env"], json!(["GEMINI_API_KEY"]));
        assert_eq!(serde_json::from_value::<AcpAgent>(value).unwrap(), agent);
    }

    #[test]
    fn a_bare_agent_takes_the_defaults() {
        let agent: AcpAgent = serde_json::from_value(json!({
            "id": "0199a7c2-0000-7000-8000-0000000000e1",
            "device": "0199a7c2-0000-7000-8000-000000000001",
            "name": "Gemini",
            "program": "gemini",
        }))
        .unwrap();
        assert!(agent.args.is_empty() && agent.env.is_empty());
        assert!(!agent.containers);
        assert_eq!(agent.problem, None);
        assert_eq!(agent.skills, AcpSkills::Agents);
    }

    #[test]
    fn features_survive_a_round_trip_and_default_to_none() {
        let features = AcpFeatures {
            images: true,
            load: true,
            plan: false,
            auth: vec![AcpAuthMethod {
                id: "oauth-personal".into(),
                name: "Log in with Google".into(),
                description: None,
            }],
        };
        let value = serde_json::to_value(&features).unwrap();
        assert_eq!(
            serde_json::from_value::<AcpFeatures>(value).unwrap(),
            features
        );
        let bare: AcpFeatures = serde_json::from_value(json!({})).unwrap();
        assert_eq!(bare, AcpFeatures::default());
    }
}
