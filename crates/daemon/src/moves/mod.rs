// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Moves of `ws-` containers the old scripts made into `mhq-` ones MaxHQ
//! runs, one step you approve at a time: look, copy, converge, switch,
//! keep with a move back, and retire.
//!
//! The look only reads, through the observer's client. Every write to the
//! original goes through the guarded calls in [`crate::incus::moves`],
//! which take this module's [`Record`] and check the step you approved.
//! Nothing ever runs a command in the original. The copy is MaxHQ's from
//! the start, and [`Host`] sets it up and resumes its agent.

mod plan;
mod record;
mod steps;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures::future::BoxFuture;
use maxhq_protocol::hosts::{MoveState, MoveStep};
use maxhq_protocol::ids::{DeviceId, MoveId, WorkspaceId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::Vendor;
use maxhq_protocol::state::{Change, Entity};
use maxhq_protocol::time::Timestamp;
use tracing::warn;

use crate::db::Db;
use crate::hub::Hub;
use crate::incus::moves::{is_old_name, MOVE_SNAPSHOT};
use crate::incus::{Incus, InstanceName, PREFIX};
use crate::observe::incus::look::Look;

pub use plan::{claude_folder, copy_path, top_folder, vendor_name};
pub use record::{Approval, Record};

/// How long the stopped original is kept after the switch.
pub const KEEP: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Where the old scripts leave a flag for a container their resume brings
/// back after the reboot.
pub const RESUME_FLAGS: &str = "/var/lib/agent-workspaces/resume";

/// Why a request refuses on a `maxd` that runs no moves.
pub const NO_MOVES: &str = "This device doesn't run container workspaces, so nothing moves here.";

/// How often a keep that ended moves on to the retire.
const KEEP_CHECK: Duration = Duration::from_secs(10 * 60);

/// What a copy is made with, from the device's workspace settings.
pub struct CopySetup {
    /// The copy's workspace.
    pub workspace: WorkspaceId,
    /// Its keys, MaxHQ's own.
    pub config: BTreeMap<String, String>,
    /// The network its NIC is on.
    pub network: String,
    /// The host socket its guest proxy listens on.
    pub guest_socket: PathBuf,
}

/// The agent the switch resumes in the copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resume {
    /// The copy's workspace.
    pub workspace: WorkspaceId,
    /// Claude or Codex.
    pub vendor: Vendor,
    /// The session or thread.
    pub session: String,
    /// The folder in the copy, which is the repo's main checkout.
    pub cwd: String,
}

/// What a move asks of the rest of `maxd` for the copy, which is MaxHQ's
/// own. None of it touches the original.
pub trait Host: Send + Sync + 'static {
    /// The keys, network and guest socket a copy starts with.
    fn copy_setup<'a>(&'a self, record: &'a Record) -> BoxFuture<'a, Result<CopySetup, String>>;
    /// Sets the copy up the way MaxHQ runs its own and starts it, with no
    /// agent.
    fn converge<'a>(&'a self, record: &'a Record) -> BoxFuture<'a, Result<(), String>>;
    /// Deletes a copy made before, for a copy again.
    fn drop_copy<'a>(&'a self, record: &'a Record) -> BoxFuture<'a, Result<(), String>>;
    /// The mode and account a resumed agent of `vendor` runs with, in words
    /// for the plan.
    fn runs_with(&self, vendor: Vendor) -> String;
    /// Resumes the original's agent in the copy, under its holder.
    fn resume(&self, resume: Resume) -> BoxFuture<'_, Result<(), String>>;
    /// Stops the agents in the copy.
    fn stop_agents(&self, workspace: WorkspaceId) -> BoxFuture<'_, Result<(), String>>;
    /// The copy's unsaved work, in words, as its guest last reported it.
    fn unsaved(&self, workspace: WorkspaceId) -> BoxFuture<'_, Vec<String>>;
}

