// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Scheduled tasks on the manager: timers across a sleep, runs that
//! follow their agent and survive a restart, and the checks each fire
//! runs.

use std::sync::Mutex;

use futures::future::BoxFuture;
use maxhq_protocol::agents::{Actor, LeaveContainer, Limits, Permission};
use maxhq_protocol::ids::{PermissionId, TaskId};
use maxhq_protocol::tasks::{
    DeviceTasks, DigestEncoding, FunnelState, Hook, HookAddress, HookRules, Maker, Run, RunCause,
    RunOutcome, SignatureCheck, Task, TaskProblem, TaskTarget, TimeOfDay, Trigger, Zone,
};
use maxhq_protocol::timeline::{Question, QuestionAnswer};
use maxhq_protocol::Secret;
use serde_json::{json, Value};

use maxhq_core::agents as rules;

use super::*;
use crate::session::agent_tools::NO_PLACE;
use crate::session::scheduled::{approval, MAX_QUEUED};
use crate::tasks::{Delivered, Fire, FireError, Tasks};

/// A [`crate::hooks::Hooks`] that keeps what it was asked.
#[derive(Clone, Default)]
pub(super) struct FakeHooks {
    pub(super) rotated: Arc<Mutex<Vec<TaskId>>>,
    pub(super) forgot: Arc<Mutex<Vec<TaskId>>>,
    /// How the device's hooks are reached, as the webhook service sends it.
    pub(super) reach: Arc<tokio::sync::watch::Sender<crate::hooks::Reach>>,
}

impl crate::hooks::Hooks for FakeHooks {
    fn rotate(&self, task: TaskId) -> BoxFuture<'_, Result<Secret, String>> {
        self.rotated.lock().unwrap().push(task);
        let url = format!("https://devbox.example.ts.net/hooks/{task}/token");
        Box::pin(std::future::ready(Ok(Secret::new(url))))
    }

    fn forget(&self, task: TaskId) -> BoxFuture<'_, Result<(), String>> {
        self.forgot.lock().unwrap().push(task);
        Box::pin(std::future::ready(Ok(())))
    }
}

const MINUTE: Duration = Duration::from_secs(60);

/// A task the user made, every hour unless `trigger` says otherwise.
pub(super) fn task(name: &str, trigger: Trigger, target: TaskTarget) -> Task {
    Task {
        id: TaskId::derive(name),
        name: name.into(),
        prompt: format!("Run {name}"),
        trigger,
        target,
        enabled: true,
        permission: PermissionId::derive("unused"),
        made_by: Maker::User,
        next_run: None,
        last_run: None,
        approval_asked: None,
    }
}

/// `agent`, as if it were on devbox.
fn away(agent: AgentId) -> TaskTarget {
    TaskTarget::Agent {
        device: DeviceId::derive("devbox"),
        agent,
    }
}

pub(super) fn hourly() -> Trigger {
    Trigger::Every { minutes: 60 }
}

/// A webhook fire with its own delivery.
fn hook(delivery: &str) -> Fire {
    Fire::Hook(Delivered {
        delivery: delivery.into(),
        prompt: format!("Delivery {delivery}"),
    })
}

impl Harness {
    /// An idle Claude agent for tasks to reach. Gives it, its holder
    /// and a target for it.
    async fn idle_target(&mut self) -> (AgentId, SessionId, TaskTarget) {
        let (agent, session) = self.start_claude(None).await;
        self.moves(session, agent, Activity::Idle).await;
        let target = TaskTarget::Agent {
            device: self.host.id,
            agent,
        };
        (agent, session, target)
    }

    /// A target that starts a Claude agent in the project folder.
    fn new_target(&self) -> TaskTarget {
        TaskTarget::New {
            device: self.host.id,
            spec: Box::new(self.spec(Vendor::Claude, None)),
        }
    }

    /// Saves a task whose target is on devbox, which this manager plays
    /// too, so devbox's copy of its permission names this device as well.
    async fn save_away(&mut self, task: Task) -> Task {
        let saved = self.save(task).await;
        let copy = rules::task_permission(&saved, self.host.id);
        self.give(Permission {
            devices: vec![DeviceId::derive("devbox"), self.host.id],
            ..copy
        })
        .await;
        saved
    }

    /// Saves a task as the user, and gives it as `maxd` keeps it.
    pub(super) async fn save(&mut self, task: Task) -> Task {
        match self.save_answer(task).await {
            ActionOutcome::TaskSaved { task, .. } => *task,
            other => panic!("the task wasn't saved: {other:?}"),
        }
    }

    async fn save_answer(&mut self, task: Task) -> ActionOutcome {
        let task = Box::new(task);
        self.act(|key| ClientMessage::SaveTask { key, task }).await
    }

    async fn run_now(&mut self, task: TaskId) -> ActionOutcome {
        self.act(|key| ClientMessage::RunTaskNow { key, task })
            .await
    }

    /// A task's runs, newest first.
    fn runs(&self, task: TaskId) -> Vec<Run> {
        self.db.runs(task).expect("the runs can be read")
    }

    /// Waits until a task has a run that `check` passes, and gives it.
    async fn run_where(&self, task: TaskId, what: &str, check: impl Fn(&Run) -> bool) -> Run {
        eventually(what, || self.runs(task).iter().any(&check)).await;
        self.runs(task).into_iter().find(check).unwrap()
    }

    /// This device's tasks as clients see them.
    pub(super) fn published(&self) -> DeviceTasks {
        match self.hub.get(&EntityKey::DeviceTasks(self.host.id)) {
            Some(Entity::DeviceTasks(tasks)) => tasks,
            other => panic!("the hub has no tasks: {other:?}"),
        }
    }

