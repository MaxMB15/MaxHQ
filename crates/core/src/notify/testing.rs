// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A sender that records what it's asked to do, for tests here and in the
//! crates that use [`Notifier`](super::Notifier).

use std::sync::{Arc, Mutex};

use super::{Notification, NotificationSender};

/// What a fake sender was asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sent {
    /// Show this notification.
    Deliver(Notification),
    /// Take these off screen.
    Remove(Vec<String>),
    /// Take everything off screen.
    RemoveAll,
}

/// Records every call, for tests.
#[derive(Clone, Default)]
pub struct FakeSender(pub Arc<Mutex<Vec<Sent>>>);

impl FakeSender {
    /// Everything sent since the last call.
    pub fn take(&self) -> Vec<Sent> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }

    /// The IDs delivered since the last call, ignoring removals.
    pub fn delivered(&self) -> Vec<String> {
        self.take()
            .into_iter()
            .filter_map(|sent| match sent {
                Sent::Deliver(n) => Some(n.id),
                _ => None,
            })
            .collect()
    }
}

impl NotificationSender for FakeSender {
    fn deliver(&self, notification: &Notification) {
        self.0
            .lock()
            .unwrap()
            .push(Sent::Deliver(notification.clone()));
    }

    fn remove(&self, ids: &[String]) {
        self.0.lock().unwrap().push(Sent::Remove(ids.to_vec()));
    }

    fn remove_all(&self) {
        self.0.lock().unwrap().push(Sent::RemoveAll);
    }
}