/// What a [`Mover`] runs with.
pub struct Config {
    pub incus: Incus,
    pub hub: Hub,
    pub db: Db,
    pub device: DeviceId,
    pub host: Arc<dyn Host>,
    /// The old scripts' resume flags.
    pub resume_flags: PathBuf,
    /// The host's uptime, for when the old reboot fires.
    pub uptime: PathBuf,
    /// How long a stop or a start may take.
    pub state_timeout: Duration,
    /// The time now.
    pub now: Arc<dyn Fn() -> Timestamp + Send + Sync>,
}

/// This device's moves.
#[derive(Clone)]
pub struct Mover {
    inner: Arc<Inner>,
}

struct Inner {
    config: Config,
    records: Mutex<BTreeMap<MoveId, Record>>,
}

static MOVER: OnceLock<Mover> = OnceLock::new();

/// Makes `mover` the one the app's requests reach, and moves a keep that
/// ended on to the retire now and then.
pub fn install(mover: Mover) {
    if MOVER.set(mover.clone()).is_ok() {
        crate::spawn_task("the workspace moves", async move {
            let mut tick = tokio::time::interval(KEEP_CHECK);
            loop {
                tick.tick().await;
                mover.keep_ended();
            }
        });
    }
}

/// Answers `PlanMove`.
pub async fn plan(instance: String) -> ActionOutcome {
    match MOVER.get() {
        Some(mover) => mover.plan(&instance),
        None => refused(NO_MOVES),
    }
}

/// Answers `RunMoveStep`.
pub async fn run_step(moving: MoveId, step: MoveStep) -> ActionOutcome {
    match MOVER.get() {
        Some(mover) => mover.run_step(moving, step).await,
        None => refused(NO_MOVES),
    }
}

/// Answers `MoveBack`.
pub async fn move_back(moving: MoveId) -> ActionOutcome {
    match MOVER.get() {
        Some(mover) => mover.move_back(moving).await,
        None => refused(NO_MOVES),
    }
}

/// Answers `RetireMove`.
pub async fn retire(moving: MoveId) -> ActionOutcome {
    match MOVER.get() {
        Some(mover) => mover.retire(moving).await,
        None => refused(NO_MOVES),
    }
}

fn refused(reason: impl Into<String>) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.into(),
    }
}

/// A step's name, as a sentence starts it.
fn step_name(step: MoveStep) -> &'static str {
    match step {
        MoveStep::Look => "The look",
        MoveStep::Copy => "The copy",
        MoveStep::Converge => "The setup",
        MoveStep::Switch => "The switch",
        MoveStep::Keep => "The keep",
        MoveStep::Retire => "The retire",
    }
}

fn order(step: MoveStep) -> usize {
    match step {
        MoveStep::Look => 0,
        MoveStep::Copy => 1,
        MoveStep::Converge => 2,
        MoveStep::Switch => 3,
        MoveStep::Keep => 4,
        MoveStep::Retire => 5,
    }
}

/// The copy's name for an original: `ws-alpha` becomes `mhq-alpha`.
pub fn target_for(source: &str) -> Option<InstanceName> {
    let rest = source.strip_prefix(crate::incus::moves::OLD_PREFIX)?;
    InstanceName::new(&format!("{PREFIX}{rest}")).ok()
}

impl Mover {
    /// Opens the moves `config.db` keeps and publishes them.
    pub fn open(config: Config) -> Self {
        let records: Vec<Record> = config.db.moves().unwrap_or_else(|error| {
            warn!(%error, "couldn't read the workspace moves");
            Vec::new()
        });
        let mut kept = BTreeMap::new();
        for mut record in records {
            // A step that was running when maxd stopped didn't finish.
            if let MoveState::Running { doing } = &record.state {
                record.state = MoveState::Failed {
                    reason: format!("maxd stopped while it was {}", lower(doing)),
                };
            }
            record.approved = None;
            kept.insert(record.id, record);
        }
        let mover = Self {
            inner: Arc::new(Inner {
                config,
                records: Mutex::new(kept),
            }),
        };
        let all: Vec<Record> = mover.lock().values().cloned().collect();
        for record in &all {
            mover.publish(record);
        }
        mover.keep_ended();
        mover
    }

