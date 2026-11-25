// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The local endpoint `maxd` serves on, named by a path: a Unix socket at
//! that path, or on Windows a named pipe whose name [`pipe_name`] derives
//! from it. No file is made for a pipe.
//!
//! The streams are tokio's, behind types of their own so callers name no
//! system's.

use std::io;
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

/// The endpoint, ready to take connections. Dropping it stops listening,
/// but leaves a Unix socket's file for the caller to remove.
#[derive(Debug)]
pub struct LocalListener(imp::Listener);

/// One connection to an endpoint, from either side.
#[derive(Debug)]
pub struct LocalStream(imp::Stream);

/// The half of a [`LocalStream`] that reads.
#[derive(Debug)]
pub struct ReadHalf(imp::ReadHalf);

/// The half of a [`LocalStream`] that writes.
#[derive(Debug)]
pub struct WriteHalf(imp::WriteHalf);

/// Who is on the other end of a [`LocalStream`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer {
    /// Its process, when the system says.
    pub pid: Option<u32>,
    /// Whether it runs as this process's user.
    pub same_user: bool,
    /// Whether it runs as root, or elevated on Windows.
    pub elevated: bool,
}

/// What [`probe`] found at an endpoint's path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    /// Nothing is there.
    Nothing,
    /// Something listens there.
    Listening,
    /// A socket file nobody listens on, as a process that crashed leaves
    /// it. Removing the file frees the path.
    Stale,
    /// A file that isn't an endpoint.
    Other,
}

impl LocalListener {
    /// Listens at `path`, for this user alone. Must run inside a Tokio
    /// runtime.
    pub fn bind(path: &Path) -> io::Result<Self> {
        imp::bind(path).map(Self)
    }

    /// The next connection.
    pub async fn accept(&self) -> io::Result<LocalStream> {
        imp::accept(&self.0).await.map(LocalStream)
    }
}

impl LocalStream {
    /// Connects to the endpoint at `path`.
    pub async fn connect(path: &Path) -> io::Result<Self> {
        imp::connect(path).await.map(Self)
    }

    /// Who is on the other end.
    pub fn peer(&self) -> io::Result<Peer> {
        imp::peer(&self.0)
    }

    /// Splits it into halves that read and write apart.
    pub fn into_split(self) -> (ReadHalf, WriteHalf) {
        let (read, write) = imp::into_split(self.0);
        (ReadHalf(read), WriteHalf(write))
    }
}

/// Two streams connected to each other, as for a test. Must run inside a
/// Tokio runtime.
pub fn pair() -> io::Result<(LocalStream, LocalStream)> {
    imp::pair().map(|(a, b)| (LocalStream(a), LocalStream(b)))
}

/// What is at the endpoint's `path`. It connects to see whether something
/// listens, and blocks until that answers.
pub fn probe(path: &Path) -> io::Result<Found> {
    imp::probe(path)
}

/// The named pipe Windows serves the endpoint at `path` on:
/// `\\.\pipe\maxhq-` and 16 hex digits of the path's SHA-256. The path is
/// compared as Windows compares it: either slash, any case, with or
/// without `\\?\` and a trailing separator. A path under the user's
/// profile gives each user a pipe of their own.
pub fn pipe_name(path: &Path) -> String {
    let path = path.to_string_lossy().replace('/', "\\").to_lowercase();
    let path = path.strip_prefix(r"\\?\").unwrap_or(&path);
    let path = path.trim_end_matches('\\');
    let digest = Sha256::digest(path.as_bytes());
    let hex: String = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!(r"\\.\pipe\maxhq-{hex}")
}

/// Reads through to the wrapped stream.
macro_rules! read_through {
    ($outer:ty) => {
        impl AsyncRead for $outer {
            fn poll_read(
                self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                buf: &mut ReadBuf<'_>,
            ) -> Poll<io::Result<()>> {
                Pin::new(&mut self.get_mut().0).poll_read(cx, buf)
            }
        }
    };
}

/// Writes through to the wrapped stream.
macro_rules! write_through {
    ($outer:ty) => {
        impl AsyncWrite for $outer {
            fn poll_write(
                self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                buf: &[u8],
            ) -> Poll<io::Result<usize>> {
                Pin::new(&mut self.get_mut().0).poll_write(cx, buf)
            }

            fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                Pin::new(&mut self.get_mut().0).poll_flush(cx)
            }

            fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                Pin::new(&mut self.get_mut().0).poll_shutdown(cx)
            }

            fn poll_write_vectored(
                self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                bufs: &[io::IoSlice<'_>],
            ) -> Poll<io::Result<usize>> {
                Pin::new(&mut self.get_mut().0).poll_write_vectored(cx, bufs)
            }

            fn is_write_vectored(&self) -> bool {
                self.0.is_write_vectored()
            }
        }
    };
}

