// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The one opening in the `mhq-` guard: the writes a workspace move makes
//! to a `ws-` container the old scripts made. Each takes the move's record
//! and runs only when the record names that container and you approved the
//! step the write belongs to.
//!
//! A move may snapshot the original as [`MOVE_SNAPSHOT`], copy that
//! snapshot to the record's `mhq-` name, stop it and set its
//! `boot.autostart` for the switch, start it and set the key back for a
//! move back, and delete the snapshot and the original for a retire.
//! Nothing here runs a command in it: an exec only ever takes an
//! [`InstanceName`].

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use hyper::Method;
use serde_json::json;

use super::{
    devices, http, Incus, IncusError, InstanceName, CHANGE_LIMIT, CREATE_LIMIT, MANAGED_KEY, NEVER,
};
use crate::moves::{Approval, Record};
use maxhq_protocol::hosts::MoveStep;

/// The only snapshot a move takes of an original.
pub const MOVE_SNAPSHOT: &str = "mhq-move";

/// What every container the old scripts made is named with.
pub const OLD_PREFIX: &str = "ws-";

/// The key that has Incus start a container when the host boots.
pub const AUTOSTART: &str = "boot.autostart";

/// The GitHub repo the old scripts named on the container they made.
pub const OLD_REPO: &str = "user.repo";

/// One write to an original, and the approval it needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Write {
    Snapshot,
    Copy,
    DropSnapshot,
    Stop,
    Start,
    Delete,
}

impl Write {
    /// Whether `approved` lets this write run.
    fn allowed(self, approved: Approval) -> bool {
        match self {
            Write::Snapshot | Write::Copy => approved == Approval::Step(MoveStep::Copy),
            Write::DropSnapshot => matches!(
                approved,
                Approval::Step(MoveStep::Copy) | Approval::Step(MoveStep::Retire)
            ),
            Write::Stop => approved == Approval::Step(MoveStep::Switch),
            Write::Start => approved == Approval::Back,
            Write::Delete => approved == Approval::Step(MoveStep::Retire),
        }
    }

    fn what(self) -> &'static str {
        match self {
            Write::Snapshot => "snapshot",
            Write::Copy => "copy",
            Write::DropSnapshot => "delete a snapshot of",
            Write::Stop => "stop",
            Write::Start => "start",
            Write::Delete => "delete",
        }
    }
}

