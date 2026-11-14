// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The work of each step, once you approved it and the order allows it.
//! Each takes the record as it stood when the step started and gives back
//! the record it leaves, or why it failed.

use std::collections::BTreeMap;
use std::path::Path;

use futures::future::BoxFuture;
use maxhq_protocol::hosts::{MoveState, MoveStep};
use maxhq_protocol::model::Vendor;
use maxhq_protocol::time::Timestamp;

use super::plan::{self, CHECKLIST};
use super::{Mover, Record, Resume, KEEP};
use crate::incus::moves::{CopyTo, AUTOSTART, MOVE_SNAPSHOT, OLD_REPO};
use crate::incus::InstanceName;
use crate::observe::incus::look::Look;
use crate::workspace::WORKSPACE_KEY;

fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

impl Mover {
    fn look(&self) -> Look {
        Look::new(self.config().incus.socket())
    }
}

/// Whether the old scripts left a resume flag for `source`. A folder that
/// can't be read counts as a flag, so the switch waits.
pub fn resume_flag(dir: &Path, source: &str) -> bool {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        path.file_name().is_some_and(|name| name == source)
            || path.file_stem().is_some_and(|stem| stem == source)
    })
}

/// The versions of `paths` in `source` now.
pub async fn versions<'a>(
    look: &mut Look,
    source: &str,
    paths: impl IntoIterator<Item = &'a String>,
) -> Result<BTreeMap<String, Option<String>>, String> {
    let mut found = BTreeMap::new();
    for path in paths {
        found.insert(path.clone(), look.version(source, path).await?);
    }
    Ok(found)
}

/// The `origin` remote's URL in a git config.
pub fn origin(config: &str) -> Option<String> {
    let mut in_origin = false;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            in_origin = line == r#"[remote "origin"]"#;
        } else if in_origin
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "url"
        {
            return Some(value.trim().to_owned());
        }
    }
    None
}

/// Reads the original and writes the plan. Nothing in it changes.
pub fn look(mover: Mover, mut record: Record) -> BoxFuture<'static, Result<Record, String>> {
    Box::pin(async move {
        let source = record.source.clone();
        let mut look = mover.look();
        let Some(original) = look.original(&source).await? else {
            return Err(format!(
                "There's no container named {source} on this device."
            ));
        };
        let session = look.session(&source).await.unwrap_or_default();
        let agent_state = look.agent_state(&source).await.unwrap_or_default();
        let found = &mut record.found;
        found.vendor = session
            .as_ref()
            .and_then(|found| match found.agent.as_str() {
                "claude" => Some(Vendor::Claude),
                "codex" => Some(Vendor::Codex),
                _ => None,
            });
        // Anything running in the original can write its session file.
        found.session = session
            .as_ref()
            .map(|found| found.session.clone())
            .filter(|session| plan::plain_session(session));
        found.cwd = session
            .as_ref()
            .map(|found| found.cwd.clone())
            .filter(|cwd| plan::plain_path(cwd));
        found.title = found
            .cwd
            .as_deref()
            .and_then(|cwd| cwd.rsplit('/').next())
            .filter(|name| !name.is_empty())
            .map(str::to_owned);
        let cwd = found.cwd.clone();
        let remote = match cwd {
            Some(cwd) => look
                .read(&source, &format!("{cwd}/.git/config"))
                .await
                .ok()
                .flatten()
                .and_then(|text| origin(&String::from_utf8_lossy(&text))),
            None => None,
        };
        let found = &mut record.found;
        // A session in the home folder, below the repo's top or in a linked
        // checkout has no git config of its own there.
        found.repo = remote.or_else(|| original.key(OLD_REPO).map(str::to_owned));
        found.pool = original.pool().map(str::to_owned);
        found.autostart = original.key(AUTOSTART).unwrap_or_default().to_owned();
        let config = mover.config();
        let mut devices: Vec<String> = original.devices.keys().cloned().collect();
        devices.sort();
        let plan = plan::Look {
            source: &source,
            target: &record.target,
            found: &record.found,
            agent_state,
            profiles: original.profiles.clone(),
            devices,
            disk_used: original.disk_used,
            resume_flag: resume_flag(&config.resume_flags, &source),
            next_reboot: plan::next_reboot(&config.uptime, mover.now()),
            runs_with: record
                .found
                .vendor
                .map(|vendor| config.host.runs_with(vendor)),
        }
        .plan();
        record.plan = Some(plan);
        record.step = MoveStep::Copy;
        record.state = MoveState::Ready;
        Ok(record)
    })
}

