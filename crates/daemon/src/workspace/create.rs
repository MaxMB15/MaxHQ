// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Making a container workspace. The checks answer the request at once,
//! and the steps after it show in `WorkspaceState::Creating`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use maxhq_core::clone_progress::{self, Lines};
use maxhq_core::names;
use maxhq_protocol::ids::WorkspaceId;
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{
    CloneStage, EgressCheck, GuestState, WorkspaceLocation, WorkspaceSpec, WorkspaceState,
};
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use tracing::{info, warn};

use super::egress::{self, Bridge};
use super::exec::{git_with_token, git_with_token_watched, IncusExec};
use super::keys::{Keys, Record};
use super::manager::{refused, Manager, Op};
use super::probe::Report;
use super::repo::Repo;
use crate::incus::{InstanceName, InstanceSpec, StateAction, PREFIX};
use crate::recipe::{self, Exec, ExecError, ExecUser, StepError, MAXD};

/// The image every workspace starts from.
pub const IMAGE: &str = "images:debian/13";

/// Where the probe's targets go in the container while it runs.
const PROBE_FILE: &str = "/run/maxhq-probe.json";

/// The longest name after `mhq-` that Incus takes.
pub(super) const NAME_MAX: usize = 59;

/// The steps before and after the recipe's.
const STEPS_BEFORE: u32 = 5;
const STEPS_AFTER: u32 = 4;

/// What the checks settled for a create.
pub(super) struct Plan {
    workspace: WorkspaceId,
    name: InstanceName,
    repo: Repo,
    network: String,
    pool: String,
    binary: PathBuf,
    config: BTreeMap<String, String>,
    op: Op,
}

/// Shows each step as it starts.
struct Progress {
    manager: Manager,
    workspace: WorkspaceId,
    next: u32,
    of: u32,
    step: String,
}

impl Progress {
    fn step(&mut self, step: impl Into<String>) {
        self.step = step.into();
        let state = WorkspaceState::Creating {
            step: self.step.clone(),
            done: self.next,
            of: self.of,
        };
        self.next += 1;
        self.manager.set_state(self.workspace, state);
    }

    /// What reads git's stderr during the clone step, and puts how much
    /// it has received after the step's text, as "Cloning owner/name, 42%".
    fn watch_clone(&self) -> impl Fn(&[u8]) + Send + Sync + '_ {
        let heard = Mutex::new((Lines::default(), None));
        move |chunk: &[u8]| {
            let mut heard = heard.lock().unwrap_or_else(PoisonError::into_inner);
            let (lines, shown) = &mut *heard;
            lines.push(chunk, |line| {
                let received = clone_progress::parse(line)
                    .filter(|progress| progress.stage == CloneStage::Receiving)
                    .and_then(|progress| progress.percent);
                if let Some(percent) = received
                    && *shown != Some(percent)
                {
                    *shown = Some(percent);
                    self.show_percent(percent);
                }
            });
        }
    }

    /// Shows the step that runs with `percent` after its text.
    fn show_percent(&self, percent: u8) {
        let state = WorkspaceState::Creating {
            step: with_percent(&self.step, percent),
            done: self.next.saturating_sub(1),
            of: self.of,
        };
        self.manager.set_state(self.workspace, state);
    }
}

/// A step's text with how far along it is.
fn with_percent(step: &str, percent: u8) -> String {
    format!("{step}, {percent}%")
}

impl Manager {
    /// Answers `CreateWorkspace`.
    pub(super) async fn create(&self, spec: WorkspaceSpec) -> ActionOutcome {
        let plan = match self.prepare(&spec).await {
            Ok(plan) => plan,
            Err(outcome) => return outcome,
        };
        let steps = (self.config().steps)(0);
        let of = STEPS_BEFORE + steps.len() as u32 + STEPS_AFTER;
        let keys = match Keys::decode(&plan.config) {
            Ok(keys) => keys,
            Err(error) => return refused(format!("maxd made keys it can't read: {error}")),
        };
        let record = Record {
            instance: plan.name.to_string(),
            keys,
            state: WorkspaceState::Creating {
                step: "Making the container".to_owned(),
                done: 0,
                of,
            },
            guest: GuestState::Installing,
            egress: EgressCheck::NotRun,
            egress_note: None,
            pool: Some(plan.pool.clone()),
            network: Some(plan.network.clone()),
            host_boot: Some(self.inner.host_boot.clone()),
            ssh_host_key: None,
            memory_soft: self.memory_soft(spec.project),
            unsaved: None,
        };
        self.ensure_project(&record.keys);
        self.put(record);
        let workspace = plan.workspace;
        info!(instance = %plan.name, "making a workspace");
        tokio::spawn(self.clone().build(plan, of));
        ActionOutcome::Created { workspace }
    }