    /// Gives `agent` a permission on this device for `vendors`.
    pub(super) async fn allow(&mut self, agent: AgentId, vendors: Vec<Vendor>) -> Permission {
        self.give(Permission {
            id: PermissionId::derive("p"),
            holder: Actor::Agent { agent },
            home: self.host.id,
            devices: vec![self.host.id],
            vendors,
            accounts: None,
            places: None,
            depth: maxhq_protocol::agents::DEPTH,
            limits: Limits::default(),
            test_kinds: Vec::new(),
            leave_container: LeaveContainer::Ask,
        })
        .await
    }
}

#[tokio::test]
async fn the_hook_address_shows_with_the_tasks() {
    let h = Harness::new();
    let own = HookAddress::Own {
        base: "https://hooks.example.com".into(),
    };
    let on = FunnelState::On {
        url: "https://hooks.example.com".into(),
    };
    h.fakes.hooks.reach.send_replace(crate::hooks::Reach {
        address: own.clone(),
        port: Some(41234),
        funnel: on.clone(),
    });
    eventually("the address shows", || h.published().address == own).await;
    assert_eq!(h.published().port, Some(41234));
    assert_eq!(h.published().funnel, on);
}

#[tokio::test]
async fn a_task_runs_on_its_timer_and_its_run_ends_with_the_turn() {
    let mut h = Harness::new();
    let (agent, session, target) = h.idle_target().await;
    let saved = h
        .save(task("Triage", Trigger::Every { minutes: 5 }, target))
        .await;
    assert_eq!(saved.next_run, Some(Timestamp(h.epoch.0 + 5 * 60_000)));
    assert_eq!(h.published().tasks, vec![saved.clone()]);

    h.clock.advance(4 * MINUTE);
    tokio::time::sleep(QUIET).await;
    assert!(h.runs(saved.id).is_empty());

    h.clock.advance(MINUTE);
    eventually("the prompt", || {
        h.sent(session).contains(&"Run Triage".to_owned())
    })
    .await;
    let run = h
        .run_where(saved.id, "the run", |run| {
            run.outcome == RunOutcome::Running
        })
        .await;
    assert_eq!((run.cause, run.agent), (RunCause::Timer, Some(agent)));

    h.moves(session, agent, Activity::Busy).await;
    h.moves(session, agent, Activity::DoneUnread).await;
    let run = h
        .run_where(saved.id, "the end", |run| run.outcome == RunOutcome::Done)
        .await;
    assert!(run.ended.is_some());
    let published = h.published().tasks.remove(0);
    assert_eq!(
        published.last_run.map(|last| last.outcome),
        Some(RunOutcome::Done)
    );
    assert_eq!(published.next_run, Some(Timestamp(h.epoch.0 + 10 * 60_000)));
}

#[tokio::test]
async fn a_turn_that_errors_fails_the_run() {
    let mut h = Harness::new();
    let (agent, session, target) = h.idle_target().await;
    let saved = h.save(task("Triage", hourly(), target)).await;
    assert_eq!(h.run_now(saved.id).await, ActionOutcome::Done);
    h.moves(session, agent, Activity::Busy).await;
    let reason = "exit status 1".to_owned();
    h.moves(
        session,
        agent,
        Activity::Error {
            reason: reason.clone(),
        },
    )
    .await;
    let run = h
        .run_where(saved.id, "the failure", |run| {
            run.outcome == RunOutcome::Failed
        })
        .await;
    assert_eq!((run.cause, run.error), (RunCause::Now, Some(reason)));
}

#[tokio::test]
async fn after_a_long_sleep_an_interval_runs_once_and_a_late_fixed_time_is_missed() {
    let mut h = Harness::new();
    let (_, session, target) = h.idle_target().await;
    let every = h.save(task("Every", hourly(), target.clone())).await;
    // The fake clock starts at 08:00 UTC.
    let at = Trigger::At {
        time: TimeOfDay { hour: 9, minute: 0 },
        days: Default::default(),
        zone: Zone("UTC".into()),
    };
    let fixed = h.save(task("Fixed", at, target)).await;
    assert_eq!(fixed.next_run, Some(Timestamp(h.epoch.0 + 60 * 60_000)));

    h.clock.advance(8 * 60 * MINUTE);
    eventually("the prompt", || {
        h.sent(session).contains(&"Run Every".to_owned())
    })
    .await;
    let missed = h
        .run_where(fixed.id, "the missed run", |run| {
            run.outcome == RunOutcome::Missed
        })
        .await;
    assert_eq!(missed.started, fixed.next_run.unwrap());
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.runs(every.id).len(), 1);
    assert_eq!(h.runs(fixed.id).len(), 1);
    assert!(!h.sent(session).contains(&"Run Fixed".to_owned()));
    let wall = h.clock.wall().0;
    let next: Vec<_> = h
        .published()
        .tasks
        .iter()
        .map(|task| task.next_run)
        .collect();
    // Every runs an hour from now, and Fixed at 09:00 the next day.
    assert_eq!(
        next,
        vec![
            Some(Timestamp(wall + 60 * 60_000)),
            Some(Timestamp(wall + 17 * 60 * 60_000))
        ]
    );
}

#[tokio::test]
async fn a_timer_fire_while_a_run_goes_is_skipped() {
    let mut h = Harness::new();
    let (agent, session, target) = h.idle_target().await;
    let saved = h
        .save(task("Triage", Trigger::Every { minutes: 1 }, target))
        .await;
    h.clock.advance(MINUTE);
    eventually("the prompt", || {
        h.sent(session).contains(&"Run Triage".to_owned())
    })
    .await;
    h.moves(session, agent, Activity::Busy).await;

    h.clock.advance(MINUTE);
    let skipped = h
        .run_where(saved.id, "the skip", |run| {
            run.outcome == RunOutcome::Missed
        })
        .await;
    assert_eq!(skipped.error.as_deref(), Some("Skipped: still running"));
    let runs = h.runs(saved.id);
    assert_eq!(runs.len(), 2);
    assert!(runs.iter().any(|run| run.outcome == RunOutcome::Running));
}

