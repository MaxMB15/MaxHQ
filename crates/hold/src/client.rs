// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The client's side: reaching a holder, reading the CLI's lines and
//! writing to it.
//!
//! A holder that's gone is no loss. Its files say everything it knew, so a
//! connection to a finished session is served from them, and a connection
//! whose holder dies reads the rest from disk and ends with the exit.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use futures::Stream;
use maxhq_protocol::ids::SessionId;
use maxhq_protocol::time::Timestamp;
use tokio::io::BufReader;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use crate::files::{self, HolderFile, LaunchFile, EXIT, HOLDER, LAUNCH, SEEN, STDERR};
use crate::log::{self, LogReader};
use crate::wire::{self, FromHolder, ToHolder};
use crate::{
    ChildState, ConnectError, Event, Exit, Hello, Holds, Limits, WriteError, WriteOutcome,
};

/// How long [`connect`] keeps trying a holder that's starting or ending.
const CONNECT_WAIT: Duration = Duration::from_secs(10);

/// How long one attempt has to connect and get a hello.
const ATTEMPT_WAIT: Duration = Duration::from_secs(5);

/// The pause between attempts.
const RETRY: Duration = Duration::from_millis(50);

/// How long a holder that closed the connection has to release its lock
/// before the client takes it as still running.
const DEATH_WAIT: Duration = Duration::from_secs(1);

/// The requests waiting for a reply, or `None` once the connection is gone.
type Waiters = Arc<Mutex<Option<HashMap<u64, oneshot::Sender<FromHolder>>>>>;

/// A connection to a held session.
#[derive(Debug)]
pub struct Connection {
    /// What the holder said when the client connected.
    pub hello: Hello,
    /// The lines after the cursor the client asked for, then live ones,
    /// then the exit.
    pub events: Events,
    /// Writes to the CLI, stops it or forgets the session.
    pub writer: Writer,
}

/// What a held session prints, in order: replayed lines, live lines, then
/// the exit.
///
/// It ends after the exit. It also ends without one when the connection
/// drops while the holder runs, and then the client connects again with
/// the last line it got.
#[derive(Debug)]
pub struct Events {
    rx: mpsc::UnboundedReceiver<Event>,
    dir: PathBuf,
    requests: Option<mpsc::UnboundedSender<ToHolder>>,
}

impl Events {
    /// The next event, or `None` at the end.
    pub async fn recv(&mut self) -> Option<Event> {
        let event = self.rx.recv().await;
        self.check(event.as_ref());
        event
    }

    /// Notes that the client has seen the exit, so `list` leaves the
    /// session out and the holder can go.
    fn check(&self, event: Option<&Event>) {
        if !matches!(event, Some(Event::Exited(_))) {
            return;
        }
        // The folder is gone when someone has forgotten the session.
        let _ = files::append(&self.dir.join(SEEN));
        if let Some(requests) = &self.requests {
            let _ = requests.send(ToHolder::SawExit);
        }
    }
}

impl Stream for Events {
    type Item = Event;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Event>> {
        let polled = self.rx.poll_recv(cx);
        if let Poll::Ready(event) = &polled {
            self.check(event.as_ref());
        }
        polled
    }
}

/// Writes to a held CLI. Clones share the connection.
#[derive(Clone, Debug)]
pub struct Writer {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    dir: PathBuf,
    /// `None` for a session served from its files.
    link: Option<Link>,
}

#[derive(Debug)]
struct Link {
    requests: mpsc::UnboundedSender<ToHolder>,
    waiters: Waiters,
    next_id: AtomicU64,
}

/// The connection is gone or there never was one.
struct Gone;

impl Writer {
    /// Sends `line` to the CLI's stdin as one line. A trailing newline is
    /// optional, and any other newline is an error.
    ///
    /// When the line answers a prompt, `answers` names the prompt's request
    /// ID, and with `answers` empty the holder works it out from the line.
    /// An answer to a prompt that's no longer pending never reaches the CLI
    /// and gets [`WriteOutcome::AlreadyAnswered`]. This returns once the
    /// line is in the CLI's stdin.
    pub async fn write(
        &self,
        line: String,
        answers: Option<String>,
    ) -> Result<WriteOutcome, WriteError> {
        match self
            .request(|id| ToHolder::Write { id, line, answers })
            .await
        {
            Ok(FromHolder::Written { outcome, .. }) => Ok(outcome),
            Ok(FromHolder::Failed { error, .. }) => Err(error.into()),
            Ok(other) => Err(unexpected(&other)),
            Err(Gone) => Err(if self.holder_gone().await {
                WriteError::Finished
            } else {
                WriteError::Disconnected
            }),
        }
    }

