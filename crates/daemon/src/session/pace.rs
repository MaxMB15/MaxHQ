// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! How often the manager sends each agent. A driver reports its agent
//! after every line the CLI prints, dozens of times a second while it
//! streams, and most of those reports only move the agent's last activity
//! time and its context. Clients show the time to the second at the finest
//! ("just now", then "12 s ago") and the context as a meter, so a change to
//! those alone waits: the context goes out at most four times a second and
//! the time once a second. The first change after a quiet spell goes at
//! once, and the newest report always goes out in the end. Any other
//! change, such as the activity, the title or the link, goes out at once
//! and takes along whatever waited, and so does a field added later.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use maxhq_protocol::ids::AgentId;
use maxhq_protocol::model::Agent;
use maxhq_protocol::state::{Change, Entity};

use super::Manager;

/// How often a change to an agent's context alone goes out.
const CONTEXT_EVERY: Duration = Duration::from_millis(250);

/// How often a change to an agent's last activity time alone goes out.
const ACTIVITY_EVERY: Duration = Duration::from_secs(1);

/// What the manager last sent of each agent, and what waits.
#[derive(Default)]
pub(super) struct Pace {
    // `Manager::publish` takes `&self`, as do many of its callers, so the
    // pace keeps its own lock.
    agents: Mutex<HashMap<AgentId, Sent>>,
}

/// An agent as last sent.
struct Sent {
    agent: Agent,
    at: Instant,
    /// The newest report not sent yet, and when it goes out.
    waiting: Option<(Agent, Instant)>,
}

impl Pace {
    /// The reports in `agents` that go out at `now`, in order. The others
    /// wait.
    pub(super) fn pass(&self, agents: Vec<Agent>, now: Instant) -> Vec<Agent> {
        let mut sent = self.lock();
        agents
            .into_iter()
            .filter_map(|mut agent| {
                let Some(last) = sent.get_mut(&agent.id) else {
                    sent.insert(agent.id, Sent::new(&agent, now));
                    return Some(agent);
                };
                let Some(every) = wait(&last.agent, &mut agent) else {
                    // Back to what clients have.
                    last.waiting = None;
                    return None;
                };
                let due = last.at + every;
                if due <= now {
                    *last = Sent::new(&agent, now);
                    return Some(agent);
                }
                last.waiting = Some((agent, due));
                None
            })
            .collect()
    }

    /// The reports that waited until `now`.
    fn due(&self, now: Instant) -> Vec<Agent> {
        let mut sent = self.lock();
        sent.values_mut()
            .filter(|last| last.waiting.as_ref().is_some_and(|(_, due)| *due <= now))
            .filter_map(|last| last.send(now))
            .collect()
    }

    /// The report waiting for `agent`, which then counts as sent at `now`.
    fn take(&self, agent: AgentId, now: Instant) -> Option<Agent> {
        self.lock().get_mut(&agent)?.send(now)
    }