#[tokio::test]
async fn a_fire_says_why_it_did_not_queue() {
    let mut h = Harness::new();
    let (agent, session, target) = h.idle_target().await;
    let busy = h.save(task("Busy", hourly(), target.clone())).await;
    let mut off = task("Off", hourly(), target);
    off.enabled = false;
    let off = h.save(off).await;
    let away = TaskTarget::Agent {
        device: DeviceId::derive("devbox"),
        agent,
    };
    let away = h.save(task("Away", hourly(), away)).await;

    let fire = |task, fire| h.sessions.fire(task, fire);
    assert_eq!(
        fire(TaskId::derive("none"), hook("a")).await,
        Err(FireError::Gone)
    );
    assert_eq!(fire(off.id, hook("a")).await, Err(FireError::Off));
    h.fakes.peers.set_down(true);
    let fire = |task, fire| h.sessions.fire(task, fire);
    assert_eq!(fire(away.id, hook("a")).await, Err(FireError::Offline));
    h.fakes.peers.set_down(false);

    // The first run waits for the busy agent's turn, and the rest queue
    // behind it. A delivery fired again counts once.
    h.moves(session, agent, Activity::Busy).await;
    let fire = |task, fire| h.sessions.fire(task, fire);
    for n in 0..=MAX_QUEUED {
        assert_eq!(fire(busy.id, hook(&n.to_string())).await, Ok(()));
    }
    assert_eq!(fire(busy.id, hook("0")).await, Ok(()));
    assert_eq!(fire(busy.id, hook("5")).await, Ok(()));
    assert_eq!(fire(busy.id, hook("full")).await, Err(FireError::QueueFull));
    let runs = h.runs(busy.id);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].delivery.as_deref(), Some("0"));

    // A paused task still runs when asked to.
    assert_eq!(h.run_now(off.id).await, ActionOutcome::Done);
    h.run_where(off.id, "the paused run", |run| run.cause == RunCause::Now)
        .await;
}

#[tokio::test]
async fn a_fire_for_another_device_reaches_its_agent_over_the_line() {
    let mut h = Harness::new();
    let (agent, session, _) = h.idle_target().await;
    let saved = h.save_away(task("Away", hourly(), away(agent))).await;
    assert_eq!(h.sessions.fire(saved.id, hook("d1")).await, Ok(()));
    eventually("the prompt", || {
        h.sent(session).contains(&"Delivery d1".to_owned())
    })
    .await;
    let run = h
        .run_where(saved.id, "the handover", |run| {
            run.outcome == RunOutcome::Done
        })
        .await;
    assert_eq!(run.cause, RunCause::Hook);
    let calls = h.fakes.peers.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, DeviceId::derive("devbox"));
    assert_eq!(calls[0].1.by, Actor::Task { task: saved.id });
    assert_eq!(calls[0].1.permission, saved.permission);

    // A revoked permission stops the next run before it leaves.
    let permission = saved.permission;
    let outcome = h
        .act(|key| ClientMessage::RevokePermission { key, permission })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    assert_eq!(h.run_now(saved.id).await, ActionOutcome::Done);
    h.run_where(saved.id, "the failure", |run| {
        run.cause == RunCause::Now && run.outcome == RunOutcome::Failed
    })
    .await;
    assert_eq!(h.fakes.peers.calls().len(), 1);
}

#[tokio::test]
async fn a_timer_fire_to_a_device_that_is_down_fails_and_the_next_one_goes() {
    let mut h = Harness::new();
    let (agent, session, _) = h.idle_target().await;
    let every = Trigger::Every { minutes: 5 };
    let saved = h.save_away(task("Away", every, away(agent))).await;
    h.fakes.peers.set_down(true);
    h.clock.advance(5 * MINUTE);
    let run = h
        .run_where(saved.id, "the failure", |run| {
            run.outcome == RunOutcome::Failed
        })
        .await;
    assert_eq!(run.cause, RunCause::Timer);

    h.fakes.peers.set_down(false);
    h.clock.advance(5 * MINUTE);
    h.run_where(saved.id, "the next run", |run| {
        run.outcome == RunOutcome::Done
    })
    .await;
    assert!(h.sent(session).contains(&"Run Away".to_owned()));
}

#[tokio::test]
async fn a_new_webhook_shows_its_url_once_and_a_deleted_task_forgets_it() {
    let mut h = Harness::new();
    let target = h.new_target();
    let made = task("Hook", Trigger::Hook(Hook::default()), target);
    let ActionOutcome::TaskSaved {
        task: saved,
        hook_url,
    } = h.save_answer(made.clone()).await
    else {
        panic!("the task wasn't saved");
    };
    assert_eq!(*h.fakes.hooks.rotated.lock().unwrap(), vec![made.id]);
    assert!(hook_url.is_some());
    assert_eq!(saved.next_run, None);

    // Saving it again keeps the URL it has, and only `maxd` turns the
    // webhook on or says it holds a secret.
    let mut again = made.clone();
    again.trigger = Trigger::Hook(Hook {
        on: true,
        signature: Some(SignatureCheck {
            header: "x-hub-signature-256".into(),
            encoding: DigestEncoding::Hex,
            prefix: "sha256=".into(),
            has_secret: true,
        }),
        ..Hook::default()
    });
    let ActionOutcome::TaskSaved {
        task: saved,
        hook_url,
    } = h.save_answer(again).await
    else {
        panic!("the task wasn't saved");
    };
    assert_eq!(hook_url, None);
    let Trigger::Hook(hook) = saved.trigger else {
        panic!("not a webhook");
    };
    assert!(!hook.on);
    assert_eq!(hook.signature.map(|check| check.has_secret), Some(false));
    assert_eq!(h.fakes.hooks.rotated.lock().unwrap().len(), 1);

    let outcome = h
        .act(|key| ClientMessage::DeleteTask { key, task: made.id })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    eventually("the forget", || {
        h.fakes.hooks.forgot.lock().unwrap().contains(&made.id)
    })
    .await;
    assert!(h.published().tasks.is_empty());
    assert_eq!(
        h.run_now(made.id).await,
        ActionOutcome::Refused {
            reason: "There's no task here with that id.".into(),
        }
    );
}

