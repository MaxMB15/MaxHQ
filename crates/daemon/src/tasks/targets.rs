// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Telling other devices which of their agents this device's tasks keep
//! active. Auto-settle leaves be an agent an enabled task sends its runs
//! to, and the agent's own device decides that, so this device tells
//! each other one the agents there its enabled tasks reach: all of them
//! each time, or none to clear what it said. It tells a device when that
//! changes, when a link to it comes up, and in a round at start and
//! every hour, which says it again to each device with agents to keep,
//! so the other side's copy doesn't lapse, and to each that hasn't taken
//! what it was told. One send per device runs at a time, and a change
//! while it runs has it send once more. A device too old for the request
//! refuses it and just lets its agents settle.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use maxhq_core::inbox;
use maxhq_protocol::agents::PeerRequest;
use maxhq_protocol::ids::{AgentId, DeviceId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::tasks::Task;
use tokio::sync::{mpsc, watch};
use tracing::debug;

use crate::agents::Unreachable;
use crate::peers::Peers;
use crate::session::clock::Clock;

/// How often every device with agents to keep is told again.
const EVERY: Duration = Duration::from_secs(60 * 60);

/// The agents on each other device this device's enabled tasks reach.
type Lists = BTreeMap<DeviceId, Vec<AgentId>>;

/// The devices this device tells, and how.
pub trait Reach: Send + Sync + 'static {
    /// The devices to tell in a round: those this device dials, and those
    /// with a link up now.
    fn devices(&self) -> BTreeSet<DeviceId>;
    /// The devices a link is up to, as links come and go.
    fn links(&self) -> watch::Receiver<BTreeSet<DeviceId>>;
    /// Tells `device` the agents there this device's enabled tasks reach.
    fn tell(
        &self,
        device: DeviceId,
        agents: Vec<AgentId>,
    ) -> BoxFuture<'static, Result<(), String>>;
}

impl Reach for Peers {
    fn devices(&self) -> BTreeSet<DeviceId> {
        self.note_targets()
    }

    fn links(&self) -> watch::Receiver<BTreeSet<DeviceId>> {
        Peers::links(self)
    }

    fn tell(
        &self,
        device: DeviceId,
        agents: Vec<AgentId>,
    ) -> BoxFuture<'static, Result<(), String>> {
        let peers = self.clone();
        Box::pin(async move {
            match peers
                .request(device, PeerRequest::TaskTargets { agents })
                .await
            {
                // A newer maxd's outcome still means it took them.
                Ok(ActionOutcome::Done | ActionOutcome::Unknown) => Ok(()),
                Ok(ActionOutcome::Refused { reason }) => Err(reason),
                Ok(other) => Err(format!("it answered oddly: {other:?}")),
                Err(Unreachable(reason)) => Err(reason),
            }
        })
    }
}

/// Starts telling the devices `reach` reaches which of their agents the
/// tasks in `tasks` keep active, until `maxd` stops. `me` is this device,
/// whose own agents its manager reads from the tasks itself.
pub fn tell(
    reach: Arc<dyn Reach>,
    tasks: watch::Receiver<Arc<[Task]>>,
    me: DeviceId,
    clock: Arc<dyn Clock>,
) {
    tokio::spawn(run(reach, tasks, me, clock));
}

/// Tells devices as tasks, links and the hour call for it.
async fn run(
    reach: Arc<dyn Reach>,
    mut tasks: watch::Receiver<Arc<[Task]>>,
    me: DeviceId,
    clock: Arc<dyn Clock>,
) {
    let mut links = reach.links();
    let mut up = links.borrow_and_update().clone();
    let mut now = lists(&tasks.borrow_and_update(), me);
    let mut sends = Sends::default();
    let (done, mut ended) = mpsc::unbounded_channel();
    // The first round is at start.
    let mut round = clock.now();
    loop {
        let mut wanted: BTreeSet<DeviceId> = BTreeSet::new();
        tokio::select! {
            Ok(()) = tasks.changed() => {
                let next = lists(&tasks.borrow_and_update(), me);
                wanted.extend(changed(&now, &next));
                now = next;
            }
            Ok(()) = links.changed() => {
                let linked = links.borrow_and_update().clone();
                wanted.extend(linked.difference(&up).filter(|device| sends.owed(&now, **device)));
                up = linked;
            }
            () = clock.sleep_until(round) => {
                wanted.extend(reach.devices().into_iter().filter(|device| sends.owed(&now, *device)));
                round = clock.now() + EVERY;
            }
            Some((device, told, result)) = ended.recv() => {
                if let Err(err) = &result {
                    debug!(%device, %err, "couldn't tell the device which agents tasks here reach");
                }
                if sends.ended(device, told, result.is_ok()) {
                    wanted.insert(device);
                }
            }
        }
        for device in wanted {
            if sends.want(device) {
                let agents = now.get(&device).cloned().unwrap_or_default();
                let (tell, done) = (reach.tell(device, agents.clone()), done.clone());
                tokio::spawn(async move {
                    let _ = done.send((device, agents, tell.await));
                });
            }
        }
    }
}

