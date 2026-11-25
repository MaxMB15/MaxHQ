// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Workflow runs. One thread follows every run in the folders this
//! device's agents work in, keeps a [`WorkflowRun`] entity for each, and
//! streams the runs clients open. It only reads: it never runs a
//! workflow's scripts or writes into a run's folder.
//!
//! [`WorkflowRun`]: maxhq_protocol::workflow::WorkflowRun

mod disk;
pub mod env;
mod facts;
mod follow;
// The guest's half, which follows the runs in a container, runs on Unix
// alone.
#[cfg_attr(not(unix), allow(dead_code))]
pub mod guest;
mod notes;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use maxhq_protocol::ids::{AgentId, WorkspaceId};
use maxhq_protocol::message::{ActionOutcome, ServerMessage};
use maxhq_protocol::workflow::{RunKey, RunSnapshot};
use tokio::sync::{mpsc as tokio_mpsc, oneshot};

pub use env::Envs;
pub use notes::Notes;

use disk::Disk;
use facts::HubWorld;
use follow::{Detail, Follower, ViewerId, TICK};
use guest::{GuestLink, Links};

use crate::guestlink::GuestSender;
use crate::hub::Hub;
use crate::session::clock::Clock;

/// The refusal for every workflow request while this device follows no
/// runs.
pub const NOT_YET: &str = "Workflows aren't set up yet.";

/// How long a run file that doesn't parse waits before it's read again,
/// since its writer may be halfway through.
const RETRY_PAUSE: Duration = Duration::from_millis(20);

/// The answer to an app request about workflows while there are none.
pub fn refused() -> ActionOutcome {
    ActionOutcome::Refused {
        reason: NOT_YET.to_owned(),
    }
}

/// What the session manager and the follower share: the agents' notes,
/// and the descriptor variables for launches.
#[derive(Clone, Default)]
pub struct Shared {
    /// The prompt heads and commands of each agent.
    pub notes: Notes,
    /// Each followed folder's variables.
    pub envs: Envs,
}

/// What the follower's thread is asked.
enum Input {
    Open(ViewerId, RunKey, tokio_mpsc::UnboundedSender<ServerMessage>),
    Close(ViewerId, RunKey),
    Gone(ViewerId),
    Detail(RunKey, String, oneshot::Sender<Answer>),
    Guest(WorkspaceId, Box<RunSnapshot>, Vec<AgentId>),
    LinkUp(WorkspaceId, Arc<dyn GuestLink>),
}

/// The follower's answer for a node's details.
enum Answer {
    Done(ActionOutcome),
    /// The run is in a container: ask its guest, if its link is up.
    Ask(Option<Arc<dyn GuestLink>>),
}

struct Handle {
    inputs: mpsc::Sender<Input>,
    viewers: AtomicU64,
}

static FOLLOWING: OnceLock<Handle> = OnceLock::new();

/// Whether runs are followed here, which clients see as the `workflows`
/// capability.
pub fn following() -> bool {
    FOLLOWING.get().is_some()
}

/// Starts following runs, once.
pub fn install(hub: Hub, shared: Shared, clock: Arc<dyn Clock>) {
    let (inputs, rx) = mpsc::channel();
    let handle = Handle {
        inputs,
        viewers: AtomicU64::new(1),
    };
    if FOLLOWING.set(handle).is_err() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("workflows".to_owned())
        .spawn(move || {
            let device = hub.device().id;
            let world = HubWorld {
                hub,
                notes: shared.notes,
                clock,
            };
            let mut follower = Follower::new(device, shared.envs, RETRY_PAUSE);
            run(&mut follower, &mut Links::default(), &world, &rx);
        });
    if let Err(err) = spawned {
        tracing::warn!("couldn't start following workflow runs: {err}");
    }
}