#[tokio::test]
async fn the_webhook_service_turns_a_hook_on_and_a_later_save_keeps_it() {
    use crate::hooks::HookEdits;

    let mut h = Harness::new();
    let target = h.new_target();
    let made = h
        .save(task("Hook", Trigger::Hook(Hook::default()), target))
        .await;
    let mut watch = h.sessions.watch();
    h.sessions
        .edit_hook(made.id, Box::new(|hook| hook.on = true))
        .await
        .expect("the hook is edited");
    let on = |tasks: &[Task]| matches!(&tasks[0].trigger, Trigger::Hook(hook) if hook.on);
    watch.wait_for(|tasks| on(tasks)).await.unwrap();
    assert!(on(&h.published().tasks));

    let mut renamed = made.clone();
    renamed.name = "Renamed".into();
    assert!(on(&[h.save(renamed).await]));

    let timer = h.save(task("Timer", hourly(), h.new_target())).await;
    assert_eq!(
        h.sessions.edit_hook(timer.id, Box::new(|_| ())).await,
        Err("That task has no webhook.".into())
    );
    assert_eq!(
        h.sessions
            .edit_hook(TaskId::derive("gone"), Box::new(|_| ()))
            .await,
        Err("That task is gone.".into())
    );
}

#[tokio::test]
async fn a_github_task_shows_why_its_repository_cannot_be_polled() {
    let mut h = Harness::new();
    let events = |repo: &str| Trigger::GithubEvents {
        repo: repo.into(),
        events: vec!["issues".into()],
        rules: HookRules::default(),
    };
    let watched = h
        .save(task("Issues", events("Octo/Repo"), h.new_target()))
        .await;
    let mut off = task("Off", events("octo/repo"), h.new_target());
    off.enabled = false;
    h.save(off).await;
    h.save(task("Other", events("octo/other"), h.new_target()))
        .await;

    let (problems, seen) = tokio::sync::watch::channel(Default::default());
    h.sessions.follow_problems(seen);
    let reason = "Add the GitHub App to octo/repo.";
    problems.send_replace([("octo/repo".to_owned(), reason.to_owned())].into());
    eventually("the problem", || !h.published().problems.is_empty()).await;
    assert_eq!(
        h.published().problems,
        vec![TaskProblem {
            task: watched.id,
            reason: reason.into(),
            link: None,
        }]
    );

    problems.send_replace(Default::default());
    eventually("the problem cleared", || h.published().problems.is_empty()).await;

    // Its webhook shows how it stands, and why it can't be set up.
    let (webhooks, seen) = tokio::sync::watch::channel(Default::default());
    h.sessions.follow_webhooks(seen);
    let at = maxhq_protocol::time::Timestamp(1_800_000_000_000);
    let link = "https://github.com/settings/apps/maxhq-test/permissions";
    let state = crate::github_events::HookState {
        delivered: Some(at),
        instant: true,
        problem: Some(crate::github_events::HookProblem {
            reason: "No permission.".into(),
            link: Some(link.into()),
        }),
    };
    webhooks.send_replace([("octo/repo".to_owned(), state)].into());
    eventually("the webhook", || !h.published().webhooks.is_empty()).await;
    let published = h.published();
    assert_eq!(
        published.webhooks,
        vec![maxhq_protocol::tasks::TaskWebhook {
            task: watched.id,
            instant: true,
            last_delivery: Some(at),
        }]
    );
    assert_eq!(
        published.problems,
        vec![TaskProblem {
            task: watched.id,
            reason: "No permission.".into(),
            link: Some(link.into()),
        }]
    );

    // A poll's problem comes first.
    problems.send_replace([("octo/repo".to_owned(), reason.to_owned())].into());
    eventually("the poll's problem", || {
        h.published()
            .problems
            .first()
            .is_some_and(|problem| problem.link.is_none())
    })
    .await;
}

#[tokio::test]
async fn a_task_with_a_bad_schedule_or_no_prompt_is_refused() {
    let mut h = Harness::new();
    let target = h.new_target();
    let mut empty = task("Empty", hourly(), target.clone());
    empty.prompt = " ".into();
    assert!(matches!(
        h.save_answer(empty).await,
        ActionOutcome::Refused { .. }
    ));
    let never = task("Never", Trigger::Every { minutes: 0 }, target);
    assert!(matches!(
        h.save_answer(never).await,
        ActionOutcome::Refused { .. }
    ));
    assert!(h.published().tasks.is_empty());
}

#[tokio::test]
async fn a_revoked_permission_fails_the_next_run() {
    let mut h = Harness::new();
    let (_, session, target) = h.idle_target().await;
    let saved = h.save(task("Triage", hourly(), target)).await;
    let permission = saved.permission;
    let outcome = h
        .act(|key| ClientMessage::RevokePermission { key, permission })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);

    assert_eq!(h.run_now(saved.id).await, ActionOutcome::Done);
    let run = h
        .run_where(saved.id, "the failure", |run| {
            run.outcome == RunOutcome::Failed
        })
        .await;
    assert!(run.error.is_some());
    assert!(!h.sent(session).contains(&"Run Triage".to_owned()));
}

