// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A-10's instance keys, which make a container its own record, and the
//! fuller record `maxd` keeps for each workspace in its database.

use std::collections::BTreeMap;

use maxhq_protocol::ids::{DeviceId, ProjectId, WorkspaceId};
use maxhq_protocol::model::{
    EgressCheck, GuestState, ManagedWorkspace, UnsavedSummary, Workspace, WorkspaceLocation,
    WorkspaceState,
};
use maxhq_protocol::time::Timestamp;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::incus::MANAGED_KEY;
use crate::session::holders::AgentSummary;

/// The schema these keys follow.
pub const SCHEMA: &str = "1";

/// The key names, each under `user.maxhq.`.
pub mod key {
    /// The schema of the other keys.
    pub const SCHEMA: &str = "user.maxhq.schema";
    /// The project's ID.
    pub const PROJECT: &str = "user.maxhq.project";
    /// The device's ID.
    pub const DEVICE: &str = "user.maxhq.device";
    /// The workspace's ID.
    pub const WORKSPACE: &str = "user.maxhq.workspace";
    /// The project's name.
    pub const TITLE: &str = "user.maxhq.title";
    /// The clone URL.
    pub const REPO: &str = "user.maxhq.repo";
    /// Whether it starts when the host boots.
    pub const BOOT: &str = "user.maxhq.boot";
    /// The recipe version applied.
    pub const RECIPE: &str = "user.maxhq.recipe";
    /// The agents, as JSON.
    pub const AGENTS: &str = "user.maxhq.agents";
    /// When the workspace was made, in milliseconds since the epoch.
    pub const CREATED: &str = "user.maxhq.created";
    /// Where the repo is, for a workspace moved in from the old scripts,
    /// whose repo stays where they put it.
    pub const PATH: &str = "user.maxhq.path";
}

/// What a container says about itself in its instance keys.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keys {
    /// The project.
    pub project: ProjectId,
    /// The device whose `maxd` made it.
    pub device: DeviceId,
    /// The workspace.
    pub workspace: WorkspaceId,
    /// The project's name.
    pub title: String,
    /// The repo's clone URL.
    pub repo: String,
    /// Whether it starts when the host boots.
    pub boot: bool,
    /// The recipe version applied. 0 until the create finishes.
    pub recipe: u32,
    /// The agents that run in it.
    pub agents: Vec<AgentSummary>,
    /// When it was made.
    pub created: Timestamp,
    /// Where the repo is, when it isn't where MaxHQ clones one: a moved
    /// workspace keeps the folder the old scripts used.
    #[serde(default)]
    pub path: Option<String>,
}

/// Why an instance's keys can't be read.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum KeyError {
    /// A key is missing.
    #[error("{0} is missing")]
    Missing(&'static str),
    /// A key holds something it can't.
    #[error("{key} holds {value:?}")]
    Bad {
        /// The key.
        key: &'static str,
        /// What it holds.
        value: String,
    },
    /// The keys follow a schema this build doesn't know.
    #[error("the keys follow schema {0}, which this maxd doesn't know")]
    Schema(String),
}

impl Keys {
    /// The keys to write on the instance, including the managed mark.
    pub fn encode(&self) -> BTreeMap<String, String> {
        let agents = serde_json::to_string(&self.agents).unwrap_or_else(|_| "[]".to_owned());
        [
            (MANAGED_KEY, "true".to_owned()),
            (key::SCHEMA, SCHEMA.to_owned()),
            (key::PROJECT, self.project.to_string()),
            (key::DEVICE, self.device.to_string()),
            (key::WORKSPACE, self.workspace.to_string()),
            (key::TITLE, self.title.clone()),
            (key::REPO, self.repo.clone()),
            (key::BOOT, self.boot.to_string()),
            (key::RECIPE, self.recipe.to_string()),
            (key::AGENTS, agents),
            (key::CREATED, self.created.0.to_string()),
        ]
        .into_iter()
        .chain(self.path.clone().map(|path| (key::PATH, path)))
        .map(|(k, v)| (k.to_owned(), v))
        .collect()
    }

