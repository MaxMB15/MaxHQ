// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The names you give agents. A name is stored apart from the held
//! record, so it covers agents seen outside MaxHQ too, and the hub lays
//! it over every title their CLIs or observers report.

use std::collections::HashMap;

use maxhq_protocol::ids::AgentId;
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::state::EntityKey;
use tracing::warn;

use super::requests::given_title;
use super::{refused, Manager};

impl Manager {
    /// Shows the names the previous run stored.
    pub(super) fn load_names(&self) {
        let names = self.db.agent_names().unwrap_or_else(|err| {
            warn!(%err, "couldn't read the agents' names");
            HashMap::new()
        });
        for (agent, name) in names {
            self.hub.set_name(agent, Some(name));
        }
    }

    /// Answers `RenameAgent`: stores the name and shows it. A blank one
    /// clears the name.
    pub(super) fn rename(&self, agent: AgentId, title: &str) -> ActionOutcome {
        if self.hub.get(&EntityKey::Agent(agent)).is_none() {
            return ActionOutcome::UnknownAgent;
        }
        let name = given_title(title);
        if let Err(err) = self.db.set_agent_name(agent, name.as_deref()) {
            return refused(format!("couldn't store the name: {err}"));
        }
        self.send_waiting(agent);
        self.hub.set_name(agent, name);
        ActionOutcome::Done
    }
}
