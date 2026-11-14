// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Scheduled tasks: what starts or messages an agent on a timer, on a
//! webhook request or on new GitHub events. Every path into a run goes
//! through [`Tasks::fire`].

use std::sync::Arc;

use futures::future::BoxFuture;
use maxhq_protocol::ids::{IdempotencyKey, TaskId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::tasks::Task;
use maxhq_protocol::time::Timestamp;
use tokio::sync::watch;

mod store;

/// What `maxd` says to a task request before scheduled tasks are built.
pub const NOT_BUILT: &str = "Scheduled tasks aren't built yet.";

/// The answer to a task request before scheduled tasks are built.
pub fn not_built() -> ActionOutcome {
    ActionOutcome::Refused {
        reason: NOT_BUILT.to_owned(),
    }
}

/// What made a task run.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub enum Fire {
    /// Its timer, which was due at this time.
    Timer(Timestamp),
    /// The user or an agent asked, with this request's idempotency key.
    Now(IdempotencyKey),
    /// A request to its webhook that passed its checks.
    Hook(Delivered),
    /// New events from its GitHub repository that passed its checks.
    Github(Delivered),
}

/// A webhook request or GitHub event that passed its task's checks, with
/// the prompt the task's template made from it. Coalesced requests come
/// as one.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct Delivered {
    /// The delivery's ID, or the first one's when they were coalesced.
    pub delivery: String,
    /// The prompt to run.
    pub prompt: String,
}

/// Why a fire didn't queue a run.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FireError {
    /// No task has this ID.
    Gone,
    /// The task is paused.
    Off,
    /// Too many of its runs are waiting.
    QueueFull,
    /// The device it targets isn't connected.
    Offline,
    /// It couldn't start, for this reason.
    Failed(String),
}

/// This device's tasks, for the webhook listener and the GitHub poller,
/// which the tasks package implements: it hands them the task list and
/// takes their fires.
#[allow(dead_code)]
pub trait Tasks: Send + Sync {
    /// Queues a run of `task`, or says why it didn't.
    fn fire(&self, task: TaskId, fire: Fire) -> BoxFuture<'_, Result<(), FireError>>;

    /// The tasks whose home is this device, sent again on every change.
    fn watch(&self) -> watch::Receiver<Arc<[Task]>>;
}

/// A [`Tasks`] for tests: it keeps each fire, gives one answer to all,
/// and sends whatever `tasks` is given.
#[cfg(test)]
pub struct FakeTasks {
    pub fired: std::sync::Mutex<Vec<(TaskId, Fire)>>,
    pub answer: std::sync::Mutex<Result<(), FireError>>,
    pub tasks: watch::Sender<Arc<[Task]>>,
}

#[cfg(test)]
impl Default for FakeTasks {
    fn default() -> Self {
        Self {
            fired: Default::default(),
            answer: std::sync::Mutex::new(Ok(())),
            tasks: watch::Sender::new(Arc::from([])),
        }
    }
}

#[cfg(test)]
impl Tasks for FakeTasks {
    fn fire(&self, task: TaskId, fire: Fire) -> BoxFuture<'_, Result<(), FireError>> {
        self.fired.lock().unwrap().push((task, fire));
        let answer = self.answer.lock().unwrap().clone();
        Box::pin(std::future::ready(answer))
    }

    fn watch(&self) -> watch::Receiver<Arc<[Task]>> {
        self.tasks.subscribe()
    }
}
