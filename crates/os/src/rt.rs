// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Running one future to its end from code that isn't async, such as a
//! command that talks to `maxd` once and exits.

use std::future::Future;
use std::io;

/// Runs `future` on a runtime of its own and returns what it gave. Inside
/// a runtime, whose threads mustn't block, it runs on a thread of its own.
///
/// The runtime goes down without waiting for blocking work the future left
/// running, such as a read of stdin that can't be called off, which would
/// otherwise keep the process up until stdin sent a line.
pub fn block_on<F>(future: F) -> io::Result<F::Output>
where
    F: Future + Send,
    F::Output: Send,
{
    if tokio::runtime::Handle::try_current().is_err() {
        return run(future);
    }
    std::thread::scope(|scope| {
        scope
            .spawn(|| run(future))
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

/// Runs `future` on a new runtime on this thread.
fn run<F: Future>(future: F) -> io::Result<F::Output> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let output = runtime.block_on(future);
    runtime.shutdown_background();
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    #[test]
    fn it_runs_outside_a_runtime() {
        let answer = block_on(async {
            tokio::time::sleep(Duration::from_millis(1)).await;
            42
        });
        assert_eq!(answer.unwrap(), 42);
    }

    #[tokio::test]
    async fn it_runs_inside_a_runtime_of_one_thread() {
        assert_eq!(block_on(async { 7 }).unwrap(), 7);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn it_runs_inside_a_runtime_of_many_threads() {
        assert_eq!(block_on(async { 7 }).unwrap(), 7);
    }

    #[test]
    fn it_returns_without_waiting_for_blocking_work() {
        let (release, blocked) = mpsc::channel::<()>();
        let (done, returned) = mpsc::channel();
        std::thread::spawn(move || {
            let ran = block_on(async move {
                // Like a read of stdin: it ends only when something else
                // lets it.
                drop(tokio::task::spawn_blocking(move || blocked.recv()));
                "done"
            });
            let _ = done.send(ran.unwrap());
        });
        let answer = returned.recv_timeout(Duration::from_secs(10));
        drop(release);
        assert_eq!(answer, Ok("done"));
    }
}
