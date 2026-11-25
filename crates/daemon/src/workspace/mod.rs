// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Container workspaces: create, start, stop and delete, snapshots, the
//! egress probe, A-11's starts at boot, and A-10's cache rebuilt from
//! instance keys. It drives [`crate::incus`] and runs [`crate::recipe`]'s steps.
//!
//! One [`Manager`] runs them. `serve` makes it and installs it, and the
//! request functions here answer through it. The server runs each as a
//! task of its own, so the connection that asked goes on meanwhile.

mod caps;
mod create;
pub(crate) mod egress;
mod exec;
mod install;
mod keys;
mod lifecycle;
mod manager;
mod manifest;
mod moved;
mod probe;
mod repo;
mod snapshots;
mod test_host;
// The tests run the manager against the fake Incus, on a Unix socket.
#[cfg(all(test, unix))]
mod tests;
mod unsaved;

use std::future::Future;
use std::path::Path;
use std::process::ExitCode;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use futures::future::BoxFuture;
use futures::FutureExt;
use maxhq_protocol::dispatch::ProjectSettingChange;
use maxhq_protocol::guest::GuestUnsaved;
use maxhq_protocol::ids::{ProjectId, WorkspaceId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{ProjectSettings, WorkspaceSpec};
use maxhq_protocol::Secret;
use tokio::time::Instant;

// XJ's sessions in containers run commands through this.
pub use caps::Caps;
#[allow(unused_imports)]
pub use exec::IncusExec;
pub use install::GuestInstall;
pub use keys::key::WORKSPACE as WORKSPACE_KEY;
pub use manager::{Config, Manager};
pub use moved::mover;
pub(crate) use repo::Repo;
#[cfg(all(test, unix))]
pub(crate) use tests::install_slow_stop;

use crate::incus::InstanceName;
use crate::server::not_yet;

/// The manager the request functions answer through.
static MANAGER: OnceLock<Manager> = OnceLock::new();

/// Makes `manager` the one that answers requests. Only the first call
/// counts.
pub fn install(manager: Manager) {
    let _ = MANAGER.set(manager);
}

/// The installed manager, for sessions in containers. XJ's take it up.
#[allow(dead_code)]
pub fn manager() -> Option<&'static Manager> {
    MANAGER.get()
}

/// Keeps the SSH host key a workspace's guest reported.
pub fn keep_ssh_host_key(workspace: WorkspaceId, key: String) {
    if let Some(manager) = MANAGER.get() {
        manager.set_ssh_host_key(workspace, key);
    }
}

/// Keeps the latest unsaved work a workspace's guest reported.
pub fn keep_unsaved(workspace: WorkspaceId, worktrees: &[GuestUnsaved]) {
    if let Some(manager) = MANAGER.get() {
        manager.set_unsaved(workspace, worktrees);
    }
}

/// Why an editor can't open SSH into a workspace now, or none when it
/// can.
pub fn ssh_blocked(workspace: WorkspaceId) -> Option<String> {
    match MANAGER.get() {
        Some(manager) => manager.ssh_blocked(workspace),
        None => Some("this device doesn't run container workspaces".to_owned()),
    }
}

/// The clone URL of the GitHub repo whose project is `project`, when this
/// device's GitHub App reaches it.
pub fn app_repo_url(project: ProjectId) -> Option<String> {
    MANAGER
        .get()?
        .app_repo(project)
        .map(|repo| repo.clone_url())
}

/// Whether this device can run container workspaces now, which `Hello`
/// tells clients.
pub fn capable() -> bool {
    MANAGER.get().is_some_and(Manager::capable)
}

/// How long after the manager starts a client's hello may wait for Incus
/// to answer. The client gives up on a hello after 10 s.
const SETTLE_WAIT: Duration = Duration::from_secs(5);

/// How often a waiting hello looks again.
const SETTLE_POLL: Duration = Duration::from_millis(50);

/// When the manager began looking for Incus, until it finds none here.
static LOOKING: Mutex<Option<Instant>> = Mutex::new(None);

fn looking() -> MutexGuard<'static, Option<Instant>> {
    LOOKING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Runs `manager` as [`Manager::run`] does, and marks it as looking for
/// Incus from this call, so [`settled`] knows to wait.
pub fn run(manager: Manager) -> impl Future<Output = ()> + Send + 'static {
    *looking() = Some(Instant::now());
    async move {
        manager.run().await;
        *looking() = None;
    }
}

/// What [`capable`] says once Incus has answered. While the manager still
/// looks for Incus, this waits for an answer until [`SETTLE_WAIT`] after
/// the manager started, so a client that says hello early still hears
/// that this device runs workspaces.
pub async fn settled() -> bool {
    settle(capable, || *looking(), SETTLE_WAIT).await
}

async fn settle(
    capable: impl Fn() -> bool,
    looking: impl Fn() -> Option<Instant>,
    wait: Duration,
) -> bool {
    loop {
        if capable() {
            return true;
        }
        match looking() {
            Some(since) if since.elapsed() < wait => tokio::time::sleep(SETTLE_POLL).await,
            _ => return false,
        }
    }
}

/// Answers `CreateWorkspace`. The checks answer at once, and the steps
/// show in the new workspace's state.
pub async fn create(spec: WorkspaceSpec) -> ActionOutcome {
    answer(|manager| async move { manager.create(spec).await }).await
}

/// Answers `StartWorkspace`.
pub async fn start(workspace: WorkspaceId) -> ActionOutcome {
    answer(|manager| async move { manager.start(workspace).await }).await
}

