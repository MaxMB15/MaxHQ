// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Settling, snoozing and pinning agents, per decision U-8: your changes
//! and when they're refused, the stop that comes with settling a held
//! agent, the wakes, auto-settle, the snooze at the usage limit, the
//! inboxes of agents that are gone and the started agents that settle
//! along with the agent that started them.

use maxhq_protocol::agents::{Actor, AgentOrigin, Limits};
use maxhq_protocol::ids::PermissionId;
use maxhq_protocol::model::{self, InboxChange, ProjectSettings, Settled, Settler};
use serde_json::json;

use super::*;
use crate::session::holders::Checkout;
use crate::session::inbox::{GONE_FOR, RECHECK_AFTER, SWEEP_EVERY};

const HOUR: Duration = Duration::from_secs(60 * 60);
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// The time `by` after `at`.
fn after(at: Timestamp, by: Duration) -> Timestamp {
    Timestamp(at.0 + u64::try_from(by.as_millis()).expect("the test's times fit"))
}

fn paused() -> Activity {
    Activity::Paused {
        reason: PauseReason::ByYou,
    }
}

fn needs_input() -> Activity {
    Activity::NeedsInput {
        attention: AttentionId::generate(),
    }
}

impl Harness {
    /// Asks for `change` to the agent's inbox and waits for the answer.
    async fn file(&mut self, agent: AgentId, change: InboxChange) -> ActionOutcome {
        self.act(|key| ClientMessage::SetInbox { key, agent, change })
            .await
    }

    /// The agent's inbox as clients see it.
    pub(super) fn inbox(&self, agent: AgentId) -> model::Inbox {
        self.agent(agent).inbox
    }

    /// The agent's inbox as stored for the next run of `maxd`.
    fn kept_inbox(&self, agent: AgentId) -> Option<model::Inbox> {
        self.db
            .inboxes()
            .expect("the inboxes can be read")
            .remove(&agent)
    }

    /// An outside agent reports again, changed by `change`.
    fn report(&self, agent: AgentId, change: impl FnOnce(&mut Agent)) {
        let mut agent = self.hub.reported(agent).expect("the agent is known");
        change(&mut agent);
        self.hub.apply([Change::Upsert {
            value: Entity::Agent(agent),
        }]);
    }

    /// A held agent's CLI reports it doing `activity`.
    async fn now_doing(&self, agent: AgentId, session: SessionId, activity: Activity) {
        let say = Say {
            agents: vec![self.reported(agent, activity)],
            ..Say::default()
        };
        self.say(session, &say).await;
    }

    /// The worktree auto-settle checks for agents in the test's folder.
    fn checkout(&self) -> Checkout {
        Checkout::Folder(self.folder.clone())
    }
}

#[tokio::test]
async fn your_changes_are_shown_stored_and_kept_across_a_restart() {
    let mut h = Harness::new();
    let agent = h.outside("s1", Activity::Idle);
    let until = after(h.epoch, HOUR);
    assert_eq!(
        h.file(agent, InboxChange::Snooze { until }).await,
        ActionOutcome::Done
    );
    let snoozed = model::Inbox {
        snoozed_until: Some(until),
        snoozed_at: Some(h.epoch),
        ..model::Inbox::default()
    };
    assert_eq!(h.inbox(agent), snoozed);
    assert_eq!(h.kept_inbox(agent), Some(snoozed));

    // A pin wakes it, and a settle unpins it.
    let pin = InboxChange::Pin { key: "a0".into() };
    assert_eq!(h.file(agent, pin).await, ActionOutcome::Done);
    assert_eq!(h.inbox(agent).snoozed_until, None);
    assert_eq!(h.inbox(agent).pin.map(|pin| pin.key), Some("a0".to_owned()));
    assert_eq!(
        h.file(agent, InboxChange::Settle).await,
        ActionOutcome::Done
    );
    let settled = Some(Settled {
        at: h.epoch,
        by: Settler::You,
    });
    assert_eq!(h.inbox(agent).settled, settled);
    assert_eq!(h.inbox(agent).pin, None);

    // An observer's next report doesn't clear it, and neither does a
    // restart.
    h.report(agent, |agent| agent.title = "Renamed".into());
    assert_eq!(h.inbox(agent).settled, settled);
    h.restart().await;
    eventually("the inbox coming back", || {
        h.hub.inbox(agent).settled == settled
    })
    .await;
    h.outside("s1", Activity::Idle);
    assert_eq!(h.inbox(agent).settled, settled);

    // An undo puts back what was there.
    let restore = InboxChange::Restore {
        inbox: model::Inbox::default(),
    };
    assert_eq!(h.file(agent, restore).await, ActionOutcome::Done);
    assert_eq!(h.inbox(agent), model::Inbox::default());
    assert_eq!(h.kept_inbox(agent), None, "the default isn't stored");
}

