// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A guest's holders and history readers, as the session manager sees
//! them. Each call goes over the workspace's current link, so a holder
//! reached through a dropped link reports it the way a local holder whose
//! socket closed does.

use std::io;
use std::sync::mpsc::{RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use futures::stream::StreamExt;
use maxhq_hold as hold;
use maxhq_protocol::guest::{GuestLaunch, GuestToHost, HolderEvent, HolderHello, HostToGuest};
use maxhq_protocol::ids::{AgentId, SessionId, WorkspaceId};
use maxhq_protocol::model::Vendor;
use maxhq_protocol::timeline::TimelineItem;
use tokio::sync::mpsc;

use super::link::{self, Failed, HistoryBatch, Link, Unread, DOWN, REQUEST_WAIT};
use super::wire;
use super::Shared;
use crate::session::clock::BoxFuture;
use crate::session::holders::{
    ConnectError, Connection, Ended, Event, HoldWriter, Holders, Launch, SpawnError, WriteError,
    WriteOutcome,
};
use crate::session::vendors::History;

/// How long a holder gets to stop its CLI when the host ends a session in
/// a guest, as a local end gives it.
const END_GRACE: Duration = Duration::from_secs(10);

/// The user a guest's CLI runs as when the launch names none.
const AGENT: &str = "agent";

/// How long a history's first read waits for the guest to read the whole
/// history.
const HISTORY_WAIT: Duration = Duration::from_secs(30);

/// One workspace's holders, reached through its guest.
pub(crate) struct GuestHolders {
    shared: Weak<Shared>,
    workspace: WorkspaceId,
}

impl GuestHolders {
    /// The holders in `workspace`'s guest, over whichever link is up when
    /// each call is made.
    pub(crate) fn new(shared: Weak<Shared>, workspace: WorkspaceId) -> Self {
        Self { shared, workspace }
    }

    fn link(&self) -> Option<Arc<Link>> {
        self.shared.upgrade()?.link(self.workspace)
    }
}

/// The guest's launch for a holder launch. Every path and argument must be
/// UTF-8, which the link carries as JSON strings.
fn guest_launch(launch: Launch) -> Result<GuestLaunch, SpawnError> {
    fn text(what: &str, value: impl Into<std::ffi::OsString>) -> Result<String, SpawnError> {
        value.into().into_string().map_err(|_| SpawnError::Holder {
            message: format!("the launch's {what} isn't UTF-8"),
        })
    }
    let pairs = |what: &str, pairs: Vec<(std::ffi::OsString, std::ffi::OsString)>| {
        pairs
            .into_iter()
            .map(|(key, value)| Ok((text(what, key)?, text(what, value)?)))
            .collect::<Result<Vec<_>, SpawnError>>()
    };
    Ok(GuestLaunch {
        session: launch.session,
        program: text("program", launch.program)?,
        args: launch
            .args
            .into_iter()
            .map(|arg| text("arguments", arg))
            .collect::<Result<_, _>>()?,
        env: pairs("environment", launch.env)?,
        env_remove: launch
            .env_remove
            .into_iter()
            .map(|key| text("environment", key))
            .collect::<Result<_, _>>()?,
        secret_env: launch
            .secret_env
            .into_iter()
            .map(|(key, value)| Ok((text("secret environment", key)?, value)))
            .collect::<Result<_, SpawnError>>()?,
        cwd: text("folder", launch.cwd)?,
        framing: wire::framing_out(launch.framing),
        user: launch
            .run_as
            .map_or_else(|| AGENT.to_owned(), |user| user.user),
    })
}

impl Holders for GuestHolders {
    fn spawn(&self, launch: Launch) -> BoxFuture<Result<(), SpawnError>> {
        let link = self.link();
        Box::pin(async move {
            let link = link.ok_or_else(|| SpawnError::Holder {
                message: DOWN.to_owned(),
            })?;
            let launch = guest_launch(launch)?;
            let failed = |message: String| SpawnError::Holder { message };
            // A session forgotten just before may share this one's agent,
            // and the guest must be done with its attachments first.
            link.forgotten().await;
            match link
                .request(REQUEST_WAIT, |req| HostToGuest::Spawn { req, launch })
                .await
            {
                Ok(GuestToHost::SpawnResult { result, .. }) => result.map_err(failed),
                Ok(other) => Err(failed(link::unexpected(&other))),
                Err(err) => Err(failed(err.to_string())),
            }
        })
    }

    fn list(&self) -> BoxFuture<io::Result<Vec<SessionId>>> {
        let link = self.link();
        Box::pin(async move {
            let link = link.ok_or_else(|| io::Error::other(DOWN))?;
            match link
                .request(REQUEST_WAIT, |req| HostToGuest::ListHolders { req })
                .await
            {
                Ok(GuestToHost::Holders { sessions, .. }) => Ok(sessions),
                Ok(other) => Err(io::Error::other(link::unexpected(&other))),
                Err(err) => Err(io::Error::other(err.to_string())),
            }
        })
    }

    fn connect(
        &self,
        session: SessionId,
        after: Option<u64>,
    ) -> BoxFuture<Result<Connection, ConnectError>> {
        let link = self.link();
        Box::pin(async move {
            let link = link.ok_or_else(|| ConnectError::Io(io::Error::other(DOWN)))?;
            connect(link, session, after).await
        })
    }

    fn end(&self, session: SessionId, holder_pid: Option<u32>) -> BoxFuture<Ended> {
        let link = self.link();
        Box::pin(async move {
            let Some(link) = link else {
                return Ended::Unconfirmed {
                    pid: holder_pid,
                    reason: DOWN.to_owned(),
                };
            };
            end(link, session, holder_pid).await
        })
    }
}

/// Connects to a holder in the guest, and waits for its whole hello.
async fn connect(
    link: Arc<Link>,
    session: SessionId,
    after: Option<u64>,
) -> Result<Connection, ConnectError> {
    let (conn, mut events) = link
        .open_conn()
        .map_err(|err| ConnectError::Io(io::Error::other(err)))?;
    let guard = Arc::new(ConnGuard {
        link: link.clone(),
        conn,
    });
    link.post(&HostToGuest::Connect {
        conn,
        session,
        after,
    });
    let hello = tokio::time::timeout(REQUEST_WAIT, hello(&mut events, session))
        .await
        .map_err(|_| {
            ConnectError::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                "the guest didn't connect to the holder in time",
            ))
        })??;
    let events =
        futures::stream::unfold((events, guard.clone()), |(mut events, guard)| async move {
            loop {
                let (event, _unread) = events.recv().await?;
                match event {
                    HolderEvent::Line(line) => {
                        return Some((Event::Line(wire::line_in(line)), (events, guard)));
                    }
                    HolderEvent::Exited(exit) => {
                        return Some((Event::Exited(wire::exit_in(exit)), (events, guard)));
                    }
                    HolderEvent::Closed { .. } => return None,
                    HolderEvent::Hello(_) => {}
                }
            }
        })
        .boxed();
    Ok(Connection {
        hello,
        events,
        writer: Arc::new(GuestWriter { guard, session }),
    })
}

