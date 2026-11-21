// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The inbox rules `maxd` and the app's outbox share: what an
//! [`InboxChange`] does to an [`Inbox`], what brings an agent back by
//! itself, and when auto-settle settles one. U-8's guards sit
//! on top of the base rules.

use maxhq_protocol::agents::Actor;
use maxhq_protocol::ids::{AgentId, DeviceId};
use maxhq_protocol::model::{
    Activity, Agent, Inbox, InboxChange, PauseReason, Pin, Settled, Settler,
};
use maxhq_protocol::tasks::{Task, TaskTarget};
use maxhq_protocol::time::Timestamp;

const DAY_MS: u64 = 24 * 60 * 60 * 1000;

/// The inbox after a change. It doesn't check whether the agent may settle
/// or snooze: `maxd` refuses those before it applies the change.
pub fn apply(inbox: &Inbox, change: &InboxChange, now: Timestamp) -> Inbox {
    let mut next = inbox.clone();
    match change {
        InboxChange::Settle => settle(
            &mut next,
            Settled {
                at: now,
                by: Settler::You,
            },
        ),
        InboxChange::Unsettle => {
            next.settled = None;
            next.kept_active = true;
        }
        InboxChange::Snooze { until } => {
            next.snoozed_until = Some(*until);
            next.snoozed_at = Some(now);
        }
        InboxChange::Wake => wake(&mut next, now),
        InboxChange::DismissWoke => next.woke_at = None,
        InboxChange::Pin { key } => {
            if next.settled.take().is_some() {
                next.kept_active = true;
            }
            wake(&mut next, now);
            let at = next.pin.as_ref().map_or(now, |pin| pin.at);
            next.pin = Some(Pin {
                at,
                key: key.clone(),
            });
        }
        InboxChange::Unpin => next.pin = None,
        InboxChange::AutoSettle { on } => next.auto_settle_off = !on,
        InboxChange::ContinueAtLimit { on } => next.continue_at_limit = Some(*on),
        InboxChange::Restore { inbox } => {
            next = inbox.clone();
            if !is_snoozed(&next, now) {
                next.snoozed_until = None;
                next.snoozed_at = None;
            }
        }
    }
    next
}

/// Settles an inbox, which unpins it and ends its snooze. An inbox that is
/// already settled keeps its first settle.
pub fn settle(inbox: &mut Inbox, settled: Settled) {
    if inbox.settled.is_none() {
        inbox.settled = Some(settled);
    }
    inbox.kept_active = false;
    inbox.pin = None;
    inbox.snoozed_until = None;
    inbox.snoozed_at = None;
}

/// Ends a snooze and leaves the woke marker. An inbox that isn't snoozed
/// stays as it is.
pub fn wake(inbox: &mut Inbox, now: Timestamp) {
    if inbox.snoozed_until.take().is_some() {
        inbox.snoozed_at = None;
        inbox.woke_at = Some(now);
    }
}

/// A message you send brings the agent back: it unsettles it, ends its
/// snooze without a marker, and lets auto-settle watch it again.
pub fn on_message(inbox: &mut Inbox) {
    inbox.settled = None;
    inbox.kept_active = false;
    inbox.snoozed_until = None;
    inbox.snoozed_at = None;
}

/// Whether the inbox is snoozed now. A snooze whose time has passed has
/// ended, even before `maxd` clears it.
pub fn is_snoozed(inbox: &Inbox, now: Timestamp) -> bool {
    inbox.snoozed_until.is_some_and(|until| until > now)
}

/// Whether an agent that turns to this activity wakes from its snooze
/// early: it needs you, or it failed. A finished turn doesn't wake it.
pub fn wakes_early(activity: &Activity) -> bool {
    matches!(
        activity,
        Activity::NeedsInput { .. }
            | Activity::Error { .. }
            | Activity::Paused {
                reason: PauseReason::ResumesMadeNoProgress
            }
    )
}