/// Ticks and answers until every sender has gone.
fn run(follower: &mut Follower, links: &mut Links, world: &HubWorld, rx: &mpsc::Receiver<Input>) {
    let mut next = Instant::now();
    loop {
        match rx.recv_timeout(next.saturating_duration_since(Instant::now())) {
            Ok(Input::Open(viewer, key, sink)) => follower.open(viewer, key, sink),
            Ok(Input::Close(viewer, key)) => follower.close(viewer, &key),
            Ok(Input::Gone(viewer)) => follower.gone(viewer),
            Ok(Input::Detail(key, node, reply)) => {
                let answer = match follower.detail(&Disk, &key, &node) {
                    Detail::Here(outcome) => Answer::Done(outcome),
                    Detail::Guest(workspace) => Answer::Ask(links.get(workspace)),
                };
                let _ = reply.send(answer);
            }
            Ok(Input::Guest(workspace, snapshot, leads)) => {
                if let Err(why) = follower.take_guest(world, workspace, *snapshot, leads) {
                    tracing::warn!(%workspace, "dropped a workflow run from the guest: {why}");
                }
            }
            Ok(Input::LinkUp(workspace, link)) => {
                links.up(workspace, link);
                links.tick(world);
            }
            Err(RecvTimeoutError::Timeout) => {
                follower.tick(&Disk, world, Instant::now());
                links.tick(world);
                next = Instant::now() + TICK;
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Takes in a run a container's guest sent, with the leads it bound.
pub fn from_guest(workspace: WorkspaceId, snapshot: Box<RunSnapshot>, leads: Vec<AgentId>) {
    if let Some(handle) = FOLLOWING.get() {
        let _ = handle.inputs.send(Input::Guest(workspace, snapshot, leads));
    }
}

/// A container's link came up, so its guest gets its agents from now on.
pub fn guest_up(workspace: WorkspaceId, sender: GuestSender) {
    if let Some(handle) = FOLLOWING.get() {
        let link = guest::linked(workspace, sender);
        let _ = handle.inputs.send(Input::LinkUp(workspace, link));
    }
}

/// The descriptor's lists for one node, once the follower answers, or the
/// container's guest for a run in a container.
pub fn detail(key: RunKey, node: String) -> tokio::task::JoinHandle<ActionOutcome> {
    let (reply, answer) = oneshot::channel();
    let sent = FOLLOWING.get().is_some_and(|h| {
        let asked = Input::Detail(key.clone(), node.clone(), reply);
        h.inputs.send(asked).is_ok()
    });
    tokio::spawn(async move {
        if !sent {
            return refused();
        }
        match answered(answer, guest::DETAIL_WAIT).await {
            Ok(Answer::Done(outcome)) => outcome,
            Ok(Answer::Ask(link)) => guest::ask_detail(link, &key, &node, guest::DETAIL_WAIT).await,
            Err(refusal) => refusal,
        }
    })
}

/// The follower's answer, or the refusal to give instead. Its one thread can
/// be busy with a slow run, and the caller waits on this.
// The refusal goes straight back to the client, once per request.
#[allow(clippy::result_large_err)]
async fn answered<T>(answer: oneshot::Receiver<T>, wait: Duration) -> Result<T, ActionOutcome> {
    match tokio::time::timeout(wait, answer).await {
        Ok(Ok(answer)) => Ok(answer),
        Ok(Err(_)) => Err(refused()),
        Err(_) => Err(ActionOutcome::Refused {
            reason: "The workflow took too long to answer.".into(),
        }),
    }
}

/// One client's open runs. Dropping it closes them all.
pub struct Viewer {
    id: ViewerId,
    inputs: mpsc::Sender<Input>,
    tx: tokio_mpsc::UnboundedSender<ServerMessage>,
    rx: tokio_mpsc::UnboundedReceiver<ServerMessage>,
}

impl Viewer {
    /// A viewer, once the follower runs.
    pub fn new() -> Option<Self> {
        let handle = FOLLOWING.get()?;
        let (tx, rx) = tokio_mpsc::unbounded_channel();
        Some(Self {
            id: handle.viewers.fetch_add(1, Ordering::Relaxed),
            inputs: handle.inputs.clone(),
            tx,
            rx,
        })
    }

    /// Opens a run, or opens it afresh: the whole run comes first.
    pub fn open(&self, run: RunKey) {
        let _ = self.inputs.send(Input::Open(self.id, run, self.tx.clone()));
    }

    /// Stops a run's messages.
    pub fn close(&self, run: RunKey) {
        let _ = self.inputs.send(Input::Close(self.id, run));
    }

    /// The next message about an open run.
    pub async fn recv(&mut self) -> Option<ServerMessage> {
        self.rx.recv().await
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        let _ = self.inputs.send(Input::Gone(self.id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_follower_that_never_answers_gets_a_refusal() {
        let (_reply, answer) = oneshot::channel::<()>();
        let wait = answered(answer, Duration::from_millis(50));
        let given_up = tokio::time::timeout(Duration::from_secs(30), wait)
            .await
            .expect("gives up by itself");
        assert_eq!(
            given_up,
            Err(ActionOutcome::Refused {
                reason: "The workflow took too long to answer.".into()
            })
        );
    }

    #[test]
    fn a_refusal_says_workflows_are_not_set_up() {
        assert_eq!(
            refused(),
            ActionOutcome::Refused {
                reason: "Workflows aren't set up yet.".into()
            }
        );
    }
}