/// Collects a hello that came in parts. A hello with pending lines is
/// followed by more parts, and a part with none ends it.
async fn hello(
    events: &mut mpsc::UnboundedReceiver<(HolderEvent, Unread)>,
    session: SessionId,
) -> Result<hold::Hello, ConnectError> {
    let mut whole: Option<HolderHello> = None;
    // The parts stay counted as unread until the whole hello goes to its reader.
    let mut unread = Vec::new();
    loop {
        let Some((event, part_unread)) = events.recv().await else {
            return Err(ConnectError::Io(io::Error::other(DOWN)));
        };
        unread.push(part_unread);
        match event {
            HolderEvent::Hello(part) => match whole.as_mut() {
                None if part.pending.is_empty() => return Ok(wire::hello_in(part)),
                None => whole = Some(part),
                Some(_) if part.pending.is_empty() => {
                    return Ok(wire::hello_in(whole.take().unwrap_or(part)));
                }
                Some(whole) => whole.pending.extend(part.pending),
            },
            HolderEvent::Closed { error } => {
                return Err(match error {
                    Some(error) => wire::connect_error_in(error, session),
                    None => ConnectError::Io(io::Error::other(
                        "the guest closed the connection before the holder's hello",
                    )),
                });
            }
            _ => {
                return Err(ConnectError::Protocol(
                    "the guest sent holder events before the hello".to_owned(),
                ));
            }
        }
    }
}

/// Ends a session in the guest, which signals only processes it confirms
/// as the session's own. With no answer, the holder may still run.
async fn end(link: Arc<Link>, session: SessionId, holder_pid: Option<u32>) -> Ended {
    let reply = link
        .request(REQUEST_WAIT + END_GRACE, |req| HostToGuest::End {
            req,
            session,
            holder_pid,
        })
        .await;
    let reason = match reply {
        Ok(GuestToHost::EndResult { ended, .. }) => return wire::ended_in(ended),
        Ok(other) => link::unexpected(&other),
        Err(err) => err.to_string(),
    };
    Ended::Unconfirmed {
        pid: holder_pid,
        reason,
    }
}

/// Keeps a connection open on the link while its events or writer live.
struct ConnGuard {
    link: Arc<Link>,
    conn: u64,
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.link.close_conn(self.conn);
    }
}

/// Writes to a holder in the guest, over the link the connection came on.
struct GuestWriter {
    guard: Arc<ConnGuard>,
    session: SessionId,
}