#[tokio::test]
async fn a_task_that_starts_an_agent_runs_as_the_task() {
    let mut h = Harness::new();
    let target = h.new_target();
    let saved = h.save(task("Fresh", hourly(), target)).await;
    let before = h.holders.sessions().len();
    assert_eq!(h.run_now(saved.id).await, ActionOutcome::Done);
    let run = h
        .run_where(saved.id, "the agent", |run| run.agent.is_some())
        .await;
    let agent = run.agent.unwrap();
    assert_eq!(h.holders.sessions().len(), before + 1);
    let origin = h.agent(agent).origin.expect("the agent has an origin");
    assert_eq!(origin.by, Actor::Task { task: saved.id });
    assert_eq!(origin.permission, saved.permission);
    let session = h.holders.last();
    eventually("the prompt", || {
        h.sent(session).contains(&"Run Fresh".to_owned())
    })
    .await;

    // It picks the agent up again after a restart, and ends with its turn.
    h.restart().await;
    eventually("the agent back", || h.holders.connects(session).len() > 1).await;
    h.moves(session, agent, Activity::Busy).await;
    h.moves(session, agent, Activity::DoneUnread).await;
    let run = h
        .run_where(saved.id, "the end", |run| run.outcome == RunOutcome::Done)
        .await;
    assert_eq!(run.agent, Some(agent));
    assert_eq!(h.holders.sessions().len(), before + 1);
}

#[tokio::test]
async fn a_restart_starts_a_run_that_never_began_and_fails_a_lost_webhook_run() {
    let mut h = Harness::new();
    let target = h.new_target();
    let timer = h.save(task("Timer", hourly(), target.clone())).await;
    let webhook = h.save(task("Webhook", hourly(), target)).await;
    let open = |task, cause| Run {
        task,
        cause,
        started: h.clock.wall(),
        ended: None,
        outcome: RunOutcome::Running,
        agent: None,
        delivery: None,
        error: None,
    };
    h.db.put_run("now:lost", &open(timer.id, RunCause::Now))
        .unwrap();
    h.db.put_run("hook:lost", &open(webhook.id, RunCause::Hook))
        .unwrap();
    let before = h.holders.sessions().len();

    h.restart().await;
    let run = h
        .run_where(timer.id, "the agent", |run| run.agent.is_some())
        .await;
    assert_eq!(run.outcome, RunOutcome::Running);
    assert_eq!(h.holders.sessions().len(), before + 1);
    let lost = h
        .run_where(webhook.id, "the failure", |run| {
            run.outcome == RunOutcome::Failed
        })
        .await;
    assert_eq!(
        lost.error.as_deref(),
        Some("MaxHQ restarted before the run began.")
    );
}

#[tokio::test]
async fn an_agent_schedules_lists_runs_and_deletes_its_own_tasks() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(Some("Lead")).await;
    let permission = h.allow(lead, vec![Vendor::Claude]).await;
    let schedule = json!({
        "request_id": "s1",
        "prompt": "Check the build\nand say how it went",
        "schedule": {"type": "interval", "everyMs": 3_600_000},
    });
    let answer = h
        .call(session, lead, "s1", "schedule_task", schedule)
        .await
        .unwrap();
    let answer: Value = serde_json::from_str(&answer).unwrap();
    assert_eq!(answer["name"], "Check the build");
    assert_eq!(answer["enabled"], false);
    assert_eq!(answer["waits_for_approval"], true);
    assert_eq!(answer["note"], approval::WAITS);
    let id: TaskId = serde_json::from_value(answer["task"].clone()).unwrap();
    let made = h.published().tasks.remove(0);
    assert_eq!(made.id, id);
    assert_eq!(made.permission, permission.id);
    assert_eq!(
        made.target,
        TaskTarget::Agent {
            device: h.host.id,
            agent: lead
        }
    );
    assert!(
        matches!(made.made_by, Maker::Tools { origin } if origin.by == Actor::Agent { agent: lead })
    );
    // It waits, off, until you approve it, and can't run before.
    assert!(!made.enabled);
    assert!(made.approval_asked.is_some());
    assert_eq!(made.next_run, None);

    let listed = h
        .call(session, lead, "l1", "list_tasks", json!({}))
        .await
        .unwrap();
    assert!(listed.contains(&id.to_string()));
    let early = json!({"request_id": "r0", "task": id});
    assert_eq!(
        h.call(session, lead, "r0", "run_task_now", early).await,
        Err(approval::STILL_WAITS.into())
    );
    assert_eq!(h.runs(id), Vec::new());

    let card = h.task_card().await;
    assert_eq!(card.agent, lead);
    assert_eq!(
        h.answer_task(card.id, approval::APPROVE).await,
        AnswerOutcome::Accepted
    );
    let approved = h.published_task(id).expect("it's kept");
    assert!(approved.enabled);
    assert_eq!(approved.approval_asked, None);
    assert!(approved.next_run.is_some());
    assert_eq!(h.task_cards(), Vec::new());

    let ran = h.call(
        session,
        lead,
        "r1",
        "run_task_now",
        json!({"request_id": "r1", "task": id}),
    );
    assert_eq!(ran.await.unwrap(), "Queued.");
    h.run_where(id, "the run", |run| run.cause == RunCause::Now)
        .await;
    // What an agent's task sends says so.
    eventually("the run's message", || {
        h.sent(session).iter().any(|text| {
            text.starts_with(&format!(
                "[MaxHQ] Not from your user. Sent by scheduled task {id}, made by an agent."
            )) && text.ends_with("\n\nCheck the build\nand say how it went")
        })
    })
    .await;

    // The user's own tasks aren't the agent's to change.
    let user = h.save(task("Mine", hourly(), h.new_target())).await;
    let delete = json!({"request_id": "d0", "task": user.id});
    assert!(h
        .call(session, lead, "d0", "delete_task", delete)
        .await
        .is_err());

    let delete = json!({"request_id": "d1", "task": id});
    assert_eq!(
        h.call(session, lead, "d1", "delete_task", delete)
            .await
            .unwrap(),
        "Deleted."
    );
    assert_eq!(h.published().tasks, vec![user]);
}