/// The agents on each other device that `tasks` keep active, each list in
/// order and without repeats.
fn lists(tasks: &[Task], me: DeviceId) -> Lists {
    let mut lists: BTreeMap<DeviceId, BTreeSet<AgentId>> = BTreeMap::new();
    for (device, agent) in inbox::automated(tasks) {
        if device != me {
            lists.entry(device).or_default().insert(agent);
        }
    }
    lists
        .into_iter()
        .map(|(device, agents)| (device, agents.into_iter().collect()))
        .collect()
}

/// The devices whose list differs between `before` and `after`.
fn changed(before: &Lists, after: &Lists) -> BTreeSet<DeviceId> {
    before
        .keys()
        .chain(after.keys())
        .filter(|device| before.get(device) != after.get(device))
        .copied()
        .collect()
}

/// Where the sends to each device stand.
#[derive(Default)]
struct Sends {
    /// The devices a send runs to.
    running: BTreeSet<DeviceId>,
    /// The devices to send to again once theirs ends.
    again: BTreeSet<DeviceId>,
    /// What each device last took.
    took: Lists,
}

impl Sends {
    /// Whether a send to `device` starts now. While one runs, it runs
    /// once more when that one ends.
    fn want(&mut self, device: DeviceId) -> bool {
        if self.running.contains(&device) {
            self.again.insert(device);
            return false;
        }
        self.running.insert(device);
        true
    }

    /// Notes that the send of `told` to `device` ended, and says whether
    /// another starts.
    fn ended(&mut self, device: DeviceId, told: Vec<AgentId>, ok: bool) -> bool {
        self.running.remove(&device);
        if ok {
            self.took.insert(device, told);
        } else {
            self.took.remove(&device);
        }
        self.again.remove(&device)
    }