    /// Ends the CLI. The holder closes its stdin after the lines already
    /// written, sends SIGTERM to its process group if it's still running
    /// after `grace`, and SIGKILL after another `grace`. This returns once
    /// the CLI has ended, and the exit also comes through [`Events`].
    pub async fn stop(&self, grace: Duration) -> Result<(), WriteError> {
        let grace_ms = u64::try_from(grace.as_millis()).unwrap_or(u64::MAX);
        match self.request(|id| ToHolder::Stop { id, grace_ms }).await {
            Ok(FromHolder::Stopped { .. }) => Ok(()),
            Ok(FromHolder::Failed { error, .. }) => Err(error.into()),
            Ok(other) => Err(unexpected(&other)),
            Err(Gone) if self.holder_gone().await => Ok(()),
            Err(Gone) => Err(WriteError::Disconnected),
        }
    }

    /// Deletes a finished session's files. The holder, if it's still up,
    /// goes too.
    pub async fn forget(&self) -> Result<(), WriteError> {
        match self.request(|id| ToHolder::Forget { id }).await {
            Ok(FromHolder::Forgotten { .. }) => Ok(()),
            Ok(FromHolder::Failed { error, .. }) => Err(error.into()),
            Ok(other) => Err(unexpected(&other)),
            Err(Gone) if self.holder_gone().await => match files::remove_session(&self.inner.dir) {
                Ok(()) => Ok(()),
                Err(err) => Err(WriteError::Io(format!(
                    "couldn't remove {}: {err}",
                    self.inner.dir.display()
                ))),
            },
            Err(Gone) => Err(WriteError::Disconnected),
        }
    }

    async fn request(&self, make: impl FnOnce(u64) -> ToHolder) -> Result<FromHolder, Gone> {
        let Some(link) = &self.inner.link else {
            return Err(Gone);
        };
        let id = link.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply, replied) = oneshot::channel();
        {
            let mut waiters = link.waiters.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(waiters) = waiters.as_mut() else {
                return Err(Gone);
            };
            waiters.insert(id, reply);
        }
        if link.requests.send(make(id)).is_err() {
            return Err(Gone);
        }
        replied.await.map_err(|_| Gone)
    }

    /// Whether the holder has gone, giving one that's ending a moment to
    /// release its lock.
    async fn holder_gone(&self) -> bool {
        wait_gone(&self.inner.dir).await
    }
}

fn unexpected(reply: &FromHolder) -> WriteError {
    WriteError::Io(format!("the holder sent an unexpected reply: {reply:?}"))
}

/// Whether the holder in `dir` has gone, giving one that's ending a moment
/// to release its lock.
pub(crate) async fn wait_gone(dir: &Path) -> bool {
    wait_gone_within(dir, DEATH_WAIT).await
}

/// Whether the holder in `dir` has gone, waiting up to `limit` for it to
/// release its lock.
pub(crate) async fn wait_gone_within(dir: &Path, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        match files::holder_alive(dir) {
            Ok(false) => return true,
            Ok(true) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            _ => return false,
        }
    }
}

pub(crate) async fn connect(
    holds: &Holds,
    session: SessionId,
    after: Option<u64>,
) -> Result<Connection, ConnectError> {
    let dir = files::session_dir(&holds.dirs.sessions, &session);
    let deadline = Instant::now() + CONNECT_WAIT;
    loop {
        match fs::metadata(&dir) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(ConnectError::NotFound { session }),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(ConnectError::NotFound { session });
            }
            Err(err) => return Err(err.into()),
        }
        let holder: Option<HolderFile> = files::read_json(&dir.join(HOLDER))?;
        let alive = files::holder_alive(&dir)?;
        let last_try = Instant::now() >= deadline;
        match holder {
            Some(holder) if !alive => return from_files(dir, Some(holder), after).await,
            None if !alive && dir.join(EXIT).exists() => {
                return from_files(dir, None, after).await;
            }
            Some(holder) => match attempt(&dir, &holder.socket(), session, after).await {
                Ok((read, write, hello)) => return Ok(live(dir, read, write, hello, after)),
                Err(err) if last_try => return Err(err),
                Err(_) => {}
            },
            // The holder hasn't written `holder.json` yet, or it gave up.
            None if last_try => return Err(ConnectError::NotStarted { session }),
            None => {}
        }
        tokio::time::sleep(RETRY).await;
    }
}

