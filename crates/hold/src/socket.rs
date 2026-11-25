// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The holder's socket: a short name that fits macOS's limit, with a lock
//! file beside it so two holders never share one.
//!
//! The name is the last 12 hex digits of the session ID, which are random
//! in a UUIDv7. When another holder already owns that name, the next holder
//! adds `-1`, `-2` and so on. Owning a name means holding its lock file,
//! and a lock only counts while the file at the path is the file that was
//! locked, since a holder that's ending removes it.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use maxhq_os::fsmode;
use maxhq_os::ipc::{LocalListener, LocalStream};
use maxhq_protocol::ids::SessionId;
use tokio::task::AbortHandle;

use crate::files;

/// The longest socket path the OS takes, counting the closing zero byte.
/// A pipe on Windows is named by a hash of the path, so any path fits.
pub(crate) const MAX_PATH: Option<usize> = if cfg!(windows) {
    None
} else if cfg!(target_os = "macos") {
    Some(104)
} else {
    Some(108)
};

/// How many names to try before giving up.
const TRIES: usize = 100;

/// How long to wait after a connection couldn't be taken.
const ACCEPT_RETRY: Duration = Duration::from_millis(200);

/// A bound socket, removed with its lock file when dropped. On Windows
/// it's a named pipe, named after the path, which goes with the listener.
pub(crate) struct Socket {
    listener: Arc<LocalListener>,
    pub path: PathBuf,
    lock_path: PathBuf,
    accepting: Option<AbortHandle>,
    _lock: File,
}

impl Socket {
    /// Hands each connection to `serve` on a task of its own, until the
    /// socket goes.
    pub(crate) fn accept_each<F, Fut>(&mut self, serve: F)
    where
        F: Fn(LocalStream) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let listener = self.listener.clone();
        let task = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok(stream) => {
                        tokio::spawn(serve(stream));
                    }
                    Err(err) => {
                        eprintln!("couldn't accept a connection: {err}");
                        tokio::time::sleep(ACCEPT_RETRY).await;
                    }
                }
            }
        });
        if let Some(old) = self.accepting.replace(task.abort_handle()) {
            old.abort();
        }
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        if let Some(accepting) = self.accepting.take() {
            accepting.abort();
        }
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(&self.lock_path);
    }
}

/// The short name for `session`'s socket, before any suffix.
pub(crate) fn short_name(session: &SessionId) -> String {
    let hex = session.0.simple().to_string();
    hex[hex.len() - 12..].to_owned()
}

/// Binds a socket for `session` in `dir`, which must exist. It's called
/// on the runtime, which the listener belongs to.
pub(crate) fn bind(dir: &Path, session: &SessionId) -> io::Result<Socket> {
    bind_named(dir, &short_name(session))
}

/// Binds a socket named `base`, or `base` with a suffix, in `dir`.
pub(crate) fn bind_named(dir: &Path, base: &str) -> io::Result<Socket> {
    for n in 0..TRIES {
        let name = if n == 0 {
            base.to_owned()
        } else {
            format!("{base}-{n}")
        };
        let path = dir.join(format!("{name}.sock"));
        if let Some(max) = MAX_PATH
            && path.as_os_str().len() >= max
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "the socket path {} is longer than the {} bytes the OS allows",
                    path.display(),
                    max - 1
                ),
            ));
        }
        let lock_path = dir.join(format!("{name}.lock"));
        let Some(lock) = take(&lock_path)? else {
            continue;
        };
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        // Only this user can connect: the file is 0600 on Unix, and the
        // pipe takes this user's clients alone on Windows.
        let listener = Arc::new(LocalListener::bind(&path)?);
        return Ok(Socket {
            listener,
            path,
            lock_path,
            accepting: None,
            _lock: lock,
        });
    }
    Err(io::Error::new(
        io::ErrorKind::AddrInUse,
        format!("every socket name starting {base} is taken"),
    ))
}

/// Locks the file at `path`, or `None` when someone else holds it.
fn take(path: &Path) -> io::Result<Option<File>> {
    for _ in 0..TRIES {
        let lock = fsmode::set_open_mode(
            OpenOptions::new().create(true).truncate(false).write(true),
            0o600,
        )
        .open(path)?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Error(err)) => return Err(err),
        }
        // A holder that was ending may have removed the file after this
        // one opened it. Then the lock is on a file nobody else can see,
        // so try again with the file that's there now.
        match fsmode::same_file(&lock, path) {
            Ok(true) => return Ok(Some(lock)),
            Ok(false) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    Ok(None)
}

/// Makes the sockets folder, readable only by this user.
pub(crate) fn make_dir(dir: &Path) -> io::Result<()> {
    files::private_dir_all(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn dir() -> Dir {
        let id = SessionId::generate().0.simple().to_string();
        let path = std::env::temp_dir().join(format!("hs-{}", &id[20..]));
        make_dir(&path).unwrap();
        Dir(path)
    }

    #[test]
    fn names_use_the_random_end_of_the_id() {
        let session: SessionId = "0199f0a2-7c3e-7a10-9b4d-1234567890ab".parse().unwrap();
        assert_eq!(short_name(&session), "1234567890ab");
    }

    #[tokio::test]
    async fn a_taken_name_moves_to_the_next_suffix() {
        let dir = dir();
        let session = SessionId::generate();
        let first = bind(&dir.0, &session).unwrap();
        let second = bind(&dir.0, &session).unwrap();
        let name = short_name(&session);
        assert!(first.path.ends_with(format!("{name}.sock")));
        assert!(second.path.ends_with(format!("{name}-1.sock")));
        drop(first);
        let third = bind(&dir.0, &session).unwrap();
        assert!(third.path.ends_with(format!("{name}.sock")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_stale_socket_file_is_replaced() {
        let dir = dir();
        let session = SessionId::generate();
        let path = dir.0.join(format!("{}.sock", short_name(&session)));
        drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
        assert!(path.exists());
        let socket = bind(&dir.0, &session).unwrap();
        assert_eq!(socket.path, path);
        drop(socket);
        assert!(!path.exists());
    }

    #[test]
    fn the_usual_macos_path_fits() {
        let base = "/Users/someone-with-a-long-name/Library/Application Support/MaxHQ/holders";
        let path = format!("{base}/0123456789ab-99.sock");
        assert!(path.len() < 104, "{} bytes", path.len());
    }

    #[test]
    fn the_guest_path_fits() {
        let base = format!("{}/holders", maxhq_core::paths::GUEST_RUNTIME_DIR);
        let name = short_name(&SessionId::generate());
        let path = format!("{base}/{name}-{}.sock", TRIES - 1);
        // The guest runs on Linux.
        assert!(path.len() < 108, "{path} is {} bytes", path.len());
    }
}
