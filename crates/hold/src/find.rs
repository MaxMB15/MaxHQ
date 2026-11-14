// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Finding what's there for a session, so a caller that restarted reaches
//! the holder that outlived it rather than starting a second one.
//!
//! The session's folder says most of it. A holder that runs holds the lock
//! and has written `holder.json`, and it proves it's the session's own by
//! answering a hello for the session with the process ID in that file,
//! which is the check [`Holds::connect`] makes too. A folder with neither
//! is a holder starting, or a spawn that died before its holder started.
//! The two are told apart by age: a holder that hasn't taken the lock long
//! after the spawn gave up on it never will.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions, TryLockError};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use maxhq_protocol::ids::SessionId;
use tokio::time::Instant;

use crate::client;
use crate::files::{self, HolderFile, EXIT, HOLDER, LOCK};
use crate::spawn::{self, READY_WAIT};
use crate::{Holds, Launch, SpawnError};

/// How old a folder whose holder never took the lock has to be before it
/// counts as abandoned. A spawn gives up on its holder after
/// [`READY_WAIT`].
const STALE_AFTER: Duration = READY_WAIT.saturating_add(Duration::from_secs(10));

/// How long [`ensure`] waits for a holder that's starting.
const ENSURE_WAIT: Duration = STALE_AFTER.saturating_add(READY_WAIT);

/// The pause between looks while a holder starts.
const RETRY: Duration = Duration::from_millis(100);

/// What's there for a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// The session has no folder, so no holder runs for it.
    Nothing,
    /// A holder is starting for it, in this process or in another.
    Starting,
    /// A spawn wrote the session's folder long ago, but no holder ever
    /// started in it.
    NotStarted,
    /// A holder runs for the session and proved it.
    Live {
        /// The holder's process ID, as its hello and `holder.json` give
        /// it.
        holder_pid: u32,
    },
    /// The holder has gone. Its files still say how the CLI ended, and
    /// [`Holds::connect`] serves them.
    Ended,
    /// Something holds the session's lock but didn't prove it's the
    /// session's holder. It may be one that's ending. Nothing should start
    /// beside it, and [`Holds::end`] can make sure it's gone.
    Unconfirmed {
        /// Why it isn't confirmed.
        reason: String,
    },
}

/// What [`Holds::ensure`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ensured {
    /// It started a holder, which is ready.
    Spawned,
    /// The session already had a holder, which it left alone: a
    /// [`Found::Live`], [`Found::Ended`] or [`Found::Unconfirmed`] one.
    Existing(Found),
}

/// The session folders this process is spawning a holder in, each with
/// how many spawns are at it.
static SPAWNING: Mutex<BTreeMap<PathBuf, usize>> = Mutex::new(BTreeMap::new());

/// Marks a session folder as being spawned in this process, until dropped.
pub(crate) struct Spawning(PathBuf);

impl Spawning {
    pub(crate) fn new(dir: &Path) -> Self {
        let mut spawning = SPAWNING.lock().unwrap_or_else(PoisonError::into_inner);
        *spawning.entry(dir.to_owned()).or_default() += 1;
        Self(dir.to_owned())
    }
}

impl Drop for Spawning {
    fn drop(&mut self) {
        let mut spawning = SPAWNING.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = spawning.get_mut(&self.0) {
            *count -= 1;
            if *count == 0 {
                spawning.remove(&self.0);
            }
        }
    }
}

fn spawning(dir: &Path) -> bool {
    SPAWNING
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains_key(dir)
}

/// What a session's folder shows before any holder is asked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Facts {
    /// The folder exists.
    pub folder: bool,
    /// This process is spawning a holder in it.
    pub spawning: bool,
    /// `holder.json` exists.
    pub holder: bool,
    /// `exit.json` exists.
    pub exit: bool,
    /// The session's lock is held.
    pub locked: bool,
    /// The folder changed within [`STALE_AFTER`].
    pub fresh: bool,
}

/// What the facts show, or that the holder has to be asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Look {
    Found(Found),
    AskHolder,
}

/// What `facts` say about the session.
pub(crate) fn classify(facts: Facts) -> Look {
    let found = match facts {
        Facts { folder: false, .. } => Found::Nothing,
        Facts { spawning: true, .. } => Found::Starting,
        Facts {
            holder: true,
            locked: true,
            ..
        } => return Look::AskHolder,
        Facts { holder: true, .. } => Found::Ended,
        // A holder takes the lock before it writes `holder.json`. Someone
        // checking the lock holds it for a moment too, so this is looked
        // at again.
        Facts { locked: true, .. } => Found::Starting,
        Facts { exit: true, .. } => Found::Ended,
        Facts { fresh: true, .. } => Found::Starting,
        Facts { .. } => Found::NotStarted,
    };
    Look::Found(found)
}

/// The facts for the folder `dir`.
fn facts(dir: &Path) -> io::Result<Facts> {
    let meta = match fs::metadata(dir) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Facts::default()),
        Err(err) => return Err(err),
    };
    if !meta.is_dir() {
        return Err(io::Error::other(format!(
            "{} isn't a folder",
            dir.display()
        )));
    }
    let fresh = meta
        .modified()
        .ok()
        .and_then(|at| SystemTime::now().duration_since(at).ok())
        .is_none_or(|age| age < STALE_AFTER);
    Ok(Facts {
        folder: true,
        spawning: spawning(dir),
        holder: dir.join(HOLDER).exists(),
        exit: dir.join(EXIT).exists(),
        locked: files::holder_alive(dir)?,
        fresh,
    })
}

