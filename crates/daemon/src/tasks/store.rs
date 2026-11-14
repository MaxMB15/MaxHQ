// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The tasks whose home is this device, and each one's latest runs, by
//! the key of the fire that made them.

use maxhq_protocol::ids::TaskId;
use maxhq_protocol::tasks::{Run, RunOutcome, Task};
use rusqlite::params;

use crate::db::{Db, RecordError};

/// How many runs each task keeps.
pub const KEEP_RUNS: usize = 50;

impl Db {
    /// Every stored task. A row that doesn't parse is skipped.
    pub fn tasks(&self) -> rusqlite::Result<Vec<Task>> {
        let conn = self.lock();
        let mut query = conn.prepare("SELECT id, record FROM tasks")?;
        let rows = query.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut found = Vec::new();
        for row in rows {
            let (id, record) = row?;
            match serde_json::from_str(&record) {
                Ok(task) => found.push(task),
                Err(_) => tracing::warn!(id, "skipping a task that doesn't parse"),
            }
        }
        Ok(found)
    }

    /// Stores a task, over the one with its id.
    pub fn put_task(&self, task: &Task) -> Result<(), RecordError> {
        let record = serde_json::to_string(task)?;
        self.lock().execute(
            "INSERT INTO tasks (id, record) VALUES (?1, ?2)
             ON CONFLICT (id) DO UPDATE SET record = excluded.record",
            params![task.id.to_string(), record],
        )?;
        Ok(())
    }

    /// Forgets a task and its runs.
    pub fn remove_task(&self, task: TaskId) -> rusqlite::Result<()> {
        let conn = self.lock();
        let id = task.to_string();
        conn.execute("DELETE FROM tasks WHERE id = ?1", params![id])?;
        conn.execute("DELETE FROM task_runs WHERE task = ?1", params![id])?;
        Ok(())
    }

    /// Stores a run under its fire's key, and drops the task's oldest runs
    /// past [`KEEP_RUNS`].
    pub fn put_run(&self, key: &str, run: &Run) -> Result<(), RecordError> {
        let record = serde_json::to_string(run)?;
        let conn = self.lock();
        let task = run.task.to_string();
        conn.execute(
            "INSERT INTO task_runs (task, fire_key, started, record) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (task, fire_key) DO UPDATE SET record = excluded.record",
            params![
                task,
                key,
                i64::try_from(run.started.0).unwrap_or(i64::MAX),
                record
            ],
        )?;
        conn.execute(
            "DELETE FROM task_runs WHERE task = ?1 AND fire_key NOT IN (
                 SELECT fire_key FROM task_runs WHERE task = ?1
                 ORDER BY started DESC, rowid DESC LIMIT ?2)",
            params![task, KEEP_RUNS as i64],
        )?;
        Ok(())
    }

    /// A task's runs, newest first.
    pub fn runs(&self, task: TaskId) -> rusqlite::Result<Vec<Run>> {
        Ok(self
            .keyed_runs(Some(task))?
            .into_iter()
            .map(|(_, run)| run)
            .collect())
    }

    /// Every run still going, with its fire's key, for a restart to pick
    /// up.
    pub fn open_runs(&self) -> rusqlite::Result<Vec<(String, Run)>> {
        let mut runs = self.keyed_runs(None)?;
        runs.retain(|(_, run)| run.outcome == RunOutcome::Running);
        Ok(runs)
    }

    /// The runs of `task`, or of every task, newest first. A row that
    /// doesn't parse is skipped.
    fn keyed_runs(&self, task: Option<TaskId>) -> rusqlite::Result<Vec<(String, Run)>> {
        let conn = self.lock();
        let mut query = conn.prepare(
            "SELECT fire_key, record FROM task_runs WHERE ?1 IS NULL OR task = ?1
             ORDER BY started DESC, rowid DESC",
        )?;
        let rows = query.query_map(params![task.map(|task| task.to_string())], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut found = Vec::new();
        for row in rows {
            let (key, record) = row?;
            match serde_json::from_str(&record) {
                Ok(run) => found.push((key, run)),
                Err(_) => tracing::warn!(key, "skipping a task run that doesn't parse"),
            }
        }
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::tasks::RunCause;
    use maxhq_protocol::time::Timestamp;

    use super::*;

    fn run(task: TaskId, started: u64, outcome: RunOutcome) -> Run {
        Run {
            task,
            cause: RunCause::Timer,
            started: Timestamp(started),
            ended: None,
            outcome,
            agent: None,
            delivery: None,
            error: None,
        }
    }

    #[test]
    fn a_task_keeps_its_newest_runs_and_loses_them_with_it() {
        let db = Db::open_in_memory().expect("the db opens");
        let (task, other) = (TaskId::derive("a"), TaskId::derive("b"));
        for n in 0..60u64 {
            db.put_run(&n.to_string(), &run(task, n, RunOutcome::Done))
                .expect("it stores");
        }
        db.put_run("x", &run(other, 5, RunOutcome::Running))
            .expect("it stores");
        let runs = db.runs(task).expect("it reads");
        assert_eq!(runs.len(), KEEP_RUNS);
        assert_eq!(runs[0].started, Timestamp(59));
        assert_eq!(runs[KEEP_RUNS - 1].started, Timestamp(10));

        let open = db.open_runs().expect("it reads");
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].0, "x");

        db.remove_task(task).expect("it removes");
        assert!(db.runs(task).expect("it reads").is_empty());
        assert_eq!(db.runs(other).expect("it reads").len(), 1);
    }
}