    /// The checks that answer the request.
    // The refusal goes straight back to the client, once per request.
    #[allow(clippy::result_large_err)]
    async fn prepare(&self, spec: &WorkspaceSpec) -> Result<Plan, ActionOutcome> {
        let device = self.inner.device;
        if spec.device != device {
            return Err(refused("That workspace is for another device."));
        }
        let incus = self.incus();
        if let Err(error) = incus.ping().await {
            return Err(refused(format!(
                "Incus doesn't answer on this device: {error}"
            )));
        }
        let (network, pool) = self.place().await.map_err(refused)?;
        let caps = &self.config().caps;
        caps.check_parent().map_err(refused)?;
        let repo = self.repo_for(spec).map_err(refused)?;
        let title = match self.config().hub.get(&EntityKey::Project(spec.project)) {
            Some(Entity::Project(project)) => project.name,
            _ => repo.title(device),
        };
        let wanted = spec
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map_or_else(|| slug(&title), str::to_owned);
        let name = InstanceName::new(&format!("{PREFIX}{wanted}")).map_err(|_| {
            refused(
                "Use lower-case letters, digits and hyphens for the name, up to 59 of them, \
                 and don't end it with a hyphen.",
            )
        })?;
        let workspace = names::workspace(
            device,
            &WorkspaceLocation::Container {
                instance: name.to_string(),
                path: repo.dir(),
            },
        );
        let Some(op) = self.begin(workspace) else {
            return Err(match self.record(workspace).map(|r| r.state) {
                Some(WorkspaceState::Creating { .. }) => ActionOutcome::Created { workspace },
                _ => refused(format!(
                    "{name} is already being changed. Wait for it to finish."
                )),
            });
        };
        let taken = self.find_instance(name.as_str()).is_some_and(|other| {
            other != workspace
                || !matches!(
                    self.record(other).map(|r| r.state),
                    Some(WorkspaceState::Failed { .. })
                )
        });
        let exists = match incus.instance(&name).await {
            Ok(found) => found.is_some(),
            Err(error) => {
                return Err(refused(format!(
                    "Incus doesn't answer on this device: {error}"
                )));
            }
        };
        if taken || exists {
            return Err(refused(format!("{name} already exists on this device.")));
        }
        self.config()
            .github
            .mint(&repo.path())
            .await
            .map_err(refused)?;
        let binary = self.guest_binary().map_err(refused)?;
        let keys = Keys {
            project: spec.project,
            device,
            workspace,
            title,
            repo: repo.clone_url(),
            boot: spec.start_at_boot,
            recipe: 0,
            agents: Vec::new(),
            created: Timestamp::now(),
            path: None,
        };
        let mut config = keys.encode();
        config.extend(caps.create_keys(name.as_str(), self.memory_soft(spec.project)));
        Ok(Plan {
            workspace,
            name,
            repo,
            network,
            pool,
            binary,
            config,
            op,
        })
    }

    /// The guest binary this device puts in its containers, once it's there.
    pub(super) fn guest_binary(&self) -> Result<PathBuf, String> {
        let binary = self
            .config()
            .guest_binary
            .clone()
            .or_else(crate::guestlink::guest_binary)
            .ok_or("There's no guest maxd on this device. Install it first.")?;
        if !binary.is_file() {
            return Err(format!(
                "There's no guest maxd at {}. Install it there first.",
                binary.display()
            ));
        }
        Ok(binary)
    }

