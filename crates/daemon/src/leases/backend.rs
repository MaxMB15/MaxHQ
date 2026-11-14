// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What runs one test host. The lease store keeps the hosts, the queue and
//! the timers, and hands each step that touches a machine to the host's
//! [`TestBackend`]: the Simulator on the Mac, the emulator, the Incus
//! container or the macOS VM.

use std::path::PathBuf;
use std::time::Duration;

use futures::future::BoxFuture;
use maxhq_protocol::ids::LeaseId;
use maxhq_protocol::leases::{RunOutput, TestHostBackend, TestKind, VmSize};

/// What a build made.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Built {
    /// The agent-device version it drives with, when it has one.
    pub agent_device: Option<String>,
    /// The VM's size, for a macOS VM.
    pub vm: Option<VmSize>,
}

/// The machine one lease drives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaseTarget {
    /// The lease.
    pub lease: LeaseId,
    /// The Simulator's UDID, the emulator's serial or the container's
    /// name, as [`TestBackend::start`] answered.
    pub target: String,
}

/// One kind of test machine on one backend on this device.
pub trait TestBackend: Send + Sync + 'static {
    /// What it offers.
    fn kind(&self) -> TestKind;

    /// What runs it.
    fn backend(&self) -> TestHostBackend;

    /// What this device lacks to run it, as the setup text the app shows,
    /// or none when it has all it needs. Detection answers here; the store
    /// asks at start and on every check.
    fn detect(&self) -> BoxFuture<'_, Option<String>>;

    /// Makes the host from nothing, or after it failed, and leaves it
    /// clean and ready for a lease.
    fn build(&self) -> BoxFuture<'_, Result<Built, String>>;

    /// Gets a machine ready for `lease` and answers its target.
    fn start(&self, lease: LeaseId) -> BoxFuture<'_, Result<String, String>>;

    /// Runs one command on the lease's machine.
    fn run<'a>(
        &'a self,
        on: &'a LeaseTarget,
        command: &'a [String],
        timeout: Duration,
    ) -> BoxFuture<'a, Result<RunOutput, String>>;

    /// A PNG of the machine's screen, at any size.
    fn screenshot<'a>(&'a self, on: &'a LeaseTarget) -> BoxFuture<'a, Result<Vec<u8>, String>>;

    /// Installs the app at `source` on the machine and says what it did.
    fn install<'a>(
        &'a self,
        on: &'a LeaseTarget,
        source: PathBuf,
    ) -> BoxFuture<'a, Result<String, String>>;

    /// Ends the lease on the machine: closes agent-device's session, then
    /// takes the machine back to a clean state.
    fn close<'a>(&'a self, on: &'a LeaseTarget) -> BoxFuture<'a, Result<(), String>>;

    /// Takes the whole host back to a clean state, such as after `maxd`
    /// restarted while a lease ran.
    fn reset(&self) -> BoxFuture<'_, Result<(), String>>;
}
