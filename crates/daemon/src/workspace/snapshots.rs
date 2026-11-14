// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! W-10's snapshots. Incus takes and expires them on the schedule the caps
//! keys set, and maxd lists them. A restore makes a new workspace from one
//! by default and leaves the original alone. A roll back takes the
//! workspace itself back, after a named snapshot of where it stands.

use maxhq_core::names;
use maxhq_protocol::hosts::WorkspaceSnapshot;
use maxhq_protocol::ids::WorkspaceId;
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{
    EgressCheck, GuestState, UnsavedSummary, WorkspaceLocation, WorkspaceState,
};
use maxhq_protocol::time::Timestamp;
use tracing::{info, warn};

use super::create::NAME_MAX;
use super::keys::{Keys, Record};
use super::lifecycle::{busy, UNKNOWN};
use super::manager::{refused, Manager};
use crate::incus::{InstanceName, Snapshot, NEVER, PREFIX};

/// How a copy's name ends: `-restored`, then `-restored-2` and on.
const RESTORED: &str = "-restored";

/// How many names a restore tries before it gives up.
const TRIES: u32 = 50;

/// The steps a restore as new shows.
const RESTORE_STEPS: u32 = 2;

impl Manager {
    /// Answers `ListSnapshots`, oldest first.
    pub(super) async fn list_snapshots(&self, workspace: WorkspaceId) -> ActionOutcome {
        let name = match self.instance_of(workspace) {
            Ok((_, name)) => name,
            Err(outcome) => return outcome,
        };
        match self.incus().snapshots(&name).await {
            Ok(found) => ActionOutcome::Snapshots {
                snapshots: listed(found),
            },
            Err(error) => refused(format!("Incus couldn't list {name}'s snapshots: {error}")),
        }
    }

    /// Answers `RestoreSnapshot`. With `as_new` the snapshot becomes a new
    /// workspace and nothing existing changes. Without it the workspace
    /// itself goes back, which refuses while it has unsaved work unless
    /// `discard_unsaved` says to lose it.
    pub(super) async fn restore_snapshot(
        &self,
        workspace: WorkspaceId,
        snapshot: String,
        as_new: bool,
        discard_unsaved: bool,
    ) -> ActionOutcome {
        let (record, name) = match self.instance_of(workspace) {
            Ok(found) => found,
            Err(outcome) => return outcome,
        };
        match self.incus().snapshots(&name).await {
            Ok(found) if found.iter().any(|s| s.name == snapshot) => {}
            Ok(_) => return refused(format!("{name} has no snapshot called {snapshot}.")),
            Err(error) => {
                return refused(format!("Incus couldn't list {name}'s snapshots: {error}"));
            }
        }
        if as_new {
            self.restore_as_new(record, name, snapshot).await
        } else {
            self.roll_back(record, name, snapshot, discard_unsaved)
        }
    }

    /// A workspace's record and instance name.
    // The refusal goes straight back to the client, once per request.
    #[allow(clippy::result_large_err)]
    fn instance_of(&self, workspace: WorkspaceId) -> Result<(Record, InstanceName), ActionOutcome> {
        let record = self.record(workspace).ok_or_else(|| refused(UNKNOWN))?;
        let name = InstanceName::new(&record.instance).map_err(|_| refused(UNKNOWN))?;
        Ok((record, name))
    }

    /// Copies the snapshot to a new `mhq-` name with keys of its own, and
    /// starts it. The original isn't changed, and holds still until the
    /// copy has its own keys, so the rebuild never sees two instances
    /// that say they're one workspace.
    async fn restore_as_new(
        &self,
        record: Record,
        from: InstanceName,
        snapshot: String,
    ) -> ActionOutcome {
        if let Err(reason) = self.config().caps.check_parent() {
            return refused(reason);
        }
        let Some(original) = self.begin(record.keys.workspace) else {
            return refused(busy(&record.instance));
        };
        let to = match self.free_name(&from).await {
            Ok(to) => to,
            Err(reason) => return refused(reason),
        };
        let workspace = names::workspace(
            self.inner.device,
            &WorkspaceLocation::Container {
                instance: to.to_string(),
                path: record.repo_dir(),
            },
        );
        let Some(op) = self.begin(workspace) else {
            return refused(busy(to.as_str()));
        };
        let keys = Keys {
            workspace,
            agents: Vec::new(),
            created: Timestamp::now(),
            ..record.keys.clone()
        };
        self.put(Record {
            instance: to.to_string(),
            keys: keys.clone(),
            state: WorkspaceState::Creating {
                step: format!("Copying {snapshot} from {from}"),
                done: 0,
                of: RESTORE_STEPS,
            },
            guest: GuestState::Installing,
            egress: EgressCheck::NotRun,
            egress_note: None,
            pool: record.pool.clone(),
            network: record.network.clone(),
            host_boot: Some(self.inner.host_boot.clone()),
            ssh_host_key: None,
            memory_soft: record.memory_soft,
            unsaved: None,
        });
        info!(%from, %snapshot, %to, "restoring a snapshot as a new workspace");
        let manager = self.clone();
        tokio::spawn(async move {
            let copied = manager.copy_snapshot(&from, &snapshot, &to, &keys).await;
            drop(original);
            match copied {
                Ok(()) => {
                    manager.set_state(
                        workspace,
                        WorkspaceState::Creating {
                            step: "Starting it".to_owned(),
                            done: 1,
                            of: RESTORE_STEPS,
                        },
                    );
                    if let Err(trouble) = manager.bring_up(workspace, true).await {
                        warn!(%workspace, ?trouble, "a restored workspace didn't start");
                    }
                }
                Err(reason) => manager.set_state(
                    workspace,
                    WorkspaceState::Failed {
                        reason: format!("Restoring {snapshot} failed: {reason}"),
                    },
                ),
            }
            drop(op);
        });
        ActionOutcome::Created { workspace }
    }

