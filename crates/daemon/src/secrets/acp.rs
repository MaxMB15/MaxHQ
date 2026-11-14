// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The values of local ACP agents' variables, one file per agent. The
//! definitions keep only the names, so a value is never in the database,
//! a log or an entity. The holder gets them as secret variables.

use std::collections::HashMap;
use std::io;
use std::sync::{Mutex, MutexGuard, PoisonError};

use maxhq_protocol::acp::AcpEnvValue;
use maxhq_protocol::ids::AcpAgentId;
use serde::{Deserialize, Serialize};
use tracing::warn;
use zeroize::Zeroizing;

use super::Secrets;

/// The values the store holds, by agent.
#[derive(Default)]
pub struct Values(Mutex<HashMap<AcpAgentId, Vec<AcpEnvValue>>>);

impl Values {
    fn lock(&self) -> MutexGuard<'_, HashMap<AcpAgentId, Vec<AcpEnvValue>>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// An agent's values, as its file holds them.
#[derive(Serialize, Deserialize)]
struct Stored {
    agent: AcpAgentId,
    values: Vec<AcpEnvValue>,
}

/// The file for the values of `agent`'s variables.
fn values_file(agent: AcpAgentId) -> String {
    format!("acp-{agent}.json")
}

impl Secrets {
    /// Keeps the values of `agent`'s variables named in `names`, with
    /// `changed` in place of what they held. A variable not in `names`
    /// loses its value, and an agent left with none loses its file.
    pub fn set_acp_env(
        &self,
        agent: AcpAgentId,
        names: &[String],
        changed: Vec<AcpEnvValue>,
    ) -> io::Result<()> {
        let mut held = self.inner.acp.lock();
        let mut values: Vec<AcpEnvValue> = held
            .get(&agent)
            .into_iter()
            .flatten()
            .filter(|value| {
                names.contains(&value.name) && !changed.iter().any(|new| new.name == value.name)
            })
            .cloned()
            .collect();
        values.extend(changed.into_iter().filter(|new| names.contains(&new.name)));
        let file = values_file(agent);
        if values.is_empty() {
            self.inner.folder.remove(&file)?;
            held.remove(&agent);
            return Ok(());
        }
        let stored = Stored { agent, values };
        let text = serde_json::to_vec(&stored)
            .map(Zeroizing::new)
            .map_err(io::Error::other)?;
        self.inner.folder.write(&file, &text)?;
        held.insert(agent, stored.values);
        Ok(())
    }

    /// The values stored for `agent`'s variables.
    pub(super) fn acp_values(&self, agent: AcpAgentId) -> Vec<AcpEnvValue> {
        let held = self.inner.acp.lock();
        held.get(&agent).cloned().unwrap_or_default()
    }

    /// Reads the values the folder holds.
    pub(super) fn load_acp(&self) {
        let names = match self.inner.folder.names() {
            Ok(names) => names,
            Err(err) => {
                warn!("couldn't list the stored ACP agent values: {err}");
                return;
            }
        };
        let mut held = self.inner.acp.lock();
        for name in names {
            if !(name.starts_with("acp-") && name.ends_with(".json")) {
                continue;
            }
            let Ok(Some(text)) = self.inner.folder.read(&name) else {
                warn!("skipped {name}: it couldn't be read");
                continue;
            };
            // The reason never quotes the file.
            match serde_json::from_str::<Stored>(text.expose()) {
                Ok(stored) if values_file(stored.agent) == name => {
                    held.insert(stored.agent, stored.values);
                }
                _ => warn!("skipped {name}: it doesn't hold an ACP agent's values"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::Secret;

    use super::*;
    use crate::secrets::testing::hub;
    use crate::secrets::DIR;
    use crate::testing::TempDir;

    fn value(name: &str, value: &str) -> AcpEnvValue {
        AcpEnvValue {
            name: name.to_owned(),
            value: Secret::new(value.to_owned()),
        }
    }

    fn names(values: &[AcpEnvValue]) -> Vec<(&str, &str)> {
        values
            .iter()
            .map(|value| (value.name.as_str(), value.value.expose()))
            .collect()
    }

    #[test]
    fn values_are_kept_replaced_and_dropped_by_name() {
        let temp = TempDir::new();
        let dir = temp.path().join(DIR);
        let secrets = Secrets::open(dir.clone(), hub()).unwrap();
        let agent = AcpAgentId::derive("gemini");
        let both = ["A".to_owned(), "B".to_owned()];
        secrets
            .set_acp_env(agent, &both, vec![value("A", "1"), value("B", "2")])
            .unwrap();
        // B keeps its value when only A changes.
        secrets
            .set_acp_env(agent, &both, vec![value("A", "3")])
            .unwrap();
        assert_eq!(names(&secrets.acp_values(agent)), [("B", "2"), ("A", "3")]);
        // A value for a name the definition doesn't have isn't kept.
        secrets
            .set_acp_env(agent, &["B".to_owned()], vec![value("C", "4")])
            .unwrap();
        assert_eq!(names(&secrets.acp_values(agent)), [("B", "2")]);

        let reopened = Secrets::open(dir.clone(), hub()).unwrap();
        assert_eq!(names(&reopened.acp_values(agent)), [("B", "2")]);

        secrets.set_acp_env(agent, &[], Vec::new()).unwrap();
        assert!(secrets.acp_values(agent).is_empty());
        assert!(!dir.join(values_file(agent)).exists());
    }
}
