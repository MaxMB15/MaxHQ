// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Test hosts and their leases. The agent tools hand each checked lease
//! call to [`Leases`], which runs it for a caller on this device, and the
//! app's requests ask it about the hosts and leases it publishes.
//!
//! Leases are local: only callers on the device that holds a host lease
//! it. [`TestHosts`] keeps the hosts, the queues and the timers, and each
//! host's [`backend::TestBackend`] does what touches a machine.

pub mod backend;
pub mod detect;
pub mod emulator;
#[cfg(test)]
mod fake;
pub mod incus;
mod lease;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod mac_host;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod mac_vm;
pub mod store;
#[cfg(test)]
mod tests;
mod text;
mod timers;
mod via_guest;

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use maxhq_core::leases::LeaseCall;
use maxhq_protocol::agents::Actor;
use maxhq_protocol::ids::{LeaseId, PermissionId, TestHostId};
use maxhq_protocol::message::ActionOutcome;

use crate::drive::ToolResult;

pub use store::TestHosts;
pub use text::kind_label;

static STORE: OnceLock<TestHosts> = OnceLock::new();

/// Makes `store` the one the app's requests and the lease tools reach.
/// Only the first call counts, since `maxd` opens one store.
pub fn install(store: TestHosts) {
    let _ = STORE.set(store);
}

/// What the lease tools run their calls through: the installed store, or
/// [`NoLeases`] before one is.
pub fn installed() -> Arc<dyn Leases> {
    match STORE.get() {
        Some(store) => Arc::new(store.clone()),
        None => Arc::new(NoLeases),
    }
}

/// Answers `CheckTestHosts` once detection looked again.
pub async fn check() -> ActionOutcome {
    match STORE.get() {
        Some(store) => {
            store.check().await;
            ActionOutcome::Done
        }
        None => refused(),
    }
}

/// Answers `EndLease`.
pub async fn end(lease: LeaseId) -> ActionOutcome {
    match STORE.get() {
        Some(store) => store.end_by_you(lease).await,
        None => refused(),
    }
}

/// Answers `ReadLeaseScreen`.
pub async fn screen(lease: LeaseId) -> ActionOutcome {
    match STORE.get() {
        Some(store) => store.screen(lease).await,
        None => refused(),
    }
}

/// Answers `BuildTestHost` once the build started; the host's state shows
/// how it goes.
pub fn build(host: TestHostId) -> ActionOutcome {
    match STORE.get() {
        Some(store) => done_or_refused(store.build(host)),
        None => refused(),
    }
}

/// Answers `ResetTestHost` once the host is clean again.
pub async fn reset(host: TestHostId) -> ActionOutcome {
    match STORE.get() {
        Some(store) => done_or_refused(store.reset(host).await),
        None => refused(),
    }
}

fn done_or_refused(result: Result<(), String>) -> ActionOutcome {
    match result {
        Ok(()) => ActionOutcome::Done,
        Err(reason) => ActionOutcome::Refused { reason },
    }
}

/// The refusal for every lease call and request while this device runs no
/// test hosts.
pub const NOT_YET: &str = "Test hosts aren't set up yet.";

/// The answer to an app request about test hosts while there are none.
pub fn refused() -> ActionOutcome {
    ActionOutcome::Refused {
        reason: NOT_YET.to_owned(),
    }
}

/// Who makes a lease call, once the agent tools checked that its
/// permission lists the kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeaseCaller {
    /// The caller, on this device.
    pub actor: Actor,
    /// The permission it calls under.
    pub permission: PermissionId,
}

/// What a lease call came to: text, text with a screenshot, or what the
/// agent reads about why it failed.
pub type LeaseAnswer<'a> = Pin<Box<dyn Future<Output = Result<ToolResult, String>> + Send + 'a>>;

/// This device's test hosts, as the lease tools reach them.
pub trait Leases: Send + Sync {
    /// Runs one lease call for `caller`.
    fn run(&self, caller: LeaseCaller, call: LeaseCall) -> LeaseAnswer<'_>;
}

/// No test hosts on this device: what the lease tools reach before the
/// store is installed.
pub struct NoLeases;

impl Leases for NoLeases {
    fn run(&self, _caller: LeaseCaller, _call: LeaseCall) -> LeaseAnswer<'_> {
        Box::pin(async { Err(NOT_YET.to_owned()) })
    }
}