#[tokio::test]
async fn an_agent_cannot_schedule_more_than_its_permission_allows() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(Some("Lead")).await;
    h.allow(lead, vec![Vendor::Codex]).await;
    let schedule = json!({
        "request_id": "s1",
        "prompt": "Check the build",
        "schedule": {"type": "interval", "everyMs": 3_600_000},
        "bind_to_current_agent": false,
    });
    assert!(h
        .call(session, lead, "s1", "schedule_task", schedule)
        .await
        .is_err());
    assert!(h.published().tasks.is_empty());
}

/// An answer to a task's card.
fn approval_pick(key: IdempotencyKey, event: AttentionId, label: &str) -> ClientMessage {
    ClientMessage::Answer {
        idempotency_key: key,
        event,
        answer: AttentionAnswer::Questions {
            answers: vec![QuestionAnswer {
                id: approval::QUESTION.into(),
                values: vec![label.into()],
            }],
        },
        refs: Vec::new(),
    }
}

/// An hourly task an agent asks for.
fn hourly_ask(key: &str, prompt: &str) -> Value {
    json!({
        "request_id": key,
        "prompt": prompt,
        "schedule": {"type": "interval", "everyMs": 3_600_000},
    })
}

impl Harness {
    /// The task cards open now.
    fn task_cards(&self) -> Vec<AttentionEvent> {
        self.hub
            .snapshot()
            .entities
            .into_iter()
            .filter_map(|entity| match entity {
                Entity::Attention(event) if event.summary.starts_with("Approve the task") => {
                    Some(event)
                }
                _ => None,
            })
            .collect()
    }

    /// Waits for the one task card there should be.
    async fn task_card(&self) -> AttentionEvent {
        eventually("a task card", || !self.task_cards().is_empty()).await;
        let cards = self.task_cards();
        assert_eq!(cards.len(), 1, "{cards:#?}");
        cards.into_iter().next().expect("there's one")
    }

    /// Answers a task's card as a person.
    async fn answer_task(&mut self, event: AttentionId, label: &str) -> AnswerOutcome {
        let key = IdempotencyKey::generate();
        self.phone.send(approval_pick(key, event, label));
        self.phone.answered(key).await
    }

    /// The question a task's card shows in `agent`'s timeline.
    async fn task_question(&mut self, agent: AgentId) -> Question {
        let items = match self.open(agent, None, 200).await {
            TimelineUpdate::Reset { items, .. } => items,
            other => panic!("expected a reset, got {other:?}"),
        };
        items
            .iter()
            .rev()
            .find_map(|item| match &item.body {
                ItemBody::Question { questions, .. } => questions
                    .first()
                    .filter(|question| question.id == approval::QUESTION)
                    .cloned(),
                _ => None,
            })
            .expect("the timeline has the card's question")
    }

    /// A task as clients see it, while it's here.
    pub(super) fn published_task(&self, id: TaskId) -> Option<Task> {
        self.published()
            .tasks
            .into_iter()
            .find(|task| task.id == id)
    }

    /// `lead` asks for a task, and gives its ID and what the tool said.
    async fn ask_task(
        &self,
        (lead, session): (AgentId, SessionId),
        key: &str,
        asked: Value,
    ) -> (TaskId, Value) {
        let answer = self
            .call(session, lead, key, "schedule_task", asked)
            .await
            .unwrap();
        let answer: Value = serde_json::from_str(&answer).unwrap();
        let id = serde_json::from_value(answer["task"].clone()).unwrap();
        (id, answer)
    }
}

#[tokio::test]
async fn only_a_person_approves_an_agents_task_and_a_new_version_asks_again() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(Some("Lead")).await;
    h.allow(lead, vec![Vendor::Claude]).await;
    let caller = (lead, session);
    let (id, _) = h
        .ask_task(caller, "s1", hourly_ask("s1", "Check the build"))
        .await;
    let card = h.task_card().await;
    let question = h.task_question(lead).await;
    let consent = question.consent.expect("it's a consent card");
    let facts: Vec<(&str, &str)> = consent
        .facts
        .iter()
        .map(|fact| (fact.label.as_str(), fact.value.as_str()))
        .collect();
    assert!(facts.contains(&("Runs", "Every hour")), "{facts:?}");
    assert!(facts.contains(&("Prompt", "Check the build")), "{facts:?}");

    // An agent's tool connection can't answer it, nor an answer it
    // doesn't offer.
    let mut tool = Inbox {
        client: h.sessions.tool_client(ClientId::derive("agent-tools")),
        kept: Vec::new(),
    };
    let key = IdempotencyKey::generate();
    tool.send(approval_pick(key, card.id, approval::APPROVE));
    assert_eq!(tool.answered(key).await, AnswerOutcome::UnknownEvent);
    assert_eq!(
        h.answer_task(card.id, "Yes").await,
        AnswerOutcome::UnknownEvent
    );
    assert!(!h.published_task(id).expect("it's kept").enabled);
    assert_eq!(
        h.answer_task(card.id, approval::APPROVE).await,
        AnswerOutcome::Accepted
    );

    // Renaming it keeps it on.
    let rename = json!({"request_id": "e1", "task": id, "title": "Build check"});
    let (_, answer) = h.ask_task(caller, "e1", rename).await;
    assert_eq!(answer["waits_for_approval"], false);
    let renamed = h.published_task(id).expect("it's kept");
    assert!(renamed.enabled);
    assert_eq!(renamed.name, "Build check");
    assert_eq!(h.task_cards(), Vec::new());

    // A new prompt waits, off, for a card of its own.
    let edit = json!({"request_id": "e2", "task": id, "prompt": "Delete the build"});
    let (_, answer) = h.ask_task(caller, "e2", edit).await;
    assert_eq!(answer["waits_for_approval"], true);
    let changed = h.published_task(id).expect("it's kept");
    assert!(!changed.enabled);
    let again = h.task_card().await;
    assert_ne!(again.id, card.id);
    let run = json!({"request_id": "r1", "task": id});
    assert_eq!(
        h.call(session, lead, "r1", "run_task_now", run).await,
        Err(approval::STILL_WAITS.into())
    );

    // Declining deletes it.
    assert_eq!(
        h.answer_task(again.id, approval::DECLINE).await,
        AnswerOutcome::Accepted
    );
    assert_eq!(h.published_task(id), None);
    assert_eq!(h.task_cards(), Vec::new());
}