    fn config(&self) -> &Config {
        &self.inner.config
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<MoveId, Record>> {
        self.inner
            .records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn now(&self) -> Timestamp {
        (self.config().now)()
    }

    /// A move's record as it stands.
    pub fn record(&self, id: MoveId) -> Option<Record> {
        self.lock().get(&id).cloned()
    }

    /// Stores and publishes a record.
    fn save(&self, record: Record) {
        if let Err(error) = self.config().db.put_move(record.id, &record) {
            warn!(%error, source = %record.source, "couldn't store a workspace move");
        }
        self.publish(&record);
        self.lock().insert(record.id, record);
    }

    fn publish(&self, record: &Record) {
        self.config().hub.apply([Change::Upsert {
            value: Entity::Move(record.entity()),
        }]);
    }

    /// Moves each keep that ended on to the retire, which then waits for
    /// you.
    pub fn keep_ended(&self) {
        let now = self.now();
        let ended: Vec<Record> = self
            .lock()
            .values()
            .filter(|record| {
                record.step == MoveStep::Keep
                    && record.state == MoveState::Ready
                    && record.kept_until.is_some_and(|until| until <= now)
            })
            .cloned()
            .collect();
        for mut record in ended {
            record.step = MoveStep::Retire;
            self.save(record);
        }
    }

    /// Starts a move of `source` at its look. A move that went back starts
    /// again at its look, so it can switch a second time.
    fn plan(&self, source: &str) -> ActionOutcome {
        if !is_old_name(source) {
            return refused(format!(
                "{source} isn't a container the old scripts made, so there's nothing to move."
            ));
        }
        let Some(target) = target_for(source) else {
            return refused(format!("{source} has no name MaxHQ can copy it to."));
        };
        let device = self.config().device;
        let id = MoveId::derive(&format!("move:{device}:{source}"));
        let record = match self.record(id) {
            Some(found) if found.state == MoveState::MovedBack => Record {
                step: MoveStep::Look,
                state: MoveState::Ready,
                plan: None,
                source_changed: false,
                kept_until: None,
                unsaved: Vec::new(),
                warned: None,
                ..found
            },
            Some(found) if found.state == MoveState::Retired => {
                return refused(format!("{source} is moved and retired already."));
            }
            Some(_) => return ActionOutcome::Done,
            None => Record::new(id, device, source.to_owned(), target.to_string()),
        };
        self.save(record);
        ActionOutcome::Done
    }

    /// Runs one step you approved, if the move is at it.
    async fn run_step(&self, id: MoveId, step: MoveStep) -> ActionOutcome {
        self.keep_ended();
        let (doing, work): (&str, Work) = match step {
            MoveStep::Look => ("Looking", steps::look),
            MoveStep::Copy => ("Copying", steps::copy),
            MoveStep::Converge => ("Setting up", steps::converge),
            MoveStep::Switch => ("Switching", steps::switch),
            MoveStep::Retire => ("Retiring", steps::retire),
            MoveStep::Keep => ("Keeping", steps::switch),
        };
        let check = |record: &Record| {
            can_run(record, step)?;
            if step == MoveStep::Keep {
                return Err(format!(
                    "{} is kept until the keep ends. Move back or retire it now instead.",
                    record.source
                ));
            }
            Ok(())
        };
        match self.claim(id, Approval::Step(step), doing, check) {
            Ok(claim) => self.run_claimed(claim, step, work).await,
            Err(reason) => refused(reason),
        }
    }

    /// Checks what the step needs now and runs it, or gives the claim
    /// back with why it can't go.
    async fn run_claimed(&self, mut claim: Claim, step: MoveStep, work: Work) -> ActionOutcome {
        let record = &mut claim.record;
        let ready = match step {
            MoveStep::Switch => self.switch_ready(record).await,
            MoveStep::Copy => {
                let warning = self.copy_warning(record).await;
                warn_once(record, warning)
            }
            MoveStep::Retire => match self.retire_warning(record).await {
                Ok(warning) => warn_once(record, warning),
                Err(reason) => Err(reason),
            },
            _ => Ok(()),
        };
        match ready {
            Ok(()) => self.start(claim.record, work),
            Err(reason) => {
                self.release(claim);
                refused(reason)
            }
        }
    }

    /// Marks the move running with your approval, if `check` lets it, in
    /// one go under the lock, so a second request finds it busy.
    fn claim(
        &self,
        id: MoveId,
        approval: Approval,
        doing: &str,
        check: impl FnOnce(&Record) -> Result<(), String>,
    ) -> Result<Claim, String> {
        let claim = {
            let mut records = self.lock();
            let Some(record) = records.get_mut(&id) else {
                return Err("There's no such move on this device.".to_owned());
            };
            check(record)?;
            let before = record.state.clone();
            record.approved = Some(approval);
            record.state = MoveState::Running {
                doing: doing.to_owned(),
            };
            Claim {
                record: record.clone(),
                before,
            }
        };
        self.save(claim.record.clone());
        Ok(claim)
    }

    /// Gives a claim back: the move is as it was, with what the checks
    /// found kept on it.
    fn release(&self, claim: Claim) {
        let mut record = claim.record;
        record.approved = None;
        record.state = claim.before;
        self.save(record);
    }

    /// Checks the switch can go now: the original's agent is idle, no old
    /// resume flag is set, and nothing changed since the snapshot. A change
    /// is kept on the move, so its card offers a copy again.
    async fn switch_ready(&self, record: &mut Record) -> Result<(), String> {
        let source = record.source.clone();
        let mut look = Look::new(self.config().incus.socket());
        if steps::resume_flag(&self.config().resume_flags, &source) {
            return Err(format!(
                "The old scripts mean to resume {source} after a reboot. Wait until that's \
                 done, then switch."
            ));
        }
        let state = look.agent_state(&source).await?;
        if matches!(state.as_deref(), Some("working" | "blocked")) {
            return Err(format!(
                "The agent in {source} is at work. Switch once it's idle."
            ));
        }
        let now = steps::versions(&mut look, &source, record.marks.keys()).await?;
        if now != record.marks {
            record.source_changed = true;
            return Err(format!(
                "{source} changed since the copy. Copy again first, then switch."
            ));
        }
        Ok(())
    }

    /// What copying again would delete: the work an earlier copy holds
    /// from while the agent ran in it.
    async fn copy_warning(&self, record: &mut Record) -> Option<String> {
        let workspace = record.workspace.filter(|_| record.copy_used)?;
        let mut look = Look::new(self.config().incus.socket());
        look.original(&record.target).await.ok().flatten()?;
        let unsaved = self.config().host.unsaved(workspace).await;
        if unsaved.is_empty() {
            return None;
        }
        let warning = format!(
            "{} holds {} from while the agent ran there. Copying again deletes it. Save it \
             first, or copy again to delete it anyway.",
            record.target,
            unsaved.join(", ")
        );
        record.unsaved = unsaved;
        Some(warning)
    }

    /// What retiring would delete besides the original: its unsaved work,
    /// and its snapshots other than the move's own.
    async fn retire_warning(&self, record: &mut Record) -> Result<Option<String>, String> {
        let mut look = Look::new(self.config().incus.socket());
        let others: Vec<String> = look
            .snapshots(&record.source)
            .await?
            .into_iter()
            .filter(|name| name != MOVE_SNAPSHOT)
            .collect();
        let mut said = Vec::new();
        if !record.original_unsaved.is_empty() {
            said.push(format!(
                "{} has {}.",
                record.source,
                record.original_unsaved.join(", ")
            ));
            record.unsaved = record.original_unsaved.clone();
        }
        if !others.is_empty() {
            said.push(format!(
                "Its own snapshots go with it: {}.",
                others.join(", ")
            ));
        }
        if said.is_empty() {
            return Ok(None);
        }
        said.push(
            "Retiring deletes it. Save what you need first, or retire again to delete it \
             anyway."
                .to_owned(),
        );
        Ok(Some(said.join(" ")))
    }

    /// Goes back to the original while it's kept.
    async fn move_back(&self, id: MoveId) -> ActionOutcome {
        match self.claim(id, Approval::Back, "Moving back", can_end) {
            Ok(claim) => self.start(claim.record, steps::move_back),
            Err(reason) => refused(reason),
        }
    }

    /// Retires the original now, without waiting for the keep to end.
    async fn retire(&self, id: MoveId) -> ActionOutcome {
        let approval = Approval::Step(MoveStep::Retire);
        match self.claim(id, approval, "Retiring", can_end) {
            Ok(claim) => {
                self.run_claimed(claim, MoveStep::Retire, steps::retire)
                    .await
            }
            Err(reason) => refused(reason),
        }
    }

    /// Runs a claimed step off the request. The move shows how it ends.
    fn start(&self, record: Record, work: Work) -> ActionOutcome {
        let mover = self.clone();
        let id = record.id;
        tokio::spawn(async move {
            let ended = work(mover.clone(), record).await;
            let record = match ended {
                Ok(mut done) => {
                    done.approved = None;
                    done
                }
                Err(reason) => {
                    let Some(mut failed) = mover.record(id) else {
                        return;
                    };
                    warn!(source = %failed.source, %reason, "a move step failed");
                    failed.approved = None;
                    failed.state = MoveState::Failed { reason };
                    failed
                }
            };
            mover.save(record);
        });
        ActionOutcome::Done
    }
}

/// A step's work.
type Work = fn(Mover, Record) -> BoxFuture<'static, Result<Record, String>>;

/// A move marked running for one request, with the state it had before.
struct Claim {
    record: Record,
    before: MoveState,
}

/// Shows `warning` the first time the step is approved, and lets it go
/// the next. A warning that changed in between shows again.
fn warn_once(record: &mut Record, warning: Option<String>) -> Result<(), String> {
    match warning {
        Some(warning) if record.warned.as_ref() != Some(&warning) => {
            record.warned = Some(warning.clone());
            Err(warning)
        }
        _ => {
            record.warned = None;
            Ok(())
        }
    }
}

/// Why `step` can't run now, if it can't.
fn can_run(record: &Record, step: MoveStep) -> Result<(), String> {
    if let MoveState::Running { doing } = &record.state {
        return Err(format!("{} is busy: {}.", record.source, lower(doing)));
    }
    if record.is_over() {
        return Err(match record.state {
            MoveState::MovedBack => format!(
                "That move went back. Plan it again to move {} again.",
                record.source
            ),
            _ => format!("{} is moved and retired already.", record.source),
        });
    }
    let copy_again =
        step == MoveStep::Copy && record.step == MoveStep::Switch && record.source_changed;
    // A look only reads, so its plan can be brought up to date until there's
    // a copy to go with it.
    let look_again =
        step == MoveStep::Look && record.step == MoveStep::Copy && record.workspace.is_none();
    if step == record.step || copy_again || look_again {
        return Ok(());
    }
    if order(step) < order(record.step) {
        return Err(format!("{} ran already.", step_name(step)));
    }
    Err(format!(
        "{} can't run yet. {} comes first.",
        step_name(step),
        step_name(record.step)
    ))
}

/// Why the move can't go back or retire now, if it can't.
fn can_end(record: &Record) -> Result<(), String> {
    if let MoveState::Running { doing } = &record.state {
        return Err(format!("{} is busy: {}.", record.source, lower(doing)));
    }
    if record.is_over() {
        return Err("That move is over.".to_owned());
    }
    if !matches!(record.step, MoveStep::Keep | MoveStep::Retire) {
        return Err(format!(
            "{} still runs, so there's nothing to go back from yet. Switch first.",
            record.source
        ));
    }
    Ok(())
}

/// A phrase that starts a sentence, to go inside one.
fn lower(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}