#[tokio::test]
async fn only_a_lead_agent_maxd_knows_has_an_inbox() {
    let mut h = Harness::new();
    let outcome = h.file(AgentId::derive("nobody"), InboxChange::Settle).await;
    assert_eq!(outcome, ActionOutcome::UnknownAgent);

    let lead = h.outside("s1", Activity::Idle);
    let helper = Agent {
        id: AgentId::derive("helper"),
        parent: Some(lead),
        ..h.agent(lead)
    };
    h.hub.apply([Change::Upsert {
        value: Entity::Agent(helper.clone()),
    }]);
    let outcome = h.file(helper.id, InboxChange::Settle).await;
    assert!(reason(&outcome).contains("only lead agents"), "{outcome:?}");
    assert_eq!(h.inbox(helper.id), model::Inbox::default());
}

#[tokio::test]
async fn an_agent_that_works_or_needs_you_stays_in_view() {
    let mut h = Harness::new();
    let agent = h.outside("s1", Activity::Busy);
    let snooze = InboxChange::Snooze {
        until: after(h.epoch, HOUR),
    };
    let outcome = h.file(agent, InboxChange::Settle).await;
    assert!(reason(&outcome).contains("still working"), "{outcome:?}");
    // A snooze can wait out the work.
    assert_eq!(h.file(agent, snooze.clone()).await, ActionOutcome::Done);
    assert_eq!(h.file(agent, InboxChange::Wake).await, ActionOutcome::Done);

    h.report(agent, |agent| agent.activity = needs_input());
    let outcome = h.file(agent, InboxChange::Settle).await;
    assert!(reason(&outcome).contains("needs your input"), "{outcome:?}");
    let outcome = h.file(agent, snooze.clone()).await;
    assert!(reason(&outcome).contains("needs your input"), "{outcome:?}");

    // A sub-agent that needs you counts for its lead.
    h.report(agent, |agent| agent.activity = Activity::Idle);
    let helper = Agent {
        id: AgentId::derive("helper"),
        parent: Some(agent),
        activity: needs_input(),
        ..h.agent(agent)
    };
    h.hub.apply([Change::Upsert {
        value: Entity::Agent(helper),
    }]);
    let outcome = h.file(agent, InboxChange::Settle).await;
    assert!(reason(&outcome).contains("needs your input"), "{outcome:?}");

    // An undo that would settle it is held to the same rule.
    let restore = InboxChange::Restore {
        inbox: model::Inbox {
            settled: Some(Settled {
                at: h.epoch,
                by: Settler::You,
            }),
            ..model::Inbox::default()
        },
    };
    let outcome = h.file(agent, restore).await;
    assert!(reason(&outcome).contains("needs your input"), "{outcome:?}");

    let past = InboxChange::Snooze { until: h.epoch };
    let outcome = h.file(agent, past).await;
    assert!(reason(&outcome).contains("in the future"), "{outcome:?}");
    assert_eq!(
        h.inbox(agent),
        model::Inbox {
            woke_at: Some(h.epoch),
            ..model::Inbox::default()
        },
        "only the wake went through"
    );
}

#[tokio::test]
async fn a_message_waiting_keeps_the_agent_from_settling_or_snoozing() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let say = Say {
        agents: vec![h.reported(agent, Activity::Idle)],
        timeline: vec![upsert(agent, queued(agent, "q1", "Then push"))],
        ..Say::default()
    };
    h.say(session, &say).await;

    let outcome = h.file(agent, InboxChange::Settle).await;
    assert!(
        reason(&outcome).contains("message is waiting"),
        "{outcome:?}"
    );
    let snooze = InboxChange::Snooze {
        until: after(h.epoch, HOUR),
    };
    let outcome = h.file(agent, snooze).await;
    assert!(
        reason(&outcome).contains("message is waiting"),
        "{outcome:?}"
    );
    tokio::time::sleep(QUIET).await;
    assert!(h.holders.stops(session).is_empty());
    assert_eq!(h.inbox(agent), model::Inbox::default());
}

#[tokio::test]
async fn settling_an_agent_maxhq_holds_stops_it_until_you_send() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    h.now_doing(agent, session, Activity::Idle).await;

    assert_eq!(
        h.file(agent, InboxChange::Settle).await,
        ActionOutcome::Done
    );
    let settled = Some(Settled {
        at: h.epoch,
        by: Settler::You,
    });
    assert_eq!(h.inbox(agent).settled, settled);
    assert_eq!(h.kept_inbox(agent).and_then(|inbox| inbox.settled), settled);
    eventually("the pause", || h.activity(agent) == paused()).await;
    eventually("the stop", || h.holders.stops(session) == [STOP_GRACE]).await;
    eventually("the holder's files going", || h.holders.forgotten(session)).await;
    assert_eq!(h.inbox(agent).settled, settled, "it stays settled");

    h.restart().await;
    eventually("the agent coming back settled and paused", || {
        matches!(
            h.hub.get(&EntityKey::Agent(agent)),
            Some(Entity::Agent(shown)) if shown.activity == paused() && shown.inbox.settled == settled
        )
    })
    .await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.holders.sessions(), [session]);

    // A message brings it back into the inbox.
    assert_eq!(h.send(agent, "go on").await, ActionOutcome::Done);
    assert_eq!(h.inbox(agent), model::Inbox::default());
    assert_eq!(h.kept_inbox(agent), None);
    assert_eq!(h.sent(h.holders.last()), ["go on"]);
}