/// Whether auto-settle settles a lead agent now. `days` is the setting that
/// applies to it, 0 for never. `unsaved` says whether its worktree has
/// changes or commits that aren't on the remote; not knowing counts as yes,
/// so auto-settle never settles work U-8 says to keep in view.
/// An agent with a linked PR that may still be open waits for the PR.
pub fn auto_settles(agent: &Agent, days: u16, unsaved: Option<bool>, now: Timestamp) -> bool {
    let inbox = &agent.inbox;
    let idle_for = now.0.saturating_sub(agent.last_activity_at.0);
    agent.parent.is_none()
        && days > 0
        && resting(agent)
        && !crate::pulls::any_open(&agent.pulls)
        && inbox.settled.is_none()
        && inbox.pin.is_none()
        && !inbox.kept_active
        && !inbox.auto_settle_off
        && !is_snoozed(inbox, now)
        && idle_for >= u64::from(days) * DAY_MS
        && unsaved == Some(false)
}

/// Whether an agent rests: it waits for a prompt, can't be reached, or
/// was paused for a reason other than a resume that went nowhere.
pub fn resting(agent: &Agent) -> bool {
    match &agent.activity {
        Activity::Idle | Activity::IdleCacheWarm { .. } | Activity::NotAccessible => true,
        Activity::Paused { reason } => *reason != PauseReason::ResumesMadeNoProgress,
        _ => false,
    }
}

/// Auto-settle's record: settled by it, dated to the agent's last activity
/// so the settled list sorts by when the work stopped.
pub fn auto_settled(agent: &Agent) -> Settled {
    Settled {
        at: agent.last_activity_at,
        by: Settler::Auto,
    }
}

/// Whether the agent is one `caller` started through the agent tools on
/// `device`, the device both run on. Sub-agents never are.
pub fn started_by(agent: &Agent, caller: AgentId, device: DeviceId) -> bool {
    agent.parent.is_none()
        && agent.id != caller
        && agent.origin.as_ref().is_some_and(|origin| {
            origin.device == device && origin.by == Actor::Agent { agent: caller }
        })
}

/// Whether a started agent settles along with the agent that started it,
/// which `settled` just settled. It goes only once it's done: one that
/// works, waits for its usage limit, needs you or failed stays, as does one
/// you pinned or snoozed, one with a PR that may still be open, and one
/// whose worktree may hold unsaved work, which `unsaved` says as it does
/// for [`auto_settles`]. When you didn't settle the caller yourself, one
/// you turned auto-settle off for stays too. One you kept in view by
/// unsettling it goes, as settling its caller is your call again.
pub fn goes_along(agent: &Agent, settled: &Settled, unsaved: Option<bool>, now: Timestamp) -> bool {
    let inbox = &agent.inbox;
    agent.parent.is_none()
        && (resting(agent) || agent.activity == Activity::DoneUnread)
        && !crate::pulls::any_open(&agent.pulls)
        && inbox.settled.is_none()
        && inbox.pin.is_none()
        && !is_snoozed(inbox, now)
        && (settled.by == Settler::You || !inbox.auto_settle_off)
        && unsaved == Some(false)
}

/// Whether the agent settled along with the agent that started it, whose
/// settle `settled` was: it holds that same record, as each one that goes
/// along takes its caller's.
pub fn settled_with(agent: &Agent, settled: &Settled) -> bool {
    agent.inbox.settled.as_ref() == Some(settled)
}