/// The session's folder, as a spawn names it.
pub(crate) fn session_dir(holds: &Holds, session: &SessionId) -> PathBuf {
    let sessions =
        std::path::absolute(&holds.dirs.sessions).unwrap_or_else(|_| holds.dirs.sessions.clone());
    files::session_dir(&sessions, session)
}

pub(crate) async fn find(holds: &Holds, session: SessionId) -> io::Result<Found> {
    let dir = session_dir(holds, &session);
    match classify(facts(&dir)?) {
        Look::Found(found) => Ok(found),
        Look::AskHolder => ask(&dir, session).await,
    }
}

/// Asks the holder in `dir` to prove it's the session's own.
async fn ask(dir: &Path, session: SessionId) -> io::Result<Found> {
    let Some(record): Option<HolderFile> = files::read_json(&dir.join(HOLDER))? else {
        return Ok(Found::Starting);
    };
    let socket = record.socket();
    match client::attempt(dir, &socket, session, None).await {
        Ok((_, _, hello)) if hello.holder_pid == record.holder_pid => Ok(Found::Live {
            holder_pid: hello.holder_pid,
        }),
        Ok((_, _, hello)) => Ok(Found::Unconfirmed {
            reason: format!(
                "the holder on {} is process {}, but {HOLDER} names {}",
                socket.display(),
                hello.holder_pid,
                record.holder_pid
            ),
        }),
        // A holder that's ending stops answering a moment before it lets
        // go of its lock.
        Err(_) if client::wait_gone(dir).await => Ok(Found::Ended),
        Err(err) => Ok(Found::Unconfirmed {
            reason: format!("the holder didn't answer: {err}"),
        }),
    }
}

pub(crate) async fn ensure(holds: &Holds, launch: &Launch) -> Result<Ensured, SpawnError> {
    let dir = session_dir(holds, &launch.session);
    let io = |source| SpawnError::Io {
        path: dir.clone(),
        source,
    };
    let deadline = Instant::now() + ENSURE_WAIT;
    loop {
        match find(holds, launch.session).await.map_err(io)? {
            Found::Nothing => match spawn::spawn(holds, launch).await {
                Ok(()) => return Ok(Ensured::Spawned),
                // Another spawn made the folder first.
                Err(SpawnError::Exists { .. }) if Instant::now() < deadline => {}
                Err(err) => return Err(err),
            },
            Found::NotStarted => discard(&dir).map_err(io)?,
            Found::Starting if Instant::now() < deadline => {}
            Found::Starting => return Err(SpawnError::Timeout),
            found => return Ok(Ensured::Existing(found)),
        }
        tokio::time::sleep(RETRY).await;
    }
}

/// Removes a folder whose holder never started. Its lock is held all the
/// while, so a holder that starts late finds the folder gone and gives up.
fn discard(dir: &Path) -> io::Result<()> {
    let lock = match OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(dir.join(LOCK))
    {
        Ok(lock) => lock,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    match lock.try_lock() {
        Ok(()) => {}
        // A holder took it after all.
        Err(TryLockError::WouldBlock) => return Ok(()),
        Err(TryLockError::Error(err)) => return Err(err),
    }
    if dir.join(HOLDER).exists() || dir.join(EXIT).exists() {
        return Ok(());
    }
    files::remove_session(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(facts: Facts) -> Found {
        match classify(facts) {
            Look::Found(found) => found,
            Look::AskHolder => panic!("{facts:?} asks the holder"),
        }
    }

    const FOLDER: Facts = Facts {
        folder: true,
        spawning: false,
        holder: false,
        exit: false,
        locked: false,
        fresh: false,
    };

    #[test]
    fn no_folder_is_nothing() {
        let facts = Facts {
            folder: false,
            ..FOLDER
        };
        assert_eq!(found(facts), Found::Nothing);
    }

    #[test]
    fn a_holder_that_runs_is_asked() {
        let facts = Facts {
            holder: true,
            locked: true,
            ..FOLDER
        };
        assert_eq!(classify(facts), Look::AskHolder);
    }

    #[test]
    fn a_holder_that_has_gone_is_ended() {
        let gone = Facts {
            holder: true,
            ..FOLDER
        };
        assert_eq!(found(gone), Found::Ended);
        let exited = Facts {
            holder: true,
            exit: true,
            fresh: true,
            ..FOLDER
        };
        assert_eq!(found(exited), Found::Ended);
        let no_record = Facts {
            exit: true,
            ..FOLDER
        };
        assert_eq!(found(no_record), Found::Ended);
    }

    #[test]
    fn a_holder_on_its_way_is_starting() {
        let locked = Facts {
            locked: true,
            ..FOLDER
        };
        assert_eq!(found(locked), Found::Starting);
        let fresh = Facts {
            fresh: true,
            ..FOLDER
        };
        assert_eq!(found(fresh), Found::Starting);
        let here = Facts {
            spawning: true,
            holder: true,
            ..FOLDER
        };
        assert_eq!(found(here), Found::Starting);
    }

    #[test]
    fn an_old_folder_with_no_holder_never_started() {
        assert_eq!(found(FOLDER), Found::NotStarted);
    }

    #[test]
    fn spawns_in_this_process_are_counted() {
        let dir = std::env::temp_dir().join("maxhq-find-test-spawning");
        assert!(!spawning(&dir));
        let first = Spawning::new(&dir);
        let second = Spawning::new(&dir);
        drop(first);
        assert!(spawning(&dir));
        drop(second);
        assert!(!spawning(&dir));
    }
}