    /// Reads the keys from an instance's own config.
    pub fn decode(config: &BTreeMap<String, String>) -> Result<Self, KeyError> {
        let get = |key: &'static str| {
            config
                .get(key)
                .map(String::as_str)
                .filter(|v| !v.is_empty())
                .ok_or(KeyError::Missing(key))
        };
        let bad = |key: &'static str, value: &str| KeyError::Bad {
            key,
            value: value.to_owned(),
        };
        if config.get(MANAGED_KEY).map(String::as_str) != Some("true") {
            return Err(KeyError::Missing(MANAGED_KEY));
        }
        let schema = get(key::SCHEMA)?;
        if schema != SCHEMA {
            return Err(KeyError::Schema(schema.to_owned()));
        }
        fn id<T: std::str::FromStr>(key: &'static str, value: &str) -> Result<T, KeyError> {
            value.parse().map_err(|_| KeyError::Bad {
                key,
                value: value.to_owned(),
            })
        }
        let boot = match get(key::BOOT)? {
            "true" => true,
            "false" => false,
            other => return Err(bad(key::BOOT, other)),
        };
        let recipe = get(key::RECIPE)?;
        let recipe = recipe.parse().map_err(|_| bad(key::RECIPE, recipe))?;
        let agents = match config.get(key::AGENTS).filter(|v| !v.is_empty()) {
            Some(json) => serde_json::from_str(json).map_err(|_| bad(key::AGENTS, json))?,
            None => Vec::new(),
        };
        let created = match config.get(key::CREATED).filter(|v| !v.is_empty()) {
            Some(ms) => Timestamp(ms.parse().map_err(|_| bad(key::CREATED, ms))?),
            None => Timestamp(0),
        };
        Ok(Self {
            project: id(key::PROJECT, get(key::PROJECT)?)?,
            device: id(key::DEVICE, get(key::DEVICE)?)?,
            workspace: id(key::WORKSPACE, get(key::WORKSPACE)?)?,
            title: get(key::TITLE)?.to_owned(),
            repo: get(key::REPO)?.to_owned(),
            boot,
            recipe,
            agents,
            created,
            path: config.get(key::PATH).filter(|v| !v.is_empty()).cloned(),
        })
    }
}

/// What `maxd` keeps for a workspace in its `workspaces` table. The keys
/// are the truth. The rest is this device's view, rebuilt when lost.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// The instance.
    pub instance: String,
    /// What the instance keys say.
    pub keys: Keys,
    /// Where it stands.
    pub state: WorkspaceState,
    /// Its guest.
    pub guest: GuestState,
    /// Its last egress check.
    pub egress: EgressCheck,
    /// Why the last egress check didn't pass when nothing answered, such
    /// as a probe that couldn't run.
    #[serde(default)]
    pub egress_note: Option<String>,
    /// The pool its root disk is on.
    #[serde(default)]
    pub pool: Option<String>,
    /// The network its NIC is on.
    #[serde(default)]
    pub network: Option<String>,
    /// The host boot it was last started for, so A-11 starts it once per
    /// boot.
    #[serde(default)]
    pub host_boot: Option<String>,
    /// The public half of its guest's SSH host key, once reported.
    #[serde(default)]
    pub ssh_host_key: Option<String>,
    /// Its soft memory limit in bytes, as its keys set it.
    #[serde(default)]
    pub memory_soft: Option<u64>,
    /// The latest unsaved work its guest reported.
    #[serde(default)]
    pub unsaved: Option<UnsavedSummary>,
}

impl Record {
    /// The repo's folder in the container.
    pub fn repo_dir(&self) -> String {
        if let Some(path) = &self.keys.path {
            return path.clone();
        }
        super::repo::Repo::parse(&self.keys.repo)
            .map_or_else(|| crate::recipe::WORK.to_owned(), |repo| repo.dir())
    }

    /// The workspace as clients see it.
    pub fn entity(&self) -> Workspace {
        Workspace {
            id: self.keys.workspace,
            project: Some(self.keys.project),
            device: self.keys.device,
            location: WorkspaceLocation::Container {
                instance: self.instance.clone(),
                path: self.repo_dir(),
            },
            managed: Some(ManagedWorkspace {
                state: self.state.clone(),
                start_at_boot: self.keys.boot,
                guest: self.guest.clone(),
                egress: self.egress.clone(),
                recipe: self.keys.recipe,
                created_at: self.keys.created,
                ssh_host_key: self.ssh_host_key.clone(),
                unsaved: self.unsaved,
                memory_soft: self.memory_soft,
            }),
            clone: None,
        }
    }

