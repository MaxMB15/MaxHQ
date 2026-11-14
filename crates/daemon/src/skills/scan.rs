// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The skills each CLI sees in a place on this device, from core's scan
//! over the disk.

use std::path::{Path, PathBuf};

use maxhq_core::git;
use maxhq_core::skills::always;
use maxhq_core::skills::disk::DiskFs;
use maxhq_core::skills::scan::{scan, ScanInput};
use maxhq_protocol::message::{ActionOutcome, AgentPlace};
use maxhq_protocol::model::WorkspaceLocation;
use maxhq_protocol::state::{Entity, EntityKey};

use crate::hub::Hub;

/// The folder an agent in `place` runs in, when it's a folder on this
/// device.
fn folder(hub: &Hub, place: &AgentPlace) -> Result<PathBuf, String> {
    let path = match place {
        AgentPlace::Folder { path } => PathBuf::from(path),
        AgentPlace::Workspace { workspace } => {
            let Some(Entity::Workspace(found)) = hub.get(&EntityKey::Workspace(*workspace)) else {
                return Err("That workspace doesn't exist.".into());
            };
            if found.device != hub.device().id {
                return Err("That workspace is on another device.".into());
            }
            match found.location {
                WorkspaceLocation::Folder { path } => PathBuf::from(path),
                WorkspaceLocation::Container { .. } => {
                    return Err("Skills in a container's workspace can't be listed yet.".into());
                }
            }
        }
    };
    if !path.is_absolute() || !path.is_dir() {
        return Err("That folder doesn't exist.".into());
    }
    Ok(path)
}

/// What a scan of `cwd` reads, with home `home`.
pub fn input(cwd: &Path, home: &Path) -> ScanInput {
    let mut input = ScanInput::new(cwd, home);
    input.repo_root = git::find_checkout(cwd).map(|checkout| checkout.root);
    input.main_checkout = input.repo_root.as_deref().and_then(git::main_checkout);
    input.always = always::NAMES
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    input.store = super::globals::store()
        .as_deref()
        .and_then(super::globals::store_root);
    input
}

/// Answers [`ClientMessage::SkillsScan`]. It reads the disk, so it runs
/// off the loop.
///
/// [`ClientMessage::SkillsScan`]: maxhq_protocol::message::ClientMessage::SkillsScan
pub fn request(hub: &Hub, place: &AgentPlace) -> ActionOutcome {
    let Some(home) = std::env::home_dir() else {
        return ActionOutcome::Refused {
            reason: "maxd has no home folder, so it can't find global skills.".into(),
        };
    };
    match folder(hub, place) {
        Ok(cwd) => ActionOutcome::Skills {
            skills: scan(&DiskFs, &input(&cwd, &home)).skills,
        },
        Err(reason) => ActionOutcome::Refused { reason },
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use maxhq_protocol::model::Vendor;
    use maxhq_protocol::skills::SkillPlace;

    use super::*;
    use crate::testing::TempDir;

    #[test]
    fn a_worktree_scan_finds_the_main_checkouts_skills() {
        let dir = TempDir::new();
        let main = dir.path().join("main");
        fs::create_dir_all(main.join(".git/worktrees/wt")).unwrap();
        fs::write(main.join(".git/worktrees/wt/commondir"), "../..\n").unwrap();
        let skill = main.join(".claude/skills/release");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: release\ndescription: Ships it.\n---\n",
        )
        .unwrap();
        let wt = dir.path().join("wt");
        fs::create_dir_all(&wt).unwrap();
        fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", main.join(".git/worktrees/wt").display()),
        )
        .unwrap();
        let home = dir.path().join("home");
        fs::create_dir_all(&home).unwrap();

        let input = input(&wt, &home);
        assert_eq!(input.repo_root.as_deref(), Some(wt.as_path()));
        assert_eq!(input.main_checkout, Some(fs::canonicalize(&main).unwrap()));
        assert_eq!(input.always, always::NAMES);
        let found: Vec<_> = scan(&DiskFs, &input)
            .skills
            .into_iter()
            .filter(|s| s.cli == Vendor::Claude && s.name == "release")
            .collect();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].place, SkillPlace::Project);
    }
}
