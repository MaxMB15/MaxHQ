// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Start, stop and delete, the updates in place on start, A-11's starts at
//! boot, and keeping up with what Incus says happened.

use std::collections::BTreeMap;

use futures::StreamExt;
use maxhq_protocol::ids::{ProjectId, WorkspaceId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{GuestState, WorkspaceState};
use maxhq_protocol::time::Timestamp;
use tokio::sync::oneshot;
use tracing::{info, warn};

use super::create::egress_problem;
use super::exec::{IncusExec, Unpushed, UNPUSHED_SCRIPT};
use super::keys::key;
use super::manager::{refused, Manager, Op};
use crate::incus::{Instance, InstanceName, Lifecycle, StateAction};
use crate::recipe::{self, Exec, ExecUser};

/// The answer for a workspace this device doesn't have.
pub(super) const UNKNOWN: &str = "There's no workspace like that on this device.";

/// The exit status [`UNPUSHED_SCRIPT`] uses when the repo isn't there.
const NO_REPO: i32 = 3;

/// Why a start didn't finish.
#[derive(Debug)]
pub(super) enum Trouble {
    /// Something that may clear, such as locked storage. A-11 tries again.
    Retry(String),
    /// Something a person has to look at.
    Failed(String),
}

impl Manager {
    /// Answers `StartWorkspace`. A start sent while a stop runs waits for
    /// it to finish.
    pub(super) async fn start(&self, workspace: WorkspaceId) -> ActionOutcome {
        let Some(record) = self.record(workspace) else {
            return refused(UNKNOWN);
        };
        if record.keys.recipe == 0 {
            return refused(format!(
                "{} didn't finish being made. Delete it and make it again.",
                record.instance
            ));
        }
        if record.state != WorkspaceState::Running
            && let Err(reason) = self.config().caps.check_parent()
        {
            return refused(reason);
        }
        let Some(op) = self.begin(workspace) else {
            if record.state != WorkspaceState::Stopping {
                return refused(busy(&record.instance));
            }
            let manager = self.clone();
            tokio::spawn(async move {
                let op = manager.begin_after(workspace).await;
                manager.start_with(workspace, op);
            });
            return ActionOutcome::Done;
        };
        self.start_with(workspace, op);
        ActionOutcome::Done
    }

    /// Starts a workspace that isn't running, holding `op` until it's up.
    fn start_with(&self, workspace: WorkspaceId, op: Op) {
        let Some(record) = self.record(workspace) else {
            return;
        };
        if record.state == WorkspaceState::Running {
            return;
        }
        self.set_state(workspace, WorkspaceState::Starting);
        let manager = self.clone();
        tokio::spawn(async move {
            if let Err(trouble) = manager.bring_up(workspace, true).await {
                warn!(%workspace, ?trouble, "a start didn't finish");
            }
            drop(op);
        });
    }

    /// Answers `StopWorkspace` once the container has stopped, or after
    /// [`Timing::stop_answer`](super::Timing) while it shows Stopping.
    pub(super) async fn stop(&self, workspace: WorkspaceId) -> ActionOutcome {
        let Some(record) = self.record(workspace) else {
            return refused(UNKNOWN);
        };
        let Some(op) = self.begin(workspace) else {
            return refused(busy(&record.instance));
        };
        if matches!(
            record.state,
            WorkspaceState::Stopped | WorkspaceState::StorageLocked
        ) {
            return ActionOutcome::Done;
        }
        let Ok(name) = InstanceName::new(&record.instance) else {
            return refused(UNKNOWN);
        };
        self.set_state(workspace, WorkspaceState::Stopping);
        let (done, stopped) = oneshot::channel();
        let manager = self.clone();
        tokio::spawn(async move {
            let failed = match manager.halt(workspace, &name).await {
                Ok(()) => {
                    manager.mark_stopped(workspace);
                    None
                }
                Err(error) => {
                    let reason = format!("Stopping it failed: {error}");
                    manager.set_state(
                        workspace,
                        WorkspaceState::Failed {
                            reason: reason.clone(),
                        },
                    );
                    Some(reason)
                }
            };
            // Let go first, so a start sent on the answer finds it free.
            drop(op);
            let _ = done.send(failed);
        });
        let wait = self.config().timing.stop_answer;
        match tokio::time::timeout(wait, stopped).await {
            Ok(Ok(Some(reason))) => refused(reason),
            _ => ActionOutcome::Done,
        }
    }

    /// Answers `DeleteWorkspace`.
    pub(super) async fn delete(&self, workspace: WorkspaceId, discard: bool) -> ActionOutcome {
        let Some(record) = self.record(workspace) else {
            return refused(UNKNOWN);
        };
        let instance = record.instance.clone();
        let Some(op) = self.begin(workspace) else {
            return refused(busy(&instance));
        };
        let Ok(name) = InstanceName::new(&instance) else {
            return refused(UNKNOWN);
        };
        let found = match self.incus().instance(&name).await {
            Ok(found) => found,
            Err(error) => {
                return refused(format!(
                    "Incus doesn't answer, so {instance} can't be deleted now: {error}"
                ));
            }
        };
        let Some(found) = found else {
            self.config().links.close(workspace);
            self.remove_socket(&name);
            self.forget(workspace);
            return ActionOutcome::Done;
        };
        if !discard && let Some(reason) = self.unpushed(&record, &found, &name).await {
            return refused(reason);
        }
        self.set_state(workspace, WorkspaceState::Deleting);
        info!(%instance, "deleting a workspace");
        let manager = self.clone();
        tokio::spawn(async move {
            let result = async {
                manager.halt(workspace, &name).await?;
                manager.incus().delete(&name).await
            }
            .await;
            match result {
                Ok(()) => {
                    manager.remove_socket(&name);
                    manager.forget(workspace);
                }
                Err(error) => manager.set_state(
                    workspace,
                    WorkspaceState::Failed {
                        reason: format!("Deleting it failed: {error}"),
                    },
                ),
            }
            drop(op);
        });
        ActionOutcome::Done
    }

    /// Why a delete would lose work, in words the person can act on, or
    /// None when it wouldn't.
    async fn unpushed(
        &self,
        record: &super::keys::Record,
        found: &Instance,
        name: &InstanceName,
    ) -> Option<String> {
        let instance = &record.instance;
        if !self.busy_agents(record).is_empty() {
            return Some(format!(
                "{instance} has agents at work. Stop them first, or delete anyway to stop them \
                 and lose their work."
            ));
        }
        if !found.is_running() {
            return Some(format!(
                "{instance} is stopped, so maxd can't check it for work that isn't pushed. \
                 Start it to check, or delete anyway to lose any work that isn't pushed."
            ));
        }
        let exec = IncusExec::new(self.incus().clone(), name.clone());
        let dir = record.repo_dir();
        let cmd = ["sh", "-c", UNPUSHED_SCRIPT, "sh", &dir];
        match exec.run(&cmd, ExecUser::Agent, None).await {
            Ok(output) if output.success() => {
                let unpushed = Unpushed::parse(&output.stdout);
                (!unpushed.is_empty()).then(|| unpushed.reason(instance))
            }
            Ok(output) if output.status == NO_REPO => None,
            Ok(output) => Some(format!(
                "maxd couldn't check {instance} for work that isn't pushed: {}. Delete anyway \
                 to lose any work that isn't pushed.",
                String::from_utf8_lossy(&output.stderr).trim()
            )),
            Err(error) => Some(format!(
                "maxd couldn't check {instance} for work that isn't pushed: {error}. Delete \
                 anyway to lose any work that isn't pushed."
            )),
        }
    }

    /// Stops an instance that runs, forcing it when a clean stop fails.
    pub(super) async fn halt(
        &self,
        workspace: WorkspaceId,
        name: &InstanceName,
    ) -> Result<(), crate::incus::IncusError> {
        self.config().links.close(workspace);
        let incus = self.incus();
        match incus.instance(name).await? {
            Some(instance) if instance.is_running() => {}
            _ => return Ok(()),
        }
        let timeout = self.config().timing.state_timeout;
        if let Err(error) = incus.set_state(name, StateAction::Stop, timeout).await {
            warn!(instance = %name, %error, "a clean stop failed, so forcing it");
            incus.force_stop(name).await?;
        }
        Ok(())
    }

    pub(super) fn mark_stopped(&self, workspace: WorkspaceId) {
        self.update(workspace, |record| {
            record.state = WorkspaceState::Stopped;
            record.guest = GuestState::Down {
                reason: "the container stopped".to_owned(),
                since: Timestamp::now(),
            };
        });
    }

    /// Starts a workspace's container when `start` says so and it isn't
    /// running, then waits for it, checks egress, updates it in place, and
    /// links the guest. Callers hold the workspace's [`Op`](super::manager::Op).
    /// Checks the parent cgroup a stopped container is about to start
    /// under, and writes the caps keys it doesn't have yet, so they take
    /// effect at this start. Only a stopped container comes here, so
    /// nothing restarts to apply them.
    async fn apply_caps(
        &self,
        workspace: WorkspaceId,
        project: ProjectId,
        name: &InstanceName,
        instance: &Instance,
    ) -> Result<(), String> {
        let caps = &self.config().caps;
        caps.check_parent()?;
        let memory = self.memory_soft(project);
        let changes = caps.changes(instance, memory);
        if !changes.is_empty() {
            info!(instance = %name, "setting its caps for this start");
            self.incus()
                .set_config(name, changes)
                .await
                .map_err(|error| format!("Incus couldn't set its caps: {error}"))?;
        }
        self.update(workspace, |record| record.memory_soft = memory);
        Ok(())
    }

    pub(super) async fn bring_up(
        &self,
        workspace: WorkspaceId,
        start: bool,
    ) -> Result<(), Trouble> {
        let Some(record) = self.record(workspace) else {
            return Err(Trouble::Failed("it's gone".to_owned()));
        };
        if record.keys.recipe == 0 {
            return Err(Trouble::Failed("its create didn't finish".to_owned()));
        }
        let name = InstanceName::new(&record.instance)
            .map_err(|error| Trouble::Failed(error.to_string()))?;
        let incus = self.incus();
        let fail = |reason: String| {
            self.set_state(
                workspace,
                WorkspaceState::Failed {
                    reason: reason.clone(),
                },
            );
            Trouble::Failed(reason)
        };
        let instance = match incus.instance(&name).await {
            Ok(Some(instance)) => instance,
            Ok(None) => return Err(fail(format!("{name} is gone from Incus."))),
            Err(error) => return Err(Trouble::Retry(error.to_string())),
        };
        if !instance.is_running() {
            if !start {
                self.mark_stopped(workspace);
                return Ok(());
            }
            let pool = instance.pool().map(str::to_owned).or(record.pool.clone());
            if let Some(pool) = &pool
                && !incus.pool_available(pool).await.unwrap_or(true)
            {
                self.set_state(workspace, WorkspaceState::StorageLocked);
                return Err(Trouble::Retry(format!("the {pool} pool isn't available")));
            }
            self.apply_caps(workspace, record.keys.project, &name, &instance)
                .await
                .map_err(fail)?;
            self.set_state(workspace, WorkspaceState::Starting);
            self.ensure_guests_dir()
                .map_err(|error| fail(format!("maxd couldn't make its socket folder: {error}")))?;
            self.remove_socket(&name);
            let timeout = self.config().timing.state_timeout;
            if let Err(error) = incus.set_state(&name, StateAction::Start, timeout).await {
                if let Some(pool) = &pool
                    && !incus.pool_available(pool).await.unwrap_or(true)
                {
                    self.set_state(workspace, WorkspaceState::StorageLocked);
                    return Err(Trouble::Retry(error.to_string()));
                }
                return Err(fail(format!("Incus couldn't start it: {error}")));
            }
        } else {
            self.set_state(workspace, WorkspaceState::Starting);
        }

        let exec = IncusExec::new(incus.clone(), name.clone());
        self.wait_ready(&exec).await.map_err(fail)?;
        let network = instance
            .network()
            .map(str::to_owned)
            .or(record.network.clone())
            .unwrap_or_default();
        let check = self.check_egress(workspace, &exec, &network).await;
        if let Some(record) = self.record(workspace)
            && let Some(problem) = egress_problem(record.egress_note.as_deref(), &check)
        {
            warn!(%workspace, %problem, "new agents stay off until the egress check passes, and agents already held come back");
        }
        self.update_in_place(workspace, &exec).await.map_err(fail)?;
        let guest = match self.link_guest(workspace, &name).await {
            Ok(guest) => guest,
            Err(reason) => GuestState::Down {
                reason,
                since: Timestamp::now(),
            },
        };
        let host_boot = self.inner.host_boot.clone();
        self.update(workspace, |record| {
            record.guest = guest;
            record.state = WorkspaceState::Running;
            record.host_boot = Some(host_boot);
        });
        Ok(())
    }

    /// Runs the recipe steps newer than the container's, when no agent in
    /// it is at work. Never recreates it.
    async fn update_in_place(
        &self,
        workspace: WorkspaceId,
        exec: &IncusExec,
    ) -> Result<(), String> {
        let Some(record) = self.record(workspace) else {
            return Ok(());
        };
        let done = record.keys.recipe;
        let target = self.config().recipe;
        if done >= target {
            return Ok(());
        }
        if !self.busy_agents(&record).is_empty() {
            info!(instance = %record.instance, "an update waits until no agent is at work");
            return Ok(());
        }
        info!(instance = %record.instance, from = done, to = target, "updating a workspace");
        let steps = (self.config().steps)(done);
        recipe::run(exec, &steps)
            .await
            .map_err(|error| format!("Updating it failed: {error}"))?;
        let keys = BTreeMap::from([(key::RECIPE.to_owned(), target.to_string())]);
        self.incus()
            .set_config(exec.name(), keys)
            .await
            .map_err(|error| format!("Updating it failed: {error}"))?;
        self.update(workspace, |record| record.keys.recipe = target);
        Ok(())
    }

    /// A-11: brings up the containers that were running, then starts the
    /// ones marked for boot that haven't run in this host boot, two at a
    /// time, trying again every [`Timing::retry`](super::Timing).
    pub(super) async fn boot(self, running: Vec<WorkspaceId>) {
        for workspace in &running {
            let Some(op) = self.begin(*workspace) else {
                continue;
            };
            let manager = self.clone();
            let workspace = *workspace;
            tokio::spawn(async move {
                if let Err(trouble) = manager.bring_up(workspace, false).await {
                    warn!(%workspace, ?trouble, "a running workspace didn't come up");
                }
                drop(op);
            });
        }
        let host_boot = self.inner.host_boot.clone();
        let mut waiting: Vec<WorkspaceId> = self
            .shared()
            .records
            .values()
            .filter(|record| {
                record.keys.boot
                    && record.keys.recipe > 0
                    && record.host_boot.as_deref() != Some(host_boot.as_str())
                    && !running.contains(&record.keys.workspace)
            })
            .map(|record| record.keys.workspace)
            .collect();
        if !waiting.is_empty() {
            info!(count = waiting.len(), "starting workspaces for this boot");
        }
        while !waiting.is_empty() {
            let manager = &self;
            waiting = futures::stream::iter(waiting)
                .map(|workspace| async move { (workspace, manager.boot_one(workspace).await) })
                .buffer_unordered(2)
                .filter_map(|(workspace, retry)| async move { retry.then_some(workspace) })
                .collect()
                .await;
            if !waiting.is_empty() {
                tokio::time::sleep(self.config().timing.retry).await;
            }
        }
    }

    /// Starts one workspace for this boot. True when it should try again.
    async fn boot_one(&self, workspace: WorkspaceId) -> bool {
        let Some(record) = self.record(workspace) else {
            return false;
        };
        if record.host_boot.as_deref() == Some(self.inner.host_boot.as_str()) {
            return false;
        }
        let Some(op) = self.begin(workspace) else {
            return true;
        };
        if self.incus().ping().await.is_err() {
            return true;
        }
        let result = self.bring_up(workspace, true).await;
        drop(op);
        match result {
            Ok(()) => false,
            Err(Trouble::Retry(why)) => {
                info!(instance = %record.instance, %why, "a boot start waits");
                true
            }
            Err(Trouble::Failed(why)) => {
                warn!(instance = %record.instance, %why, "a boot start failed");
                false
            }
        }
    }

    /// Follows Incus's lifecycle events for MaxHQ's instances.
    pub(super) async fn watch_events(self) {
        let mut events = self.incus().lifecycle();
        while let Some(event) = events.next().await {
            self.on_event(event);
        }
    }

    fn on_event(&self, event: Lifecycle) {
        let Some(workspace) = self.find_instance(&event.instance) else {
            return;
        };
        if self.changing(workspace) {
            return;
        }
        match event.action.as_str() {
            "instance-started" | "instance-restarted" => self.spawn_bring_up(workspace),
            "instance-stopped" | "instance-shutdown" => {
                self.config().links.close(workspace);
                self.mark_stopped(workspace);
            }
            "instance-deleted" => {
                self.config().links.close(workspace);
                if let Ok(name) = InstanceName::new(&event.instance) {
                    self.remove_socket(&name);
                }
                self.forget(workspace);
            }
            _ => {}
        }
    }

    /// Checks and links a container that started outside a request.
    fn spawn_bring_up(&self, workspace: WorkspaceId) {
        let Some(op) = self.begin(workspace) else {
            return;
        };
        let manager = self.clone();
        tokio::spawn(async move {
            if let Err(trouble) = manager.bring_up(workspace, false).await {
                warn!(%workspace, ?trouble, "a started workspace didn't come up");
            }
            drop(op);
        });
    }

    /// Catches up with changes whose events went missing.
    pub(super) async fn resync(&self) {
        let instances = match self.incus().instances().await {
            Ok(instances) => instances,
            Err(error) => {
                warn!(%error, "couldn't list the instances");
                return;
            }
        };
        let by_name: BTreeMap<&str, &Instance> = instances
            .iter()
            .map(|instance| (instance.name.as_str(), instance))
            .collect();
        let records: Vec<_> = self.shared().records.values().cloned().collect();
        for record in records {
            let workspace = record.keys.workspace;
            if self.changing(workspace) || record.keys.recipe == 0 {
                continue;
            }
            match by_name.get(record.instance.as_str()) {
                None => {
                    if !matches!(record.state, WorkspaceState::Failed { .. }) {
                        self.config().links.close(workspace);
                        self.forget(workspace);
                    }
                }
                Some(instance) if instance.is_running() => {
                    if matches!(
                        record.state,
                        WorkspaceState::Stopped
                            | WorkspaceState::StorageLocked
                            | WorkspaceState::Stopping
                            | WorkspaceState::Starting
                    ) {
                        self.spawn_bring_up(workspace);
                    }
                }
                Some(instance) => match record.state {
                    WorkspaceState::Running | WorkspaceState::Starting => {
                        self.config().links.close(workspace);
                        self.mark_stopped(workspace);
                    }
                    WorkspaceState::StorageLocked => {
                        let pool = instance.pool().map(str::to_owned).or(record.pool.clone());
                        let available = match pool {
                            Some(pool) => self.incus().pool_available(&pool).await.unwrap_or(false),
                            None => true,
                        };
                        if available {
                            self.set_state(workspace, WorkspaceState::Stopped);
                        }
                    }
                    _ => {}
                },
            }
        }
        self.publish_host().await;
    }
}

/// The answer while another change to the workspace runs.
pub(super) fn busy(instance: &str) -> String {
    format!("{instance} is already being changed. Wait for it to finish.")
}