/// A write error for a request that got no answer.
fn write_failed(err: Failed) -> WriteError {
    match err {
        Failed::Down => WriteError::Disconnected,
        err => WriteError::Io(err.to_string()),
    }
}

/// The outcome of a `Done` answer.
fn done(reply: Result<GuestToHost, Failed>) -> Result<(), WriteError> {
    match reply {
        Ok(GuestToHost::Done { result, .. }) => result.map_err(wire::write_error_in),
        Ok(other) => Err(WriteError::Io(link::unexpected(&other))),
        Err(err) => Err(write_failed(err)),
    }
}

impl HoldWriter for GuestWriter {
    fn write(
        &self,
        line: String,
        answers: Option<String>,
    ) -> BoxFuture<Result<WriteOutcome, WriteError>> {
        let link = self.guard.link.clone();
        let conn = self.guard.conn;
        Box::pin(async move {
            let reply = link
                .request(REQUEST_WAIT, |req| HostToGuest::Write {
                    req,
                    conn,
                    line,
                    answers,
                })
                .await;
            match reply {
                Ok(GuestToHost::WriteResult { outcome, .. }) => {
                    outcome.map(wire::outcome_in).map_err(wire::write_error_in)
                }
                Ok(other) => Err(WriteError::Io(link::unexpected(&other))),
                Err(err) => Err(write_failed(err)),
            }
        })
    }

    fn stop(&self, grace: Duration) -> BoxFuture<Result<(), WriteError>> {
        let link = self.guard.link.clone();
        let conn = self.guard.conn;
        let grace_ms = u64::try_from(grace.as_millis()).unwrap_or(u64::MAX);
        Box::pin(async move {
            let reply = link
                .request(REQUEST_WAIT + grace, |req| HostToGuest::Stop {
                    req,
                    conn,
                    grace_ms,
                })
                .await;
            done(reply)
        })
    }

    fn forget(&self) -> BoxFuture<Result<(), WriteError>> {
        let link = self.guard.link.clone();
        // A spawn asked for after this call waits for the answer.
        let forgetting = link.forgetting();
        let session = self.session;
        Box::pin(async move {
            let reply = link
                .request(REQUEST_WAIT, |req| HostToGuest::Forget { req, session })
                .await;
            drop(forgetting);
            done(reply)
        })
    }
}

/// A session's history, read by the vendor's reader in the guest. The
/// guest polls the CLI's files and sends what's new, so each read hands
/// over what came since the last one.
///
/// The first read waits for the guest's first, which ends with an empty
/// batch, so it holds the whole history as a local reader's first read
/// does. Reads run on blocking threads, so the wait blocks.
pub(crate) struct GuestHistory {
    link: Arc<Link>,
    req: u64,
    items: std::sync::mpsc::Receiver<HistoryBatch>,
    caught_up: bool,
}

impl GuestHistory {
    /// Starts the reader in the guest, taking at most `bytes` of the newest
    /// history in its first read. None while the link is down or has as
    /// many histories open as it may.
    pub(crate) fn open(
        link: Arc<Link>,
        vendor: Vendor,
        config_dir: String,
        session: String,
        agent: AgentId,
        bytes: Option<u64>,
    ) -> Option<Self> {
        let (req, items) = link.open_history(vendor, config_dir, session, agent, bytes)?;
        Some(Self {
            link,
            req,
            items,
            caught_up: false,
        })
    }

    /// Takes the batches of the guest's first read, up to its empty one. They
    /// stay counted in `unread` until the caller lets go.
    fn catch_up(&mut self, unread: &mut Vec<Unread>) -> io::Result<Vec<TimelineItem>> {
        let until = Instant::now() + HISTORY_WAIT;
        let mut out = Vec::new();
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match self.items.recv_timeout(left) {
                Ok((items, _)) if items.is_empty() => {
                    self.caught_up = true;
                    return Ok(out);
                }
                Ok((items, batch)) => {
                    out.extend(items);
                    unread.push(batch);
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "the guest didn't send the history in time",
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::new(io::ErrorKind::BrokenPipe, DOWN));
                }
            }
        }
    }
}

impl History for GuestHistory {
    fn read(&mut self) -> io::Result<Vec<TimelineItem>> {
        let mut unread = Vec::new();
        let mut out = if self.caught_up {
            Vec::new()
        } else {
            self.catch_up(&mut unread)?
        };
        loop {
            match self.items.try_recv() {
                Ok((items, batch)) => {
                    out.extend(items);
                    unread.push(batch);
                }
                Err(TryRecvError::Empty) => return Ok(out),
                Err(TryRecvError::Disconnected) if out.is_empty() => {
                    return Err(io::Error::new(io::ErrorKind::BrokenPipe, DOWN));
                }
                Err(TryRecvError::Disconnected) => return Ok(out),
            }
        }
    }
}

impl Drop for GuestHistory {
    fn drop(&mut self) {
        self.link.close_history(self.req);
    }
}
