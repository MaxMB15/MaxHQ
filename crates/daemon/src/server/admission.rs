// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A connection's side of the admission card: asking for one when the
//! gate refuses it or the device won't trust its key yet, answering
//! another's, and taking the key you allowed.

use maxhq_os::ipc::LocalStream;
use maxhq_protocol::ids::AdmissionId;
use maxhq_protocol::message::ActionOutcome;
use tokio::sync::broadcast;
use tracing::info;

use super::proof::{refused, KEYS_UNREAD};
use super::Connection;
use crate::admission::{Answer, Ask, Asked, Ended, Seat, LIMITS};
use crate::app_keys::AppKeys;
use crate::codesign;

/// The seat for a connection on `stream`, whose audit token was `token` as
/// it connected.
pub(super) fn seat_for(
    stream: &LocalStream,
    keys: &AppKeys,
    token: Option<codesign::Token>,
) -> Seat {
    let pid = stream
        .peer()
        .ok()
        .and_then(|peer| peer.pid)
        .and_then(|pid| i32::try_from(pid).ok());
    keys.admissions().seat(pid, token)
}

impl Connection {
    /// Asks for a card about letting this connection in, after the gate
    /// refused it. A connection that has one, had one or finds too many
    /// open asks nothing more. A pairing asks nothing: it may only pair a
    /// phone, and only its one-time key lets one in.
    pub(super) fn ask_admission(&mut self) {
        if self.pairs {
            return;
        }
        let phone = self.phone.is_some();
        let _ = self.seat.ask(&self.hub, Ask::Admit, phone);
    }

    /// Asks for a card about trusting `public_key`, which this connection
    /// proved it holds, under `label`, since the device won't trust it on
    /// its own.
    pub(super) fn ask_enrollment(
        &mut self,
        public_key: String,
        label: String,
        revoked: broadcast::Receiver<String>,
    ) -> ActionOutcome {
        let device = self.hub.device().name.clone();
        let phone = self.phone.is_some();
        let ask = Ask::Enroll { public_key, label };
        match self.seat.ask(&self.hub, ask, phone) {
            Asked::Open => {
                self.seat.keep(revoked);
                ActionOutcome::Asked
            }
            Asked::Full => refused(format!(
                "{device} already has {} programs waiting for an answer, so it didn't ask about \
                 this one. Answer their cards, then reconnect.",
                LIMITS.cap
            )),
            Asked::Ended(Ended::Denied) => refused(format!(
                "{device} asked about this program and the answer was Don't allow, or nobody \
                 answered in time. Reconnect to ask again."
            )),
            Asked::Ended(Ended::Allowed) => refused(format!(
                "This program may already do everything until it disconnects. To have {device} \
                 trust its key, reconnect and ask again."
            )),
        }
    }

    /// Answers card `admission`. Only MaxHQ's desktop app, proven with a
    /// key the device trusts, answers, and never about itself.
    pub(super) async fn answer_admission(
        &mut self,
        admission: AdmissionId,
        allow: bool,
    ) -> ActionOutcome {
        let app = self.app.is_some()
            && self.phone.is_none()
            && self.peer.is_none()
            && !self.mcp
            && !self.pairs;
        if !app {
            return refused(format!(
                "Only MaxHQ's app, once it has shown {} its key, can answer that card.",
                self.hub.device().name
            ));
        }
        let admissions = self.keys.admissions().clone();
        match admissions.answer(admission, allow, self.seat.id()).await {
            Answer::Done => ActionOutcome::Done,
            Answer::Own => refused("A program can't answer its own card."),
            Answer::Gone => refused(
                "That card has closed. Someone answered it, its program disconnected or it \
                 timed out.",
            ),
            Answer::Unread => refused(KEYS_UNREAD),
        }
    }

    /// This connection proved `key`, which the device trusts. It closes
    /// when `revoked` says the key went.
    pub(super) fn now_proven(&mut self, key: String, revoked: broadcast::Receiver<String>) {
        self.app = Some(key);
        self.revoked = Some(revoked);
        self.seat.proven(self.phone.is_some());
    }

    /// You allowed the card that asked whether to trust `key`, so the
    /// device trusts it and this connection is proven with it.
    pub(super) fn approved(&mut self, key: String) {
        let revoked = self
            .seat
            .take_revoked()
            .unwrap_or_else(|| self.keys.removed());
        info!(%key, "a connection is proven with the app key you allowed");
        self.held = Some(key.clone());
        self.now_proven(key, revoked);
    }
}

#[cfg(test)]
mod tests;
