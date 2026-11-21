// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Agents an automation keeps active: auto-settle leaves be an agent an
//! enabled task reaches, whether the task lives here or on another
//! device, while you can still settle it and the next run wakes it.

use maxhq_protocol::ids::{PermissionId, TaskId};
use maxhq_protocol::model::{InboxChange, Settled, Settler};
use maxhq_protocol::tasks::{HookRules, Maker, Task, TaskTarget, Trigger};

use super::*;
use crate::session::holders::Checkout;
use crate::session::inbox::automated::LAPSE;
use crate::session::inbox::SWEEP_EVERY;

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// A task the user made that runs `agent` on new issues, which these
/// tests never poll for, so it only runs when asked.
fn task(name: &str, enabled: bool, device: DeviceId, agent: AgentId) -> Task {
    Task {
        id: TaskId::derive(name),
        name: name.into(),
        prompt: format!("Run {name}"),
        trigger: Trigger::GithubEvents {
            repo: "owner/repo".into(),
            events: vec!["issues".into()],
            rules: HookRules::default(),
        },
        target: TaskTarget::Agent { device, agent },
        enabled,
        permission: PermissionId::derive("unused"),
        made_by: Maker::User,
        next_run: None,
        last_run: None,
        approval_asked: None,
        by_dispatcher: false,
    }
}

impl Harness {
    async fn save_task(&mut self, task: Task) {
        let task = Box::new(task);
        let outcome = self.act(|key| ClientMessage::SaveTask { key, task }).await;
        assert!(
            matches!(outcome, ActionOutcome::TaskSaved { .. }),
            "{outcome:?}"
        );
    }

    /// Whether the agent is settled now.
    fn settled(&self, agent: AgentId) -> bool {
        self.inbox(agent).settled.is_some()
    }
}

#[tokio::test]
async fn an_enabled_task_here_keeps_its_agent_from_settling() {
    let mut h = Harness::new();
    h.fakes
        .unsaved
        .answer(Checkout::Folder(h.folder.clone()), Some(false));
    let plain = h.outside("plain", Activity::Idle);
    let kept = h.outside("kept", Activity::Idle);
    let paused = h.outside("paused", Activity::Idle);
    let here = h.host.id;
    h.save_task(task("Keep", true, here, kept)).await;
    h.save_task(task("Off", false, here, paused)).await;

    h.clock.advance(5 * DAY + SWEEP_EVERY);
    eventually("the plain agent settling", || h.settled(plain)).await;
    eventually("the agent of a paused task settling", || h.settled(paused)).await;
    tokio::time::sleep(QUIET).await;
    assert!(!h.settled(kept), "an enabled task keeps its agent");
    assert_eq!(
        h.fakes.unsaved.asked().len(),
        2,
        "the kept agent's worktree isn't checked"
    );

    // Pausing the task lets it settle on the next beat.
    h.save_task(task("Keep", false, here, kept)).await;
    h.clock.advance(SWEEP_EVERY);
    eventually("the kept agent settling", || h.settled(kept)).await;
}

#[tokio::test]
async fn another_devices_tasks_keep_their_agents_here_until_it_says_otherwise_or_goes_quiet() {
    let mut h = Harness::new();
    let folder = Checkout::Folder(h.folder.clone());
    h.fakes.unsaved.answer(folder, Some(false));
    let plain = h.outside("plain", Activity::Idle);
    let kept = h.outside("kept", Activity::Idle);
    let quiet = h.outside("quiet", Activity::Idle);
    let own = h.outside("own", Activity::Idle);
    let devbox = DeviceId::derive("devbox");
    let laptop = DeviceId::derive("laptop");
    h.sessions.task_targets(devbox, vec![kept]).unwrap();
    h.sessions.task_targets(laptop, vec![quiet]).unwrap();
    // This device reads its own tasks where they're kept, so its own word
    // over a link doesn't count.
    h.sessions.task_targets(h.host.id, vec![own]).unwrap();
    tokio::time::sleep(QUIET).await;

    h.clock.advance(5 * DAY + SWEEP_EVERY);
    eventually("the plain agent settling", || h.settled(plain)).await;
    eventually("the device's own word not counting", || h.settled(own)).await;
    tokio::time::sleep(QUIET).await;
    assert!(!h.settled(kept));
    assert!(!h.settled(quiet));

    // What they said holds across a restart.
    h.restart().await;
    tokio::time::sleep(QUIET).await;
    for name in ["kept", "quiet"] {
        h.outside(name, Activity::Idle);
    }
    h.clock.advance(SWEEP_EVERY);
    tokio::time::sleep(QUIET).await;
    assert!(!h.settled(kept));
    assert!(!h.settled(quiet));

    // The devbox no longer has a task for it.
    h.sessions.task_targets(devbox, Vec::new()).unwrap();
    tokio::time::sleep(QUIET).await;
    h.clock.advance(SWEEP_EVERY);
    eventually("the agent the devbox let go settling", || h.settled(kept)).await;
    tokio::time::sleep(QUIET).await;
    assert!(!h.settled(quiet));

    // The laptop never says it again, so its word lapses.
    h.clock.advance(LAPSE - 5 * DAY);
    eventually("the lapsed agent settling", || h.settled(quiet)).await;
}

#[tokio::test]
async fn you_can_settle_an_agent_a_task_keeps_and_its_next_run_wakes_it() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    h.moves(session, agent, Activity::Idle).await;
    let here = h.host.id;
    let wake = task("Wake", true, here, agent);
    h.save_task(wake.clone()).await;

    let settle = |key| ClientMessage::SetInbox {
        key,
        agent,
        change: InboxChange::Settle,
    };
    assert_eq!(h.act(settle).await, ActionOutcome::Done);
    assert_eq!(
        h.inbox(agent).settled,
        Some(Settled {
            at: h.epoch,
            by: Settler::You,
        })
    );
    eventually("the stop", || h.holders.stops(session) == [STOP_GRACE]).await;
    eventually("the holder's files going", || h.holders.forgotten(session)).await;

    let run = |key| ClientMessage::RunTaskNow { key, task: wake.id };
    assert_eq!(h.act(run).await, ActionOutcome::Done);
    eventually("the run waking it", || !h.settled(agent)).await;
    eventually("the prompt", || {
        h.holders.last() != session && h.sent(h.holders.last()).contains(&"Run Wake".to_owned())
    })
    .await;
}