    /// The copy and its own keys and guest socket. A copy that doesn't get
    /// them goes again, since it would say it's the original.
    async fn copy_snapshot(
        &self,
        from: &InstanceName,
        snapshot: &str,
        to: &InstanceName,
        keys: &Keys,
    ) -> Result<(), String> {
        let incus = self.incus();
        incus
            .copy(from, Some(snapshot), to)
            .await
            .map_err(|error| error.to_string())?;
        let made = async {
            incus
                .set_config(to, keys.encode())
                .await
                .map_err(|error| error.to_string())?;
            self.ensure_guests_dir()
                .map_err(|error| format!("maxd couldn't make its socket folder: {error}"))?;
            incus
                .set_guest_socket(to, &self.socket(to))
                .await
                .map_err(|error| error.to_string())
        }
        .await;
        if made.is_err()
            && let Err(error) = incus.delete(to).await
        {
            warn!(instance = %to, %error, "couldn't delete a copy that didn't get its keys");
        }
        made
    }

    /// A name for a copy of `from` that's free on this device.
    async fn free_name(&self, from: &InstanceName) -> Result<InstanceName, String> {
        let base = from.as_str().strip_prefix(PREFIX).unwrap_or(from.as_str());
        for n in 1..=TRIES {
            let suffix = match n {
                1 => RESTORED.to_owned(),
                n => format!("{RESTORED}-{n}"),
            };
            let mut stem = base.to_owned();
            stem.truncate(NAME_MAX.saturating_sub(suffix.len()));
            let stem = stem.trim_end_matches('-');
            let Ok(name) = InstanceName::new(&format!("{PREFIX}{stem}{suffix}")) else {
                continue;
            };
            if self.find_instance(name.as_str()).is_some() {
                continue;
            }
            match self.incus().instance(&name).await {
                Ok(None) => return Ok(name),
                Ok(Some(_)) => continue,
                Err(error) => return Err(format!("Incus doesn't answer on this device: {error}")),
            }
        }
        Err(format!(
            "{from} has too many restored copies. Delete some first."
        ))
    }

    /// Stops the workspace, takes a named snapshot that never expires of
    /// where it stands, takes it back to `snapshot` and starts it.
    fn roll_back(
        &self,
        record: Record,
        name: InstanceName,
        snapshot: String,
        discard_unsaved: bool,
    ) -> ActionOutcome {
        let workspace = record.keys.workspace;
        if !self.busy_agents(&record).is_empty() {
            return refused(format!(
                "{name} has agents at work. Stop them first, or restore the snapshot as a new \
                 workspace."
            ));
        }
        if !discard_unsaved && let Some(reason) = unsaved(&record) {
            return refused(reason);
        }
        // Checked before anything stops, since the start after needs it.
        if let Err(reason) = self.config().caps.check_parent() {
            return refused(reason);
        }
        let Some(op) = self.begin(workspace) else {
            return refused(busy(&record.instance));
        };
        self.set_state(workspace, WorkspaceState::Stopping);
        info!(instance = %name, %snapshot, "rolling a workspace back");
        let manager = self.clone();
        tokio::spawn(async move {
            match manager.take_back(workspace, &name, &snapshot).await {
                Ok(()) => {
                    if let Err(trouble) = manager.bring_up(workspace, true).await {
                        warn!(%workspace, ?trouble, "a rolled back workspace didn't start");
                    }
                }
                Err(reason) => manager.set_state(
                    workspace,
                    WorkspaceState::Failed {
                        reason: format!("Rolling back to {snapshot} failed: {reason}"),
                    },
                ),
            }
            drop(op);
        });
        ActionOutcome::Done
    }

