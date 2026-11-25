// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A-10's manifest for a folder workspace: `<data>/workspaces/<id>.json`,
//! with the same fields as a container's instance keys.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use maxhq_core::fs::{write_atomic, Atomic};
use maxhq_protocol::ids::{DeviceId, ProjectId, WorkspaceId};
use serde::{Deserialize, Serialize};

use super::keys::SCHEMA;
use crate::session::holders::AgentSummary;

/// The folder under the data folder that holds the manifests.
pub const DIR: &str = "workspaces";

/// What a folder workspace's manifest holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The schema, as in `user.maxhq.schema`.
    pub schema: String,
    /// The project, when the folder has one.
    pub project: Option<ProjectId>,
    /// The device.
    pub device: DeviceId,
    /// The workspace.
    pub workspace: WorkspaceId,
    /// The project's name.
    pub title: Option<String>,
    /// The repo's remote.
    pub repo: Option<String>,
    /// The folder.
    pub path: String,
    /// The agents that run there.
    pub agents: Vec<AgentSummary>,
}

impl Manifest {
    /// A manifest in this build's schema.
    pub fn new(
        device: DeviceId,
        workspace: WorkspaceId,
        path: String,
        agents: Vec<AgentSummary>,
    ) -> Self {
        Self {
            schema: SCHEMA.to_owned(),
            project: None,
            device,
            workspace,
            title: None,
            repo: None,
            path,
            agents,
        }
    }
}

/// Where a workspace's manifest lives.
pub fn path(data: &Path, workspace: WorkspaceId) -> PathBuf {
    data.join(DIR).join(format!("{workspace}.json"))
}

/// Writes a manifest whole, so a reader sees the old one or the new one.
/// The folder is 0700 and the file 0600.
pub fn write(data: &Path, manifest: &Manifest) -> io::Result<()> {
    let dir = data.join(DIR);
    maxhq_os::fsmode::private_dir_all(&dir)?;
    let target = path(data, manifest.workspace);
    let mut bytes = serde_json::to_vec_pretty(manifest).map_err(io::Error::other)?;
    bytes.push(b'\n');
    write_atomic(&target, &bytes, Atomic::PRIVATE)
}

/// Reads a manifest, or None when there's none.
#[cfg_attr(not(test), allow(dead_code))]
pub fn read(data: &Path, workspace: WorkspaceId) -> io::Result<Option<Manifest>> {
    match fs::read(path(data, workspace)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::ids::{AccountId, AgentId};
    use maxhq_protocol::model::Vendor;

    use super::*;
    use crate::testing::TempDir;

    #[test]
    fn a_manifest_round_trips_and_stays_private() {
        let dir = TempDir::new();
        let workspace = WorkspaceId::derive("w");
        assert_eq!(read(dir.path(), workspace).unwrap(), None);

        let mut manifest = Manifest::new(
            DeviceId::derive("mac"),
            workspace,
            "/Users/max/Code/maxhq".to_owned(),
            vec![AgentSummary {
                agent: AgentId::derive("a"),
                vendor: Vendor::Codex,
                account: AccountId::derive("acc"),
                session: "019a".to_owned(),
                branch: "main".to_owned(),
                worktree: "/Users/max/Code/maxhq".to_owned(),
            }],
        );
        manifest.project = Some(ProjectId::derive("p"));
        manifest.title = Some("maxhq".to_owned());
        write(dir.path(), &manifest).unwrap();
        assert_eq!(read(dir.path(), workspace).unwrap(), Some(manifest.clone()));

        manifest.agents.clear();
        write(dir.path(), &manifest).unwrap();
        assert_eq!(read(dir.path(), workspace).unwrap(), Some(manifest));

        let mode = |p: &Path| maxhq_os::fsmode::mode_of(&fs::metadata(p).unwrap()) & 0o777;
        assert_eq!(mode(&dir.path().join(DIR)), 0o700);
        assert_eq!(mode(&path(dir.path(), workspace)), 0o600);
        let names: Vec<_> = fs::read_dir(dir.path().join(DIR)).unwrap().collect();
        assert_eq!(names.len(), 1, "no temp file is left behind");
    }
}