/// Answers `StopWorkspace`.
pub async fn stop(workspace: WorkspaceId) -> ActionOutcome {
    answer(|manager| async move { manager.stop(workspace).await }).await
}

/// Answers `DeleteWorkspace`. Without `discard_unpushed` it refuses while
/// the workspace has work that isn't pushed, and says what.
pub async fn delete(workspace: WorkspaceId, discard_unpushed: bool) -> ActionOutcome {
    answer(|manager| async move { manager.delete(workspace, discard_unpushed).await }).await
}

/// Answers `ListSnapshots`.
pub async fn list_snapshots(workspace: WorkspaceId) -> ActionOutcome {
    answer(|manager| async move { manager.list_snapshots(workspace).await }).await
}

/// Answers `RestoreSnapshot`. A roll back refuses while the workspace has
/// unsaved work, unless `discard_unsaved` says to lose it.
pub async fn restore_snapshot(
    workspace: WorkspaceId,
    snapshot: String,
    as_new: bool,
    discard_unsaved: bool,
) -> ActionOutcome {
    answer(|manager| async move {
        manager
            .restore_snapshot(workspace, snapshot, as_new, discard_unsaved)
            .await
    })
    .await
}

/// Answers `SetProjectSettings`.
pub async fn set_project_settings(settings: ProjectSettings) -> ActionOutcome {
    answer(|manager| async move { manager.set_project_settings(settings).await }).await
}

/// Makes some changes to a project's settings on this device, for the
/// dispatcher.
pub async fn change_project_settings(
    project: ProjectId,
    changes: Vec<ProjectSettingChange>,
) -> ActionOutcome {
    answer(|manager| async move {
        let mut settings = manager.project_settings(project);
        maxhq_core::tools::admin::apply_project(&mut settings, &changes);
        manager.set_project_settings(settings).await
    })
    .await
}

/// `maxd guest probe TARGETS.json`: tries each target from inside a
/// container and prints what answered as JSON. The host runs it through
/// Incus exec.
pub fn probe(targets: &Path) -> ExitCode {
    probe::run(targets)
}

/// Runs a request on the installed manager.
async fn answer<F, Fut>(request: F) -> ActionOutcome
where
    F: FnOnce(Manager) -> Fut,
    Fut: Future<Output = ActionOutcome>,
{
    let Some(manager) = MANAGER.get() else {
        return not_yet();
    };
    request(manager.clone()).await
}

/// Mints GitHub tokens for a repo, as "owner/name".
pub trait Github: Send + Sync + 'static {
    /// A token for `repo`, or why there's none in words a person can act on.
    fn mint<'a>(&'a self, repo: &'a str) -> BoxFuture<'a, Result<Secret, String>>;
}

/// The device's GitHub App, through XH's broker.
pub struct Broker;

impl Github for Broker {
    fn mint<'a>(&'a self, repo: &'a str) -> BoxFuture<'a, Result<Secret, String>> {
        async move {
            crate::secrets::github(repo)
                .await
                .map(|token| token.value)
                .map_err(|error| error.to_string())
        }
        .boxed()
    }
}

/// The host's links to the guests, which XE runs.
pub trait GuestLinks: Send + Sync + 'static {
    /// Starts linking to the guest in `instance` through its proxy socket,
    /// at [`socket_path`](crate::guestlink::socket_path) in the runtime
    /// folder both share. `repo` is the URL the workspace checks out.
    fn open(&self, workspace: WorkspaceId, instance: &InstanceName, repo: Option<&str>);

    /// Drops the link, as when the container stops.
    fn close(&self, workspace: WorkspaceId);

    /// Waits up to `wait` for the guest to link, and gives its version.
    /// None when this `maxd` doesn't link to guests.
    fn wait_up(
        &self,
        workspace: WorkspaceId,
        wait: Duration,
    ) -> BoxFuture<'_, Result<Option<String>, String>>;

    /// Forgets what the links keep about a deleted workspace, such as its
    /// ports, and tells the session manager, which forgets the agents that
    /// ran there.
    fn forget(&self, _workspace: WorkspaceId) {}
}

/// No guest links, for a device without them.
pub struct NoLinks;

impl GuestLinks for NoLinks {
    fn open(&self, _workspace: WorkspaceId, _instance: &InstanceName, _repo: Option<&str>) {}

    fn close(&self, _workspace: WorkspaceId) {}

    fn wait_up(
        &self,
        _workspace: WorkspaceId,
        _wait: Duration,
    ) -> BoxFuture<'_, Result<Option<String>, String>> {
        async { Ok(None) }.boxed()
    }
}

#[cfg(test)]
mod settling {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use tokio::time::Instant;

    use super::settle;

    #[tokio::test]
    async fn a_hello_waits_for_incus_only_while_the_manager_looks() {
        let wait = Duration::from_millis(400);

        // Nothing looks for Incus, so the answer comes at once.
        let started = Instant::now();
        assert!(!settle(|| false, || None, wait).await);
        assert!(started.elapsed() < Duration::from_millis(100));

        // Incus answers while the hello waits.
        let capable = AtomicBool::new(false);
        let looking = Some(Instant::now());
        let answer = settle(|| capable.load(Ordering::Relaxed), || looking, wait);
        let answered = async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            capable.store(true, Ordering::Relaxed);
        };
        let (capable, ()) = tokio::join!(answer, answered);
        assert!(capable);

        // Incus never answers, so the hello goes out without workspaces.
        let started = Instant::now();
        assert!(!settle(|| false, || Some(started), wait).await);
        assert!(started.elapsed() >= wait);
    }
}
