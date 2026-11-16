// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Follows the behaviour of MIT-licensed code listed in THIRD_PARTY_NOTICES:
// a merge settles only with settle on merge on, a close always does, the PR
// must end at or after the agent started and your last message to it, and
// the agent must be resting, unpinned, not snoozed and not kept out of
// auto-settle. A turn you haven't read yet counts as resting. MaxHQ adds that
// no other linked PR is open or unread and that the link was there before
// the PR ended.

//! When a linked PR merging or closing settles its agent.

use maxhq_protocol::model::{Activity, Agent, Settled, Settler};
use maxhq_protocol::pulls::{PullLink, PullState};
use maxhq_protocol::time::Timestamp;

use crate::inbox::{is_snoozed, resting};

/// How a lead agent settles now that its PRs have ended, if one of them
/// settles it. `settle_on_merge` is the setting that applies to it, and
/// `last_message` when you last sent it a message.
pub fn settles(
    agent: &Agent,
    settle_on_merge: bool,
    last_message: Option<Timestamp>,
    now: Timestamp,
) -> Option<Settled> {
    let inbox = &agent.inbox;
    let candidate = agent.parent.is_none()
        && (resting(agent) || agent.activity == Activity::DoneUnread)
        && inbox.settled.is_none()
        && inbox.pin.is_none()
        && !inbox.kept_active
        && !inbox.auto_settle_off
        && !is_snoozed(inbox, now);
    if !candidate || super::any_open(&agent.pulls) {
        return None;
    }
    let (link, ended) = agent
        .pulls
        .iter()
        .filter(|link| super::shown(link))
        .filter_map(|link| Some((link, ended_at(link)?)))
        .max_by_key(|(_, ended)| *ended)?;
    let anchor = last_message.map_or(agent.started_at, |at| at.max(agent.started_at));
    let by = match link.snapshot.as_ref()?.state {
        PullState::Merged if settle_on_merge => Settler::Merged(link.key.clone()),
        PullState::Closed => Settler::Closed(link.key.clone()),
        _ => return None,
    };
    (link.at <= ended && ended >= anchor).then_some(Settled {
        at: agent.last_activity_at,
        by,
    })
}

/// When a link's PR merged or closed. GitHub gives no closing time in
/// the list, so a closed PR's is its last update.
fn ended_at(link: &PullLink) -> Option<Timestamp> {
    let snapshot = link.snapshot.as_ref()?;
    match snapshot.state {
        PullState::Open => None,
        PullState::Merged => snapshot.merged_at.or(Some(snapshot.updated_at)),
        PullState::Closed => Some(snapshot.updated_at),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pulls::fixtures::{link, snapshot};
    use maxhq_protocol::ids::{AgentId, WorkspaceId};
    use maxhq_protocol::model::{AgentLink, Control, Inbox, Pin, Vendor};
    use maxhq_protocol::pulls::{LinkSource, PullKey, PullSnapshot};

    const NOW: Timestamp = Timestamp(10_000);

    fn agent(pulls: Vec<PullLink>) -> Agent {
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
            title: "Fix it".into(),
            activity: Activity::Idle,
            context: None,
            started_at: Timestamp(100),
            last_activity_at: Timestamp(900),
            inbox: Inbox::default(),
            origin: None,
            pulls,
            taking_over: false,
            asleep: false,
            effort: None,
            running: None,
            moved: None,
        }
    }

    fn ended(number: u64, state: PullState, at: u64) -> PullLink {
        let snapshot = PullSnapshot {
            state,
            updated_at: Timestamp(at),
            merged_at: (state == PullState::Merged).then_some(Timestamp(at)),
            ..snapshot()
        };
        link(number, 200, Some(snapshot))
    }

    fn key(number: u64) -> PullKey {
        crate::pulls::key("owner/repo", number)
    }

    #[test]
    fn a_merge_settles_its_resting_agent() {
        let merged = agent(vec![ended(12, PullState::Merged, 1000)]);
        assert_eq!(
            settles(&merged, true, Some(Timestamp(500)), NOW),
            Some(Settled {
                at: Timestamp(900),
                by: Settler::Merged(key(12)),
            })
        );
        assert_eq!(settles(&merged, false, None, NOW), None);
        let closed = agent(vec![ended(12, PullState::Closed, 1000)]);
        assert_eq!(
            settles(&closed, false, None, NOW).map(|settled| settled.by),
            Some(Settler::Closed(key(12)))
        );
    }

    #[test]
    fn what_keeps_an_agent_from_settling() {
        let merged = || agent(vec![ended(12, PullState::Merged, 1000)]);
        let mut working = merged();
        working.activity = Activity::Busy;
        let mut pinned = merged();
        pinned.inbox.pin = Some(Pin {
            at: Timestamp(1),
            key: "a".into(),
        });
        let mut kept = merged();
        kept.inbox.kept_active = true;
        let mut off = merged();
        off.inbox.auto_settle_off = true;
        let mut child = merged();
        child.parent = Some(AgentId(uuid::Uuid::max()));
        let mut other_open = merged();
        other_open.pulls.push(link(13, 200, Some(snapshot())));
        let mut unread = merged();
        unread.pulls.push(link(14, 200, None));
        let mut linked_late = merged();
        linked_late.pulls[0].at = Timestamp(2000);
        let mut started_late = merged();
        started_late.started_at = Timestamp(1500);
        let cases = [
            ("working", working, None),
            ("pinned", pinned, None),
            ("kept active", kept, None),
            ("auto-settle off", off, None),
            ("a sub-agent", child, None),
            ("another PR open", other_open, None),
            ("another PR unread", unread, None),
            ("linked after the merge", linked_late, None),
            ("started after the merge", started_late, None),
            ("messaged after the merge", merged(), Some(Timestamp(1500))),
        ];
        for (name, agent, last_message) in cases {
            assert_eq!(settles(&agent, true, last_message, NOW), None, "{name}");
        }
    }

    #[test]
    fn the_latest_ended_pr_settles_and_a_dismissed_one_is_left_out() {
        let mut dismissed = link(9, 200, None);
        dismissed.source = LinkSource::Dismissed;
        let agent = agent(vec![
            ended(12, PullState::Merged, 1000),
            ended(13, PullState::Closed, 1200),
            dismissed,
        ]);
        assert_eq!(
            settles(&agent, true, None, NOW).map(|settled| settled.by),
            Some(Settler::Closed(key(13)))
        );
    }
}
