// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! How `maxd run` hears that it should stop, and how another process asks
//! it to.
//!
//! On Unix that's the signals launchd and systemd send: `SIGTERM` asks,
//! and `SIGINT` is Ctrl-C in a terminal. Windows has no signals. Ctrl-C,
//! Ctrl-Break and a closing console reach `maxd` as console events, but
//! Task Scheduler sends nothing when it ends a task. So on Windows `maxd`
//! also serves a stop endpoint at [`endpoint`], beside its socket, which
//! [`ask`] connects to. It's this user's alone, as the socket is, and
//! takes one line: `stop` or `restart`.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

/// Why [`Stops::next`] returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stopped {
    /// The service manager, or someone through [`ask`], asked it to stop.
    Asked,
    /// Ctrl-C, in the terminal it runs in.
    Interrupted,
    /// [`ask`] asked it to stop so that it starts again.
    Restart,
}

/// What [`ask`] asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// Stop for good.
    Stop,
    /// Stop, so whatever runs `maxd` starts it again.
    Restart,
}

impl Request {
    /// The line the stop endpoint reads.
    #[cfg_attr(unix, allow(dead_code))]
    fn line(self) -> &'static [u8] {
        match self {
            Self::Stop => b"stop\n",
            Self::Restart => b"restart\n",
        }
    }
}

/// What tells `maxd` to stop, from the moment [`Stops::listen`] returns.
#[derive(Debug)]
pub struct Stops(imp::Stops);

impl Stops {
    /// Starts listening for the `maxd` serving at `socket`. Must run
    /// inside a Tokio runtime.
    pub fn listen(socket: &Path) -> io::Result<Self> {
        imp::listen(socket).map(Self)
    }

    /// Waits until something says to stop.
    pub async fn next(&mut self) -> Stopped {
        self.0.next().await
    }
}

/// Asks the `maxd` serving at `socket` to stop or restart, and returns
/// its process ID once it has taken the request. It fails with
/// [`io::ErrorKind::NotFound`] when no `maxd` serves there, and with
/// [`io::ErrorKind::Unsupported`] on Unix, where the service manager's
/// signals do this.
pub async fn ask(socket: &Path, request: Request) -> io::Result<u32> {
    imp::ask(socket, request).await
}

/// Where the stop endpoint of the `maxd` serving at `socket` is: the
/// socket's path with `.stop` after it.
pub fn endpoint(socket: &Path) -> PathBuf {
    let mut path = socket.as_os_str().to_owned();
    path.push(".stop");
    PathBuf::from(path)
}

/// How long the endpoint waits for a request's line, and [`ask`] for the
/// answer.
#[cfg_attr(unix, allow(dead_code))]
const REQUEST_WAIT: Duration = Duration::from_secs(5);

/// The longest request line the endpoint reads.
#[cfg_attr(unix, allow(dead_code))]
const REQUEST_MAX: u64 = 64;

/// The answer to a request the endpoint took.
#[cfg_attr(unix, allow(dead_code))]
const TAKEN: &str = "ok\n";

/// Reads one request from a client of the stop endpoint, and answers one
/// it knows. Anything else is dropped unanswered.
#[cfg_attr(unix, allow(dead_code))]
async fn answer(stream: crate::ipc::LocalStream) -> Option<Stopped> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    let (read, mut write) = stream.into_split();
    let mut line = String::new();
    let mut reader = BufReader::new(read.take(REQUEST_MAX));
    match tokio::time::timeout(REQUEST_WAIT, reader.read_line(&mut line)).await {
        Ok(Ok(_)) => {}
        Ok(Err(_)) | Err(_) => return None,
    }
    let stopped = match line.trim_end() {
        "stop" => Stopped::Asked,
        "restart" => Stopped::Restart,
        _ => return None,
    };
    // Whoever asked may have gone already, and it's taken either way.
    let _ = write.write_all(TAKEN.as_bytes()).await;
    let _ = write.flush().await;
    Some(stopped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stop_endpoint_sits_beside_the_socket() {
        assert_eq!(
            endpoint(Path::new("/run/user/1000/maxhq/maxd.sock")),
            Path::new("/run/user/1000/maxhq/maxd.sock.stop")
        );
    }

    /// Listening isn't tested here: tokio keeps a signal it caught once
    /// caught, and Ctrl-C would no longer end the tests.
    #[cfg(unix)]
    #[tokio::test]
    async fn unix_asks_through_the_service_manager() {
        let path = std::env::temp_dir().join(format!("maxhq-os-stop-{}.sock", std::process::id()));
        let err = ask(&path, Request::Stop).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
    }

    /// A path no other test or run uses.
    #[cfg(windows)]
    fn socket(test: &str) -> PathBuf {
        std::env::temp_dir().join(format!("maxhq-os-stop-{test}-{}.sock", std::process::id()))
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_request_over_the_endpoint_stops_or_restarts() {
        crate::within(async {
            for (request, expected) in [
                (Request::Stop, Stopped::Asked),
                (Request::Restart, Stopped::Restart),
            ] {
                let path = socket(&format!("{request:?}"));
                let mut stops = Stops::listen(&path).unwrap();
                let asked = tokio::spawn(async move { ask(&path, request).await });
                assert_eq!(stops.next().await, expected);
                assert_eq!(asked.await.unwrap().unwrap(), std::process::id());
            }
        })
        .await;
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_line_it_doesnt_know_is_ignored() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        crate::within(async {
            let path = socket("unknown");
            let mut stops = Stops::listen(&path).unwrap();
            let client = endpoint(&path);
            let asked = tokio::spawn(async move {
                let mut stream = crate::ipc::LocalStream::connect(&client).await.unwrap();
                stream.write_all(b"reboot\n").await.unwrap();
                // The endpoint drops it unanswered, which reads as the end
                // or as a broken pipe.
                let mut answer = Vec::new();
                let _ = stream.read_to_end(&mut answer).await;
                assert!(answer.is_empty(), "{answer:?}");
                ask(&path, Request::Stop).await
            });
            assert_eq!(stops.next().await, Stopped::Asked);
            asked.await.unwrap().unwrap();
        })
        .await;
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn nobody_serving_is_not_found() {
        let err = ask(&socket("nobody"), Request::Stop).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_second_maxd_cant_take_the_endpoint() {
        let path = socket("twice");
        let _first = Stops::listen(&path).unwrap();
        assert!(Stops::listen(&path).is_err());
    }
}