#[tokio::test]
async fn a_message_ends_a_snooze() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    h.now_doing(agent, session, Activity::Idle).await;
    let snooze = InboxChange::Snooze {
        until: after(h.epoch, HOUR),
    };
    assert_eq!(h.file(agent, snooze).await, ActionOutcome::Done);
    assert!(h.inbox(agent).snoozed_until.is_some());

    assert_eq!(h.send(agent, "one more thing").await, ActionOutcome::Done);
    assert_eq!(h.inbox(agent), model::Inbox::default());
}

#[tokio::test]
async fn settling_an_agent_you_started_yourself_sends_it_no_signal() {
    let mut h = Harness::new();
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("sleep runs");
    let agent = h.outside("s1", Activity::Idle);
    h.report(agent, |agent| agent.pid = Some(child.id()));

    let outcome = h.file(agent, InboxChange::Settle).await;
    tokio::time::sleep(QUIET).await;
    let exited = child.try_wait().expect("the process can be checked");
    let _ = child.kill();
    let _ = child.wait();

    assert_eq!(outcome, ActionOutcome::Done);
    assert_eq!(exited, None, "the process still runs");
    assert!(h.holders.sessions().is_empty(), "nothing attached to it");
    let shown = h.agent(agent);
    assert_eq!(shown.control, Control::Observed);
    assert_eq!(shown.activity, Activity::Idle);
    assert_eq!(
        shown.inbox.settled,
        Some(Settled {
            at: h.epoch,
            by: Settler::You,
        })
    );
}

#[tokio::test]
async fn a_snooze_wakes_at_its_end_or_once_the_agent_needs_you() {
    let mut h = Harness::new();
    let agent = h.outside("s1", Activity::Idle);
    let until = after(h.epoch, HOUR);
    assert_eq!(
        h.file(agent, InboxChange::Snooze { until }).await,
        ActionOutcome::Done
    );
    h.clock.advance(HOUR - Duration::from_secs(1));
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.inbox(agent).snoozed_until, Some(until));
    h.clock.advance(Duration::from_secs(1));
    eventually("the wake", || h.inbox(agent).snoozed_until.is_none()).await;
    assert_eq!(h.inbox(agent).woke_at, Some(until));
    assert_eq!(
        h.kept_inbox(agent).and_then(|inbox| inbox.woke_at),
        Some(until)
    );

    // A snooze ends early when the agent comes to need you.
    let snooze = InboxChange::Snooze {
        until: after(h.epoch, 3 * HOUR),
    };
    assert_eq!(h.file(agent, snooze).await, ActionOutcome::Done);
    h.clock.advance(Duration::from_secs(30));
    let now = h.clock.wall();
    h.report(agent, |agent| {
        agent.activity = needs_input();
        agent.last_activity_at = now;
    });
    eventually("the early wake", || h.inbox(agent).snoozed_until.is_none()).await;
    assert_eq!(h.inbox(agent).woke_at, Some(now));
}

#[tokio::test]
async fn a_turn_you_start_in_your_own_terminal_brings_the_agent_back() {
    let mut h = Harness::new();
    let agent = h.outside("s1", Activity::Idle);
    assert_eq!(
        h.file(agent, InboxChange::Settle).await,
        ActionOutcome::Done
    );

    // You type into its terminal, and its observer sees the new turn.
    h.clock.advance(Duration::from_secs(30));
    let now = h.clock.wall();
    h.report(agent, |agent| {
        agent.activity = Activity::Busy;
        agent.last_activity_at = now;
    });
    eventually("the agent coming back", || h.inbox(agent).settled.is_none()).await;
    assert_eq!(h.kept_inbox(agent), None);

    // A snooze ends the same way, without the marker a wake leaves.
    h.report(agent, |agent| agent.activity = Activity::Idle);
    let snooze = InboxChange::Snooze {
        until: after(now, HOUR),
    };
    assert_eq!(h.file(agent, snooze).await, ActionOutcome::Done);
    h.clock.advance(Duration::from_secs(30));
    let now = h.clock.wall();
    h.report(agent, |agent| {
        agent.activity = Activity::Busy;
        agent.last_activity_at = now;
    });
    eventually("the snooze ending", || {
        h.inbox(agent).snoozed_until.is_none()
    })
    .await;
    assert_eq!(h.inbox(agent), model::Inbox::default());
}

