// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Scheduled tasks on the manager's loop. Each task is a caller of the
//! agent tools as `Actor::Task`, so every run goes through the same check
//! and keyed requests an agent's call does. The loop's own timer wakes at
//! the soonest run, at most [`TICK`] away, and compares the wall clock
//! with the stored times, so a Mac that slept notices within the tick.
//! A run ends when its agent's turn does, and survives a restart of `maxd`
//! as the agent does.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use maxhq_core::agents::{self as rules, Ask, Caller, Refusal, Turn};
use maxhq_core::schedule;
use maxhq_core::tools::agents::{Message, Start, WAIT};
use maxhq_core::tools::tasks::{Aim, Named, NewAgent, Schedule};
use maxhq_core::tools::Call;
use maxhq_protocol::agents::{Actor, AgentOrigin, Permission, ToolCall};
use maxhq_protocol::ids::{IdempotencyKey, TaskId};
use maxhq_protocol::message::{ActionOutcome, AgentPlace, AgentSpec, ClientMessage, SendMode};
use maxhq_protocol::model::Activity;
use maxhq_protocol::state::{Change, Entity};
use maxhq_protocol::tasks::{
    DeviceTasks, Hook, Maker, Run, RunCause, RunOutcome, RunSummary, Task, TaskProblem, TaskTarget,
    TaskWebhook, Trigger, Zone,
};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::Secret;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::warn;

use super::agent_tools::{no_account, request_key, Calling, Reply, NO_PLACE};
use super::idem::{Outcome, Seen};
use super::{is_queued, refused, Input, Manager};
use crate::db::Db;
use crate::github_events::pollers::Problems;
use crate::github_events::HookStates;
use crate::hooks::{Hooks, Reach};
use crate::secrets::RepoName;
use crate::tasks::{Fire, FireError};

pub(super) mod approval;

/// The longest the loop sleeps between looks at the wall clock.
const TICK: Duration = Duration::from_secs(30);

/// How many runs of a task may wait for the one going.
pub(super) const MAX_QUEUED: usize = 20;

/// What a timer run more than ten minutes late reads in the run log.
const MISSED: &str = "Missed: the device was asleep or off";

/// What a timer run reads when the task's last run still goes.
const SKIPPED: &str = "Skipped: still running";

/// What a run that `maxd` stopped before it began reads, when its prompt
/// came from a request that's gone.
const LOST: &str = "MaxHQ restarted before the run began.";

/// What the manager keeps for scheduled tasks.
pub(super) struct Book {
    /// Each task whose home is this device.
    tasks: HashMap<TaskId, Task>,
    /// Each task's run in progress.
    active: HashMap<TaskId, Active>,
    /// The runs waiting for each task's run in progress.
    queued: HashMap<TaskId, VecDeque<Queued>>,
    /// Webhook URLs and what's kept for them.
    hooks: Arc<dyn Hooks>,
    /// How this device's webhooks are reached and whether they can be, as
    /// last heard.
    funnel: Reach,
    /// Changes to it, until the loop watches them.
    funnel_changes: Option<watch::Receiver<Reach>>,
    /// Why each GitHub repository can't be polled, as last heard.
    problems: Problems,
    /// How each GitHub repository's webhook stands, as last heard.
    webhooks: HookStates,
    /// The tasks, for the webhook listener and the GitHub poller.
    watch: watch::Sender<Arc<[Task]>>,
}

impl Book {
    /// The stored tasks.
    pub(super) fn load(db: &Db, hooks: Arc<dyn Hooks>, funnel: watch::Receiver<Reach>) -> Self {
        let tasks = db.tasks().unwrap_or_else(|err| {
            warn!(%err, "couldn't read the stored tasks");
            Vec::new()
        });
        let state = funnel.borrow().clone();
        Self {
            watch: watch::Sender::new(Arc::from(tasks.clone())),
            tasks: tasks.into_iter().map(|task| (task.id, task)).collect(),
            active: HashMap::new(),
            queued: HashMap::new(),
            hooks,
            funnel: state,
            funnel_changes: Some(funnel),
            problems: Problems::new(),
            webhooks: HookStates::new(),
        }
    }

    /// A new line to the task list.
    pub(super) fn watch(&self) -> watch::Receiver<Arc<[Task]>> {
        self.watch.subscribe()
    }
}

/// A run waiting its turn.
struct Queued {
    /// The fire's own part of the run's key.
    id: String,
    cause: RunCause,
    delivery: Option<String>,
    prompt: String,
}

/// A run in progress.
struct Active {
    /// The fire's own part of the run's key.
    id: String,
    /// The key of the start or message the run sent.
    action: IdempotencyKey,
    run: Run,
    /// The prompt, while it's still to be sent.
    prompt: Option<String>,
    /// Whether the agent was busy with a turn of its own when its message
    /// was queued, so the turn that ends next isn't the run's.
    waiting: bool,
    /// Whether the run's turn was seen working.
    seen: bool,
}

/// What reaches the loop for scheduled tasks.
pub(super) enum Later {
    /// A webhook request, a GitHub event or a caller fires a task.
    Fire {
        task: TaskId,
        fire: Fire,
        done: oneshot::Sender<Result<(), FireError>>,
    },
    /// A new hook task's URL was made, so the save goes on.
    Rotated {
        key: IdempotencyKey,
        task: Box<Task>,
        url: Result<Secret, String>,
    },
    /// The Funnel's state changed.
    Funnel(Reach),
    /// The webhook service changes what `maxd` keeps on a task's hook.
    EditHook {
        task: TaskId,
        edit: Box<dyn FnOnce(&mut Hook) + Send>,
        done: oneshot::Sender<Result<(), String>>,
    },
    /// The GitHub poller's problems changed.
    Problems(Problems),
    /// How the GitHub webhooks stand changed.
    Webhooks(HookStates),
    /// A fire for another device's target looked at its link.
    Reached {
        task: TaskId,
        fire: Fire,
        done: oneshot::Sender<Result<(), FireError>>,
        up: bool,
    },
}

