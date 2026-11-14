// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The driver for one held local ACP command: an agent MaxHQ doesn't know
//! by name, which you define on the device by its program, arguments and
//! variables. One process holds one session.
//!
//! Its flavor of the shared ACP driver adds nothing to ACP: no extensions,
//! no failure texts, no limit codes and no tool timeout. Full access is
//! maxd answering every request, and plan mode is a mode the session
//! lists, so neither takes a new process. A CLI that asks for a sign-in
//! gets `authenticate` with the method it runs itself.

use std::path::PathBuf;
use std::time::Duration;

use maxhq_protocol::acp::{AcpAgent, AcpEnvValue};
use maxhq_protocol::ids::AgentId;
use maxhq_protocol::model::{PermissionMode, Vendor};
use serde_json::{json, Value};

use super::acp::rpc::RpcError;
use super::acp::{AcpSession, Ext, ExtPrompt, Failure, Flavor, Kept};
use crate::drive::{Driver, Framing, Launch, Login, LoginAuth, Place};

/// What the stream's item IDs start with.
const BASE: &str = "acp";

/// What a local agent's launch runs: the definition, and the values of its
/// variables, which go only in the holder's secret variables.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcpCommand {
    /// The definition as the device keeps it.
    pub agent: AcpAgent,
    /// A value for each of `agent.env` the device has one for.
    pub env: Vec<AcpEnvValue>,
}

/// How to run the command `login` holds for an agent in `place`. A login
/// without one runs nothing.
pub fn launch(login: &Login, place: &Place) -> Launch {
    match &login.auth {
        LoginAuth::AcpCommand(command) => super::acp::launch::launch(command, place),
        _ => Launch {
            program: PathBuf::new(),
            args: Vec::new(),
            env: Vec::new(),
            env_remove: Vec::new(),
            cwd: place.cwd.clone(),
            framing: Framing::JsonRpc,
            secret_env: Vec::new(),
            run_as: None,
        },
    }
}

/// The driver for a held local agent.
pub fn session(
    login: &Login,
    place: &Place,
    agent: AgentId,
    kept: Kept,
    mode: PermissionMode,
) -> Box<dyn Driver> {
    Box::new(AcpSession::new(
        Box::new(Local::of(login)),
        login,
        place,
        agent,
        kept,
        mode,
    ))
}

/// A local command's flavor, named as you named the command.
#[derive(Clone, Debug)]
pub struct Local {
    name: String,
}

impl Local {
    /// The flavor for the command `login` runs.
    pub fn of(login: &Login) -> Self {
        match &login.auth {
            LoginAuth::AcpCommand(command) => Self::new(&command.agent),
            _ => Self {
                name: "ACP agent".to_owned(),
            },
        }
    }

    /// The flavor for `agent`.
    pub fn new(agent: &AcpAgent) -> Self {
        Self {
            name: agent.name.clone(),
        }
    }
}

impl Flavor for Local {
    fn name(&self) -> &str {
        &self.name
    }

    fn vendor(&self) -> Vendor {
        Vendor::Acp
    }

    fn base(&self) -> &str {
        BASE
    }

    fn client_meta(&self) -> Value {
        json!({})
    }

    fn notification(&self, _method: &str, _params: &Value) -> Option<Ext> {
        None
    }

    fn request(&self, _method: &str, _params: &Value) -> Option<ExtPrompt> {
        None
    }

    fn reply_failure(&self, _reply: &str) -> Option<Failure> {
        None
    }

    fn is_limit(&self, _error: &RpcError) -> bool {
        false
    }

    fn tool_timeout(&self) -> Option<Duration> {
        None
    }

    fn auto_model(&self) -> Option<&str> {
        None
    }

    fn modes_are_launch_flags(&self) -> bool {
        false
    }

    fn authenticates(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drive::cursor::Cursor;
    use maxhq_protocol::ids::{AcpAgentId, DeviceId};

    fn agent() -> AcpAgent {
        AcpAgent {
            id: AcpAgentId::derive("gemini"),
            device: DeviceId::derive("devbox"),
            name: "Gemini".to_owned(),
            program: "gemini".to_owned(),
            args: Vec::new(),
            env: Vec::new(),
            containers: false,
            problem: None,
            skills: Default::default(),
        }
    }

    #[test]
    fn a_local_command_adds_nothing_to_acp_and_cursor_keeps_its_own() {
        let local = Local::new(&agent());
        let error = RpcError {
            code: -32001,
            message: "Slow down".to_owned(),
            data: json!({ "code": "resource_exhausted" }),
        };
        assert_eq!(local.name(), "Gemini");
        assert_eq!(local.vendor(), Vendor::Acp);
        assert_eq!(local.base(), "acp");
        assert_eq!(local.reply_failure("Authentication required"), None);
        assert!(!local.is_limit(&error));
        assert_eq!(local.tool_timeout(), None);
        assert_eq!(local.auto_model(), None);
        assert!(!local.modes_are_launch_flags());
        assert!(local.authenticates());

        assert!(Cursor.is_limit(&error));
        assert!(Cursor.tool_timeout().is_some());
        assert!(Cursor.auto_model().is_some());
        assert!(Cursor.modes_are_launch_flags());
        assert!(!Cursor.authenticates());
    }
}
