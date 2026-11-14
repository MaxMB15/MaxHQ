// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the install lock
// holds its owner's PID, a lock whose owner is gone is broken, and an empty
// lock gets a few tries for its owner to write the PID first.

//! Each tool's lock in each place, so two installs of one tool take turns.
//! Installs from this maxd queue in memory first. The lock file in the
//! tool's folder then keeps out another maxd and outlives a crash: its
//! owner's PID and token say whether it's stale. maxd checks them itself,
//! since the place may be a container that can't see maxd's processes.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::{Duration, Instant};

use tokio::sync::OwnedMutexGuard;

use super::scripts::{LOCK, UNLOCK};
use super::{Error, InstallError, Spot};

/// How often a waiting install looks at the lock again.
const POLL: Duration = Duration::from_millis(250);
/// How many looks an empty lock gets before it's broken.
const UNOWNED_TRIES: u32 = 5;

/// The in-memory queue per tool folder in each place.
static TURNS: LazyLock<Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(Mutex::default);

/// The tokens this maxd holds locks with now.
static HELD: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);
/// Makes each token this maxd takes a lock with its own.
static NEXT: AtomicU64 = AtomicU64::new(1);

fn held() -> std::sync::MutexGuard<'static, HashSet<String>> {
    HELD.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Waits for this maxd's turn at `dir` in the place `label` names.
pub async fn turn(label: &str, dir: &str) -> OwnedMutexGuard<()> {
    let key = format!("{label}\n{dir}");
    let mutex = {
        let mut turns = TURNS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        turns.retain(|_, weak| weak.strong_count() > 0);
        match turns.get(&key).and_then(Weak::upgrade) {
            Some(mutex) => mutex,
            None => {
                let mutex = Arc::new(tokio::sync::Mutex::new(()));
                turns.insert(key, Arc::downgrade(&mutex));
                mutex
            }
        }
    };
    mutex.lock_owned().await
}

/// The lock file, while this maxd holds it.
pub struct Held {
    dir: String,
    token: String,
}

/// A new token: this maxd's PID and a number of its own.
fn token() -> String {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    format!("{}:{n}", std::process::id())
}

/// Takes the lock in `dir`, waiting up to `wait` for another owner.
pub async fn take(spot: &Spot<'_>, dir: &str, name: &str, wait: Duration) -> Result<Held, Error> {
    let token = token();
    let deadline = Instant::now() + wait;
    let mut unowned = 0;
    loop {
        let output = spot.sh(LOCK, &[dir, &token]).await?;
        match output.status {
            0 => {
                held().insert(token.clone());
                return Ok(Held {
                    dir: dir.to_owned(),
                    token,
                });
            }
            3 => {}
            _ => return Err(spot.failed(LOCK, &output)),
        }
        let owner = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let stale = if owner.is_empty() {
            unowned += 1;
            unowned >= UNOWNED_TRIES
        } else {
            unowned = 0;
            stale(&owner)
        };
        if stale {
            spot.sh(UNLOCK, &[dir, &owner]).await?;
            continue;
        }
        if Instant::now() >= deadline {
            return Err(Error::Install(InstallError::Failed(format!(
                "Another install of {name} has held its lock for too long."
            ))));
        }
        tokio::time::sleep(POLL).await;
    }
}

impl Held {
    /// Drops the lock. A lock this can't drop is stale to the next taker.
    pub async fn release(self, spot: &Spot<'_>) {
        let _ = spot.sh(UNLOCK, &[&self.dir, &self.token]).await;
    }
}

impl Drop for Held {
    /// An install that stopped part way leaves its lock file, which is
    /// stale once its token is no longer held.
    fn drop(&mut self) {
        held().remove(&self.token);
    }
}

/// True when the lock's owner is gone: its process isn't running, or it's
/// this maxd and the token isn't one it holds.
fn stale(owner: &str) -> bool {
    let Some(pid) = owner
        .split(':')
        .next()
        .and_then(|pid| pid.parse::<i32>().ok())
    else {
        return true;
    };
    if pid <= 0 {
        return true;
    }
    if pid as u32 == std::process::id() {
        return !held().contains(owner);
    }
    #[allow(unsafe_code)]
    // SAFETY: signal 0 only checks that the process exists.
    let alive = unsafe { libc::kill(pid, 0) } == 0
        || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
    !alive
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lock_is_stale_once_its_owner_is_gone() {
        let mine = token();
        assert!(stale(&mine));
        held().insert(mine.clone());
        assert!(!stale(&mine));
        held().remove(&mine);
        assert!(stale("not a pid"));
        assert!(stale("0"));
        // PID 1 always runs.
        assert!(!stale("1"));
    }
}