/// The key of the repository an enabled GitHub events task watches.
fn github_repo(task: &Task) -> Option<String> {
    let Trigger::GithubEvents { repo, .. } = &task.trigger else {
        return None;
    };
    task.enabled
        .then(|| RepoName::parse(repo).map(|repo| repo.key()))?
}

/// Hands the loop each change `changes` sees, as `later` makes it, until
/// either side ends.
pub(super) fn forward<T: Clone + Send + Sync + 'static>(
    tx: mpsc::UnboundedSender<Input>,
    mut changes: watch::Receiver<T>,
    later: fn(T) -> Later,
) {
    tokio::spawn(async move {
        while changes.changed().await.is_ok() {
            let value = changes.borrow_and_update().clone();
            if tx.send(Input::Tasks(Box::new(later(value)))).is_err() {
                return;
            }
        }
    });
}

impl Manager {
    /// Starts watching the Funnel's state, and picks up the runs a
    /// previous `maxd` left going.
    pub(super) fn start_tasks(&mut self) {
        if let Some(changes) = self.tasks.funnel_changes.take() {
            forward(self.tx.clone(), changes, Later::Funnel);
        }
        self.resume_runs();
    }

    /// Handles what came back for scheduled tasks.
    pub(super) fn task_later(&mut self, later: Later) {
        match later {
            Later::Fire { task, fire, done } => {
                let remote = self.tasks.tasks.get(&task).and_then(|found| {
                    let device = rules::target_device(&found.target);
                    (device != self.device && found.enabled).then_some(device)
                });
                let Some(device) = remote else {
                    let _ = done.send(self.fire_task(task, fire));
                    return;
                };
                // A request that fires another device's target is answered
                // only once the link to that device is up.
                let peers = self.agent_tools.peers.clone();
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    let up = peers.reach(device).await.is_ok();
                    let later = Later::Reached {
                        task,
                        fire,
                        done,
                        up,
                    };
                    let _ = tx.send(Input::Tasks(Box::new(later)));
                });
            }
            Later::Reached {
                task,
                fire,
                done,
                up,
            } => {
                let fired = if up {
                    self.fire_task(task, fire)
                } else {
                    Err(FireError::Offline)
                };
                let _ = done.send(fired);
            }
            Later::Rotated { key, task, url } => {
                let outcome = match url {
                    Ok(url) => self.saved(*task, Some(url)),
                    Err(reason) => refused(format!("couldn't make the webhook's URL: {reason}")),
                };
                self.finish(key, Outcome::Action(outcome));
            }
            Later::Funnel(state) => {
                self.tasks.funnel = state;
                self.publish_tasks();
            }
            Later::EditHook { task, edit, done } => {
                let _ = done.send(self.edit_hook(task, edit));
            }
            Later::Problems(problems) => {
                self.tasks.problems = problems;
                self.publish_tasks();
            }
            Later::Webhooks(webhooks) => {
                self.tasks.webhooks = webhooks;
                self.publish_tasks();
            }
        }
    }

    /// Handles the app's task messages.
    pub(super) fn task_message(&mut self, conn: u64, message: ClientMessage) {
        let key = match &message {
            ClientMessage::SaveTask { key, .. }
            | ClientMessage::SetTaskEnabled { key, .. }
            | ClientMessage::DeleteTask { key, .. }
            | ClientMessage::RunTaskNow { key, .. }
            | ClientMessage::ListRuns { key, .. } => *key,
            _ => return,
        };
        match self.keys.begin(key, conn) {
            Seen::New => {}
            Seen::Running => return,
            Seen::Done(outcome) => {
                self.tell(conn, outcome.message(key));
                return;
            }
        }
        let outcome = match message {
            ClientMessage::SaveTask { task, .. } => match self.save_task(key, *task) {
                Some(outcome) => outcome,
                None => return,
            },
            ClientMessage::SetTaskEnabled { task, enabled, .. } => {
                self.set_task_enabled(task, enabled)
            }
            ClientMessage::DeleteTask { task, .. } => match self.delete_task(task) {
                Ok(()) => ActionOutcome::Done,
                Err(reason) => refused(reason),
            },
            ClientMessage::RunTaskNow { task, .. } => match self.fire_task(task, Fire::Now(key)) {
                Ok(()) => ActionOutcome::Done,
                Err(err) => refused(fire_text(&err)),
            },
            ClientMessage::ListRuns { task, .. } => match self.db.runs(task) {
                Ok(runs) => ActionOutcome::Runs { runs },
                Err(err) => refused(format!("couldn't read the task's runs: {err}")),
            },
            _ => return,
        };
        self.finish(key, Outcome::Action(outcome));
    }

    /// Checks and stores a task the app sent, or answers none while a new
    /// webhook's URL is made. A task keeps who made it and its last run.
    /// One the user made gets a permission of its own, scoped to its
    /// target; one an agent made keeps that agent's.
    fn save_task(&mut self, key: IdempotencyKey, mut task: Task) -> Option<ActionOutcome> {
        if task.prompt.trim().is_empty() {
            return Some(refused("A task needs a prompt."));
        }
        if task.name.trim().is_empty() {
            return Some(refused("A task needs a name."));
        }
        if let Err(reason) = schedule::validate(&task.trigger) {
            return Some(refused(reason));
        }
        let existing = self.tasks.tasks.get(&task.id);
        task.made_by = existing.map_or(Maker::User, |old| old.made_by);
        task.last_run = existing.and_then(|old| old.last_run);
        // You saved it, so it no longer waits for you.
        task.approval_asked = None;
        task.permission = match (task.made_by, existing) {
            (Maker::Tools { .. }, Some(old)) => old.permission,
            _ => rules::user_permission(task.id),
        };
        let old_hook = existing.and_then(|old| match &old.trigger {
            Trigger::Hook(hook) => Some(hook.clone()),
            _ => None,
        });
        if let Trigger::Hook(hook) = &mut task.trigger {
            keep_hook_state(hook, old_hook.as_ref());
        }
        task.next_run = self.next_due(&task);
        if !matches!(task.trigger, Trigger::Hook(_)) || old_hook.is_some() {
            return Some(self.saved(task, None));
        }
        let hooks = self.tasks.hooks.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let url = hooks.rotate(task.id).await;
            let later = Later::Rotated {
                key,
                task: Box::new(task),
                url,
            };
            let _ = tx.send(Input::Tasks(Box::new(later)));
        });
        None
    }

    /// Stores a checked task, and answers it with its new webhook's URL.
    fn set_task_enabled(&mut self, id: TaskId, enabled: bool) -> ActionOutcome {
        let Some(task) = self.tasks.tasks.get(&id) else {
            return refused("This device doesn't hold that task.");
        };
        let mut task = task.clone();
        task.enabled = enabled;
        // Turning it on approves it, and off declines to for now.
        task.approval_asked = None;
        task.next_run = self.next_due(&task);
        self.saved(task, None)
    }

    fn saved(&mut self, task: Task, hook_url: Option<Secret>) -> ActionOutcome {
        match self.keep_task(task.clone()) {
            Ok(()) => ActionOutcome::TaskSaved {
                task: Box::new(task),
                hook_url,
            },
            Err(reason) => refused(reason),
        }
    }

    /// Stores a task, with the permission of one the user made, and
    /// publishes the change.
    pub(super) fn keep_task(&mut self, task: Task) -> Result<(), String> {
        if task.made_by == Maker::User
            && let ActionOutcome::Refused { reason } =
                self.grant(maxhq_core::agents::task_permission(&task, self.device))
        {
            return Err(reason);
        }
        self.store_task(task)
    }

    /// Stores a task and publishes the change, and asks you to approve it
    /// when it starts to wait for you, or waits for a new version of
    /// itself.
    fn store_task(&mut self, task: Task) -> Result<(), String> {
        self.db
            .put_task(&task)
            .map_err(|err| format!("couldn't store the task: {err}"))?;
        let before = self.tasks.tasks.insert(task.id, task.clone());
        self.sync_task_card(before.as_ref(), &task);
        self.publish_tasks();
        Ok(())
    }

    /// Applies the webhook service's `edit` to a task's hook, and stores
    /// it.
    fn edit_hook(
        &mut self,
        task: TaskId,
        edit: Box<dyn FnOnce(&mut Hook) + Send>,
    ) -> Result<(), String> {
        let mut task = self
            .tasks
            .tasks
            .get(&task)
            .cloned()
            .ok_or_else(|| "That task is gone.".to_owned())?;
        let Trigger::Hook(hook) = &mut task.trigger else {
            return Err("That task has no webhook.".to_owned());
        };
        edit(hook);
        self.store_task(task)
    }

    /// Forgets a task, its runs and its webhook, and the permission of
    /// one the user made. A run in progress goes on, unfollowed.
    fn delete_task(&mut self, task: TaskId) -> Result<(), String> {
        self.db
            .remove_task(task)
            .map_err(|err| format!("couldn't remove the task: {err}"))?;
        self.tasks.active.remove(&task);
        self.tasks.queued.remove(&task);
        if let Some(old) = self.tasks.tasks.remove(&task) {
            self.withdraw_task_card(&old);
            if old.made_by == Maker::User {
                self.revoke(old.permission);
            }
        }
        let hooks = self.tasks.hooks.clone();
        tokio::spawn(async move {
            if let Err(err) = hooks.forget(task).await {
                warn!(%err, %task, "couldn't forget a deleted task's webhook");
            }
        });
        self.publish_tasks();
        Ok(())
    }

    /// When a task runs next on its timer, if it does.
    fn next_due(&self, task: &Task) -> Option<Timestamp> {
        task.enabled
            .then(|| schedule::next_run(&task.trigger, self.clock.wall()))
            .flatten()
    }

    /// Queues a run of a task, or says why it didn't. A timer fire while
    /// a run goes is skipped, and a fire already queued or going is taken
    /// as done.
    fn fire_task(&mut self, task: TaskId, fire: Fire) -> Result<(), FireError> {
        let Some(found) = self.tasks.tasks.get(&task) else {
            return Err(FireError::Gone);
        };
        if !found.enabled && !matches!(fire, Fire::Now(_)) {
            return Err(FireError::Off);
        }
        let (id, cause, delivery, prompt) = match fire {
            Fire::Timer(due) => (
                due.0.to_string(),
                RunCause::Timer,
                None,
                found.prompt.clone(),
            ),
            Fire::Now(key) => (
                format!("now:{key}"),
                RunCause::Now,
                None,
                found.prompt.clone(),
            ),
            Fire::Hook(hook) => (
                format!("hook:{}", hook.delivery),
                RunCause::Hook,
                Some(hook.delivery),
                hook.prompt,
            ),
            Fire::Github(event) => (
                format!("github:{}", event.delivery),
                RunCause::Github,
                Some(event.delivery),
                event.prompt,
            ),
        };
        let going = self.tasks.active.get(&task);
        let queue = self.tasks.queued.entry(task).or_default();
        if going.is_some_and(|run| run.id == id) || queue.iter().any(|queued| queued.id == id) {
            return Ok(());
        }
        if cause == RunCause::Timer && (going.is_some() || !queue.is_empty()) {
            let now = self.clock.wall();
            self.log_run(&id, ended(task, cause, now, RunOutcome::Missed, SKIPPED));
            return Ok(());
        }
        if queue.len() >= MAX_QUEUED {
            return Err(FireError::QueueFull);
        }
        queue.push_back(Queued {
            id,
            cause,
            delivery,
            prompt,
        });
        self.begin_next(task);
        Ok(())
    }

    /// Begins a task's next queued run, unless one goes.
    fn begin_next(&mut self, task: TaskId) {
        if self.tasks.active.contains_key(&task) {
            return;
        }
        let Some(queued) = self
            .tasks
            .queued
            .get_mut(&task)
            .and_then(VecDeque::pop_front)
        else {
            return;
        };
        let Some(target) = self
            .tasks
            .tasks
            .get(&task)
            .map(|found| found.target.clone())
        else {
            return;
        };
        let (agent, waiting) = match target {
            TaskTarget::Agent { agent, .. } => (
                Some(agent),
                matches!(self.turn_of(agent), Turn::Working | Turn::NeedsInput),
            ),
            TaskTarget::New { .. } => (None, false),
        };
        let run = Run {
            task,
            cause: queued.cause,
            started: self.clock.wall(),
            ended: None,
            outcome: RunOutcome::Running,
            agent,
            delivery: queued.delivery,
            error: None,
        };
        self.store_run(&queued.id, &run);
        let active = Active {
            action: request_key(Actor::Task { task }, &queued.id),
            id: queued.id,
            run,
            prompt: Some(queued.prompt),
            waiting,
            seen: false,
        };
        self.tasks.active.insert(task, active);
        self.send_run(task);
    }

    /// Sends a run's start or message, as the task, under its permission.
    fn send_run(&mut self, task: TaskId) {
        let Some(found) = self.tasks.tasks.get(&task) else {
            return;
        };
        let Some(active) = self.tasks.active.get_mut(&task) else {
            return;
        };
        let Some(prompt) = active.prompt.take() else {
            return;
        };
        let request_id = active.id.clone();
        // A run stays in the container its maker ran in, as the maker would.
        let (level, container) = match found.made_by {
            Maker::User => (0, None),
            Maker::Tools { origin } => (origin.level, origin.container),
        };
        let caller = Calling {
            actor: Actor::Task { task },
            permission: Some(found.permission),
            level,
            mode: None,
            agent: None,
            device: None,
            container: container.map(|workspace| (self.device, workspace)),
        };
        let call = match &found.target {
            TaskTarget::Agent { device, agent } => Call::SendAgent(Message {
                request_id: request_id.clone(),
                device: Some(*device),
                agent: *agent,
                text: prompt,
                mode: SendMode::Queue,
            }),
            TaskTarget::New { device, spec } => Call::StartAgent(Start {
                request_id: request_id.clone(),
                device: Some(*device),
                vendor: spec.vendor,
                account: Some(spec.account),
                place: Some(spec.place.clone()),
                model: spec.model.clone(),
                mode: spec.permission,
                title: spec.title.clone().or_else(|| Some(found.name.clone())),
                task: prompt,
                context_from: None,
                wait: false,
                timeout: WAIT,
                wake: false,
            }),
        };
        // A call for another device goes over the line as these arguments.
        let raw = ToolCall {
            tool: call.tool().name().to_owned(),
            arguments: match &call {
                Call::SendAgent(message) => message.arguments(),
                Call::StartAgent(start) => start.arguments(),
                _ => unreachable!("a run sends or starts"),
            },
        };
        let reply = Reply::Task {
            task,
            id: request_id,
        };
        self.run_agent_call(reply, caller, call, raw, false);
    }

    /// Hears that a run's start or message didn't go.
    pub(super) fn run_failed(&mut self, task: TaskId, id: &str, reason: String) {
        if self.tasks.active.get(&task).is_some_and(|run| run.id == id) {
            self.end_run(task, RunOutcome::Failed, Some(reason));
        }
    }

    /// Hears that a run's start or message reached another device's
    /// target. This device can't follow an agent there, so the run ends
    /// once the other device takes it.
    pub(super) fn run_sent(&mut self, task: TaskId, id: &str, answer: &str) {
        let remote = self
            .tasks
            .tasks
            .get(&task)
            .is_some_and(|found| rules::target_device(&found.target) != self.device);
        let Some(active) = self.tasks.active.get_mut(&task) else {
            return;
        };
        if !remote || active.id != id {
            return;
        }
        if let Ok(started) = serde_json::from_str::<Value>(answer)
            && let Some(agent) = started["agent"]
                .as_str()
                .and_then(|agent| agent.parse().ok())
        {
            active.run.agent = Some(agent);
        }
        self.end_run(task, RunOutcome::Done, None);
    }

    /// Hears that an action finished, for the run that started an agent.
    pub(super) fn task_action_done(&mut self, key: IdempotencyKey, outcome: &ActionOutcome) {
        let ActionOutcome::Started { agent } = outcome else {
            return;
        };
        let Some((task, active)) = self
            .tasks
            .active
            .iter_mut()
            .find(|(_, run)| run.action == key && run.run.agent.is_none())
        else {
            return;
        };
        let task = *task;
        active.run.agent = Some(*agent);
        let (id, run) = (active.id.clone(), active.run.clone());
        self.store_run(&id, &run);
        self.follow(task, None);
    }

    /// Follows the runs whose agent moved.
    pub(super) fn task_agent_moved(&mut self, agent: maxhq_protocol::ids::AgentId, to: &Activity) {
        let tasks: Vec<TaskId> = self
            .tasks
            .active
            .iter()
            .filter(|(_, run)| run.run.agent == Some(agent))
            .map(|(task, _)| *task)
            .collect();
        for task in tasks {
            self.follow(task, Some(to));
        }
    }

    /// Ends a run once its agent's turn does. A turn counts once it was
    /// seen working, after the turn the agent was busy with when its
    /// message was queued. An agent that needs input or waits for a limit
    /// is still on the run's turn.
    fn follow(&mut self, task: TaskId, moved: Option<&Activity>) {
        let Some(agent) = self.tasks.active.get(&task).and_then(|run| run.run.agent) else {
            return;
        };
        let activity = self.agent_now(agent).map(|agent| agent.activity);
        let queued = self
            .timelines
            .get(&agent)
            .is_some_and(|timeline| timeline.kept().any(is_queued));
        let Some(active) = self.tasks.active.get_mut(&task) else {
            return;
        };
        let working = |activity: &Activity| {
            rules::turn(activity) == Turn::Working && !matches!(activity, Activity::Starting)
        };
        if !active.waiting && (moved.is_some_and(working) || activity.as_ref().is_some_and(working))
        {
            active.seen = true;
        }
        let (outcome, error) = match &activity {
            None => (RunOutcome::Stopped, Some("The agent is gone.".to_owned())),
            Some(now) => match (rules::turn(now), now) {
                (Turn::Working | Turn::NeedsInput, _)
                | (_, Activity::WaitingForLimitReset { .. }) => {
                    return;
                }
                (Turn::Done, _) => (RunOutcome::Done, None),
                (_, Activity::Error { reason }) => (RunOutcome::Failed, Some(reason.clone())),
                (Turn::Stopped, _) => (RunOutcome::Stopped, None),
            },
        };
        if activity.is_some() {
            if active.waiting {
                // The agent's own turn ended. While the run's message is
                // still queued, the next turn is the run's.
                if queued {
                    active.waiting = false;
                    return;
                }
            } else if !active.seen {
                return;
            }
        }
        self.end_run(task, outcome, error);
    }

    /// Ends a task's run in progress, and begins its next.
    fn end_run(&mut self, task: TaskId, outcome: RunOutcome, error: Option<String>) {
        let Some(active) = self.tasks.active.remove(&task) else {
            return;
        };
        let mut run = active.run;
        run.ended = Some(self.clock.wall());
        run.outcome = outcome;
        run.error = error;
        self.log_run(&active.id, run);
        self.begin_next(task);
    }

    /// Stores a run, and keeps it as its task's last unless another run
    /// goes.
    fn log_run(&mut self, id: &str, run: Run) {
        self.store_run(id, &run);
        let task = run.task;
        if run.outcome != RunOutcome::Running && self.tasks.active.contains_key(&task) {
            return;
        }
        let Some(found) = self.tasks.tasks.get_mut(&task) else {
            return;
        };
        found.last_run = Some(RunSummary {
            at: run.started,
            outcome: run.outcome,
        });
        if let Err(err) = self.db.put_task(found) {
            warn!(%err, %task, "couldn't store a task's last run");
        }
        self.publish_tasks();
    }

    /// Stores a run, or warns.
    fn store_run(&mut self, id: &str, run: &Run) {
        if let Err(err) = self.db.put_run(id, run) {
            warn!(%err, task = %run.task, "couldn't store a task's run");
        }
        if run.outcome == RunOutcome::Running
            && let Some(found) = self.tasks.tasks.get_mut(&run.task)
        {
            found.last_run = Some(RunSummary {
                at: run.started,
                outcome: run.outcome,
            });
            if let Err(err) = self.db.put_task(found) {
                warn!(%err, task = %run.task, "couldn't store a task's last run");
            }
            self.publish_tasks();
        }
    }

    /// Picks up the runs a previous `maxd` left going. One that has its
    /// agent follows it. One without finds the agent its start made, or
    /// sends the start again with the same key.
    fn resume_runs(&mut self) {
        let mut open = self.db.open_runs().unwrap_or_else(|err| {
            warn!(%err, "couldn't read the task runs in progress");
            Vec::new()
        });
        open.sort_by_key(|(_, run)| run.started);
        for (id, mut run) in open {
            let task = run.task;
            let Some(prompt) = self
                .tasks
                .tasks
                .get(&task)
                .map(|found| found.prompt.clone())
            else {
                continue;
            };
            if let Some(older) = self.tasks.active.remove(&task) {
                let mut older_run = older.run;
                older_run.ended = Some(self.clock.wall());
                older_run.outcome = RunOutcome::Failed;
                older_run.error = Some(LOST.to_owned());
                self.store_run(&older.id, &older_run);
            }
            let by = Actor::Task { task };
            run.agent = run.agent.or_else(|| {
                self.held
                    .values()
                    .find(|held| {
                        held.origin.is_some_and(|origin| origin.by == by)
                            && held.agent.started_at >= run.started
                    })
                    .map(|held| held.agent.id)
            });
            let prompt = match (run.agent, run.cause) {
                (Some(_), _) => None,
                (None, RunCause::Timer | RunCause::Now) => Some(prompt),
                (None, RunCause::Hook | RunCause::Github) => {
                    run.ended = Some(self.clock.wall());
                    run.outcome = RunOutcome::Failed;
                    run.error = Some(LOST.to_owned());
                    self.log_run(&id, run);
                    continue;
                }
            };
            let queued = run.agent.is_some_and(|agent| {
                self.timelines
                    .get(&agent)
                    .is_some_and(|timeline| timeline.kept().any(is_queued))
            });
            self.store_run(&id, &run);
            let active = Active {
                action: request_key(by, &id),
                id,
                run,
                prompt,
                waiting: queued,
                seen: !queued,
            };
            self.tasks.active.insert(task, active);
            if self.tasks.active[&task].prompt.is_some() {
                self.send_run(task);
            } else {
                self.follow(task, None);
            }
        }
    }

    /// When the next task is due, at most a tick away.
    pub(super) fn task_deadline(&self) -> Option<Instant> {
        let due = self
            .tasks
            .tasks
            .values()
            .filter(|task| task.enabled)
            .filter_map(|task| task.next_run)
            .min()?;
        let wall = self.clock.wall();
        let wait = Duration::from_millis(due.0.saturating_sub(wall.0)).min(TICK);
        Some(self.clock.now() + wait)
    }

    /// Fires the tasks that are due. An `Every` task that's overdue fires
    /// once, and an `At` run more than ten minutes late is logged as
    /// missed. Each moves on to its next time after now.
    pub(super) fn task_timers(&mut self) {
        let wall = self.clock.wall();
        let due: Vec<(TaskId, Timestamp)> = self
            .tasks
            .tasks
            .values()
            .filter(|task| task.enabled)
            .filter_map(|task| Some((task.id, task.next_run?)))
            .filter(|(_, at)| *at <= wall)
            .collect();
        for (task, at) in due {
            let Some(found) = self.tasks.tasks.get_mut(&task) else {
                continue;
            };
            found.next_run = schedule::next_run(&found.trigger, wall);
            let missed =
                matches!(found.trigger, Trigger::At { .. }) && schedule::is_missed(at, wall);
            if let Err(err) = self.db.put_task(found) {
                warn!(%err, %task, "couldn't store a task's next run");
            }
            let id = at.0.to_string();
            if missed {
                let mut run = ended(task, RunCause::Timer, wall, RunOutcome::Missed, MISSED);
                run.started = at;
                self.log_run(&id, run);
            } else if let Err(err) = self.fire_task(task, Fire::Timer(at)) {
                let run = ended(
                    task,
                    RunCause::Timer,
                    wall,
                    RunOutcome::Failed,
                    &fire_text(&err),
                );
                self.log_run(&id, run);
            }
            self.publish_tasks();
        }
    }

    /// Publishes this device's tasks to clients and to the listener and
    /// poller.
    pub(super) fn publish_tasks(&self) {
        let mut tasks: Vec<Task> = self.tasks.tasks.values().cloned().collect();
        tasks.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        self.tasks.watch.send_replace(Arc::from(tasks.clone()));
        let problems = tasks.iter().filter_map(|task| self.problem(task)).collect();
        let webhooks = tasks.iter().filter_map(|task| self.webhook(task)).collect();
        let entity = Entity::DeviceTasks(DeviceTasks {
            device: self.device,
            tasks,
            address: self.tasks.funnel.address.clone(),
            port: self.tasks.funnel.port,
            funnel: self.tasks.funnel.funnel.clone(),
            problems,
            webhooks,
        });
        self.hub.apply([Change::Upsert { value: entity }]);
    }

    /// Why an enabled GitHub events task can't run now, if the poller
    /// said, or else why its repository's webhook can't be set up.
    fn problem(&self, task: &Task) -> Option<TaskProblem> {
        let key = github_repo(task)?;
        let (reason, link) = match self.tasks.problems.get(&key) {
            Some(reason) => (reason.clone(), None),
            None => {
                let problem = self.tasks.webhooks.get(&key)?.problem.clone()?;
                (problem.reason, problem.link)
            }
        };
        Some(TaskProblem {
            task: task.id,
            reason,
            link,
        })
    }

    /// How an enabled GitHub events task's webhook stands, when its
    /// repository has one.
    fn webhook(&self, task: &Task) -> Option<TaskWebhook> {
        let state = self.tasks.webhooks.get(&github_repo(task)?)?;
        Some(TaskWebhook {
            task: task.id,
            instant: state.instant,
            last_delivery: state.delivered,
        })
    }

    /// Makes or changes a task for an agent's `schedule_task`. It runs as
    /// the agent, under the agent's permission, so it can do no more than
    /// the agent. By default each run messages the agent; unbound, each
    /// starts a new agent like it.
    pub(super) fn schedule_task(
        &mut self,
        caller: Calling,
        permission: &Permission,
        asked: Schedule,
    ) -> Result<String, String> {
        let existing = match asked.task {
            Some(task) => Some(self.own_task(caller, task)?.clone()),
            None => None,
        };
        let trigger = match &asked.schedule {
            Some(shape) => schedule::parse(shape, &Zone(system_zone()))?,
            None => existing
                .as_ref()
                .map(|old| old.trigger.clone())
                .ok_or(NO_SCHEDULE)?,
        };
        let prompt = asked
            .prompt
            .or_else(|| existing.as_ref().map(|old| old.prompt.clone()))
            .ok_or(NO_SCHEDULE)?;
        let target = match (asked.target, &existing) {
            (None, Some(old)) => old.target.clone(),
            (aim, _) => self.tool_target(caller, permission, aim.unwrap_or(Aim::Caller))?,
        };
        let id = existing.as_ref().map_or_else(
            || {
                TaskId::derive(&format!(
                    "tool:{}",
                    request_key(caller.actor, &asked.request_id)
                ))
            },
            |old| old.id,
        );
        let made_by = existing.as_ref().map_or(
            Maker::Tools {
                origin: AgentOrigin {
                    permission: permission.id,
                    by: caller.actor,
                    device: self.device,
                    level: caller.level,
                    wake: false,
                    container: caller.container.map(|(_, workspace)| workspace),
                },
            },
            |old| old.made_by,
        );
        let mut task = Task {
            id,
            name: asked
                .name
                .or_else(|| existing.as_ref().map(|old| old.name.clone()))
                .unwrap_or_else(|| name_from(&prompt)),
            prompt,
            trigger,
            target,
            enabled: false,
            permission: permission.id,
            made_by,
            next_run: None,
            last_run: existing.as_ref().and_then(|old| old.last_run),
            approval_asked: None,
        };
        // An agent's task runs once you approve it: one it makes, turns on
        // or changes in what it runs waits, off, until you do. Renaming one
        // you approved, or turning it off, doesn't ask.
        let wants_on = asked.enabled.unwrap_or_else(|| {
            existing
                .as_ref()
                .is_none_or(|old| old.enabled || old.approval_asked.is_some())
        });
        let same = existing.as_ref().is_some_and(|old| {
            old.prompt == task.prompt && old.trigger == task.trigger && old.target == task.target
        });
        let waited = existing.as_ref().and_then(|old| old.approval_asked);
        let approved = existing
            .as_ref()
            .is_some_and(|old| old.enabled && waited.is_none());
        if wants_on && approved && same {
            task.enabled = true;
        } else if wants_on {
            let now = self.clock.wall();
            task.approval_asked = Some(match waited {
                Some(old) if same => old,
                // A new version asks with a card of its own.
                Some(old) if old >= now => Timestamp(old.0 + 1),
                _ => now,
            });
        }
        task.next_run = self.next_due(&task);
        self.keep_task(task.clone())?;
        let waits = task.approval_asked.is_some();
        let mut answer = json!({
            "task": task.id,
            "name": task.name,
            "enabled": task.enabled,
            "waits_for_approval": waits,
            "next_run": task.next_run,
        });
        let mut notes = Vec::new();
        if waits {
            notes.push(approval::WAITS);
        }
        if matches!(task.trigger, Trigger::Hook(_)) {
            notes.push("Its webhook is off until the user turns it on in MaxHQ.");
        }
        if !notes.is_empty() {
            answer["note"] = notes.join(" ").into();
        }
        Ok(answer.to_string())
    }

    /// The tasks an agent's `list_tasks` shows: those it may change.
    pub(super) fn list_tasks(&self, caller: Calling) -> String {
        self.task_list(|task| self.may_change_task(caller, task))
    }

    /// The tasks `show` keeps, as `list_tasks` answers.
    pub(super) fn task_list(&self, show: impl Fn(&Task) -> bool) -> String {
        let tasks: Vec<&Task> = self
            .tasks
            .tasks
            .values()
            .filter(|task| show(task))
            .collect();
        json!({ "tasks": tasks }).to_string()
    }

    /// An agent's `delete_task`.
    pub(super) fn delete_own_task(
        &mut self,
        caller: Calling,
        named: &Named,
    ) -> Result<String, String> {
        let task = self.own_task(caller, named.task)?.id;
        self.delete_task(task)?;
        Ok("Deleted.".into())
    }

    /// An agent's `run_task_now`, keyed by its request so it runs once.
    pub(super) fn run_own_task(
        &mut self,
        caller: Calling,
        named: &Named,
    ) -> Result<String, String> {
        let found = self.own_task(caller, named.task)?;
        // Only a task you approved and left on runs at an agent's word.
        if found.approval_asked.is_some() {
            return Err(approval::STILL_WAITS.into());
        }
        if !found.enabled {
            return Err(approval::OFF.into());
        }
        let task = found.id;
        let key = request_key(caller.actor, &named.request_id);
        self.fire_task(task, Fire::Now(key))
            .map(|()| "Queued.".to_owned())
            .map_err(|err| fire_text(&err))
    }

    /// A task the caller may change.
    fn own_task(&self, caller: Calling, task: TaskId) -> Result<&Task, String> {
        self.tasks
            .tasks
            .get(&task)
            .filter(|found| self.may_change_task(caller, found))
            .ok_or_else(|| format!("There's no task here with id {task} you may change."))
    }

    /// Who made a task, while it's here.
    /// The tasks stored here whose runs reach `target`.
    pub(super) fn tasks_targeting(&self, target: &TaskTarget) -> Vec<Task> {
        self.tasks
            .tasks
            .values()
            .filter(|task| task.target == *target)
            .cloned()
            .collect()
    }

    pub(super) fn task_maker(&self, task: TaskId) -> Option<Maker> {
        self.tasks.tasks.get(&task).map(|task| task.made_by)
    }

    /// A task's name, while it's here.
    pub(super) fn task_name(&self, task: TaskId) -> Option<String> {
        self.tasks.tasks.get(&task).map(|task| task.name.clone())
    }

    /// Whether a task is on, while it's here.
    pub(super) fn task_enabled(&self, task: TaskId) -> Option<bool> {
        self.tasks.tasks.get(&task).map(|task| task.enabled)
    }

    /// Whether the caller made the task, sits above whoever made it, or is
    /// a run of the task itself, all under the permission it was made with.
    /// Another agent under that permission may not touch it.
    fn may_change_task(&self, caller: Calling, task: &Task) -> bool {
        let Maker::Tools { origin } = task.made_by else {
            return false;
        };
        Some(origin.permission) == caller.permission
            && (self.holds_permission(origin.permission, caller.actor)
                || self.comes_from(origin.by, caller.actor)
                || self.comes_from(caller.actor, Actor::Task { task: task.id }))
    }

    /// Whether `actor` is `ancestor` or sits below it, following who started
    /// each agent and who made each task. A chain ends at an agent that's
    /// gone from this device. A level fits a `u8`, so a real chain ends
    /// within these steps.
    fn comes_from(&self, mut actor: Actor, ancestor: Actor) -> bool {
        for _ in 0..=u8::MAX {
            if actor == ancestor {
                return true;
            }
            let origin = match actor {
                Actor::Agent { agent } => self.held.get(&agent).and_then(|held| held.origin),
                Actor::Task { task } => match self.task_maker(task) {
                    Some(Maker::Tools { origin }) => Some(origin),
                    _ => None,
                },
                Actor::Key { .. } => None,
            };
            let Some(origin) = origin else {
                return false;
            };
            actor = origin.by;
        }
        false
    }

    /// Where an agent's task runs: the agent itself or a new agent like
    /// it, an agent that's there, or a new agent from a spec. Its
    /// permission must allow it to message or start that agent, as
    /// send_agent and start_agent check it.
    fn tool_target(
        &self,
        caller: Calling,
        permission: &Permission,
        aim: Aim,
    ) -> Result<TaskTarget, String> {
        let now = self.clock.wall();
        // A run that reaches outside the caller's container asks you when
        // it fires, so only Never refuses it here.
        let reach =
            |device, target| match rules::check_reach(permission, caller.rules(), device, target) {
                Ok(()) | Err(Refusal::NeedsConsent) => Ok(()),
                Err(refusal) => Err(refusal.to_string()),
            };
        let check = |ask: &Ask| {
            let outside = Caller {
                container: None,
                ..caller.rules()
            };
            rules::check(Some(permission), outside, ask, now).map_err(|refusal| refusal.to_string())
        };
        let held = caller
            .agent
            .and_then(|lead| Some((lead, self.held.get(&lead)?)));
        let new = match aim {
            Aim::Agent { device, agent } => {
                let device = device.unwrap_or(self.device);
                check(&Ask::Act { device })?;
                reach(device, self.container_of(device, agent))?;
                if device == self.device && self.agent_now(agent).is_none() {
                    return Err(format!("There's no agent here with id {agent}."));
                }
                return Ok(TaskTarget::Agent { device, agent });
            }
            Aim::New(new) => new,
            Aim::Caller | Aim::LikeCaller => {
                let Some((lead, held)) = held else {
                    return Err(
                        "Only an agent this device runs can schedule a task for itself.".into(),
                    );
                };
                if aim == Aim::Caller {
                    return Ok(TaskTarget::Agent {
                        device: self.device,
                        agent: lead,
                    });
                }
                NewAgent {
                    device: None,
                    vendor: held.agent.vendor,
                    account: Some(held.account),
                    place: None,
                    model: held.model.clone(),
                    mode: None,
                }
            }
        };
        let device = new.device.unwrap_or(self.device);
        let account = new
            .account
            .or_else(|| self.first_account(permission, new.vendor, device))
            .ok_or_else(|| no_account(new.vendor))?;
        // The caller's workspace is only a place on the caller's device.
        let place = new
            .place
            .or_else(|| {
                let workspace = held.filter(|_| device == self.device)?.1.agent.workspace;
                Some(AgentPlace::Workspace { workspace })
            })
            .ok_or(NO_PLACE)?;
        let workspace = match &place {
            AgentPlace::Workspace { workspace } => Some(*workspace),
            AgentPlace::Folder { .. } => None,
        };
        let mode = check(&Ask::Start(rules::Start {
            device,
            vendor: new.vendor,
            account,
            place: &place,
            workspace,
            mode: new.mode,
            counts: None,
        }))?;
        reach(device, workspace)?;
        Ok(TaskTarget::New {
            device,
            spec: Box::new(AgentSpec {
                effort: None,
                vendor: new.vendor,
                account,
                place,
                model: new.model,
                permission: mode,
                title: None,
                first: None,
                branch: None,
                origin: None,
            }),
        })
    }
}

