// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A lease's calls on a test host whose `maxd guest` answers them over the
//! guest link, as the `linux` container's and the macOS VM's do.

use std::time::Duration;

use base64::Engine as _;
use futures::future::BoxFuture;
use maxhq_protocol::guest::{GuestToHost, HostToGuest};
use maxhq_protocol::ids::LeaseId;
use maxhq_protocol::leases::RunOutput;

use crate::transfer::MakeRequest;

/// How long the host waits past a run's own timeout for its answer.
pub(super) const RUN_SLACK: Duration = Duration::from_secs(30);

/// How long the host waits for any other answer.
pub(super) const ANSWER_WAIT: Duration = Duration::from_secs(600);

/// What the guest's answer of the wrong kind says.
pub(super) const WRONG: &str = "the test host answered something else";

/// A test host's guest, which answers requests.
pub(super) trait Guest: Send + Sync {
    /// Sends the guest the request `make` builds and waits up to `wait`
    /// for its answer.
    fn ask(&self, wait: Duration, make: MakeRequest) -> BoxFuture<'_, Result<GuestToHost, String>>;
}

/// Runs `command` for `lease`.
pub(super) async fn run<G: Guest + ?Sized>(
    guest: &G,
    lease: LeaseId,
    command: &[String],
    timeout: Duration,
) -> Result<RunOutput, String> {
    let command = command.to_vec();
    let timeout_seconds = u32::try_from(timeout.as_secs()).unwrap_or(u32::MAX);
    let make = Box::new(move |req| HostToGuest::LeaseRun {
        req,
        lease,
        command,
        timeout_seconds,
    });
    match guest.ask(timeout + RUN_SLACK, make).await? {
        GuestToHost::LeaseRan { result, .. } => result,
        _ => Err(WRONG.to_owned()),
    }
}

/// The PNG of `lease`'s screen.
pub(super) async fn screenshot<G: Guest + ?Sized>(
    guest: &G,
    lease: LeaseId,
) -> Result<Vec<u8>, String> {
    let make = Box::new(move |req| HostToGuest::LeaseScreenshot { req, lease });
    match guest.ask(ANSWER_WAIT, make).await? {
        GuestToHost::LeaseScreen { result, .. } => base64::engine::general_purpose::STANDARD
            .decode(result?.data)
            .map_err(|_| "the screenshot doesn't read".to_owned()),
        _ => Err(WRONG.to_owned()),
    }
}

/// Installs the upload the guest finds at `path`.
pub(super) async fn install<G: Guest + ?Sized>(
    guest: &G,
    lease: LeaseId,
    path: String,
) -> Result<(), String> {
    let make = Box::new(move |req| HostToGuest::LeaseInstall { req, lease, path });
    match guest.ask(ANSWER_WAIT, make).await? {
        GuestToHost::LeaseDone { result, .. } => result,
        _ => Err(WRONG.to_owned()),
    }
}

/// Ends `lease`'s session in the guest.
pub(super) async fn close<G: Guest + ?Sized>(guest: &G, lease: LeaseId) -> Result<(), String> {
    let make = Box::new(move |req| HostToGuest::LeaseClose { req, lease });
    match guest.ask(ANSWER_WAIT, make).await? {
        GuestToHost::LeaseDone { result, .. } => result,
        _ => Err(WRONG.to_owned()),
    }
}