/// Snapshots the original as `mhq-move` and copies it, stopped, to the
/// `mhq-` name. A copy made before goes first, with its snapshot.
pub fn copy(mover: Mover, mut record: Record) -> BoxFuture<'static, Result<Record, String>> {
    Box::pin(async move {
        let config = mover.config();
        let incus = &config.incus;
        let source = record.source.clone();
        let pool = record
            .found
            .pool
            .clone()
            .ok_or_else(|| format!("{source} has no root disk on a pool to copy."))?;
        let name = InstanceName::new(&record.target).map_err(text)?;
        // A copy again deletes the copy made before, so only that one goes.
        if let Some(there) = incus.instance(&name).await.map_err(text)? {
            let theirs = there.config.get(WORKSPACE_KEY).cloned();
            if theirs.is_none() || theirs != record.workspace.map(|id| id.to_string()) {
                return Err(format!(
                    "{name} is there already and isn't this move's copy, so it stays. \
                     Delete or rename it, then copy again."
                ));
            }
        }
        config.host.drop_copy(&record).await?;
        // What the card listed went with that copy.
        record.unsaved.clear();
        incus
            .drop_snapshot_for_move(&source, MOVE_SNAPSHOT, &record)
            .await
            .map_err(text)?;
        // Taken before the snapshot, so a change in between shows as one.
        let mut look = mover.look();
        let marks = plan::marks(&record.found);
        record.marks = versions(&mut look, &source, &marks).await?;
        incus
            .snapshot_for_move(&source, MOVE_SNAPSHOT, &record)
            .await
            .map_err(text)?;
        let setup = config.host.copy_setup(&record).await?;
        // Kept before the copy starts, so a copy that fails part way can be
        // told from someone else's when it's tried again.
        record.workspace = Some(setup.workspace);
        mover.save(record.clone());
        let to = CopyTo {
            name: &name,
            config: setup.config,
            pool: &pool,
            network: &setup.network,
            guest_socket: &setup.guest_socket,
        };
        incus
            .copy_for_move(&source, MOVE_SNAPSHOT, to, &record)
            .await
            .map_err(text)?;
        record.source_changed = false;
        record.copy_used = false;
        record.step = MoveStep::Converge;
        record.state = MoveState::Ready;
        Ok(record)
    })
}

/// Sets the copy up the way MaxHQ runs its own. Only the copy changes.
pub fn converge(mover: Mover, mut record: Record) -> BoxFuture<'static, Result<Record, String>> {
    Box::pin(async move {
        mover.config().host.converge(&record).await?;
        record.step = MoveStep::Switch;
        record.state = MoveState::Ready;
        Ok(record)
    })
}

/// Stops the original, keeps it from starting at boot, and resumes its
/// agent in the copy. The keep starts.
pub fn switch(mover: Mover, mut record: Record) -> BoxFuture<'static, Result<Record, String>> {
    Box::pin(async move {
        let config = mover.config();
        let workspace = record
            .workspace
            .ok_or_else(|| "The copy has no workspace yet. Copy again.".to_owned())?;
        config
            .incus
            .stop_for_move(&record.source, config.state_timeout, &record)
            .await
            .map_err(text)?;
        // The copy is the original as it stopped, so its unsaved work is
        // the original's until the agent runs on in the copy.
        record.original_unsaved = config.host.unsaved(workspace).await;
        if let (Some(vendor), Some(session), Some(cwd)) = (
            record.found.vendor,
            record.found.session.clone(),
            plan::copy_path(&record.found),
        ) {
            config
                .host
                .resume(Resume {
                    workspace,
                    vendor,
                    session,
                    cwd,
                })
                .await?;
        }
        record.copy_used = true;
        record.step = MoveStep::Keep;
        record.state = MoveState::Ready;
        record.kept_until = Some(later(mover.now()));
        Ok(record)
    })
}

/// Stops the copy's agent, notes its unsaved work, and starts the original
/// with its `boot.autostart` as it was.
pub fn move_back(mover: Mover, mut record: Record) -> BoxFuture<'static, Result<Record, String>> {
    Box::pin(async move {
        let config = mover.config();
        if let Some(workspace) = record.workspace {
            config.host.stop_agents(workspace).await?;
            record.unsaved = config.host.unsaved(workspace).await;
        }
        config
            .incus
            .start_for_move(
                &record.source,
                &record.found.autostart,
                config.state_timeout,
                &record,
            )
            .await
            .map_err(text)?;
        record.state = MoveState::MovedBack;
        record.kept_until = None;
        Ok(record)
    })
}

/// Deletes the original and its `mhq-move` snapshot. After the last one,
/// the move lists what's left to do by hand.
pub fn retire(mover: Mover, mut record: Record) -> BoxFuture<'static, Result<Record, String>> {
    Box::pin(async move {
        let incus = &mover.config().incus;
        let source = record.source.clone();
        incus
            .drop_snapshot_for_move(&source, MOVE_SNAPSHOT, &record)
            .await
            .map_err(text)?;
        incus
            .delete_for_move(&source, &record)
            .await
            .map_err(text)?;
        record.step = MoveStep::Retire;
        record.state = MoveState::Retired;
        record.kept_until = None;
        if mover
            .look()
            .old_names()
            .await
            .is_ok_and(|left| left.is_empty())
        {
            record.checklist = CHECKLIST.iter().map(|line| (*line).to_owned()).collect();
        }
        Ok(record)
    })
}

fn later(now: Timestamp) -> Timestamp {
    let keep = u64::try_from(KEEP.as_millis()).unwrap_or(u64::MAX);
    Timestamp(now.0.saturating_add(keep))
}