read_through!(LocalStream);
read_through!(ReadHalf);
write_through!(LocalStream);
write_through!(WriteHalf);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pipe_name_ignores_how_the_path_is_written() {
        let name = pipe_name(Path::new(r"C:\Users\<user>\AppData\Local\MaxHQ\maxd.sock"));
        assert!(name.starts_with(r"\\.\pipe\maxhq-"));
        assert_eq!(name.len(), r"\\.\pipe\maxhq-".len() + 16);
        for same in [
            "C:/Users/<user>/AppData/Local/MaxHQ/maxd.sock",
            r"c:\users\<USER>\appdata\local\maxhq\MAXD.SOCK",
            r"\\?\C:\Users\<user>\AppData\Local\MaxHQ\maxd.sock",
            r"C:\Users\<user>\AppData\Local\MaxHQ\maxd.sock\",
        ] {
            assert_eq!(pipe_name(Path::new(same)), name, "{same}");
        }
        let other = pipe_name(Path::new(r"C:\Users\<other>\AppData\Local\MaxHQ\maxd.sock"));
        assert_ne!(other, name);
    }

    #[cfg(unix)]
    #[test]
    fn a_listener_takes_a_connection_and_knows_its_peer() {
        let _forks = crate::forks();
        crate::block_on(listen_and_connect());
    }

    #[cfg(unix)]
    async fn listen_and_connect() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let dir = std::env::temp_dir().join(format!("maxhq-os-ipc-{}", std::process::id()));
        crate::fsmode::private_dir_all(&dir).unwrap();
        let path = dir.join("test.sock");
        let _ = std::fs::remove_file(&path);
        assert_eq!(probe(&path).unwrap(), Found::Nothing);

        let listener = LocalListener::bind(&path).unwrap();
        assert_eq!(
            crate::fsmode::mode_of(&std::fs::metadata(&path).unwrap()) & 0o777,
            0o600
        );
        let accept = tokio::spawn(async move {
            let stream = listener.accept().await.unwrap();
            (listener, stream)
        });
        let mut client = LocalStream::connect(&path).await.unwrap();
        let (listener, server) = accept.await.unwrap();
        let peer = server.peer().unwrap();
        assert_eq!(peer.pid, Some(std::process::id()));
        assert!(peer.same_user);
        assert_eq!(peer.elevated, crate::proc::is_root());

        let (mut read, mut write) = server.into_split();
        client.write_all(b"ping").await.unwrap();
        let mut got = [0; 4];
        read.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"ping");
        write.write_all(b"pong").await.unwrap();
        client.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"pong");

        let probed = path.clone();
        let found = tokio::task::spawn_blocking(move || probe(&probed))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found, Found::Listening);
        drop(listener);
        assert_eq!(probe(&path).unwrap(), Found::Stale);
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "not a socket").unwrap();
        assert_eq!(probe(&path).unwrap(), Found::Other);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_pair_is_connected() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let (mut a, mut b) = pair().unwrap();
        a.write_all(b"hi").await.unwrap();
        let mut got = [0; 2];
        b.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"hi");
    }

    /// A path no other test or run uses, for a pipe of its own.
    #[cfg(windows)]
    fn pipe_path(test: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("maxhq-os-{test}-{}.sock", std::process::id()))
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_pipe_takes_connections_and_knows_its_peer() {
        crate::within(take_connections()).await;
    }

    #[cfg(windows)]
    async fn take_connections() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let path = pipe_path("listen");
        assert_eq!(probe(&path).unwrap(), Found::Nothing);
        let listener = LocalListener::bind(&path).unwrap();
        let accept = tokio::spawn(async move {
            let one = listener.accept().await.unwrap();
            let two = listener.accept().await.unwrap();
            (listener, one, two)
        });
        let mut client = LocalStream::connect(&path).await.unwrap();
        let mut second = LocalStream::connect(&path).await.unwrap();
        let (listener, server, other) = accept.await.unwrap();

        let me = Peer {
            pid: Some(std::process::id()),
            same_user: true,
            elevated: crate::proc::is_root(),
        };
        assert_eq!(server.peer().unwrap(), me);
        assert_eq!(client.peer().unwrap(), me);

        let (mut read, mut write) = server.into_split();
        client.write_all(b"ping").await.unwrap();
        let mut got = [0; 4];
        read.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"ping");
        write.write_all(b"pong").await.unwrap();
        client.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"pong");

        let (mut other_read, other_write) = other.into_split();
        second.write_all(b"two!").await.unwrap();
        other_read.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"two!");

        let probed = path.clone();
        let found = tokio::task::spawn_blocking(move || probe(&probed))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found, Found::Listening);

        // A client's end closing is the server's end of file.
        drop(client);
        assert_eq!(read.read(&mut got).await.unwrap(), 0);
        drop((listener, read, write, other_read, other_write, second));
        // Each end closes once Windows has cancelled what it was waiting
        // for, which takes the runtime a turn or two.
        let mut found = probe(&path).unwrap();
        while found != Found::Nothing {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            found = probe(&path).unwrap();
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_pipe_lets_in_this_user_alone() {
        crate::within(async {
            let path = pipe_path("dacl");
            let listener = LocalListener::bind(&path).unwrap();
            let accept = tokio::spawn(async move { listener.accept().await.unwrap() });
            let client = LocalStream::connect(&path).await.unwrap();
            let _server = accept.await.unwrap();
            let sid = crate::win::sid_string(&crate::win::own_user().unwrap()).unwrap();
            // One entry, which lets this user in, inherited from nowhere:
            // no Everyone, no Administrators, no anonymous logon.
            let dacl = super::windows::dacl_of(&client.0).unwrap();
            let (flags, entries) = dacl
                .strip_prefix("D:")
                .and_then(|rest| rest.split_once('('))
                .unwrap_or_else(|| panic!("{dacl}"));
            assert!(flags.starts_with('P'), "{dacl}");
            let entries: Vec<&str> = entries.trim_end_matches(')').split(")(").collect();
            let [entry] = entries[..] else {
                panic!("{dacl}");
            };
            let fields: Vec<&str> = entry.split(';').collect();
            assert_eq!(fields.first(), Some(&"A"), "{dacl}");
            assert_eq!(fields.last(), Some(&sid.as_str()), "{dacl}");
        })
        .await;
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_pipe_name_someone_else_holds_is_refused() {
        use tokio::net::windows::named_pipe::ServerOptions;

        // A squatter got the name first, with the default DACL.
        let path = pipe_path("squat");
        let _squatter = ServerOptions::new().create(pipe_name(&path)).unwrap();
        let err = LocalListener::bind(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AddrInUse, "{err}");

        // Two maxds don't serve one pipe either.
        let path = pipe_path("twice");
        let _first = LocalListener::bind(&path).unwrap();
        let err = LocalListener::bind(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AddrInUse, "{err}");
    }

    #[cfg(windows)]
    #[test]
    fn a_server_of_another_user_is_refused() {
        // The System process, which runs as SYSTEM: a client that found
        // it serving the pipe goes no further.
        let err = super::windows::served_by_this_user(r"\\.\pipe\maxhq-test", 4).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
        super::windows::served_by_this_user(r"\\.\pipe\maxhq-test", std::process::id()).unwrap();
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_pair_is_connected_both_ways() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        crate::within(async {
            let (mut a, mut b) = pair().unwrap();
            a.write_all(b"hi").await.unwrap();
            let mut got = [0; 2];
            b.read_exact(&mut got).await.unwrap();
            assert_eq!(&got, b"hi");
            b.write_all(b"yo").await.unwrap();
            a.read_exact(&mut got).await.unwrap();
            assert_eq!(&got, b"yo");
            // No half-close: shutting down flushes, and dropping ends it.
            a.shutdown().await.unwrap();
            drop(a);
            assert_eq!(b.read(&mut got).await.unwrap(), 0);
        })
        .await;
    }
}