    /// The network and pool this device's settings name, once they're
    /// there and usable.
    pub(super) async fn place(&self) -> Result<(String, String), String> {
        let incus = self.incus();
        let device = self.config().hub.device().name.clone();
        let settings = self.device_settings();
        let networks = incus.networks().await.map_err(|e| e.to_string())?;
        let pools = incus.pools().await.map_err(|e| e.to_string())?;
        let bridges: Vec<&str> = networks
            .iter()
            .filter(|network| network.is_managed_bridge())
            .map(|network| network.name.as_str())
            .collect();
        let network = settings
            .workspace_network
            .ok_or_else(|| choose("network", &device, &bridges))?;
        if !bridges.contains(&network.as_str()) {
            return Err(format!(
                "The {network} network isn't an Incus bridge on this device."
            ));
        }
        let names: Vec<&str> = pools.iter().map(|pool| pool.name.as_str()).collect();
        let pool = settings
            .workspace_pool
            .ok_or_else(|| choose("pool", &device, &names))?;
        if !names.contains(&pool.as_str()) {
            return Err(format!("There's no {pool} pool on this device."));
        }
        match incus.pool_available(&pool).await {
            Ok(true) => Ok((network, pool)),
            Ok(false) => Err(format!(
                "The {pool} pool isn't available. Its storage may be locked."
            )),
            Err(error) => Err(format!("Incus couldn't check the {pool} pool: {error}")),
        }
    }

    /// The project's GitHub repo: its remote, or one the GitHub App reaches
    /// whose project it is.
    fn repo_for(&self, spec: &WorkspaceSpec) -> Result<Repo, String> {
        if let Some(Entity::Project(project)) =
            self.config().hub.get(&EntityKey::Project(spec.project))
        {
            return match project.repo.as_deref().and_then(Repo::parse) {
                Some(repo) => Ok(repo),
                None => Err(format!(
                    "{} has no GitHub remote, so it can't have a container.",
                    project.name
                )),
            };
        }
        self.app_repo(spec.project)
            .ok_or_else(|| "maxd doesn't know that project's repository on this device.".to_owned())
    }

    /// Runs the steps after the answer, and marks the workspace Failed at
    /// the first that fails. The container stays for a look.
    async fn build(self, plan: Plan, of: u32) {
        let mut progress = Progress {
            manager: self.clone(),
            workspace: plan.workspace,
            next: 0,
            of,
            step: String::new(),
        };
        let result = self.build_steps(&plan, &mut progress).await;
        let workspace = plan.workspace;
        match result {
            Ok(()) => info!(instance = %plan.name, "the workspace is ready"),
            Err(error) => {
                warn!(instance = %plan.name, step = %progress.step, %error, "a create failed");
                self.config().links.close(workspace);
                self.set_state(
                    workspace,
                    WorkspaceState::Failed {
                        reason: format!("{} failed: {error}", progress.step),
                    },
                );
            }
        }
        drop(plan.op);
    }

