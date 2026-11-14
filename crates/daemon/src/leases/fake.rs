// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A test host that runs nothing, for the store's tests and the backends'
//! callers. It logs each call, answers what the test set, and can hold a
//! start until the test lets it go.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures::future::BoxFuture;
use maxhq_protocol::ids::LeaseId;
use maxhq_protocol::leases::{RunOutput, TestHostBackend, TestKind};
use tokio::sync::watch;

use super::backend::{Built, LeaseTarget, TestBackend};

/// What the fake answers, and what it was asked.
#[derive(Default)]
pub struct Script {
    /// What detection says is missing.
    pub setup: Option<String>,
    /// Why a build fails, if it should.
    pub build_fails: Option<String>,
    /// Why a start fails, if it should.
    pub start_fails: Option<String>,
    /// Why a reset fails, if it should.
    pub reset_fails: Option<String>,
    /// The PNG a screenshot answers.
    pub screen: Vec<u8>,
    /// Each call, in order.
    pub log: Vec<String>,
}

/// A fake [`TestBackend`].
pub struct FakeBackend {
    kind: TestKind,
    backend: TestHostBackend,
    script: Mutex<Script>,
    /// Starts wait while this is false.
    open: watch::Sender<bool>,
}

impl FakeBackend {
    /// A host of `kind` on `backend` that has all it needs.
    pub fn new(kind: TestKind, backend: TestHostBackend) -> Self {
        Self {
            kind,
            backend,
            script: Mutex::default(),
            open: watch::Sender::new(true),
        }
    }

    /// What it answers and logs.
    pub fn script(&self) -> MutexGuard<'_, Script> {
        self.script.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The calls so far.
    pub fn log(&self) -> Vec<String> {
        self.script().log.clone()
    }

    /// Holds every start until [`FakeBackend::release_starts`].
    pub fn hold_starts(&self) {
        self.open.send_replace(false);
    }

    /// Lets held starts go on.
    pub fn release_starts(&self) {
        self.open.send_replace(true);
    }

    fn note(&self, call: String) {
        self.script().log.push(call);
    }

    fn fails(&self, pick: impl Fn(&Script) -> Option<String>) -> Result<(), String> {
        pick(&self.script()).map_or(Ok(()), Err)
    }
}

impl TestBackend for FakeBackend {
    fn kind(&self) -> TestKind {
        self.kind
    }

    fn backend(&self) -> TestHostBackend {
        self.backend
    }

    fn detect(&self) -> BoxFuture<'_, Option<String>> {
        Box::pin(async move { self.script().setup.clone() })
    }

    fn build(&self) -> BoxFuture<'_, Result<Built, String>> {
        Box::pin(async move {
            self.note("build".into());
            self.fails(|script| script.build_fails.clone())?;
            Ok(Built {
                agent_device: Some("0.0.0-fake".into()),
                vm: None,
            })
        })
    }

    fn start(&self, lease: LeaseId) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async move {
            self.note(format!("start {lease}"));
            let mut open = self.open.subscribe();
            let _ = open.wait_for(|open| *open).await;
            self.fails(|script| script.start_fails.clone())?;
            Ok(format!("fake-{lease}"))
        })
    }

    fn run<'a>(
        &'a self,
        on: &'a LeaseTarget,
        command: &'a [String],
        _timeout: Duration,
    ) -> BoxFuture<'a, Result<RunOutput, String>> {
        Box::pin(async move {
            self.note(format!("run {} {}", on.target, command.join(" ")));
            Ok(RunOutput {
                exit_code: Some(0),
                stdout: format!("ran {}", command.join(" ")),
                stderr: String::new(),
                truncated: false,
                timed_out: false,
            })
        })
    }

    fn screenshot<'a>(&'a self, on: &'a LeaseTarget) -> BoxFuture<'a, Result<Vec<u8>, String>> {
        Box::pin(async move {
            self.note(format!("screenshot {}", on.target));
            Ok(self.script().screen.clone())
        })
    }

    fn install<'a>(
        &'a self,
        on: &'a LeaseTarget,
        source: PathBuf,
    ) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            self.note(format!("install {} {}", on.target, source.display()));
            Ok(format!("Installed {}.", source.display()))
        })
    }

    fn close<'a>(&'a self, on: &'a LeaseTarget) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.note(format!("close {}", on.target));
            Ok(())
        })
    }

    fn reset(&self) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            self.note("reset".into());
            self.fails(|script| script.reset_fails.clone())
        })
    }
}
