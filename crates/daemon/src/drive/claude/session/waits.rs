// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Waits for a usage limit to reset. A turn the limit stops leaves a wait
//! on the lead's timeline, open until the limit resets or the API answers
//! the lead again. A turn the CLI starts by itself, as it does when a
//! background task ends, can make no API call, so it leaves the wait
//! open. A message you queue while the agent waits idle stays with
//! `maxd`, since the CLI would only run into the limit again, and goes
//! once the reset passes. While any message is held, the wait says the
//! agent goes on by itself.

use maxhq_protocol::message::SendMode;
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{Delivery, ItemBody, ItemId, ItemStatus};
use serde_json::Value;

use super::ClaudeSession;
use crate::drive::claude::convo;
use crate::drive::Effects;

/// A message held until the usage limit resets.
#[derive(Clone, Debug)]
pub struct Held {
    /// Its UUID, which its item and its line for the CLI carry.
    pub uuid: String,
    /// The blocks for its `message.content`.
    pub blocks: Vec<Value>,
}

impl ClaudeSession {
    /// Keeps the limit wait `id` open, after closing an older one.
    pub(super) fn open_wait(&mut self, id: ItemId, at: Timestamp) {
        if self.wait.as_ref().is_some_and(|open| *open != id) {
            self.close_wait(at);
        }
        let resume = !self.held.is_empty();
        self.convo.book.update(&id, |item| {
            item.status = ItemStatus::Running;
            item.completed_at = None;
            if let ItemBody::LimitWait { auto_resume, .. } = &mut item.body {
                *auto_resume = resume;
            }
        });
        self.wait = Some(id);
    }

    /// Closes the open limit wait at `at`.
    pub(super) fn close_wait(&mut self, at: Timestamp) {
        let Some(id) = self.wait.take() else {
            return;
        };
        self.convo.book.update(&id, |item| {
            item.status = ItemStatus::Completed;
            item.completed_at = Some(at);
        });
    }

    /// When the open wait's limit resets, as the wait shows it.
    pub(super) fn wait_resets(&self) -> Option<Timestamp> {
        let item = self.convo.book.get(self.wait.as_ref()?)?;
        match item.body {
            ItemBody::LimitWait { resets_at, .. } => resets_at,
            _ => None,
        }
    }

    /// Shows a newer reset time on the open wait.
    pub(super) fn wait_until(&mut self, resets: Timestamp) {
        let Some(id) = self.wait.clone() else {
            return;
        };
        self.convo.book.update(&id, |item| {
            if let ItemBody::LimitWait { resets_at, .. } = &mut item.body {
                *resets_at = Some(resets);
            }
        });
    }

    /// Whether a message sent with `mode` waits with `maxd` for the reset.
    pub(super) fn holds(&self, mode: SendMode) -> bool {
        mode == SendMode::Queue && !self.working() && self.limited().is_some()
    }

    /// Holds a message until the reset.
    pub(super) fn hold(&mut self, uuid: String, blocks: Vec<Value>) {
        self.held.push(Held { uuid, blocks });
        self.show_resume();
    }

    /// Takes back the held message `uuid`, if it's held.
    pub(super) fn unhold(&mut self, uuid: &str, at: Timestamp) -> bool {
        let Some(index) = self.held.iter().position(|held| held.uuid == uuid) else {
            return false;
        };
        self.held.remove(index);
        self.convo.book.update(&convo::item_id(uuid), |item| {
            item.status = ItemStatus::Cancelled;
            item.completed_at = Some(at);
            if let ItemBody::UserMessage { delivery, .. } = &mut item.body {
                *delivery = Delivery::Cancelled;
            }
        });
        self.show_resume();
        true
    }

    /// Says on the open wait whether a held message goes on its own.
    fn show_resume(&mut self) {
        let Some(id) = self.wait.clone() else {
            return;
        };
        let resume = !self.held.is_empty();
        self.convo.book.update(&id, |item| {
            if let ItemBody::LimitWait { auto_resume, .. } = &mut item.body {
                *auto_resume = resume;
            }
        });
    }

    /// Once the limit has reset: sends the held messages in order when the
    /// agent is idle, then closes the wait. A wait with no known reset
    /// time stays open until the API answers the lead.
    pub(super) fn resume(&mut self, fx: &mut Effects) {
        if self.exited || self.limited().is_some() {
            return;
        }
        if !self.working() {
            for held in std::mem::take(&mut self.held) {
                self.write_message(&held.uuid, held.blocks, SendMode::Queue, fx);
            }
        }
        if self
            .convo
            .resets_at
            .take()
            .or_else(|| self.wait_resets())
            .is_some()
        {
            self.close_wait(self.now());
        }
    }
}