#[tokio::test]
async fn a_report_of_the_old_turn_leaves_the_agent_settled() {
    let mut h = Harness::new();
    let agent = h.outside("s1", Activity::Idle);
    h.clock.advance(Duration::from_secs(30));
    assert_eq!(
        h.file(agent, InboxChange::Settle).await,
        ActionOutcome::Done
    );
    let settled = h.inbox(agent).settled;

    // A rescan reports a turn from before the settle.
    h.report(agent, |agent| agent.activity = Activity::Busy);
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.inbox(agent).settled, settled);
    h.report(agent, |agent| agent.activity = Activity::Idle);

    // So does the first report after a restart.
    let seen = h.agent(agent);
    h.restart().await;
    eventually("the inbox coming back", || {
        h.hub.inbox(agent).settled == settled
    })
    .await;
    h.outside("s1", Activity::Busy);
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.inbox(agent).settled, settled);

    // A turn newer than the settle brings it back, even one its observer
    // reported before the manager started.
    h.stop().await;
    h.clock.advance(Duration::from_secs(30));
    h.kept.push(Entity::Agent(Agent {
        activity: Activity::Busy,
        last_activity_at: h.clock.wall(),
        ..seen
    }));
    h.boot();
    eventually("the agent coming back", || {
        h.kept_inbox(agent).is_none() && h.hub.inbox(agent).settled.is_none()
    })
    .await;
}

#[tokio::test]
async fn an_agent_maxhq_holds_comes_back_only_for_what_you_send() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    h.now_doing(agent, session, Activity::Idle).await;
    let snooze = InboxChange::Snooze {
        until: after(h.epoch, HOUR),
    };
    assert_eq!(h.file(agent, snooze).await, ActionOutcome::Done);

    // Its CLI starts a turn you didn't send, such as maxd's own nudge.
    h.clock.advance(Duration::from_secs(30));
    let now = h.clock.wall();
    let turn = Say {
        agents: vec![Agent {
            last_activity_at: now,
            ..h.reported(agent, Activity::Busy)
        }],
        ..Say::default()
    };
    h.say(session, &turn).await;
    eventually("the turn", || h.agent(agent).last_activity_at == now).await;
    tokio::time::sleep(QUIET).await;
    assert!(h.inbox(agent).snoozed_until.is_some());
}

#[tokio::test]
async fn snoozes_that_ended_while_maxd_was_down_wake_when_it_starts() {
    let mut h = Harness::new();
    let early = h.outside("s1", Activity::Idle);
    let late = h.outside("s2", Activity::Idle);
    let ends = after(h.epoch, HOUR);
    let later = after(h.epoch, 3 * HOUR);
    let snooze = |until| InboxChange::Snooze { until };
    assert_eq!(h.file(early, snooze(ends)).await, ActionOutcome::Done);
    assert_eq!(h.file(late, snooze(later)).await, ActionOutcome::Done);

    h.stop().await;
    h.clock.advance(2 * HOUR);
    h.boot();
    eventually("the snooze that ended waking", || {
        h.hub.inbox(early).woke_at == Some(ends)
    })
    .await;
    assert_eq!(h.hub.inbox(late).snoozed_until, Some(later));

    h.clock.advance(HOUR);
    eventually("the other snooze waking on time", || {
        h.hub.inbox(late).woke_at == Some(later)
    })
    .await;
    assert_eq!(
        h.kept_inbox(late).and_then(|inbox| inbox.woke_at),
        Some(later)
    );
    h.outside("s1", Activity::Idle);
    assert_eq!(
        h.inbox(early).woke_at,
        Some(ends),
        "an agent seen again shows it"
    );
}

#[tokio::test]
async fn auto_settle_settles_an_idle_agent_once_its_work_is_saved() {
    let h = Harness::new();
    let agent = h.outside("s1", Activity::Idle);

    // Short of the device's five days, nothing is checked.
    h.clock.advance(4 * DAY);
    tokio::time::sleep(QUIET).await;
    assert!(h.fakes.unsaved.asked().is_empty());

    // Once they're up, its worktree is, and work the check can't tell
    // about keeps it.
    h.clock.advance(DAY + SWEEP_EVERY);
    eventually("the first check", || h.fakes.unsaved.asked().len() == 1).await;
    assert_eq!(h.fakes.unsaved.asked(), [h.checkout()]);
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.inbox(agent), model::Inbox::default());

    // So does work it hasn't saved, and the check waits a while before it
    // runs again.
    h.fakes.unsaved.answer(h.checkout(), Some(true));
    h.clock.advance(SWEEP_EVERY);
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.fakes.unsaved.asked().len(), 1);
    h.clock.advance(RECHECK_AFTER);
    eventually("the second check", || h.fakes.unsaved.asked().len() == 2).await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.inbox(agent).settled, None);

    // With everything saved, it settles as of its last activity, and only
    // moves in the list.
    h.fakes.unsaved.answer(h.checkout(), Some(false));
    h.clock.advance(RECHECK_AFTER);
    eventually("the settle", || h.inbox(agent).settled.is_some()).await;
    assert_eq!(
        h.inbox(agent).settled,
        Some(Settled {
            at: h.epoch,
            by: Settler::Auto,
        })
    );
    assert_eq!(
        h.kept_inbox(agent).and_then(|inbox| inbox.settled),
        h.inbox(agent).settled
    );
    assert_eq!(h.agent(agent).control, Control::Observed);
    assert!(h.holders.sessions().is_empty());
}

