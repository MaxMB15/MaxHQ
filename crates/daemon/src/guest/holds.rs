// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The container's holders, as the guest runs them.

use std::collections::HashMap;
use std::io;

use maxhq_hold::{Ensured, Found, Holds};
use maxhq_protocol::ids::SessionId;

use super::link::HolderPids;
use crate::session::clock::BoxFuture;
use crate::session::holders::{ConnectError, Connection, Ended, Holders, Launch, SpawnError};

/// The container's holders. A spawn starts a holder only when the session
/// has none, so a host that sends the same spawn again after its link
/// dropped never starts a second scope with the same name.
pub(crate) struct GuestHolds(Holds);

impl GuestHolds {
    /// The holders `holds` keeps.
    pub(crate) fn new(holds: Holds) -> Self {
        Self(holds)
    }
}

impl Holders for GuestHolds {
    fn spawn(&self, launch: Launch) -> BoxFuture<Result<(), SpawnError>> {
        let holds = self.0.clone();
        Box::pin(async move {
            match holds.ensure(&launch).await? {
                Ensured::Spawned | Ensured::Existing(Found::Live { .. } | Found::Starting) => {
                    Ok(())
                }
                Ensured::Existing(_) => Err(SpawnError::Exists {
                    session: launch.session,
                }),
            }
        })
    }

    fn list(&self) -> BoxFuture<io::Result<Vec<SessionId>>> {
        <Holds as Holders>::list(&self.0)
    }

    fn connect(
        &self,
        session: SessionId,
        after: Option<u64>,
    ) -> BoxFuture<Result<Connection, ConnectError>> {
        <Holds as Holders>::connect(&self.0, session, after)
    }

    fn end(&self, session: SessionId, holder_pid: Option<u32>) -> BoxFuture<Ended> {
        <Holds as Holders>::end(&self.0, session, holder_pid)
    }
}

impl HolderPids for GuestHolds {
    fn holder_pids(&self) -> BoxFuture<HashMap<u32, SessionId>> {
        let holds = self.0.clone();
        Box::pin(async move {
            let sessions = holds.list().unwrap_or_else(|err| {
                tracing::warn!("couldn't list the holders: {err}");
                Vec::new()
            });
            let mut pids = HashMap::new();
            for session in sessions {
                if let Ok(Found::Live { holder_pid }) = holds.find(session).await {
                    pids.insert(holder_pid, session);
                }
            }
            pids
        })
    }
}