#[tokio::test]
async fn an_agent_cant_turn_on_a_task_you_turned_off_and_your_switch_approves() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(Some("Lead")).await;
    h.allow(lead, vec![Vendor::Claude]).await;
    let caller = (lead, session);
    let (id, _) = h
        .ask_task(caller, "s1", hourly_ask("s1", "Check the build"))
        .await;
    h.task_card().await;

    // Turning it on in Settings approves it and takes the card away.
    let on = h
        .act(|key| ClientMessage::SetTaskEnabled {
            key,
            task: id,
            enabled: true,
        })
        .await;
    assert!(matches!(on, ActionOutcome::TaskSaved { .. }), "{on:?}");
    let approved = h.published_task(id).expect("it's kept");
    assert!(approved.enabled);
    assert_eq!(approved.approval_asked, None);
    eventually("the card goes", || h.task_cards().is_empty()).await;

    // You turn it off, and the agent can neither run it nor turn it back
    // on by itself.
    h.act(|key| ClientMessage::SetTaskEnabled {
        key,
        task: id,
        enabled: false,
    })
    .await;
    let run = json!({"request_id": "r1", "task": id});
    assert_eq!(
        h.call(session, lead, "r1", "run_task_now", run).await,
        Err(approval::OFF.into())
    );
    let on = json!({"request_id": "e1", "task": id, "enabled": true});
    let (_, answer) = h.ask_task(caller, "e1", on).await;
    assert_eq!(answer["waits_for_approval"], true);
    assert!(!h.published_task(id).expect("it's kept").enabled);
    h.task_card().await;
    assert_eq!(h.runs(id), Vec::new());
}

#[tokio::test]
async fn a_task_card_comes_back_after_a_restart() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(Some("Lead")).await;
    h.allow(lead, vec![Vendor::Claude]).await;
    let (id, _) = h
        .ask_task((lead, session), "s1", hourly_ask("s1", "Check the build"))
        .await;
    let card = h.task_card().await;

    h.restart().await;
    let back = h.task_card().await;
    assert_eq!(back.id, card.id);
    assert!(!h.published_task(id).expect("it's kept").enabled);
    assert_eq!(
        h.answer_task(back.id, approval::APPROVE).await,
        AnswerOutcome::Accepted
    );
    assert!(h.published_task(id).expect("it's kept").enabled);
}

#[tokio::test]
async fn a_task_the_user_makes_is_on_at_once() {
    let mut h = Harness::new();
    let (_, _, target) = h.idle_target().await;
    let saved = h.save(task("Mine", hourly(), target)).await;
    assert!(saved.enabled);
    assert_eq!(saved.approval_asked, None);
    assert!(saved.next_run.is_some());
    assert_eq!(h.task_cards(), Vec::new());
}

impl Harness {
    /// `lead` schedules a task with `target`. Gives where its runs go, or
    /// why it was refused.
    async fn aim(
        &self,
        (lead, session): (AgentId, SessionId),
        key: &str,
        target: Value,
    ) -> Result<TaskTarget, String> {
        let schedule = json!({
            "request_id": key,
            "prompt": "Review the new PRs",
            "schedule": {"type": "github_events", "repo": "owner/repo", "events": ["pull_request"]},
            "target": target,
        });
        let answer = self
            .call(session, lead, key, "schedule_task", schedule)
            .await?;
        let answer: Value = serde_json::from_str(&answer).unwrap();
        let id: TaskId = serde_json::from_value(answer["task"].clone()).unwrap();
        let made = self
            .published()
            .tasks
            .into_iter()
            .find(|task| task.id == id);
        Ok(made.expect("the task shows").target)
    }
}

#[tokio::test]
async fn an_agent_aims_a_task_at_itself_another_agent_or_a_new_agent() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(Some("Lead")).await;
    let (other, _, _) = h.idle_target().await;
    let cursor = h.cursor_account(LoginKind::ConfigDir);
    h.allow(lead, vec![Vendor::Claude, Vendor::Cursor]).await;
    let caller = (lead, session);
    let here = h.host.id;

    let me = h.aim(caller, "s1", json!({"type": "current_agent"})).await;
    assert_eq!(
        me,
        Ok(TaskTarget::Agent {
            device: here,
            agent: lead
        })
    );
    let them = json!({"type": "agent", "agent": other});
    let them = h.aim(caller, "s2", them).await;
    assert_eq!(
        them,
        Ok(TaskTarget::Agent {
            device: here,
            agent: other
        })
    );

    let Ok(TaskTarget::New { device, spec }) = h
        .aim(caller, "s3", json!({"type": "new_like_current"}))
        .await
    else {
        panic!("not a new agent");
    };
    assert_eq!(device, here);
    assert_eq!((spec.vendor, spec.account), (Vendor::Claude, h.claude));
    let workspace = h.agent(lead).workspace;
    assert_eq!(spec.place, AgentPlace::Workspace { workspace });

    let folder = h.folder.display().to_string();
    let reviewer =
        json!({"type": "new_agent", "vendor": "cursor", "folder": folder, "model": "auto"});
    let Ok(TaskTarget::New { device, spec }) = h.aim(caller, "s4", reviewer).await else {
        panic!("not a new agent");
    };
    assert_eq!(device, here);
    assert_eq!((spec.vendor, spec.account), (Vendor::Cursor, cursor));
    assert_eq!(spec.place, AgentPlace::Folder { path: folder });
    assert_eq!(spec.model.as_deref(), Some("auto"));

    // bind_to_current_agent still names the caller.
    let schedule = json!({
        "request_id": "s5",
        "prompt": "Check the build",
        "schedule": {"type": "interval", "everyMs": 3_600_000},
        "bind_to_current_agent": true,
    });
    h.call(session, lead, "s5", "schedule_task", schedule)
        .await
        .unwrap();
    assert_eq!(h.published().tasks.len(), 5);
}