#[tokio::test]
async fn auto_settle_leaves_agents_you_keep_in_view_and_agents_at_work() {
    let mut h = Harness::new();
    h.fakes.unsaved.answer(h.checkout(), Some(false));
    let plain = h.outside("plain", Activity::Idle);
    let pinned = h.outside("pinned", Activity::Idle);
    let kept = h.outside("kept", Activity::Idle);
    let off = h.outside("off", Activity::Idle);
    let snoozed = h.outside("snoozed", Activity::Idle);
    let busy = h.outside("busy", Activity::Busy);
    let asking = h.outside("asking", needs_input());
    let limited = h.outside(
        "limited",
        Activity::WaitingForLimitReset { resets_at: None },
    );
    let pin = InboxChange::Pin { key: "a0".into() };
    assert_eq!(h.file(pinned, pin).await, ActionOutcome::Done);
    assert_eq!(h.file(kept, InboxChange::Settle).await, ActionOutcome::Done);
    assert_eq!(
        h.file(kept, InboxChange::Unsettle).await,
        ActionOutcome::Done
    );
    let auto_off = InboxChange::AutoSettle { on: false };
    assert_eq!(h.file(off, auto_off).await, ActionOutcome::Done);
    let snooze = InboxChange::Snooze {
        until: after(h.epoch, 10 * DAY),
    };
    assert_eq!(h.file(snoozed, snooze).await, ActionOutcome::Done);

    h.clock.advance(5 * DAY + SWEEP_EVERY);
    eventually("the plain agent settling", || {
        h.inbox(plain).settled.is_some()
    })
    .await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(
        h.fakes.unsaved.asked(),
        [h.checkout()],
        "only the agent that may settle has its worktree checked"
    );
    for agent in [pinned, kept, off, snoozed, busy, asking, limited] {
        assert_eq!(h.inbox(agent).settled, None, "{agent}");
    }
}

#[tokio::test]
async fn a_projects_own_days_count_and_a_run_that_ends_is_looked_at_at_once() {
    let mut h = Harness::new();
    h.fakes.unsaved.answer(h.checkout(), Some(false));
    let agent = h.outside("s1", Activity::Busy);
    let project = match h.hub.get(&EntityKey::Workspace(h.agent(agent).workspace)) {
        Some(Entity::Workspace(workspace)) => workspace.project.expect("a folder has a project"),
        other => panic!("no workspace: {other:?}"),
    };
    h.keep(Entity::ProjectSettings(ProjectSettings {
        project,
        device: device().id,
        resume_on_restart: true,
        ignored_ports: Vec::new(),
        git_author: None,
        auto_settle_days: Some(1),
        settle_on_merge: true,
        workflow_env: true,
        memory_soft: None,
        workflow_env_allowed: None,
    }));

    h.clock.advance(DAY + SWEEP_EVERY);
    tokio::time::sleep(QUIET).await;
    assert!(
        h.fakes.unsaved.asked().is_empty(),
        "a busy agent isn't checked"
    );

    // The observer sees the run end late, a day after its last activity.
    // Auto-settle looks right away, without waiting for its beat.
    h.report(agent, |agent| agent.activity = Activity::Idle);
    eventually("the settle", || h.inbox(agent).settled.is_some()).await;
    assert_eq!(
        h.inbox(agent).settled,
        Some(Settled {
            at: h.epoch,
            by: Settler::Auto,
        })
    );
}

#[tokio::test]
async fn auto_settle_stops_an_agent_maxhq_holds() {
    let mut h = Harness::new();
    h.fakes.unsaved.answer(h.checkout(), Some(false));
    let (agent, session) = h.start_claude(None).await;
    h.now_doing(agent, session, Activity::Idle).await;
    let since = h.agent(agent).last_activity_at;

    h.clock.advance(5 * DAY + SWEEP_EVERY);
    eventually("the settle", || h.inbox(agent).settled.is_some()).await;
    assert_eq!(
        h.inbox(agent).settled,
        Some(Settled {
            at: since,
            by: Settler::Auto,
        })
    );
    assert_eq!(h.fakes.unsaved.asked(), [h.checkout()]);
    eventually("the pause", || h.activity(agent) == paused()).await;
    eventually("the stop", || h.holders.stops(session) == [STOP_GRACE]).await;
}

