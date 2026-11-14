// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Moving this device's Codex app-servers onto a new Codex binary. Each one
//! keeps running the binary it started with, so after an update it's
//! stopped once none of its threads is busy or waits on you. The next send
//! starts the new binary and adopts the threads again, as after any stop.

use maxhq_protocol::model::Vendor;

use super::held::{is_busy, ServerKey};
use super::Manager;

impl Manager {
    /// Marks every app-server on this device to stop once it's idle.
    pub(super) fn recycle_codex(&mut self) {
        let running: Vec<ServerKey> = self
            .servers
            .iter()
            .filter(|(key, server)| key.workspace.is_none() && server.session.is_some())
            .map(|(key, _)| *key)
            .collect();
        self.recycle.extend(running);
        self.recycle_idle();
    }

    /// Stops each marked app-server that has gone idle.
    pub(super) fn recycle_idle(&mut self) {
        if self.recycle.is_empty() {
            return;
        }
        let marked: Vec<ServerKey> = self.recycle.iter().copied().collect();
        for key in marked {
            let Some(session) = self.servers.get(&key).and_then(|server| server.session) else {
                // It stopped on its own, so the next start runs the new binary.
                self.recycle.remove(&key);
                continue;
            };
            if self.server_idle(key, session) {
                self.recycle.remove(&key);
                tracing::info!(%session, "stopping an idle Codex app-server so it runs the new binary");
                self.stop_hold(session);
            }
        }
    }

    /// Whether no thread on the app-server is busy, waits on you or has an
    /// action in flight.
    pub(super) fn server_idle(
        &self,
        key: ServerKey,
        session: maxhq_protocol::ids::SessionId,
    ) -> bool {
        let waiting = self
            .servers
            .get(&key)
            .is_some_and(|server| !server.waiting.is_empty());
        let busy = self.held.values().any(|held| {
            held.vendor() == Vendor::Codex
                && held.server_key() == key
                && is_busy(&held.agent.activity)
        });
        let in_flight = self.routed.values().any(|routed| *routed == session);
        let reaching = self
            .holds
            .get(&session)
            .is_some_and(|hold| hold.reaching.is_some());
        !(waiting || busy || in_flight || reaching)
    }
}