    async fn build_steps(&self, plan: &Plan, progress: &mut Progress) -> Result<(), String> {
        let incus = self.incus();
        let timing = self.config().timing;
        let name = &plan.name;
        let workspace = plan.workspace;
        let socket = self.socket(name);

        progress.step("Making the container");
        self.ensure_guests_dir().map_err(|e| e.to_string())?;
        let spec = InstanceSpec {
            image: IMAGE.to_owned(),
            config: plan.config.clone(),
            pool: plan.pool.clone(),
            network: plan.network.clone(),
            guest_socket: socket.clone(),
        };
        incus.create(name, spec).await.map_err(|e| e.to_string())?;

        progress.step("Starting the container");
        incus
            .set_state(name, StateAction::Start, timing.state_timeout)
            .await
            .map_err(|e| e.to_string())?;

        progress.step("Waiting for systemd and DNS");
        let exec = IncusExec::new(incus.clone(), name.clone());
        self.wait_ready(&exec).await?;

        progress.step("Putting maxd in the container");
        super::install::install(incus, name, &plan.binary)
            .await
            .map_err(|e| e.to_string())?;

        progress.step("Checking egress");
        let check = self.check_egress(workspace, &exec, &plan.network).await;
        if let Some(problem) = self
            .record(workspace)
            .and_then(|r| egress_problem(r.egress_note.as_deref(), &check))
        {
            return Err(problem);
        }

        for step in (self.config().steps)(0) {
            progress.step(format!("Recipe step {}", step.id));
            recipe::ensure(&exec, &step).await.map_err(step_error)?;
        }

        progress.step("Waiting for the guest");
        let guest = self.link_guest(workspace, name).await?;
        self.set_guest(workspace, guest);

        progress.step(format!("Cloning {}", plan.repo.path()));
        let token = self.config().github.mint(&plan.repo.path()).await?;
        let dir = plan.repo.dir();
        let git_dir = format!("{dir}/.git");
        let cloned = exec
            .run(&["test", "-d", &git_dir], ExecUser::Agent, None)
            .await
            .map_err(|e| e.to_string())?
            .success();
        if !cloned {
            let url = plan.repo.clone_url();
            let watch = progress.watch_clone();
            let args = ["clone", "--progress", "--", &url, &dir];
            let output = git_with_token_watched(&exec, &args, &token, &watch)
                .await
                .map_err(|e| e.to_string())?;
            if !output.success() {
                return Err(ExecError::failed(&["git", "clone", &url], &output).to_string());
            }
        }
        if let Some(record) = self.record(workspace) {
            self.apply_author(&record).await?;
        }

        progress.step("Checking that pushes work");
        let has_head = exec
            .run(
                &["git", "-C", &dir, "rev-parse", "--verify", "-q", "HEAD"],
                ExecUser::Agent,
                None,
            )
            .await
            .map_err(|e| e.to_string())?
            .success();
        if has_head {
            let push = ["-C", dir.as_str(), "push", "--dry-run", "origin", "HEAD"];
            let output = git_with_token(&exec, &push, &token)
                .await
                .map_err(|e| e.to_string())?;
            if !output.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                return Err(format!(
                    "git push --dry-run didn't work, so agents couldn't push. Check that the \
                     GitHub App may write to {}. Git said: {stderr}",
                    plan.repo.path()
                ));
            }
        }
        drop(token);

        progress.step("Saving the workspace");
        let Some(mut keys) = self.record(workspace).map(|r| r.keys) else {
            return Err("its record is gone".to_owned());
        };
        keys.recipe = self.config().recipe;
        incus
            .set_config(name, keys.encode())
            .await
            .map_err(|e| e.to_string())?;
        let host_boot = self.inner.host_boot.clone();
        self.update(workspace, |record| {
            record.keys = keys;
            record.state = WorkspaceState::Running;
            record.host_boot = Some(host_boot);
        });
        Ok(())
    }

    /// Waits until systemd finished booting and DNS answers, per report
    /// 01's lessons.
    pub(super) async fn wait_ready(&self, exec: &IncusExec) -> Result<(), String> {
        let timing = self.config().timing;
        let tries = timing.ready_tries.max(1);
        let mut last = String::new();
        for i in 0..tries {
            let ready = tokio::time::timeout(timing.state_timeout, ready(exec)).await;
            match ready {
                Ok(Ok(())) => return Ok(()),
                Ok(Err(why)) => last = why,
                Err(_) => last = "systemd didn't answer in time".to_owned(),
            }
            if i + 1 < tries {
                tokio::time::sleep(timing.ready_pause).await;
            }
        }
        Err(format!("the container didn't come up: {last}"))
    }

    /// Runs the egress probe in the container and keeps what it found.
    pub(super) async fn check_egress(
        &self,
        workspace: WorkspaceId,
        exec: &IncusExec,
        network: &str,
    ) -> EgressCheck {
        let (check, note) = self.egress(exec, network).await;
        if let EgressCheck::Failed { reached, .. } = &check {
            warn!(%workspace, ?reached, ?note, "the egress check failed");
        }
        self.update(workspace, |record| {
            record.egress = check.clone();
            record.egress_note = note;
        });
        check
    }

    /// What the egress probe found in the container, with a note on why
    /// it failed when it says.
    pub(super) async fn egress(
        &self,
        exec: &IncusExec,
        network: &str,
    ) -> (EgressCheck, Option<String>) {
        let now = Timestamp::now();
        match self.probe(exec, network).await {
            Ok(verdict) => verdict.check(now),
            Err(error) => (
                EgressCheck::Failed {
                    at: now,
                    reached: Vec::new(),
                },
                Some(format!("the probe didn't run: {error}")),
            ),
        }
    }

    async fn probe(&self, exec: &IncusExec, network: &str) -> Result<egress::Verdict, String> {
        let networks = self.incus().networks().await.map_err(|e| e.to_string())?;
        let bridges: Vec<Bridge> = networks.iter().filter_map(Bridge::from_network).collect();
        let facts = (self.config().host_facts)();
        let targets = egress::targets(&facts, &bridges, network);
        let json = serde_json::to_vec(&targets).map_err(|e| e.to_string())?;
        exec.write(PROBE_FILE, &json, 0o600, ExecUser::Root)
            .await
            .map_err(|e| e.to_string())?;
        let cmd = [MAXD, "guest", "probe", PROBE_FILE];
        let output = exec.run(&cmd, ExecUser::Root, None).await;
        let _ = exec
            .run(&["rm", "-f", PROBE_FILE], ExecUser::Root, None)
            .await;
        let output = output.map_err(|e| e.to_string())?;
        if !output.success() {
            return Err(ExecError::failed(&cmd, &output).to_string());
        }
        let report: Report = serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("its report doesn't read: {e}"))?;
        Ok(egress::judge(&targets, &report))
    }

    /// Opens the guest link and waits for it.
    pub(super) async fn link_guest(
        &self,
        workspace: WorkspaceId,
        name: &InstanceName,
    ) -> Result<GuestState, String> {
        let links = &self.config().links;
        let repo = self.record(workspace).map(|record| record.keys.repo);
        links.open(workspace, name, repo.as_deref());
        match links
            .wait_up(workspace, self.config().timing.guest_wait)
            .await?
        {
            Some(version) => Ok(GuestState::Connected { version }),
            None => Ok(GuestState::Down {
                reason: "this maxd doesn't link to guests yet".to_owned(),
                since: Timestamp::now(),
            }),
        }
    }
}