/// Connects to the socket at `path` and trades hellos, showing the key from
/// the session's folder `dir` when it has one.
pub(crate) async fn attempt(
    dir: &Path,
    path: &Path,
    session: SessionId,
    after: Option<u64>,
) -> Result<(BufReader<OwnedReadHalf>, OwnedWriteHalf, Hello), ConnectError> {
    let key = files::read_key(dir)?;
    let timed_out = |what: &str| {
        ConnectError::Io(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("the holder didn't {what} in time"),
        ))
    };
    let stream = tokio::time::timeout(ATTEMPT_WAIT, UnixStream::connect(path))
        .await
        .map_err(|_| timed_out("take the connection"))??;
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    let hello = ToHolder::Hello {
        session,
        after,
        key,
    };
    wire::send(&mut write, &hello).await?;
    drop(hello);
    let mut buf = Vec::new();
    let reply = tokio::time::timeout(ATTEMPT_WAIT, wire::recv(&mut read, &mut buf))
        .await
        .map_err(|_| timed_out("say hello"))?;
    match reply {
        Ok(Some(FromHolder::Hello {
            holder_pid,
            child,
            first_seq,
            last_seq,
            pending,
        })) => {
            let hello = Hello {
                holder_pid,
                child,
                first_seq,
                last_seq,
                pending,
            };
            Ok((read, write, hello))
        }
        Ok(Some(FromHolder::Refused { message })) => Err(ConnectError::Refused { message }),
        Ok(Some(other)) => Err(ConnectError::Protocol(format!(
            "expected a hello, got {other:?}"
        ))),
        Ok(None) => Err(ConnectError::Io(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the holder closed the connection",
        ))),
        Err(err) if err.kind() == io::ErrorKind::InvalidData => {
            Err(ConnectError::Protocol(err.to_string()))
        }
        Err(err) => Err(err.into()),
    }
}

/// A connection to a running holder.
fn live(
    dir: PathBuf,
    read: BufReader<OwnedReadHalf>,
    write: OwnedWriteHalf,
    hello: Hello,
    after: Option<u64>,
) -> Connection {
    let cursor = after.map_or(hello.last_seq, |after| after.min(hello.last_seq));
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let (requests_tx, requests_rx) = mpsc::unbounded_channel();
    let waiters: Waiters = Arc::new(Mutex::new(Some(HashMap::new())));
    tokio::spawn(send_requests(write, requests_rx));
    tokio::spawn(read_holder(
        read,
        events_tx,
        waiters.clone(),
        dir.clone(),
        cursor,
    ));
    Connection {
        hello,
        events: Events {
            rx: events_rx,
            dir: dir.clone(),
            requests: Some(requests_tx.clone()),
        },
        writer: Writer {
            inner: Arc::new(Inner {
                dir,
                link: Some(Link {
                    requests: requests_tx,
                    waiters,
                    next_id: AtomicU64::new(1),
                }),
            }),
        },
    }
}

async fn send_requests(mut write: OwnedWriteHalf, mut requests: mpsc::UnboundedReceiver<ToHolder>) {
    while let Some(request) = requests.recv().await {
        if wire::send(&mut write, &request).await.is_err() {
            return;
        }
    }
}