/// Each agent an enabled task sends its runs to, with the device it's on.
/// Auto-settle leaves these agents be: you keep them for the task. A task
/// that starts a new agent for each run keeps none.
pub fn automated<'a>(
    tasks: impl IntoIterator<Item = &'a Task>,
) -> impl Iterator<Item = (DeviceId, AgentId)> {
    tasks
        .into_iter()
        .filter(|task| task.enabled)
        .filter_map(|task| match &task.target {
            TaskTarget::Agent { device, agent } => Some((*device, *agent)),
            TaskTarget::New { .. } => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use maxhq_protocol::ids::{AgentId, AttentionId, WorkspaceId};
    use maxhq_protocol::model::{AgentLink, Control, Vendor};

    const NOW: Timestamp = Timestamp(100 * DAY_MS);

    fn at(days_ago: u64) -> Timestamp {
        Timestamp(NOW.0 - days_ago * DAY_MS)
    }

    fn agent(activity: Activity, idle_days: u64) -> Agent {
        Agent {
            id: AgentId(uuid::Uuid::nil()),
            workspace: WorkspaceId(uuid::Uuid::nil()),
            branch: None,
            parent: None,
            vendor: Vendor::Claude,
            account: None,
            model: None,
            permission: None,
            session: None,
            vendor_session_id: None,
            control: Control::Held,
            pid: None,
            link: AgentLink::Live,
            title: "fix the login".to_owned(),
            activity,
            context: None,
            started_at: at(30),
            last_activity_at: at(idle_days),
            inbox: Inbox::default(),
            origin: None,
            pulls: Vec::new(),
            taking_over: false,
            asleep: false,
            effort: None,
            running: None,
            moved: None,
            local: None,
        }
    }

    fn pinned(key: &str) -> Inbox {
        Inbox {
            pin: Some(Pin {
                at: at(3),
                key: key.to_owned(),
            }),
            ..Inbox::default()
        }
    }

    #[test]
    fn settling_unpins_and_ends_the_snooze() {
        let inbox = Inbox {
            snoozed_until: Some(at(0).after(std::time::Duration::from_secs(60))),
            snoozed_at: Some(at(1)),
            ..pinned("m")
        };
        let next = apply(&inbox, &InboxChange::Settle, NOW);
        assert_eq!(
            next.settled,
            Some(Settled {
                at: NOW,
                by: Settler::You
            })
        );
        assert_eq!(next.pin, None);
        assert_eq!(next.snoozed_until, None);
        assert_eq!(next.woke_at, None, "settling isn't waking");
    }

    #[test]
    fn settling_again_keeps_the_first_settle() {
        let first = apply(&Inbox::default(), &InboxChange::Settle, at(2));
        let again = apply(&first, &InboxChange::Settle, NOW);
        assert_eq!(again.settled.map(|settled| settled.at), Some(at(2)));
    }

    #[test]
    fn unsettling_keeps_auto_settle_away_until_a_message() {
        let settled = apply(&Inbox::default(), &InboxChange::Settle, at(1));
        let mut back = apply(&settled, &InboxChange::Unsettle, NOW);
        assert_eq!(back.settled, None);
        assert!(back.kept_active);
        on_message(&mut back);
        assert!(!back.kept_active);
    }

    #[test]
    fn pinning_brings_a_settled_or_snoozed_agent_back() {
        let snoozed = Inbox {
            settled: Some(Settled {
                at: at(1),
                by: Settler::Auto,
            }),
            snoozed_until: Some(at(0).after(std::time::Duration::from_secs(60))),
            snoozed_at: Some(at(1)),
            ..Inbox::default()
        };
        let next = apply(&snoozed, &InboxChange::Pin { key: "m".into() }, NOW);
        assert_eq!(next.settled, None);
        assert!(next.kept_active);
        assert_eq!(next.snoozed_until, None);
        assert_eq!(next.woke_at, Some(NOW));
        assert_eq!(
            next.pin.map(|pin| (pin.at, pin.key)),
            Some((NOW, "m".into()))
        );
    }

    #[test]
    fn pinning_a_pinned_agent_moves_it_and_keeps_when_it_was_pinned() {
        let next = apply(&pinned("m"), &InboxChange::Pin { key: "c".into() }, NOW);
        assert_eq!(
            next.pin.map(|pin| (pin.at, pin.key)),
            Some((at(3), "c".into()))
        );
    }

    #[test]
    fn waking_leaves_a_marker_until_dismissed() {
        let snoozed = apply(
            &Inbox::default(),
            &InboxChange::Snooze {
                until: at(0).after(std::time::Duration::from_secs(3600)),
            },
            NOW,
        );
        assert!(is_snoozed(&snoozed, NOW));
        let woke = apply(&snoozed, &InboxChange::Wake, NOW);
        assert!(!is_snoozed(&woke, NOW));
        assert_eq!(woke.woke_at, Some(NOW));
        let dismissed = apply(&woke, &InboxChange::DismissWoke, NOW);
        assert_eq!(dismissed.woke_at, None);
    }

    #[test]
    fn a_snooze_ends_on_time_without_maxd() {
        let snoozed = Inbox {
            snoozed_until: Some(at(1)),
            snoozed_at: Some(at(2)),
            ..Inbox::default()
        };
        assert!(!is_snoozed(&snoozed, NOW));
    }

    #[test]
    fn a_message_ends_a_snooze_without_a_marker() {
        let mut inbox = Inbox {
            snoozed_until: Some(at(0).after(std::time::Duration::from_secs(60))),
            snoozed_at: Some(at(1)),
            ..Inbox::default()
        };
        on_message(&mut inbox);
        assert_eq!(inbox, Inbox::default());
    }

    #[test]
    fn restoring_drops_a_snooze_that_has_ended() {
        let before = Inbox {
            snoozed_until: Some(at(1)),
            snoozed_at: Some(at(2)),
            ..pinned("m")
        };
        let settled = apply(&before, &InboxChange::Settle, NOW);
        let restored = apply(&settled, &InboxChange::Restore { inbox: before }, NOW);
        assert_eq!(restored, pinned("m"));
    }

    #[test]
    fn needing_you_or_failing_wakes_early_and_finishing_does_not() {
        let needs = Activity::NeedsInput {
            attention: AttentionId(uuid::Uuid::nil()),
        };
        assert!(wakes_early(&needs));
        assert!(wakes_early(&Activity::Error {
            reason: "exited".into()
        }));
        assert!(!wakes_early(&Activity::DoneUnread));
        assert!(!wakes_early(&Activity::Idle));
    }

    #[test]
    fn auto_settle_takes_an_agent_idle_for_the_set_days() {
        assert!(auto_settles(&agent(Activity::Idle, 5), 5, Some(false), NOW));
        assert!(!auto_settles(
            &agent(Activity::Idle, 4),
            5,
            Some(false),
            NOW
        ));
        let stopped = Activity::Paused {
            reason: PauseReason::ByYou,
        };
        assert!(auto_settles(&agent(stopped, 6), 5, Some(false), NOW));
    }

    #[test]
    fn auto_settle_keeps_what_u8_says_to_keep() {
        assert!(
            !auto_settles(&agent(Activity::Idle, 9), 0, Some(false), NOW),
            "off"
        );
        assert!(
            !auto_settles(&agent(Activity::Idle, 9), 5, Some(true), NOW),
            "unsaved"
        );
        assert!(
            !auto_settles(&agent(Activity::Idle, 9), 5, None, NOW),
            "unknown"
        );
        for activity in [
            Activity::DoneUnread,
            Activity::Busy,
            Activity::Starting,
            Activity::NeedsInput {
                attention: AttentionId(uuid::Uuid::nil()),
            },
            Activity::WaitingForLimitReset { resets_at: None },
            Activity::Error {
                reason: "exited".into(),
            },
            Activity::Paused {
                reason: PauseReason::ResumesMadeNoProgress,
            },
        ] {
            assert!(
                !auto_settles(&agent(activity.clone(), 9), 5, Some(false), NOW),
                "{activity:?}"
            );
        }
        let mut kept = agent(Activity::Idle, 9);
        kept.inbox = pinned("m");
        assert!(!auto_settles(&kept, 5, Some(false), NOW), "pinned");
        kept.inbox = Inbox {
            kept_active: true,
            ..Inbox::default()
        };
        assert!(
            !auto_settles(&kept, 5, Some(false), NOW),
            "unsettled by hand"
        );
        kept.inbox = Inbox {
            auto_settle_off: true,
            ..Inbox::default()
        };
        assert!(!auto_settles(&kept, 5, Some(false), NOW), "turned off");
        kept.inbox = Inbox {
            snoozed_until: Some(at(0).after(std::time::Duration::from_secs(60))),
            ..Inbox::default()
        };
        assert!(!auto_settles(&kept, 5, Some(false), NOW), "snoozed");
        let mut sub = agent(Activity::Idle, 9);
        sub.parent = Some(AgentId(uuid::Uuid::nil()));
        assert!(!auto_settles(&sub, 5, Some(false), NOW), "sub-agent");
        let mut waiting = agent(Activity::Idle, 9);
        waiting.pulls = vec![crate::pulls::fixtures::link(12, 0, None)];
        assert!(
            !auto_settles(&waiting, 5, Some(false), NOW),
            "a PR may be open"
        );
    }

    #[test]
    fn only_an_enabled_task_that_reaches_an_agent_keeps_it() {
        use maxhq_protocol::ids::{PermissionId, TaskId};
        use maxhq_protocol::tasks::{Maker, Trigger};

        let task = |name: &str, enabled: bool, target: TaskTarget| Task {
            id: TaskId::derive(name),
            name: name.to_owned(),
            prompt: "Look".to_owned(),
            trigger: Trigger::Every { minutes: 60 },
            target,
            enabled,
            permission: PermissionId::derive("p"),
            made_by: Maker::User,
            next_run: None,
            last_run: None,
            approval_asked: None,
        };
        let (mac, devbox) = (DeviceId::derive("mac"), DeviceId::derive("devbox"));
        let (here, there) = (AgentId::derive("here"), AgentId::derive("there"));
        let on = |device, agent| TaskTarget::Agent { device, agent };
        let spec = maxhq_protocol::message::AgentSpec {
            vendor: Vendor::Claude,
            account: maxhq_protocol::ids::AccountId::derive("claude"),
            place: maxhq_protocol::message::AgentPlace::Workspace {
                workspace: WorkspaceId(uuid::Uuid::nil()),
            },
            model: None,
            permission: None,
            title: None,
            first: None,
            branch: None,
            origin: None,
            effort: None,
        };
        let tasks = [
            task("here", true, on(mac, here)),
            task("there", true, on(devbox, there)),
            task("paused", false, on(mac, AgentId::derive("paused"))),
            task(
                "new",
                true,
                TaskTarget::New {
                    device: mac,
                    spec: Box::new(spec),
                },
            ),
        ];
        assert_eq!(
            automated(&tasks).collect::<Vec<_>>(),
            [(mac, here), (devbox, there)]
        );
    }

    #[test]
    fn auto_settle_dates_the_settle_to_the_last_activity() {
        let idle = agent(Activity::Idle, 7);
        let mut inbox = Inbox::default();
        settle(&mut inbox, auto_settled(&idle));
        assert_eq!(
            inbox.settled,
            Some(Settled {
                at: at(7),
                by: Settler::Auto
            })
        );
    }

    fn started(caller: AgentId, device: DeviceId) -> Agent {
        Agent {
            id: AgentId::derive("started"),
            origin: Some(maxhq_protocol::agents::AgentOrigin {
                permission: maxhq_protocol::ids::PermissionId::derive("p"),
                by: Actor::Agent { agent: caller },
                device,
                level: 1,
                wake: false,
                container: None,
            }),
            ..agent(Activity::Idle, 0)
        }
    }

    const YOURS: Settled = Settled {
        at: NOW,
        by: Settler::You,
    };

    #[test]
    fn a_started_agent_belongs_to_the_agent_that_started_it_here() {
        let (caller, mac) = (AgentId::derive("caller"), DeviceId::derive("mac"));
        let agent = started(caller, mac);
        assert!(started_by(&agent, caller, mac));
        assert!(!started_by(&agent, AgentId::derive("other"), mac));
        assert!(!started_by(&agent, caller, DeviceId::derive("devbox")));
        let mut sub = started(caller, mac);
        sub.parent = Some(caller);
        assert!(!started_by(&sub, caller, mac), "sub-agent");
        let by_key = Agent {
            origin: None,
            ..started(caller, mac)
        };
        assert!(!started_by(&by_key, caller, mac));
    }

    #[test]
    fn a_finished_started_agent_settles_along_with_its_caller() {
        let mac = DeviceId::derive("mac");
        let agent = started(AgentId::derive("caller"), mac);
        assert!(goes_along(&agent, &YOURS, Some(false), NOW));
        let unread = Agent {
            activity: Activity::DoneUnread,
            ..agent.clone()
        };
        assert!(goes_along(&unread, &YOURS, Some(false), NOW), "unread");
        let kept = Agent {
            inbox: Inbox {
                kept_active: true,
                ..Inbox::default()
            },
            ..agent.clone()
        };
        assert!(goes_along(&kept, &YOURS, Some(false), NOW), "kept in view");
        let paused = Agent {
            activity: Activity::Paused {
                reason: PauseReason::ByYou,
            },
            ..agent
        };
        assert!(goes_along(&paused, &YOURS, Some(false), NOW), "paused");
    }

    #[test]
    fn a_started_agent_that_isnt_done_stays_behind() {
        let agent = started(AgentId::derive("caller"), DeviceId::derive("mac"));
        assert!(!goes_along(&agent, &YOURS, Some(true), NOW), "unsaved");
        assert!(!goes_along(&agent, &YOURS, None, NOW), "unknown");
        for activity in [
            Activity::Busy,
            Activity::BusyInSubagents,
            Activity::Starting,
            Activity::NeedsInput {
                attention: AttentionId(uuid::Uuid::nil()),
            },
            Activity::WaitingForLimitReset { resets_at: None },
            Activity::Error {
                reason: "exited".into(),
            },
            Activity::Paused {
                reason: PauseReason::ResumesMadeNoProgress,
            },
        ] {
            let busy = Agent {
                activity: activity.clone(),
                ..agent.clone()
            };
            assert!(!goes_along(&busy, &YOURS, Some(false), NOW), "{activity:?}");
        }
        let with = |inbox: Inbox| Agent {
            inbox,
            ..agent.clone()
        };
        assert!(
            !goes_along(&with(pinned("m")), &YOURS, Some(false), NOW),
            "pinned"
        );
        let snoozed = Inbox {
            snoozed_until: Some(at(0).after(std::time::Duration::from_secs(60))),
            ..Inbox::default()
        };
        assert!(
            !goes_along(&with(snoozed), &YOURS, Some(false), NOW),
            "snoozed"
        );
        let before = Inbox {
            settled: Some(Settled {
                at: at(3),
                by: Settler::You,
            }),
            ..Inbox::default()
        };
        assert!(
            !goes_along(&with(before), &YOURS, Some(false), NOW),
            "settled before"
        );
        let mut waiting = agent.clone();
        waiting.pulls = vec![crate::pulls::fixtures::link(12, 0, None)];
        assert!(
            !goes_along(&waiting, &YOURS, Some(false), NOW),
            "a PR may be open"
        );
        let mut sub = agent;
        sub.parent = Some(AgentId::derive("caller"));
        assert!(!goes_along(&sub, &YOURS, Some(false), NOW), "sub-agent");
    }

    #[test]
    fn only_your_settle_takes_one_you_turned_auto_settle_off_for() {
        let off = Agent {
            inbox: Inbox {
                auto_settle_off: true,
                ..Inbox::default()
            },
            ..started(AgentId::derive("caller"), DeviceId::derive("mac"))
        };
        assert!(goes_along(&off, &YOURS, Some(false), NOW));
        let auto = Settled {
            at: at(4),
            by: Settler::Auto,
        };
        assert!(!goes_along(&off, &auto, Some(false), NOW));
    }

    #[test]
    fn an_agent_settled_with_its_caller_holds_the_callers_settle() {
        let mut agent = started(AgentId::derive("caller"), DeviceId::derive("mac"));
        assert!(!settled_with(&agent, &YOURS));
        settle(&mut agent.inbox, YOURS);
        assert!(settled_with(&agent, &YOURS));
        let earlier = Settled {
            at: at(1),
            by: Settler::You,
        };
        assert!(!settled_with(&agent, &earlier));
        let auto = Settled {
            at: NOW,
            by: Settler::Auto,
        };
        assert!(!settled_with(&agent, &auto));
    }
}