#[tokio::test]
async fn an_agent_waiting_for_its_limit_snoozes_until_the_reset_when_the_device_says_to() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let reset = after(h.epoch, HOUR);
    let waiting = Activity::WaitingForLimitReset {
        resets_at: Some(reset),
    };
    h.now_doing(agent, session, waiting.clone()).await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.inbox(agent), model::Inbox::default(), "it's off at first");
    h.now_doing(agent, session, Activity::Busy).await;

    let settings = DeviceSettings {
        device: device().id,
        default_permission: PermissionMode::Auto,
        workspace_network: None,
        workspace_pool: None,
        auto_settle_days: 5,
        snooze_at_limit: true,
        continue_claude_at_limit: false,
        continue_codex_at_limit: false,
        claude_effort: None,
        codex_effort: None,
        placement: Default::default(),
        tool_updates: Vec::new(),
        cursor_usage: false,
        keep_awake: Default::default(),
        test_hosts: Default::default(),
        skill_library: None,
        network: Default::default(),
        idle_release_minutes: 60,
        memory_items: 256,
        history_mib: 256,
        claude_setup_token: false,
        shell_integration: true,
        read_other_agents: true,
        dispatcher_acts: true,
        chat_gap_minutes: 30,
        local_models: Default::default(),
    };
    let outcome = h
        .act(|key| ClientMessage::SetDeviceSettings { key, settings })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    h.now_doing(agent, session, waiting.clone()).await;
    eventually("the snooze", || h.inbox(agent).snoozed_until == Some(reset)).await;
    assert_eq!(h.inbox(agent).snoozed_at, Some(h.epoch));

    // Back at work before the reset, it wakes.
    h.now_doing(agent, session, Activity::Busy).await;
    eventually("the wake", || h.inbox(agent).snoozed_until.is_none()).await;
    assert_eq!(h.inbox(agent).woke_at, Some(h.epoch));

    // Otherwise the snooze ends with the wait.
    h.now_doing(agent, session, waiting.clone()).await;
    eventually("the second snooze", || {
        h.inbox(agent).snoozed_until == Some(reset)
    })
    .await;
    h.clock.advance(HOUR);
    eventually("the wake at the reset", || {
        h.inbox(agent).woke_at == Some(reset)
    })
    .await;

    // An agent you settled stays settled.
    let outside = h.outside("s1", Activity::Idle);
    assert_eq!(
        h.file(outside, InboxChange::Settle).await,
        ActionOutcome::Done
    );
    let later = after(h.clock.wall(), HOUR);
    h.report(outside, |agent| {
        agent.activity = Activity::WaitingForLimitReset {
            resets_at: Some(later),
        };
    });
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.inbox(outside).snoozed_until, None);
    assert!(h.inbox(outside).settled.is_some());
}

#[tokio::test]
async fn an_agent_thats_gone_takes_its_inbox_with_it() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    h.now_doing(agent, session, Activity::Idle).await;
    let pin = InboxChange::Pin { key: "a0".into() };
    assert_eq!(h.file(agent, pin).await, ActionOutcome::Done);
    assert!(h.kept_inbox(agent).is_some());

    let say = Say {
        gone: vec![agent],
        ..Say::default()
    };
    h.say(session, &say).await;
    eventually("the agent going", || {
        h.hub.get(&EntityKey::Agent(agent)).is_none()
    })
    .await;
    assert_eq!(h.kept_inbox(agent), None);
    assert_eq!(h.hub.inbox(agent), model::Inbox::default());
}

/// Has every agent pinned, and waits for each answer.
async fn pin(h: &mut Harness, agents: &[AgentId]) {
    for agent in agents {
        let pin = InboxChange::Pin { key: "a0".into() };
        assert_eq!(h.file(*agent, pin).await, ActionOutcome::Done);
    }
}

/// The observers stop reporting an agent.
fn leave(agent: AgentId) -> Change {
    Change::Remove {
        key: EntityKey::Agent(agent),
    }
}

#[tokio::test]
async fn an_agent_you_started_yourself_takes_its_inbox_once_its_gone_for_good() {
    let mut h = Harness::new();
    let mut first = h.hub.first_pass();
    let agent = h.outside("s1", Activity::Idle);
    pin(&mut h, &[agent]).await;
    h.hub.apply([leave(agent)]);

    // Until every observer has reported, it may only not be seen yet.
    h.clock.advance(SWEEP_EVERY);
    tokio::time::sleep(QUIET).await;
    h.clock.advance(GONE_FOR);
    tokio::time::sleep(QUIET).await;
    assert!(h.kept_inbox(agent).is_some());
    assert!(h.db.missing_since().unwrap().is_empty(), "no count starts");

    // Once they have, the next beat starts the count.
    first.done();
    h.clock.advance(SWEEP_EVERY);
    tokio::time::sleep(QUIET).await;
    h.clock.advance(GONE_FOR - Duration::from_secs(1));
    tokio::time::sleep(QUIET).await;
    assert!(h.kept_inbox(agent).is_some(), "not before its time is up");

    h.clock.advance(SWEEP_EVERY);
    eventually("the inbox going", || h.kept_inbox(agent).is_none()).await;
    assert_eq!(h.hub.inbox(agent), model::Inbox::default());
}