/// What a task change without a schedule or prompt reads.
const NO_SCHEDULE: &str = "A new task needs a `prompt` and a `schedule`.";

/// Why a fire didn't queue, as a run log or a caller reads it.
fn fire_text(err: &FireError) -> String {
    match err {
        FireError::Gone => "There's no task here with that id.".into(),
        FireError::Off => "The task is paused.".into(),
        FireError::QueueFull => "Too many of the task's runs are waiting.".into(),
        FireError::Offline => "The device the task targets isn't connected.".into(),
        FireError::Failed(reason) => reason.clone(),
    }
}

/// A run that ended as soon as it began.
fn ended(task: TaskId, cause: RunCause, at: Timestamp, outcome: RunOutcome, error: &str) -> Run {
    Run {
        task,
        cause,
        started: at,
        ended: Some(at),
        outcome,
        agent: None,
        delivery: None,
        error: Some(error.to_owned()),
    }
}

/// Keeps what `maxd` sets on a saved webhook: whether it's on, which
/// only `SetHookOn` changes, and whether it holds a secret, which only
/// `SetHookSecret` does. A new webhook is off and holds none.
fn keep_hook_state(hook: &mut Hook, old: Option<&Hook>) {
    hook.on = old.is_some_and(|old| old.on);
    if let Some(signature) = &mut hook.signature {
        signature.has_secret = old
            .and_then(|old| old.signature.as_ref())
            .is_some_and(|old| old.has_secret);
    }
}

/// A task's name from the first line of its prompt.
fn name_from(prompt: &str) -> String {
    let line = prompt.lines().map(str::trim).find(|line| !line.is_empty());
    match line {
        Some(line) => line.chars().take(60).collect(),
        None => "Scheduled task".into(),
    }
}

/// This device's IANA time zone, for a time an agent gives, or UTC when
/// it has none.
pub(super) fn system_zone() -> String {
    jiff::tz::TimeZone::system()
        .iana_name()
        .unwrap_or("UTC")
        .to_owned()
}