/// Passes lines and the exit on, and replies to whoever asked. When the
/// holder dies first, the rest comes from its files.
async fn read_holder(
    mut read: BufReader<OwnedReadHalf>,
    events: mpsc::UnboundedSender<Event>,
    waiters: Waiters,
    dir: PathBuf,
    mut cursor: u64,
) {
    let mut buf = Vec::new();
    let mut exited = false;
    while let Ok(Some(message)) = wire::recv::<_, FromHolder>(&mut read, &mut buf).await {
        let id = match &message {
            FromHolder::Line(line) => {
                cursor = line.seq;
                None
            }
            FromHolder::Exited(_) => {
                exited = true;
                None
            }
            FromHolder::Written { id, .. }
            | FromHolder::Stopped { id }
            | FromHolder::Forgotten { id }
            | FromHolder::Failed { id, .. } => Some(*id),
            FromHolder::Hello { .. } | FromHolder::Refused { .. } => continue,
        };
        match (id, message) {
            (Some(id), reply) => {
                let waiter = waiters
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_mut()
                    .and_then(|waiters| waiters.remove(&id));
                if let Some(waiter) = waiter {
                    let _ = waiter.send(reply);
                }
            }
            (None, FromHolder::Line(line)) => {
                let _ = events.send(Event::Line(line));
            }
            (None, FromHolder::Exited(exit)) => {
                let _ = events.send(Event::Exited(exit));
            }
            (None, _) => {}
        }
    }
    // Dropping the waiters tells each request the connection is gone.
    waiters
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    if exited || !wait_gone(&dir).await {
        return;
    }
    let _ = tokio::task::spawn_blocking(move || {
        if !dir.join(HOLDER).exists() {
            return;
        }
        if let Ok(mut reader) = LogReader::open(&dir) {
            while let Some(entry) = reader.next_after(cursor) {
                cursor = entry.seq;
                if events.send(Event::Line(entry.line(false))).is_err() {
                    return;
                }
            }
        }
        if let Ok(exit) = exit_from_files(&dir) {
            let _ = events.send(Event::Exited(exit));
        }
    })
    .await;
}

/// A connection to a session whose holder has gone, served from its files.
async fn from_files(
    dir: PathBuf,
    holder: Option<HolderFile>,
    after: Option<u64>,
) -> Result<Connection, ConnectError> {
    let reading = dir.clone();
    let read = tokio::task::spawn_blocking(move || -> io::Result<_> {
        let (first_seq, last_seq) = log::scan(&reading)?;
        Ok((first_seq, last_seq, exit_from_files(&reading)?))
    })
    .await
    .map_err(io::Error::other)?;
    let (first_seq, last_seq, exit) = read?;
    let hello = Hello {
        holder_pid: holder.map_or(0, |holder| holder.holder_pid),
        child: ChildState::Exited(exit.clone()),
        first_seq,
        last_seq,
        pending: Vec::new(),
    };
    let cursor = after.map_or(last_seq, |after| after.min(last_seq));
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let replaying = dir.clone();
    tokio::task::spawn_blocking(move || {
        if let Ok(mut reader) = LogReader::open(&replaying) {
            let mut cursor = cursor;
            while let Some(entry) = reader.next_after(cursor) {
                if entry.seq > last_seq {
                    break;
                }
                cursor = entry.seq;
                if events_tx.send(Event::Line(entry.line(true))).is_err() {
                    return;
                }
            }
        }
        let _ = events_tx.send(Event::Exited(exit));
    });
    Ok(Connection {
        hello,
        events: Events {
            rx: events_rx,
            dir: dir.clone(),
            requests: None,
        },
        writer: Writer {
            inner: Arc::new(Inner { dir, link: None }),
        },
    })
}

/// The exit from `exit.json`. A holder that died before the CLI left none,
/// so then the exit has no code or signal and is written down, so every
/// client sees the same one.
fn exit_from_files(dir: &Path) -> io::Result<Exit> {
    if let Some(exit) = files::read_json(&dir.join(EXIT))? {
        return Ok(exit);
    }
    let limit = files::read_json::<LaunchFile>(&dir.join(LAUNCH))
        .ok()
        .flatten()
        .map_or(Limits::default().stderr_tail, |launch| launch.stderr_tail);
    let exit = Exit {
        code: None,
        signal: None,
        at: Timestamp::now(),
        stderr_tail: files::read_tail(&dir.join(STDERR), limit)?,
    };
    // Without the file the next client makes up its own, which differs
    // only in when.
    let _ = files::write_json(&dir.join(EXIT), &exit);
    Ok(exit)
}