#[tokio::test]
async fn an_agent_back_before_its_time_keeps_its_inbox_and_its_count_starts_over() {
    let mut h = Harness::new();
    h.hub.first_pass().done();
    let agent = h.outside("s1", Activity::Idle);
    pin(&mut h, &[agent]).await;
    h.hub.apply([leave(agent)]);
    h.clock.advance(SWEEP_EVERY);
    eventually("the count starting", || {
        h.db.missing_since().unwrap().contains_key(&agent)
    })
    .await;

    // You resume the session a day before its time is up.
    let day = Duration::from_secs(24 * 60 * 60);
    h.clock.advance(GONE_FOR - day);
    tokio::time::sleep(QUIET).await;
    h.outside("s1", Activity::Idle);
    assert_eq!(h.inbox(agent).pin.map(|pin| pin.key), Some("a0".to_owned()));
    h.clock.advance(SWEEP_EVERY);
    eventually("the count stopping", || {
        h.db.missing_since().unwrap().is_empty()
    })
    .await;

    // Gone again, it has a full count ahead of it.
    h.hub.apply([leave(agent)]);
    h.clock.advance(SWEEP_EVERY);
    tokio::time::sleep(QUIET).await;
    h.clock.advance(2 * day);
    tokio::time::sleep(QUIET).await;
    assert!(h.kept_inbox(agent).is_some());
}

#[tokio::test]
async fn the_count_toward_gone_for_good_survives_a_restart() {
    let mut h = Harness::new();
    h.hub.first_pass().done();
    let agent = h.outside("s1", Activity::Idle);
    pin(&mut h, &[agent]).await;
    h.hub.apply([leave(agent)]);
    h.clock.advance(SWEEP_EVERY);
    eventually("the count starting", || {
        h.db.missing_since().unwrap().contains_key(&agent)
    })
    .await;
    let half = GONE_FOR / 2;
    h.clock.advance(half);
    tokio::time::sleep(QUIET).await;

    // `maxd` restarts halfway, and the count carries on from there.
    h.restart().await;
    h.hub.first_pass().done();
    h.clock.advance(SWEEP_EVERY);
    tokio::time::sleep(QUIET).await;
    assert!(h.kept_inbox(agent).is_some());
    h.clock.advance(half);
    eventually("the inbox going", || h.kept_inbox(agent).is_none()).await;
}

#[tokio::test]
async fn after_a_restart_only_the_inboxes_of_agents_no_one_has_go() {
    let mut h = Harness::new();
    let outside = h.outside("s1", Activity::Idle);
    let (held, session) = h.start_claude(None).await;
    h.now_doing(held, session, Activity::Idle).await;
    pin(&mut h, &[outside, held]).await;
    // An agent held under a record this `maxd` can't read.
    let unreadable = AgentId::derive("unreadable");
    let pinned = h.kept_inbox(outside).expect("the pin is kept");
    h.db.put_inbox(unreadable, &pinned).unwrap();
    h.db.execute(&format!(
        "INSERT INTO held (agent, record) VALUES ('{unreadable}', 'not json')"
    ))
    .unwrap();

    // The outside agent isn't reported again, and nothing goes until the
    // observers have reported.
    h.restart().await;
    let mut first = h.hub.first_pass();
    eventually("the held agent back", || {
        h.hub.get(&EntityKey::Agent(held)).is_some()
    })
    .await;
    h.clock.advance(SWEEP_EVERY);
    tokio::time::sleep(QUIET).await;
    h.clock.advance(GONE_FOR);
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.kept_inbox(outside), Some(pinned.clone()));

    first.done();
    h.clock.advance(SWEEP_EVERY);
    tokio::time::sleep(QUIET).await;
    h.clock.advance(GONE_FOR);
    eventually("the outside agent's inbox going", || {
        h.kept_inbox(outside).is_none()
    })
    .await;
    assert!(h.kept_inbox(held).is_some());
    assert_eq!(
        h.kept_inbox(unreadable),
        Some(pinned),
        "its agent may still be held"
    );
}

#[tokio::test]
async fn no_inbox_goes_while_its_unknown_which_agents_are_held() {
    let mut h = Harness::new();
    h.hub.first_pass().done();
    let agent = h.outside("s1", Activity::Idle);
    pin(&mut h, &[agent]).await;
    h.hub.apply([leave(agent)]);
    h.db.execute("INSERT INTO held (agent, record) VALUES ('not-an-id', '{}')")
        .unwrap();
    h.clock.advance(SWEEP_EVERY);
    tokio::time::sleep(QUIET).await;
    h.clock.advance(GONE_FOR);
    tokio::time::sleep(QUIET).await;
    assert!(h.kept_inbox(agent).is_some());

    // Once the table can say, it goes at the next beat.
    h.db.execute("DELETE FROM held WHERE agent = 'not-an-id'")
        .unwrap();
    h.clock.advance(SWEEP_EVERY);
    eventually("the inbox going", || h.kept_inbox(agent).is_none()).await;
}

/// The origin of an agent `caller` started on `device` through the agent
/// tools.
fn by(caller: AgentId, device: DeviceId) -> AgentOrigin {
    AgentOrigin {
        permission: PermissionId::derive("permission"),
        by: Actor::Agent { agent: caller },
        device,
        level: 1,
        wake: false,
        container: None,
    }
}

/// An outside agent `caller` started on `device` through the agent tools.
fn started(
    h: &Harness,
    session: &str,
    caller: AgentId,
    device: DeviceId,
    activity: Activity,
) -> AgentId {
    let agent = h.outside(session, activity);
    h.report(agent, |agent| agent.origin = Some(by(caller, device)));
    agent
}

