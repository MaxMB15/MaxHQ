// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Slow work a driver hands off the loop, such as reading a long
//! transcript when it meets a session already under way. A few jobs run
//! at once, so a restart with many agents doesn't hold every transcript
//! in memory together, and each one's result goes back to its driver.

use std::collections::VecDeque;

use maxhq_protocol::ids::SessionId;

use super::{Input, Manager};
use crate::drive::{Effects, Job, JobDone};

/// The most jobs that run at once, and history reads at a restart.
pub(super) const AT_ONCE: usize = 2;

/// The jobs waiting to run, and how many run.
#[derive(Default)]
pub(super) struct Jobs {
    waiting: VecDeque<(SessionId, Job)>,
    running: usize,
}

impl Manager {
    /// Queues the jobs a holder's driver asked for, and starts on them.
    pub(super) fn run_jobs(&mut self, session: SessionId, jobs: Vec<Job>) {
        let Some(hold) = self.holds.get_mut(&session) else {
            return;
        };
        hold.jobs += jobs.len();
        self.jobs
            .waiting
            .extend(jobs.into_iter().map(|job| (session, job)));
        self.next_jobs();
    }

    /// Starts the jobs waiting, while there's room. A job whose holder is
    /// gone is dropped.
    fn next_jobs(&mut self) {
        while self.jobs.running < AT_ONCE
            && let Some((session, job)) = self.jobs.waiting.pop_front()
        {
            if !self.holds.contains_key(&session) {
                continue;
            }
            self.jobs.running += 1;
            let tx = self.tx.clone();
            tokio::spawn(async move {
                let done = tokio::task::spawn_blocking(move || job.run())
                    .await
                    .ok()
                    .flatten();
                let _ = tx.send(Input::JobDone { session, done });
            });
        }
    }

    /// Hands a finished job back to its driver, and starts the next. A CLI
    /// that ended while the driver's jobs ran ends for it now.
    pub(super) async fn driver_job_done(&mut self, session: SessionId, done: Option<JobDone>) {
        self.jobs.running = self.jobs.running.saturating_sub(1);
        self.next_jobs();
        let Some(hold) = self.holds.get_mut(&session) else {
            return;
        };
        hold.jobs = hold.jobs.saturating_sub(1);
        let mut fx = Effects::default();
        hold.driver.job_done(done, &mut fx);
        self.apply_fx(session, fx).await;
        if let Some(hold) = self.holds.get_mut(&session)
            && hold.jobs == 0
            && let Some(exit) = hold.exit.take()
        {
            self.exited(session, exit).await;
        }
    }
}