#[tokio::test]
async fn a_task_aimed_where_its_maker_may_not_send_or_start_is_refused() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(Some("Lead")).await;
    h.allow(lead, vec![Vendor::Claude, Vendor::Cursor]).await;
    let caller = (lead, session);

    let missing = AgentId::derive("missing");
    let gone = h
        .aim(caller, "s1", json!({"type": "agent", "agent": missing}))
        .await;
    assert_eq!(
        gone,
        Err(format!("There's no agent here with id {missing}."))
    );
    let devbox = DeviceId::derive("devbox");
    let away = json!({"type": "agent", "device": devbox, "agent": lead});
    assert!(h.aim(caller, "s2", away).await.is_err());

    let folder = h.folder.display().to_string();
    let codex = json!({"type": "new_agent", "vendor": "codex", "folder": folder});
    assert!(h.aim(caller, "s3", codex).await.is_err());
    // Cursor is allowed, but this device has no Cursor account.
    let cursor = json!({"type": "new_agent", "vendor": "cursor", "folder": folder});
    let none = h.aim(caller, "s4", cursor).await;
    assert_eq!(
        none,
        Err("There's no Cursor account here you may use.".into())
    );
    // The lead's workspace is no place on another device.
    let claude = h.claude;
    let elsewhere =
        json!({"type": "new_agent", "vendor": "claude", "device": devbox, "account": claude});
    let placeless = h.aim(caller, "s5", elsewhere).await;
    assert_eq!(placeless, Err(NO_PLACE.into()));
    assert!(h.published().tasks.is_empty());
}

#[tokio::test]
async fn a_github_run_that_waited_reaches_a_busy_agent_as_a_queued_message() {
    let mut h = Harness::new();
    let (agent, session, target) = h.idle_target().await;
    let events = Trigger::GithubEvents {
        repo: "owner/repo".into(),
        events: vec!["pull_request".into()],
        rules: HookRules {
            coalesce_key: Some("all".into()),
            coalesce_secs: Some(900),
            coalesce_restart: true,
            ..HookRules::default()
        },
    };
    let saved = h.save(task("Reviews", events, target)).await;
    h.moves(session, agent, Activity::Busy).await;
    let prompt = "2 requests came in while this task waited, oldest first.\n\n\
                  Request 1 of 2:\nReview PR 4\n\nRequest 2 of 2:\nReview PR 7";
    let fire = Fire::Github(Delivered {
        delivery: "all".into(),
        prompt: prompt.into(),
    });
    assert_eq!(h.sessions.fire(saved.id, fire).await, Ok(()));

    // It goes in behind the agent's turn, which keeps going.
    eventually("the prompt", || {
        h.sent(session).contains(&prompt.to_owned())
    })
    .await;
    assert!(h.holders.stops(session).is_empty());
    let run = h
        .run_where(saved.id, "the run", |run| run.agent == Some(agent))
        .await;
    assert_eq!(run.cause, RunCause::Github);
}

#[tokio::test]
async fn a_switch_changes_only_the_tasks_switch() {
    let mut h = Harness::new();
    let target = h.new_target();
    let kept = h.save(task("switched", hourly(), target)).await;
    assert!(kept.enabled);
    let answer = h
        .act(|key| ClientMessage::SetTaskEnabled {
            key,
            task: kept.id,
            enabled: false,
        })
        .await;
    let ActionOutcome::TaskSaved {
        task: off,
        hook_url,
    } = answer
    else {
        panic!("the task wasn't switched: {answer:?}");
    };
    assert_eq!(hook_url, None);
    assert_eq!(off.next_run, None, "a task that's off isn't due");
    assert_eq!(
        *off,
        Task {
            enabled: false,
            next_run: None,
            ..kept.clone()
        }
    );
    let answer = h
        .act(|key| ClientMessage::SetTaskEnabled {
            key,
            task: kept.id,
            enabled: true,
        })
        .await;
    let ActionOutcome::TaskSaved { task: on, .. } = answer else {
        panic!("the task wasn't switched: {answer:?}");
    };
    assert!(on.next_run.is_some(), "a task that's on is due again");
    assert_eq!(on.target, kept.target);
    assert_eq!(on.permission, kept.permission);
    assert_eq!(on.made_by, kept.made_by);
}

#[tokio::test]
async fn a_switch_is_refused_for_a_task_this_device_doesnt_hold() {
    let mut h = Harness::new();
    let answer = h
        .act(|key| ClientMessage::SetTaskEnabled {
            key,
            task: TaskId::derive("not held"),
            enabled: true,
        })
        .await;
    assert!(
        matches!(answer, ActionOutcome::Refused { .. }),
        "{answer:?}"
    );
    assert_eq!(h.db.tasks().unwrap(), []);
}