    /// The roll back while the workspace is stopped. Its keys stay this
    /// device's, apart from the recipe version, which goes with the disk,
    /// so the start updates it in place again.
    async fn take_back(
        &self,
        workspace: WorkspaceId,
        name: &InstanceName,
        snapshot: &str,
    ) -> Result<(), String> {
        let incus = self.incus();
        self.halt(workspace, name)
            .await
            .map_err(|error| format!("stopping it failed: {error}"))?;
        self.mark_stopped(workspace);
        let before = before_name(Timestamp::now());
        incus
            .snapshot(name, &before)
            .await
            .map_err(|error| format!("the {before} snapshot failed: {error}"))?;
        incus
            .restore(name, snapshot)
            .await
            .map_err(|error| error.to_string())?;
        let restored = incus
            .instance(name)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("{name} is gone from Incus"))?;
        let Some(mut keys) = self.record(workspace).map(|record| record.keys) else {
            return Err("it's gone".to_owned());
        };
        if let Ok(then) = Keys::decode(&restored.config) {
            keys.recipe = then.recipe;
        }
        incus
            .set_config(name, keys.encode())
            .await
            .map_err(|error| error.to_string())?;
        self.update(workspace, |record| record.keys = keys);
        Ok(())
    }
}

/// Incus's snapshots as clients see them, oldest first.
fn listed(found: Vec<Snapshot>) -> Vec<WorkspaceSnapshot> {
    let mut listed: Vec<WorkspaceSnapshot> = found
        .into_iter()
        .map(|snapshot| WorkspaceSnapshot {
            taken_at: timestamp(&snapshot.created_at).unwrap_or(Timestamp(0)),
            expires_at: (snapshot.expires_at != NEVER)
                .then(|| timestamp(&snapshot.expires_at))
                .flatten(),
            name: snapshot.name,
        })
        .collect();
    listed.sort_by(|a, b| a.taken_at.cmp(&b.taken_at).then(a.name.cmp(&b.name)));
    listed
}

/// An RFC 3339 time from Incus. None for one that doesn't read or is
/// before 1970.
fn timestamp(text: &str) -> Option<Timestamp> {
    let at: jiff::Timestamp = text.parse().ok()?;
    u64::try_from(at.as_millisecond()).ok().map(Timestamp)
}

/// The name of the snapshot a roll back takes first, by the time in UTC.
fn before_name(now: Timestamp) -> String {
    let at = i64::try_from(now.0)
        .ok()
        .and_then(|ms| jiff::Timestamp::from_millisecond(ms).ok())
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH);
    format!("mhq-before-rollback-{}", at.strftime("%Y-%m-%d-%H%M%S"))
}

/// Why a roll back would lose work, or None when the guest's last report
/// says it wouldn't.
fn unsaved(record: &Record) -> Option<String> {
    let instance = &record.instance;
    let Some(summary) = record.unsaved else {
        return Some(format!(
            "maxd hasn't heard from {instance} about unsaved work yet, so a roll back could \
             lose some. Start it and wait a minute, or restore the snapshot as a new workspace."
        ));
    };
    let parts = unsaved_parts(&summary);
    if parts.is_empty() {
        return None;
    }
    Some(format!(
        "{instance} has {}. Rolling back loses it. Push it first, or restore the snapshot as a \
         new workspace.",
        parts.join(", ")
    ))
}

/// What a summary counts, in words.
pub(super) fn unsaved_parts(summary: &UnsavedSummary) -> Vec<String> {
    let count = |n: u32, one: &str, many: &str| match n {
        0 => None,
        1 => Some(format!("1 {one}")),
        n => Some(format!("{n} {many}")),
    };
    [
        count(
            summary.changed,
            "changed file not committed",
            "changed files not committed",
        ),
        count(summary.commits, "commit not pushed", "commits not pushed"),
        count(
            summary.no_upstream,
            "branch with no upstream",
            "branches with no upstream",
        ),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_list_oldest_first_and_a_kept_one_has_no_expiry() {
        let found = vec![
            Snapshot {
                name: "auto-2026-11-02-1400".to_owned(),
                created_at: "2026-11-02T14:00:00Z".to_owned(),
                expires_at: "2026-11-09T14:00:00Z".to_owned(),
            },
            Snapshot {
                name: "mhq-before-rollback-2026-11-01-090000".to_owned(),
                created_at: "2026-11-01T09:00:00Z".to_owned(),
                expires_at: NEVER.to_owned(),
            },
        ];
        let listed = listed(found);
        assert_eq!(listed[0].name, "mhq-before-rollback-2026-11-01-090000");
        assert_eq!(listed[0].expires_at, None);
        assert_eq!(listed[1].taken_at, Timestamp(1_793_628_000_000));
        assert_eq!(listed[1].expires_at, Some(Timestamp(1_794_232_800_000)));
    }

    #[test]
    fn the_before_snapshot_is_named_by_the_time() {
        assert_eq!(
            before_name(Timestamp(1_793_628_000_000)),
            "mhq-before-rollback-2026-11-02-140000"
        );
    }
}