    /// Whether a round or a new link tells `device`: it has agents to
    /// keep, so its copy stays fresh, or it hasn't taken that it has none.
    fn owed(&self, lists: &Lists, device: DeviceId) -> bool {
        lists.get(&device).is_some_and(|agents| !agents.is_empty())
            || self
                .took
                .get(&device)
                .is_none_or(|agents| !agents.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use maxhq_protocol::ids::{PermissionId, TaskId};
    use maxhq_protocol::tasks::{Maker, TaskTarget, Trigger};

    use super::*;
    use crate::session::clock::FakeClock;

    /// Devices to tell, which keep what they're told.
    struct Fake {
        devices: Mutex<BTreeSet<DeviceId>>,
        links: watch::Sender<BTreeSet<DeviceId>>,
        told: Mutex<Vec<(DeviceId, Vec<AgentId>)>>,
        /// The devices that can't be reached.
        down: Mutex<BTreeSet<DeviceId>>,
    }

    impl Fake {
        fn new(devices: &[DeviceId]) -> Arc<Self> {
            Arc::new(Self {
                devices: Mutex::new(devices.iter().copied().collect()),
                links: watch::Sender::new(BTreeSet::new()),
                told: Mutex::new(Vec::new()),
                down: Mutex::new(BTreeSet::new()),
            })
        }

        /// What was told, in order, and forgets it.
        fn take(&self) -> Vec<(DeviceId, Vec<AgentId>)> {
            std::mem::take(&mut *self.told.lock().unwrap())
        }
    }

    impl Reach for Fake {
        fn devices(&self) -> BTreeSet<DeviceId> {
            self.devices.lock().unwrap().clone()
        }

        fn links(&self) -> watch::Receiver<BTreeSet<DeviceId>> {
            self.links.subscribe()
        }

        fn tell(
            &self,
            device: DeviceId,
            agents: Vec<AgentId>,
        ) -> BoxFuture<'static, Result<(), String>> {
            let result = if self.down.lock().unwrap().contains(&device) {
                Err("That device is down.".to_owned())
            } else {
                self.told.lock().unwrap().push((device, agents));
                Ok(())
            };
            Box::pin(std::future::ready(result))
        }
    }

    fn mac() -> DeviceId {
        DeviceId::derive("mac")
    }

    fn devbox() -> DeviceId {
        DeviceId::derive("devbox")
    }

    fn laptop() -> DeviceId {
        DeviceId::derive("laptop")
    }

    fn agent(name: &str) -> AgentId {
        AgentId::derive(name)
    }

    fn task(name: &str, enabled: bool, device: DeviceId, agent: AgentId) -> Task {
        Task {
            id: TaskId::derive(name),
            name: name.into(),
            prompt: "Look".into(),
            trigger: Trigger::Every { minutes: 60 },
            target: TaskTarget::Agent {
                device,
                agent,
                fresh: false,
            },
            enabled,
            permission: PermissionId::derive("p"),
            made_by: Maker::User,
            next_run: None,
            last_run: None,
            approval_asked: None,
            by_dispatcher: false,
        }
    }

    /// Starts telling, with `tasks` as the first list.
    fn start(reach: &Arc<Fake>, tasks: Vec<Task>) -> (watch::Sender<Arc<[Task]>>, Arc<FakeClock>) {
        let (send, tasks) = watch::channel(Arc::from(tasks));
        let clock = Arc::new(FakeClock::new());
        tell(reach.clone(), tasks, mac(), clock.clone());
        (send, clock)
    }

    /// Waits until the fake was told `want`, in any order, and nothing
    /// else, then forgets it.
    async fn told(reach: &Fake, mut want: Vec<(DeviceId, Vec<AgentId>)>) {
        want.sort();
        let mut got = Vec::new();
        for _ in 0..200 {
            got.extend(reach.take());
            if got.len() >= want.len() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
        got.extend(reach.take());
        got.sort();
        assert_eq!(got, want);
    }

    /// Lets the loop go to sleep, so the clock passes its next round.
    async fn settle(clock: &FakeClock) {
        for _ in 0..200 {
            if clock.sleepers() > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    #[test]
    fn each_other_device_gets_its_agents_in_order_once() {
        let tasks = [
            task("b", true, devbox(), agent("b")),
            task("a", true, devbox(), agent("a")),
            task("again", true, devbox(), agent("a")),
            task("paused", false, laptop(), agent("c")),
            task("here", true, mac(), agent("d")),
        ];
        let mut want = vec![agent("a"), agent("b")];
        want.sort();
        assert_eq!(lists(&tasks, mac()), BTreeMap::from([(devbox(), want)]));
    }

    #[tokio::test]
    async fn the_first_round_tells_every_device_and_an_hour_later_only_those_with_agents() {
        let reach = Fake::new(&[devbox(), laptop()]);
        let (_tasks, clock) = start(&reach, vec![task("t", true, devbox(), agent("a"))]);
        told(
            &reach,
            vec![(devbox(), vec![agent("a")]), (laptop(), vec![])],
        )
        .await;

        settle(&clock).await;
        clock.advance(EVERY);
        told(&reach, vec![(devbox(), vec![agent("a")])]).await;
    }

    #[tokio::test]
    async fn a_changed_task_tells_only_the_devices_whose_agents_changed() {
        let reach = Fake::new(&[devbox(), laptop()]);
        let (tasks, _clock) = start(&reach, vec![task("t", true, devbox(), agent("a"))]);
        told(
            &reach,
            vec![(devbox(), vec![agent("a")]), (laptop(), vec![])],
        )
        .await;

        // A new task for the laptop tells only the laptop.
        tasks.send_replace(Arc::from(vec![
            task("t", true, devbox(), agent("a")),
            task("u", true, laptop(), agent("b")),
        ]));
        told(&reach, vec![(laptop(), vec![agent("b")])]).await;

        // A run that only changes a task's last run tells no one.
        let mut ran = task("t", true, devbox(), agent("a"));
        ran.prompt = "Look again".into();
        tasks.send_replace(Arc::from(vec![ran, task("u", true, laptop(), agent("b"))]));
        told(&reach, vec![]).await;

        // Pausing the devbox's task clears its agents there.
        tasks.send_replace(Arc::from(vec![
            task("t", false, devbox(), agent("a")),
            task("u", true, laptop(), agent("b")),
        ]));
        told(&reach, vec![(devbox(), vec![])]).await;
    }

    #[tokio::test]
    async fn a_new_link_and_a_failed_send_are_told_again() {
        let reach = Fake::new(&[devbox()]);
        reach.down.lock().unwrap().insert(devbox());
        let (_tasks, clock) = start(&reach, vec![task("t", true, laptop(), agent("a"))]);
        // The devbox is down and the laptop isn't one it dials yet.
        told(&reach, vec![]).await;

        // The laptop links in and hears of its agent.
        reach.links.send_replace(BTreeSet::from([laptop()]));
        told(&reach, vec![(laptop(), vec![agent("a")])]).await;

        // The devbox comes back, and the next round tells it it has none.
        reach.down.lock().unwrap().clear();
        reach.devices.lock().unwrap().insert(laptop());
        settle(&clock).await;
        clock.advance(EVERY);
        told(
            &reach,
            vec![(devbox(), vec![]), (laptop(), vec![agent("a")])],
        )
        .await;

        // Once it took that, a round leaves it out.
        settle(&clock).await;
        clock.advance(EVERY);
        told(&reach, vec![(laptop(), vec![agent("a")])]).await;
    }
}