    /// Why agents can't start in it now. None once it runs and its egress
    /// check passed.
    pub fn blocked(&self) -> Option<String> {
        let state = match &self.state {
            WorkspaceState::Running => None,
            WorkspaceState::Creating { step, .. } => Some(format!("it's still being made: {step}")),
            WorkspaceState::Starting => Some("it's starting".to_owned()),
            WorkspaceState::Stopping => Some("it's stopping".to_owned()),
            WorkspaceState::Stopped => Some("it's stopped".to_owned()),
            WorkspaceState::Deleting => Some("it's being deleted".to_owned()),
            WorkspaceState::StorageLocked => Some("its storage is locked".to_owned()),
            WorkspaceState::Failed { reason } => Some(format!("it failed: {reason}")),
        };
        if let Some(state) = state {
            return Some(format!(
                "{} can't run agents because {state}",
                self.instance
            ));
        }
        match &self.egress {
            EgressCheck::Passed { .. } => None,
            EgressCheck::NotRun => Some(format!(
                "{} can't run agents until its egress check passes",
                self.instance
            )),
            EgressCheck::Failed { reached, .. } if reached.is_empty() => Some(format!(
                "{} can't run agents because its egress check didn't finish: {}",
                self.instance,
                self.egress_note
                    .as_deref()
                    .unwrap_or("the probe gave no answer")
            )),
            EgressCheck::Failed { reached, .. } => Some(format!(
                "{} can't run agents because it reached {}, which it shouldn't",
                self.instance,
                reached.join(", ")
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::ids::{AccountId, AgentId};
    use maxhq_protocol::model::Vendor;

    use super::*;

    pub(crate) fn keys() -> Keys {
        Keys {
            project: ProjectId::derive("git:github.com/example-user/maxhq"),
            device: DeviceId::derive("devbox"),
            workspace: WorkspaceId::derive("ws"),
            title: "maxhq".to_owned(),
            repo: "https://github.com/Example-User/MaxHQ.git".to_owned(),
            boot: true,
            recipe: 1,
            agents: vec![AgentSummary {
                agent: AgentId::derive("a"),
                vendor: Vendor::Claude,
                account: AccountId::derive("acc"),
                session: "5d0c".to_owned(),
                branch: "fix-x".to_owned(),
                worktree: "/home/agent/work/.worktrees/fix-x".to_owned(),
            }],
            created: Timestamp(1_700_000_000_000),
            path: None,
        }
    }

    fn record(state: WorkspaceState, egress: EgressCheck) -> Record {
        Record {
            instance: "mhq-maxhq".to_owned(),
            keys: keys(),
            state,
            guest: GuestState::Installing,
            egress,
            egress_note: None,
            pool: Some("agents".to_owned()),
            network: Some("agentbr0".to_owned()),
            host_boot: None,
            ssh_host_key: None,
            memory_soft: None,
            unsaved: None,
        }
    }

    #[test]
    fn keys_round_trip() {
        let keys = keys();
        let encoded = keys.encode();
        assert_eq!(encoded[MANAGED_KEY], "true");
        assert_eq!(encoded[key::SCHEMA], "1");
        assert_eq!(encoded[key::BOOT], "true");
        assert_eq!(Keys::decode(&encoded).unwrap(), keys);

        let mut empty = keys.clone();
        empty.agents.clear();
        empty.boot = false;
        assert_eq!(Keys::decode(&empty.encode()).unwrap(), empty);
    }

    #[test]
    fn decoding_names_what_is_wrong() {
        let mut config = keys().encode();
        config.remove(key::AGENTS);
        config.remove(key::CREATED);
        let decoded = Keys::decode(&config).unwrap();
        assert!(decoded.agents.is_empty());
        assert_eq!(decoded.created, Timestamp(0));

        let mut config = keys().encode();
        config.insert(MANAGED_KEY.to_owned(), "false".to_owned());
        assert_eq!(Keys::decode(&config), Err(KeyError::Missing(MANAGED_KEY)));

        let mut config = keys().encode();
        config.insert(key::SCHEMA.to_owned(), "2".to_owned());
        assert_eq!(Keys::decode(&config), Err(KeyError::Schema("2".to_owned())));

        let mut config = keys().encode();
        config.remove(key::REPO);
        assert_eq!(Keys::decode(&config), Err(KeyError::Missing(key::REPO)));

        let mut config = keys().encode();
        config.insert(key::BOOT.to_owned(), "yes".to_owned());
        assert!(matches!(
            Keys::decode(&config),
            Err(KeyError::Bad { key: key::BOOT, .. })
        ));

        let mut config = keys().encode();
        config.insert(key::PROJECT.to_owned(), "not-an-id".to_owned());
        assert!(matches!(
            Keys::decode(&config),
            Err(KeyError::Bad {
                key: key::PROJECT,
                ..
            })
        ));

        let mut config = keys().encode();
        config.insert(key::AGENTS.to_owned(), "{".to_owned());
        assert!(matches!(
            Keys::decode(&config),
            Err(KeyError::Bad {
                key: key::AGENTS,
                ..
            })
        ));
    }

    #[test]
    fn a_record_shows_as_a_container_workspace() {
        let record = record(WorkspaceState::Running, EgressCheck::NotRun);
        let entity = record.entity();
        assert_eq!(
            entity.location,
            WorkspaceLocation::Container {
                instance: "mhq-maxhq".to_owned(),
                path: "/home/agent/work/MaxHQ".to_owned(),
            }
        );
        let managed = entity.managed.unwrap();
        assert!(managed.start_at_boot);
        assert_eq!(managed.recipe, 1);
        assert_eq!(managed.created_at, Timestamp(1_700_000_000_000));

        let back: Record = serde_json::from_str(&serde_json::to_string(&record).unwrap()).unwrap();
        assert_eq!(back, record);
    }

    #[test]
    fn agents_wait_for_a_running_workspace_and_a_passed_check() {
        let passed = EgressCheck::Passed { at: Timestamp(1) };
        assert_eq!(
            record(WorkspaceState::Running, passed.clone()).blocked(),
            None
        );
        let stopped = record(WorkspaceState::Stopped, passed).blocked().unwrap();
        assert!(stopped.contains("stopped"), "{stopped}");
        let not_run = record(WorkspaceState::Running, EgressCheck::NotRun)
            .blocked()
            .unwrap();
        assert!(not_run.contains("egress"), "{not_run}");
        let failed = record(
            WorkspaceState::Running,
            EgressCheck::Failed {
                at: Timestamp(1),
                reached: vec!["10.100.2.1:22".to_owned()],
            },
        )
        .blocked()
        .unwrap();
        assert!(failed.contains("10.100.2.1:22"), "{failed}");
    }
}
