// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The IDs of things `maxd` finds rather than creates. Each one is derived
//! from what identifies the thing, so every device and every run of `maxd`
//! gives it the same ID. Change a name here and every ID derived from it
//! changes, which splits a project's history.

use maxhq_protocol::ids::{AccountId, AgentId, DeviceId, ProjectId, WorkspaceId};
use maxhq_protocol::model::{Vendor, WorkspaceLocation};

use crate::git::normalize_remote;

/// A project and the name the sidebar shows for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectName {
    /// The derived ID.
    pub id: ProjectId,
    /// The repository's name, or the folder's when there's no remote.
    pub name: String,
}

/// The project for a checkout. Checkouts of one remote share a project on
/// every device. A checkout with no usable remote is a project of its own,
/// named by device and folder.
pub fn project(device: DeviceId, root: &str, remote: Option<&str>) -> ProjectName {
    if let Some(remote) = remote.and_then(normalize_remote) {
        let name = remote.rsplit('/').next().unwrap_or(&remote).to_owned();
        return ProjectName {
            id: ProjectId::derive(&format!("git:{remote}")),
            name,
        };
    }
    let name = root
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|n| !n.is_empty())
        .unwrap_or(root)
        .to_owned();
    ProjectName {
        id: ProjectId::derive(&format!("folder:{device}:{root}")),
        name,
    }
}

/// A folder or container on a device.
pub fn workspace(device: DeviceId, location: &WorkspaceLocation) -> WorkspaceId {
    match location {
        WorkspaceLocation::Folder { path } => {
            WorkspaceId::derive(&format!("folder:{device}:{path}"))
        }
        WorkspaceLocation::Container { instance, path } => {
            WorkspaceId::derive(&format!("container:{device}:{instance}:{path}"))
        }
    }
}

/// An agent session, from the CLI's own session or thread ID. Those are
/// UUIDs already, so the device isn't part of the name, and a resume keeps
/// the agent.
pub fn agent(vendor: Vendor, vendor_session_id: &str) -> AgentId {
    AgentId::derive(&format!("{}:{vendor_session_id}", vendor_name(vendor)))
}

/// A sub-agent, from its parent and the ID the CLI gives it within the
/// parent's session.
pub fn subagent(parent: AgentId, sub_id: &str) -> AgentId {
    AgentId::derive(&format!("sub:{parent}:{sub_id}"))
}

/// A login, from the config folder that holds it.
pub fn account(device: DeviceId, vendor: Vendor, config_dir: &str) -> AccountId {
    AccountId::derive(&format!("{}:{device}:{config_dir}", vendor_name(vendor)))
}

fn vendor_name(vendor: Vendor) -> &'static str {
    match vendor {
        Vendor::Claude => "claude",
        Vendor::Codex => "codex",
        Vendor::Cursor => "cursor",
        Vendor::Acp => "acp",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mac() -> DeviceId {
        DeviceId(uuid::Uuid::from_u128(1))
    }

    fn node() -> DeviceId {
        DeviceId(uuid::Uuid::from_u128(2))
    }

    #[test]
    fn one_remote_is_one_project_on_every_device() {
        let here = project(
            mac(),
            "/Users/max/MaxHQ",
            Some("git@github.com:Example-User/MaxHQ.git"),
        );
        let there = project(
            node(),
            "/work/maxhq",
            Some("https://github.com/Example-User/MaxHQ"),
        );
        assert_eq!(here, there);
        assert_eq!(here.name, "maxhq");
    }

    #[test]
    fn folders_without_a_remote_stay_apart() {
        let a = project(mac(), "/Users/max/scratch/", None);
        let b = project(node(), "/Users/max/scratch/", None);
        assert_ne!(a.id, b.id);
        assert_eq!(a.name, "scratch");
    }

    #[test]
    fn ids_differ_by_vendor_and_location() {
        assert_ne!(agent(Vendor::Claude, "s1"), agent(Vendor::Codex, "s1"));
        assert_ne!(agent(Vendor::Cursor, "s1"), agent(Vendor::Codex, "s1"));
        let folder = WorkspaceLocation::Folder { path: "/w".into() };
        let container = WorkspaceLocation::Container {
            instance: "ws".into(),
            path: "/w".into(),
        };
        assert_ne!(workspace(mac(), &folder), workspace(mac(), &container));
        assert_ne!(workspace(mac(), &folder), workspace(node(), &folder));
    }
}