    /// When the soonest waiting report goes out. None when nothing waits.
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.lock()
            .values()
            .filter_map(|last| last.waiting.as_ref().map(|(_, due)| *due))
            .min()
    }

    /// Forgets an agent that clients no longer have, with what waited.
    pub(super) fn forget(&self, agent: AgentId) {
        self.lock().remove(&agent);
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<AgentId, Sent>> {
        self.agents.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Sent {
    fn new(agent: &Agent, at: Instant) -> Self {
        Self {
            agent: agent.clone(),
            at,
            waiting: None,
        }
    }

    /// Sends the waiting report at `now`, if any.
    fn send(&mut self, now: Instant) -> Option<Agent> {
        let (agent, _) = self.waiting.take()?;
        *self = Self::new(&agent, now);
        Some(agent)
    }
}

/// How long after the last send a report may go out, or none when it
/// shows nothing new. A report that changes more than the time and the
/// context needn't wait.
fn wait(sent: &Agent, report: &mut Agent) -> Option<Duration> {
    // The rest is compared in place, since a copy for every line adds up.
    let context = std::mem::replace(&mut report.context, sent.context);
    let at = std::mem::replace(&mut report.last_activity_at, sent.last_activity_at);
    let more = report != sent;
    report.context = context;
    report.last_activity_at = at;
    if more {
        Some(Duration::ZERO)
    } else if context != sent.context {
        Some(CONTEXT_EVERY)
    } else if at != sent.last_activity_at {
        Some(ACTIVITY_EVERY)
    } else {
        None
    }
}

impl Manager {
    /// Sends agents to clients.
    pub(super) fn send_agents(&self, agents: Vec<Agent>) {
        if agents.is_empty() {
            return;
        }
        self.hub
            .apply_held(agents.into_iter().map(|agent| Change::Upsert {
                value: Entity::Agent(agent),
            }));
    }

    /// Sends the reports that waited until now.
    pub(super) fn send_due(&self) {
        self.send_agents(self.pace.due(self.clock.now()));
    }

    /// Sends what waits for `agent` now, ahead of a change to it that goes
    /// at once, so clients get both together.
    pub(super) fn send_waiting(&self, agent: AgentId) {
        if let Some(agent) = self.pace.take(agent, self.clock.now()) {
            self.send_agents(vec![agent]);
        }
    }
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::ids::WorkspaceId;
    use maxhq_protocol::model::{Activity, AgentLink, ContextUsage, Control, Vendor};
    use maxhq_protocol::time::Timestamp;

    use super::*;

    const MS: Duration = Duration::from_millis(1);

    /// The agent doing `activity`, last active at `at`, with `used` tokens
    /// of context.
    fn agent(activity: Activity, at: u64, used: u32) -> Agent {
        Agent {
            id: AgentId::derive("a"),
            workspace: WorkspaceId::derive("w"),
            parent: None,
            vendor: Vendor::Claude,
            account: None,
            model: None,
            permission: None,
            session: None,
            vendor_session_id: Some("s".into()),
            control: Control::Held,
            pid: Some(42),
            link: AgentLink::Live,
            title: "maxhq".into(),
            activity,
            context: Some(ContextUsage {
                used_tokens: used,
                window_tokens: 200_000,
            }),
            started_at: Timestamp(0),
            last_activity_at: Timestamp(at),
            branch: None,
            inbox: Default::default(),
            origin: None,
            pulls: Vec::new(),
            taking_over: false,
            asleep: false,
            effort: None,
            running: None,
        }
    }

    /// Streams 100 chunks over a second, each moving the time and the
    /// context, and returns what went out.
    fn stream(pace: &Pace, start: Instant, activity: impl Fn(u32) -> Activity) -> Vec<Agent> {
        let mut sent = Vec::new();
        for n in 0..100 {
            let now = start + MS * 10 * n;
            sent.extend(pace.due(now));
            let chunk = agent(activity(n), u64::from(n) * 10, 1000 + n);
            sent.extend(pace.pass(vec![chunk], now));
        }
        sent
    }

    #[test]
    fn a_fast_stream_goes_out_a_few_times_a_second_and_ends_on_its_last_chunk() {
        let pace = Pace::default();
        let start = Instant::now();
        let mut sent = stream(&pace, start, |_| Activity::Busy);
        let due = pace.deadline().expect("the last chunk waits");
        assert!(due <= start + MS * 1000, "it waits until {due:?}");
        sent.extend(pace.due(due));
        assert!(sent.len() <= 5, "{} upserts", sent.len());
        let last = sent.last().expect("the stream went out");
        assert_eq!(last.last_activity_at, Timestamp(990));
        assert_eq!(last.context.map(|context| context.used_tokens), Some(1099));
        assert_eq!(pace.deadline(), None);
    }

    #[test]
    fn a_change_to_the_activity_goes_at_once_with_what_waited() {
        let pace = Pace::default();
        let start = Instant::now();
        let sent = stream(&pace, start, |n| {
            if n < 50 {
                Activity::Busy
            } else {
                Activity::WaitingForLimitReset { resets_at: None }
            }
        });
        let limited = sent
            .iter()
            .find(|agent| agent.activity != Activity::Busy)
            .expect("the limit went out");
        assert_eq!(limited.last_activity_at, Timestamp(500));
        assert_eq!(
            limited.context.map(|context| context.used_tokens),
            Some(1050)
        );
    }

    #[test]
    fn the_time_alone_goes_out_once_a_second() {
        let pace = Pace::default();
        let start = Instant::now();
        assert_eq!(pace.pass(vec![agent(Activity::Busy, 0, 1)], start).len(), 1);
        for n in 1..100 {
            let report = agent(Activity::Busy, u64::from(n) * 10, 1);
            assert!(pace.pass(vec![report], start + MS * 10 * n).is_empty());
        }
        assert_eq!(pace.deadline(), Some(start + ACTIVITY_EVERY));
    }

    #[test]
    fn the_first_change_after_a_quiet_spell_goes_at_once() {
        let pace = Pace::default();
        let start = Instant::now();
        pace.pass(vec![agent(Activity::Busy, 0, 1)], start);
        let later = start + Duration::from_secs(5);
        let sent = pace.pass(vec![agent(Activity::Busy, 5000, 2)], later);
        assert_eq!(sent.len(), 1);
        assert_eq!(pace.deadline(), None);
    }

    #[test]
    fn a_quiet_agent_sends_nothing() {
        let pace = Pace::default();
        let start = Instant::now();
        pace.pass(vec![agent(Activity::Idle, 0, 1)], start);
        for n in 1..10 {
            let now = start + Duration::from_secs(n);
            assert!(pace.pass(vec![agent(Activity::Idle, 0, 1)], now).is_empty());
            assert!(pace.due(now).is_empty());
        }
        assert_eq!(pace.deadline(), None);
    }

    #[test]
    fn a_report_back_to_what_was_sent_stops_the_wait() {
        let pace = Pace::default();
        let start = Instant::now();
        pace.pass(vec![agent(Activity::Busy, 0, 1)], start);
        pace.pass(vec![agent(Activity::Busy, 0, 2)], start + MS);
        assert!(pace.deadline().is_some());
        pace.pass(vec![agent(Activity::Busy, 0, 1)], start + MS * 2);
        assert_eq!(pace.deadline(), None);
    }

    #[test]
    fn a_waiting_report_can_be_taken_and_a_forgotten_agent_starts_over() {
        let pace = Pace::default();
        let start = Instant::now();
        let first = agent(Activity::Busy, 0, 1);
        let id = first.id;
        pace.pass(vec![first.clone()], start);
        pace.pass(vec![agent(Activity::Busy, 10, 2)], start + MS * 10);
        let taken = pace.take(id, start + MS * 20).expect("a report waits");
        assert_eq!(taken.last_activity_at, Timestamp(10));
        assert_eq!(pace.deadline(), None);
        assert_eq!(pace.take(id, start + MS * 30), None);

        // Clients drop it, so the same report goes out again.
        pace.forget(id);
        assert_eq!(pace.pass(vec![first], start + MS * 40).len(), 1);
    }
}