/// One check that the container is up.
async fn ready(exec: &IncusExec) -> Result<(), String> {
    let output = exec
        .run(
            &["systemctl", "is-system-running", "--wait"],
            ExecUser::Root,
            None,
        )
        .await
        .map_err(|e| e.to_string())?;
    let state = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if state != "running" && state != "degraded" {
        return Err(format!("systemd says {state:?}"));
    }
    let output = exec
        .run(&["getent", "hosts", "deb.debian.org"], ExecUser::Root, None)
        .await
        .map_err(|e| e.to_string())?;
    if !output.success() {
        return Err("DNS doesn't answer yet".to_owned());
    }
    Ok(())
}

/// What a failed egress check found, in words, with the check's `note`, or
/// None when it passed.
pub(super) fn egress_problem(note: Option<&str>, check: &EgressCheck) -> Option<String> {
    match check {
        EgressCheck::Passed { .. } => None,
        EgressCheck::NotRun => Some("the egress check didn't run".to_owned()),
        EgressCheck::Failed { reached, .. } if reached.is_empty() => {
            Some(note.unwrap_or("the probe gave no answer").to_owned())
        }
        EgressCheck::Failed { reached, .. } => Some(format!(
            "the container reached {}, which it shouldn't",
            reached.join(", ")
        )),
    }
}

/// Asks for a network or pool in the device's settings, naming what
/// Incus offers.
fn choose(what: &str, device: &str, offered: &[&str]) -> String {
    match offered {
        [] => format!("{device}'s Incus has no workspace {what} to offer."),
        _ => format!(
            "Pick a workspace {what} for {device} in its settings first. It offers {}.",
            offered.join(", ")
        ),
    }
}

/// A recipe step's error without the step's ID, which the state names.
pub(super) fn step_error(error: StepError) -> String {
    match error {
        StepError::Failed { error, .. } => error.to_string(),
        StepError::NotDone { .. } => "it still isn't done after it applied".to_owned(),
    }
}

/// A container name from a project's name: lower case, with each run of
/// other characters as one hyphen.
pub(super) fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.truncate(NAME_MAX);
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("workspace");
    }
    out
}