/// True for a name the old scripts would give a container: `ws-`, then
/// a to z, 0 to 9 and `-`.
pub fn is_old_name(name: &str) -> bool {
    name.strip_prefix(OLD_PREFIX).is_some_and(|rest| {
        !rest.is_empty()
            && rest.len() <= 59
            && rest
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

/// Refuses a write to `source` unless the record names it and approved
/// the step the write belongs to.
fn guard(source: &str, record: &Record, write: Write) -> Result<(), IncusError> {
    let refuse = |why: &str| {
        Err(IncusError::Guarded {
            name: source.to_owned(),
            why: why.to_owned(),
        })
    };
    if !is_old_name(source) {
        return refuse("only a container the old scripts made can move");
    }
    if record.source != source {
        return refuse("its move is for another container");
    }
    if InstanceName::new(&record.target).is_err() {
        return refuse("its move copies to a name outside mhq-");
    }
    match record.approved {
        Some(approved) if write.allowed(approved) => Ok(()),
        _ => refuse(&format!(
            "you haven't approved the step that would {} it",
            write.what()
        )),
    }
}

/// Refuses any snapshot name but [`MOVE_SNAPSHOT`].
fn snapshot_name(source: &str, snapshot: &str) -> Result<(), IncusError> {
    if snapshot == MOVE_SNAPSHOT {
        return Ok(());
    }
    Err(IncusError::Guarded {
        name: source.to_owned(),
        why: format!("a move only takes the {MOVE_SNAPSHOT} snapshot"),
    })
}

/// An original's URI, with `rest` after it.
fn old_uri(source: &str, rest: &str) -> String {
    format!("/1.0/instances/{source}{rest}")
}

/// Where a copy goes and what it starts with.
pub struct CopyTo<'a> {
    /// The copy, which must be the record's target.
    pub name: &'a InstanceName,
    /// Its keys, on top of the original's. The managed mark is added.
    pub config: BTreeMap<String, String>,
    /// The pool its root disk is on.
    pub pool: &'a str,
    /// The network its NIC is on.
    pub network: &'a str,
    /// The host socket its guest proxy listens on.
    pub guest_socket: &'a Path,
}

impl Incus {
    /// Takes the [`MOVE_SNAPSHOT`] snapshot of an original, without its
    /// memory or a restart, that never expires.
    pub async fn snapshot_for_move(
        &self,
        source: &str,
        snapshot: &str,
        record: &Record,
    ) -> Result<(), IncusError> {
        guard(source, record, Write::Snapshot)?;
        snapshot_name(source, snapshot)?;
        let body = json!({ "name": snapshot, "stateful": false, "expires_at": NEVER });
        http::run(
            &self.socket,
            Method::POST,
            &old_uri(source, "/snapshots"),
            Some(&body),
            CHANGE_LIMIT,
        )
        .await?;
        Ok(())
    }

    /// Copies an original's [`MOVE_SNAPSHOT`] to a new stopped `mhq-`
    /// instance, with no profiles, MaxHQ's managed mark and its own root,
    /// NIC and guest proxy. Incus keeps any other device the original has
    /// itself, which the converge drops.
    pub async fn copy_for_move(
        &self,
        source: &str,
        snapshot: &str,
        to: CopyTo<'_>,
        record: &Record,
    ) -> Result<(), IncusError> {
        guard(source, record, Write::Copy)?;
        snapshot_name(source, snapshot)?;
        if to.name.as_str() != record.target {
            return Err(IncusError::Guarded {
                name: source.to_owned(),
                why: "its move copies to another name".to_owned(),
            });
        }
        if self.instance(to.name).await?.is_some() {
            return Err(IncusError::Api {
                status: 409,
                message: format!("{} is there already", to.name),
            });
        }
        let mut config = to.config;
        config.insert(MANAGED_KEY.to_owned(), "true".to_owned());
        let body = json!({
            "name": to.name.as_str(),
            "source": {
                "type": "copy",
                "source": format!("{source}/{snapshot}"),
                "instance_only": true,
            },
            "profiles": [],
            "config": config,
            "devices": devices(to.pool, to.network, to.guest_socket)?,
        });
        http::run(
            &self.socket,
            Method::POST,
            "/1.0/instances",
            Some(&body),
            CREATE_LIMIT,
        )
        .await?;
        Ok(())
    }

    /// Sets an original's `boot.autostart` to false and stops it, for the
    /// switch.
    pub async fn stop_for_move(
        &self,
        source: &str,
        timeout: Duration,
        record: &Record,
    ) -> Result<(), IncusError> {
        guard(source, record, Write::Stop)?;
        self.old_autostart(source, "false").await?;
        self.old_state(source, "stop", timeout).await
    }

    /// Sets an original's `boot.autostart` back to what it was, empty for
    /// unset, and starts it, for a move back.
    pub async fn start_for_move(
        &self,
        source: &str,
        autostart: &str,
        timeout: Duration,
        record: &Record,
    ) -> Result<(), IncusError> {
        guard(source, record, Write::Start)?;
        self.old_autostart(source, autostart).await?;
        self.old_state(source, "start", timeout).await
    }

    /// Deletes an original's [`MOVE_SNAPSHOT`], for a copy again or a
    /// retire. One that's gone already is fine.
    pub async fn drop_snapshot_for_move(
        &self,
        source: &str,
        snapshot: &str,
        record: &Record,
    ) -> Result<(), IncusError> {
        guard(source, record, Write::DropSnapshot)?;
        snapshot_name(source, snapshot)?;
        let rest = format!("/snapshots/{}", http::escape(snapshot));
        match http::run(
            &self.socket,
            Method::DELETE,
            &old_uri(source, &rest),
            None,
            CHANGE_LIMIT,
        )
        .await
        {
            Err(error) if error.is_not_found() => Ok(()),
            other => other.map(|_| ()),
        }
    }

    /// Deletes a stopped original, for a retire. One that's gone already
    /// is fine.
    pub async fn delete_for_move(&self, source: &str, record: &Record) -> Result<(), IncusError> {
        guard(source, record, Write::Delete)?;
        match http::run(
            &self.socket,
            Method::DELETE,
            &old_uri(source, ""),
            None,
            CHANGE_LIMIT,
        )
        .await
        {
            Err(error) if error.is_not_found() => Ok(()),
            other => other.map(|_| ()),
        }
    }

    /// Drops every profile and device a moved copy brought along, keeping
    /// its keys, and gives it MaxHQ's own root, NIC and guest proxy. The
    /// copy is MaxHQ's, so this is an ordinary managed write.
    pub async fn own_devices(
        &self,
        name: &InstanceName,
        pool: &str,
        network: &str,
        guest_socket: &Path,
    ) -> Result<(), IncusError> {
        let instance = self.managed(name).await?;
        let body = json!({
            "config": instance.config,
            "devices": devices(pool, network, guest_socket)?,
            "profiles": [],
            "ephemeral": false,
            "stateful": false,
        });
        http::run(
            &self.socket,
            Method::PUT,
            &super::instance_uri(name, ""),
            Some(&body),
            CHANGE_LIMIT,
        )
        .await?;
        Ok(())
    }

    async fn old_autostart(&self, source: &str, value: &str) -> Result<(), IncusError> {
        let body = json!({ "config": { AUTOSTART: value } });
        http::run(
            &self.socket,
            Method::PATCH,
            &old_uri(source, ""),
            Some(&body),
            CHANGE_LIMIT,
        )
        .await?;
        Ok(())
    }

    async fn old_state(
        &self,
        source: &str,
        action: &str,
        timeout: Duration,
    ) -> Result<(), IncusError> {
        // A stop of one that's stopped, or a start of one that runs, fails
        // in Incus, and a move back or a switch may meet either.
        let found = http::call(&self.socket, Method::GET, &old_uri(source, ""), None).await?;
        let status = found.metadata["status"].as_str().unwrap_or_default();
        if (action == "stop" && status == "Stopped") || (action == "start" && status == "Running") {
            return Ok(());
        }
        let seconds = timeout.as_secs().max(1);
        let body = json!({ "action": action, "timeout": seconds, "force": false });
        http::run(
            &self.socket,
            Method::PUT,
            &old_uri(source, "/state"),
            Some(&body),
            timeout + Duration::from_secs(30),
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::ids::{DeviceId, MoveId};

    use super::super::fake::{FakeIncus, State};
    use super::*;

    const WAIT: Duration = Duration::from_secs(5);

    fn record(source: &str, target: &str, approved: Option<Approval>) -> Record {
        let mut record = Record::new(
            MoveId::derive(source),
            DeviceId::derive("devbox"),
            source.into(),
            target.into(),
        );
        record.approved = approved;
        record
    }

    fn fake() -> FakeIncus {
        let mut state = State::default();
        for name in ["ws-alpha", "ws-bravo"] {
            state.add(name, "Running", &[(AUTOSTART, "true")]);
            state.instances.get_mut(name).unwrap()["devices"] =
                json!({ "root": { "type": "disk", "path": "/", "pool": "default" } });
        }
        FakeIncus::start(state)
    }

    /// Tries every write to ws-alpha under `record` and gives back the
    /// errors, one per write.
    async fn every_write(incus: &Incus, record: &Record, snapshot: &str) -> Vec<IncusError> {
        let dir = std::env::temp_dir();
        let target = InstanceName::new("mhq-alpha").unwrap();
        let to = CopyTo {
            name: &target,
            config: BTreeMap::new(),
            pool: "default",
            network: "agentbr0",
            guest_socket: &dir.join("mhq-alpha.sock"),
        };
        let source = "ws-alpha";
        [
            incus.snapshot_for_move(source, snapshot, record).await,
            incus.copy_for_move(source, snapshot, to, record).await,
            incus.stop_for_move(source, WAIT, record).await,
            incus.start_for_move(source, "true", WAIT, record).await,
            incus.drop_snapshot_for_move(source, snapshot, record).await,
            incus.delete_for_move(source, record).await,
        ]
        .into_iter()
        .filter_map(Result::err)
        .collect()
    }

    async fn refused_all(record: Record, snapshot: &str, why: &str) {
        let fake = fake();
        let incus = Incus::new(&fake.socket);
        let errors = every_write(&incus, &record, snapshot).await;
        assert_eq!(errors.len(), 6, "{errors:?}");
        for error in errors {
            match error {
                IncusError::Guarded { name, why: said } => {
                    assert_eq!(name, "ws-alpha");
                    assert!(said.contains(why), "{said}");
                }
                other => panic!("not the guard: {other:?}"),
            }
        }
        let state = fake.state();
        assert!(state.writes().is_empty(), "{:?}", state.writes());
        assert!(state.execs.is_empty());
        assert_eq!(state.instances["ws-alpha"]["status"], "Running");
    }

    #[tokio::test]
    async fn no_approval_writes_nothing() {
        refused_all(
            record("ws-alpha", "mhq-alpha", None),
            MOVE_SNAPSHOT,
            "approved",
        )
        .await;
    }

    #[tokio::test]
    async fn a_move_for_another_container_writes_nothing() {
        for step in [MoveStep::Copy, MoveStep::Switch, MoveStep::Retire] {
            let other = record("ws-bravo", "mhq-bravo", Some(Approval::Step(step)));
            refused_all(other, MOVE_SNAPSHOT, "another container").await;
        }
        let back = record("ws-bravo", "mhq-bravo", Some(Approval::Back));
        refused_all(back, MOVE_SNAPSHOT, "another container").await;
    }

    #[tokio::test]
    async fn a_step_not_yet_approved_writes_nothing() {
        for step in [MoveStep::Look, MoveStep::Converge, MoveStep::Keep] {
            let early = record("ws-alpha", "mhq-alpha", Some(Approval::Step(step)));
            refused_all(early, MOVE_SNAPSHOT, "approved").await;
        }
        // Each approval opens only its own writes.
        let fake = fake();
        let incus = Incus::new(&fake.socket);
        let copy = record(
            "ws-alpha",
            "mhq-alpha",
            Some(Approval::Step(MoveStep::Copy)),
        );
        assert!(incus.stop_for_move("ws-alpha", WAIT, &copy).await.is_err());
        assert!(incus.delete_for_move("ws-alpha", &copy).await.is_err());
        let switch = record(
            "ws-alpha",
            "mhq-alpha",
            Some(Approval::Step(MoveStep::Switch)),
        );
        assert!(incus
            .snapshot_for_move("ws-alpha", MOVE_SNAPSHOT, &switch)
            .await
            .is_err());
        assert!(incus.delete_for_move("ws-alpha", &switch).await.is_err());
        let back = record("ws-alpha", "mhq-alpha", Some(Approval::Back));
        assert!(incus.stop_for_move("ws-alpha", WAIT, &back).await.is_err());
        assert!(incus.delete_for_move("ws-alpha", &back).await.is_err());
        assert!(fake.state().writes().is_empty());
    }

    #[tokio::test]
    async fn a_target_outside_mhq_writes_nothing() {
        for target in ["ws-bravo", "alpha", "", "mhq-"] {
            for approved in [
                Approval::Step(MoveStep::Copy),
                Approval::Step(MoveStep::Switch),
                Approval::Step(MoveStep::Retire),
                Approval::Back,
            ] {
                let wrong = record("ws-alpha", target, Some(approved));
                refused_all(wrong, MOVE_SNAPSHOT, "outside mhq-").await;
            }
        }
    }

    #[tokio::test]
    async fn only_the_move_snapshot_is_taken_or_dropped() {
        for approved in [
            Approval::Step(MoveStep::Copy),
            Approval::Step(MoveStep::Retire),
        ] {
            let fake = fake();
            let incus = Incus::new(&fake.socket);
            let approved = record("ws-alpha", "mhq-alpha", Some(approved));
            for snapshot in ["daily", "mhq-move2", ""] {
                let taken = incus
                    .snapshot_for_move("ws-alpha", snapshot, &approved)
                    .await;
                assert!(
                    matches!(taken, Err(IncusError::Guarded { .. })),
                    "{taken:?}"
                );
                let to = CopyTo {
                    name: &InstanceName::new("mhq-alpha").unwrap(),
                    config: BTreeMap::new(),
                    pool: "default",
                    network: "agentbr0",
                    guest_socket: &std::env::temp_dir().join("mhq-alpha.sock"),
                };
                let copied = incus
                    .copy_for_move("ws-alpha", snapshot, to, &approved)
                    .await;
                assert!(
                    matches!(copied, Err(IncusError::Guarded { .. })),
                    "{copied:?}"
                );
                let dropped = incus
                    .drop_snapshot_for_move("ws-alpha", snapshot, &approved)
                    .await;
                assert!(
                    matches!(dropped, Err(IncusError::Guarded { .. })),
                    "{dropped:?}"
                );
            }
            assert!(fake.state().writes().is_empty());
        }
    }

    #[tokio::test]
    async fn the_copy_goes_only_to_the_records_name() {
        let fake = fake();
        let incus = Incus::new(&fake.socket);
        let copy = record(
            "ws-alpha",
            "mhq-alpha",
            Some(Approval::Step(MoveStep::Copy)),
        );
        let other = InstanceName::new("mhq-bravo").unwrap();
        let to = CopyTo {
            name: &other,
            config: BTreeMap::new(),
            pool: "default",
            network: "agentbr0",
            guest_socket: &std::env::temp_dir().join("mhq-bravo.sock"),
        };
        let copied = incus
            .copy_for_move("ws-alpha", MOVE_SNAPSHOT, to, &copy)
            .await;
        assert!(
            matches!(copied, Err(IncusError::Guarded { .. })),
            "{copied:?}"
        );
        assert!(fake.state().writes().is_empty());
    }

    #[tokio::test]
    async fn a_name_outside_ws_is_not_an_original() {
        let fake = fake();
        let incus = Incus::new(&fake.socket);
        for source in ["mhq-alpha", "alpha", "ws-", "ws-Alpha", "ws-alpha/../x"] {
            let mut approved = record(source, "mhq-alpha", Some(Approval::Step(MoveStep::Retire)));
            approved.source = source.into();
            let deleted = incus.delete_for_move(source, &approved).await;
            assert!(
                matches!(deleted, Err(IncusError::Guarded { .. })),
                "{deleted:?}"
            );
        }
        assert!(fake.state().writes().is_empty());
    }

    #[test]
    fn no_command_can_name_an_original() {
        // An exec takes an InstanceName, and that is only ever mhq-.
        for name in ["ws-alpha", "ws-bravo", "ws-echo"] {
            assert!(InstanceName::new(name).is_err(), "{name}");
        }
        assert!(InstanceName::new("mhq-alpha").is_ok());
    }

    #[tokio::test]
    async fn each_approved_write_reaches_only_its_original() {
        let fake = fake();
        let incus = Incus::new(&fake.socket);
        let copy = record(
            "ws-alpha",
            "mhq-alpha",
            Some(Approval::Step(MoveStep::Copy)),
        );
        // The copy takes the snapshot, copies it and may drop it again; the stop,
        // start and delete stay shut.
        assert_eq!(every_write(&incus, &copy, MOVE_SNAPSHOT).await.len(), 3);
        let state = fake.state();
        assert!(state.instances.contains_key("mhq-alpha"));
        assert!(state
            .writes()
            .iter()
            .all(|logged| { !logged.target.contains("ws-bravo") }));
        assert_eq!(state.instances["ws-alpha"]["status"], "Running");
        assert!(state.execs.is_empty());
    }
}