#[tokio::test]
async fn settling_an_agent_settles_the_done_agents_it_started_and_theirs() {
    let mut h = Harness::new();
    h.fakes.unsaved.answer(h.checkout(), Some(false));
    let here = device().id;
    let caller = h.outside("caller", Activity::Idle);
    let done = started(&h, "done", caller, here, Activity::Idle);
    let below = started(&h, "below", done, here, Activity::Idle);
    let busy = started(&h, "busy", caller, here, Activity::Busy);
    let asking = started(&h, "asking", caller, here, needs_input());
    let pinned = started(&h, "pinned", caller, here, Activity::Idle);
    pin(&mut h, &[pinned]).await;
    let other_device = DeviceId::derive("other");
    let elsewhere = started(&h, "elsewhere", caller, other_device, Activity::Idle);
    let unsaved = started(&h, "unsaved", caller, here, Activity::Idle);
    let other = h.folder.with_file_name("other");
    fs::create_dir_all(&other).expect("the other folder can be made");
    let workspace = h.hub.locate_folder(&other);
    h.report(unsaved, |agent| agent.workspace = workspace);
    h.fakes.unsaved.answer(Checkout::Folder(other), Some(true));
    // One you kept in view goes along, and one you settled before keeps
    // its own settle.
    assert_eq!(h.file(done, InboxChange::Settle).await, ActionOutcome::Done);
    assert_eq!(
        h.file(done, InboxChange::Unsettle).await,
        ActionOutcome::Done
    );
    let before = started(&h, "before", caller, here, Activity::Idle);
    assert_eq!(
        h.file(before, InboxChange::Settle).await,
        ActionOutcome::Done
    );
    let own = h.inbox(before).settled;
    h.clock.advance(Duration::from_secs(60));

    assert_eq!(
        h.file(caller, InboxChange::Settle).await,
        ActionOutcome::Done
    );
    let settled = h.inbox(caller).settled;
    assert_ne!(settled, own);
    eventually("the started agents settling along", || {
        h.inbox(done).settled == settled && h.inbox(below).settled == settled
    })
    .await;
    assert_eq!(h.kept_inbox(below).and_then(|inbox| inbox.settled), settled);
    tokio::time::sleep(QUIET).await;
    for agent in [busy, asking, pinned, elsewhere, unsaved] {
        assert_eq!(h.inbox(agent).settled, None, "{agent}");
    }
    assert_eq!(h.inbox(before).settled, own);

    // Bringing the caller back brings back only the ones that went along.
    assert_eq!(
        h.file(caller, InboxChange::Unsettle).await,
        ActionOutcome::Done
    );
    for agent in [done, below] {
        assert_eq!(h.inbox(agent).settled, None, "{agent}");
        assert!(h.inbox(agent).kept_active, "{agent}");
    }
    assert_eq!(h.inbox(before).settled, own);
}

#[tokio::test]
async fn auto_settle_takes_the_agents_an_agent_started_along_with_it() {
    let mut h = Harness::new();
    h.fakes.unsaved.answer(h.checkout(), Some(false));
    let here = device().id;
    let caller = h.outside("caller", Activity::Idle);
    let child = started(&h, "child", caller, here, Activity::Idle);
    let off = started(&h, "off", caller, here, Activity::Idle);
    let auto_off = InboxChange::AutoSettle { on: false };
    assert_eq!(h.file(off, auto_off).await, ActionOutcome::Done);
    // Both worked later than their caller, too recently to settle alone.
    h.clock.advance(4 * DAY);
    let later = after(h.epoch, 4 * DAY);
    for agent in [child, off] {
        h.report(agent, |agent| agent.last_activity_at = later);
    }

    h.clock.advance(DAY + SWEEP_EVERY);
    eventually("the caller settling", || h.inbox(caller).settled.is_some()).await;
    let settled = Some(Settled {
        at: h.epoch,
        by: Settler::Auto,
    });
    assert_eq!(h.inbox(caller).settled, settled);
    eventually("the child settling along", || {
        h.inbox(child).settled == settled
    })
    .await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(
        h.inbox(off).settled,
        None,
        "you turned auto-settle off for it"
    );
}

#[tokio::test]
async fn an_agent_maxhq_holds_stops_when_it_settles_along() {
    let mut h = Harness::new();
    h.fakes.unsaved.answer(h.checkout(), Some(false));
    let (caller, session) = h.start_claude(Some("Plan it")).await;
    h.moves(session, caller, Activity::Idle).await;
    h.grant(Actor::Agent { agent: caller }, Limits::default())
        .await;
    let (child, holder) = h.start_child(session, caller, "s1", json!({})).await;
    h.moves(holder, child, Activity::Idle).await;

    assert_eq!(
        h.file(caller, InboxChange::Settle).await,
        ActionOutcome::Done
    );
    let settled = h.inbox(caller).settled;
    eventually("the child settling along", || {
        h.inbox(child).settled == settled
    })
    .await;
    eventually("the pause", || h.activity(child) == paused()).await;
    eventually("the stop", || h.holders.stops(holder) == [STOP_GRACE]).await;
}
