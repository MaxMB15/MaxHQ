// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What a workspace move reads of a container the old scripts made. It
//! goes through the observer's client, so every call is a GET or a HEAD
//! and nothing here can change the container.

use std::collections::HashMap;
use std::path::Path;

use super::api::Client;
use super::report::{SessionFile, StatusFile};

pub(crate) use super::{SESSION_PATH, STATUS_PATH};

/// A container as a move reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Original {
    /// Such as `Running` or `Stopped`.
    pub status: String,
    /// Its keys merged over its profiles'.
    pub config: HashMap<String, String>,
    /// The profiles it takes keys and devices from.
    pub profiles: Vec<String>,
    /// Its devices merged over its profiles', by name.
    pub devices: HashMap<String, HashMap<String, String>>,
    /// The disk its root uses, in bytes, when Incus reports it.
    pub disk_used: Option<u64>,
}

impl Original {
    /// A key's value, unless it's unset or empty.
    pub fn key(&self, key: &str) -> Option<&str> {
        self.config
            .get(key)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    }

    /// The pool its root disk is on.
    pub fn pool(&self) -> Option<&str> {
        self.devices
            .get("root")
            .and_then(|root| root.get("pool"))
            .map(String::as_str)
    }
}

/// What the old hook's session file says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Session {
    /// `claude` or `codex`.
    pub agent: String,
    /// The CLI's session or thread ID.
    pub session: String,
    /// The folder the CLI runs in.
    pub cwd: String,
}

/// A read-only line to Incus for a move.
pub struct Look {
    client: Client,
}

impl Look {
    /// Reads through the Incus socket at `socket`.
    pub fn new(socket: &Path) -> Self {
        Self {
            client: Client::new(socket),
        }
    }

    /// The container, or None when there's none by that name.
    pub async fn original(&mut self, name: &str) -> Result<Option<Original>, String> {
        let Some(found) = self
            .client
            .instance(name)
            .await
            .map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };
        let disk_used = self.client.disk_used(name).await.unwrap_or(None);
        Ok(Some(Original {
            status: found.status,
            config: found.expanded_config,
            profiles: found.profiles,
            devices: found.expanded_devices,
            disk_used,
        }))
    }

    /// The containers the old scripts made that are still there.
    pub async fn old_names(&mut self) -> Result<Vec<String>, String> {
        let all = self
            .client
            .instances()
            .await
            .map_err(|error| error.to_string())?;
        Ok(all
            .into_iter()
            .map(|instance| instance.name)
            .filter(|name| crate::incus::moves::is_old_name(name))
            .collect())
    }

    /// The names of a container's snapshots, none when it's not there.
    pub async fn snapshots(&mut self, name: &str) -> Result<Vec<String>, String> {
        self.client
            .snapshot_names(name)
            .await
            .map(Option::unwrap_or_default)
            .map_err(|error| error.to_string())
    }

    /// A file's version, compared as it is, or None when it's not there.
    pub async fn version(&mut self, name: &str, path: &str) -> Result<Option<String>, String> {
        self.client
            .version(name, path)
            .await
            .map(|found| found.map(|version| version.tag))
            .map_err(|error| error.to_string())
    }

    /// A file's contents, or None when it's not there.
    pub async fn read(&mut self, name: &str, path: &str) -> Result<Option<Vec<u8>>, String> {
        self.client
            .read(name, path)
            .await
            .map(|found| found.map(|file| file.body.to_vec()))
            .map_err(|error| error.to_string())
    }

    /// The old hook's session file, when it has a session.
    pub async fn session(&mut self, name: &str) -> Result<Option<Session>, String> {
        let Some(bytes) = self.read(name, SESSION_PATH).await? else {
            return Ok(None);
        };
        let file: SessionFile = serde_json::from_slice(&bytes)
            .map_err(|error| format!("its session file doesn't read: {error}"))?;
        Ok((!file.session_id.is_empty()).then_some(Session {
            agent: file.agent,
            session: file.session_id,
            cwd: file.cwd,
        }))
    }

    /// What the old hook's status file says the agent is doing, such as
    /// `working` or `idle`, or None without one.
    pub async fn agent_state(&mut self, name: &str) -> Result<Option<String>, String> {
        let Some(bytes) = self.read(name, STATUS_PATH).await? else {
            return Ok(None);
        };
        let file: StatusFile = serde_json::from_slice(&bytes).unwrap_or_default();
        Ok(Some(file.status).filter(|status| !status.is_empty()))
    }
}
