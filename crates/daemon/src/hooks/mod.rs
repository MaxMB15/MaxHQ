// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Webhooks: the public URL a task's requests come in on, and the checks
//! each request passes before it fires the task.

use std::collections::BTreeMap;

use futures::future::BoxFuture;
use hyper::StatusCode;
use maxhq_protocol::ids::TaskId;
use maxhq_protocol::tasks::{DeliveryOutcome, FunnelState, HookAddress, Task};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::Secret;

pub(crate) mod deliver;
mod funnel;
mod listener;
mod rate;
mod service;
mod store;
#[cfg(test)]
mod tests;

pub use funnel::{funnels, FUNNEL_PORT};
pub use listener::{port, PORT_FILE};
pub(crate) use rate::{Counter, Slot};
pub use service::{
    install, list_deliveries, rotate_token, set_address, set_on, set_secret, HookEdits,
    HookSecrets, Installed, Parts, Webhooks, FUNNEL_PREFIX,
};

/// How a device's webhooks are reached, and whether they can be there, as
/// `DeviceTasks` shows them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reach {
    pub address: HookAddress,
    /// The port the hook listener takes requests on.
    pub port: Option<u16>,
    pub funnel: FunnelState,
}

impl Default for Reach {
    fn default() -> Self {
        Self {
            address: HookAddress::Local,
            port: None,
            funnel: FunnelState::Off,
        }
    }
}

/// What makes a path of `maxd`'s webhook listener reachable at the
/// device's [`HookAddress`], such as Tailscale Funnel.
pub trait Funnel: Send + Sync {
    /// Makes the path public.
    fn open(&self, path: &str) -> BoxFuture<'_, Result<(), String>>;

    /// Takes the path off the public internet.
    fn close(&self, path: &str) -> BoxFuture<'_, Result<(), String>>;

    /// Whether it's on, and where, or what the device needs first.
    fn state(&self) -> BoxFuture<'_, FunnelState>;

    /// The base hook URLs start with, such as
    /// `https://devbox.example.ts.net`.
    fn base(&self) -> BoxFuture<'_, Result<String, String>>;
}

/// What answers GitHub's repository webhooks on the listener: the GitHub
/// events package's doorbell, at a path of its own beside the task hooks'.
pub trait Doorbell: Send + Sync {
    /// Whether a request to `path` is for the doorbell: its path with its
    /// token after it, whole or without the prefix the Funnel may drop.
    fn rings(&self, path: &str) -> bool;

    /// Checks a request's headers before its body is read, and answers
    /// the refusal for one that can't be from this device's webhooks.
    fn admit(&self, headers: &BTreeMap<String, String>) -> Result<(), (StatusCode, &'static str)>;

    /// Answers one request with its status and what came of it, once the
    /// listener checked its method and size and the doorbell admitted it.
    fn ring(
        &self,
        headers: BTreeMap<String, String>,
        body: Vec<u8>,
    ) -> BoxFuture<'_, (StatusCode, &'static str)>;
}

/// The webhook side of saving and deleting a task, which the webhooks
/// package implements and the tasks package calls.
pub trait Hooks: Send + Sync {
    /// Makes a new URL for a hook task, so any old one stops working, and
    /// answers it. Only its token's hash is kept, so this is the one time
    /// the URL is shown.
    fn rotate(&self, task: TaskId) -> BoxFuture<'_, Result<Secret, String>>;

    /// Forgets a task's hook: its URL, signing secret, deliveries and
    /// public path.
    fn forget(&self, task: TaskId) -> BoxFuture<'_, Result<(), String>>;
}

/// A request from outside for a task: a webhook request, or a GitHub
/// event as a webhook would carry it.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct Request {
    /// The delivery's ID, from its header or made up.
    pub delivery: String,
    /// When `maxd` got it.
    pub received: Timestamp,
    /// Its headers, by lowercase name.
    pub headers: BTreeMap<String, String>,
    /// Its query string, without the `?`.
    pub query: String,
    /// Its body.
    pub body: Vec<u8>,
}

/// Where a task's requests go after the checks only an HTTP request has
/// (method, size, token, rate, signature and age): its filters, replay
/// key, coalescing and template, then its run. The webhooks package
/// implements it, and its listener and the GitHub poller hand it their
/// requests. Each one is kept in the task's delivery log.
#[allow(dead_code)]
pub trait Deliver: Send + Sync {
    /// Takes `request` for `task` and says what came of it.
    fn deliver<'a>(&'a self, task: &'a Task, request: Request) -> BoxFuture<'a, DeliveryOutcome>;
}

/// A [`Deliver`] for tests: it keeps each request and gives one answer to
/// all.
#[cfg(test)]
pub struct FakeDeliver {
    pub delivered: std::sync::Mutex<Vec<(TaskId, Request)>>,
    pub answer: DeliveryOutcome,
}

#[cfg(test)]
impl Default for FakeDeliver {
    fn default() -> Self {
        Self {
            delivered: Default::default(),
            answer: DeliveryOutcome::Accepted,
        }
    }
}

#[cfg(test)]
impl Deliver for FakeDeliver {
    fn deliver<'a>(&'a self, task: &'a Task, request: Request) -> BoxFuture<'a, DeliveryOutcome> {
        self.delivered.lock().unwrap().push((task.id, request));
        Box::pin(std::future::ready(self.answer))
    }
}
