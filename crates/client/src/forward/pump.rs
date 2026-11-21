// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! One stream on this side: a connection accepted on a forward's local
//! port, pumped to and from the link with credit each way.
//!
//! This end sends nothing until `maxd` says the stream opened. Then it may
//! send [`STREAM_CREDIT`] bytes, and more as `maxd` grants. `maxd` may send
//! as much, and more as this end writes its bytes to the socket. Bytes
//! past that reset the stream. `maxd`'s [`TunnelEnd::Write`] shuts the
//! socket's write side once its bytes are written, and the socket's end of
//! file sends one back. A reset either way aborts the socket.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use maxhq_protocol::frame::MAX_TUNNEL_CHUNK;
use maxhq_protocol::ids::StreamId;
use maxhq_protocol::message::TunnelEnd;
use maxhq_protocol::ClientMessage;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Notify};

use super::{ForwardLink, Gone, Out, STREAM_CREDIT};

/// The most credit a stream holds at once, in bytes. Grants pay back bytes
/// already sent, so real ones never come near it, and grants past it are
/// ignored.
const MOST_CREDIT: u64 = 4 * STREAM_CREDIT;

/// How many bytes written to the socket earn `maxd` more credit at once.
const GRANT_STEP: u64 = MAX_TUNNEL_CHUNK as u64;

/// What `maxd` sent that the socket's writer acts on, in order.
enum Event {
    Data(Vec<u8>),
    Shut,
}

/// Sets `value` to what `change` makes of it. False, and nothing changes,
/// when `change` gives none.
fn update(value: &AtomicU64, change: impl Fn(u64) -> Option<u64>) -> bool {
    let mut now = value.load(Ordering::Acquire);
    loop {
        let Some(next) = change(now) else {
            return false;
        };
        match value.compare_exchange_weak(now, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(actual) => now = actual,
        }
    }
}

/// Bytes this end may still send, and a wake for when it may send more.
struct Credit {
    left: AtomicU64,
    more: Notify,
}

impl Credit {
    fn grant(&self, granted: u64) {
        update(&self.left, |left| {
            Some(left.saturating_add(granted).min(MOST_CREDIT))
        });
        self.more.notify_one();
    }

    /// Waits until there's credit, and gives how much.
    async fn wait(&self) -> u64 {
        loop {
            let left = self.left.load(Ordering::Acquire);
            if left > 0 {
                return left;
            }
            self.more.notified().await;
        }
    }

    fn spend(&self, n: u64) {
        self.left.fetch_sub(n, Ordering::AcqRel);
    }
}

/// The handle [`Forwards`](super::Forwards) holds for one stream.
/// Dropping it aborts the stream.
pub(super) struct End {
    events: mpsc::UnboundedSender<Event>,
    /// Bytes `maxd` may still send this end.
    window: Arc<AtomicU64>,
    credit: Arc<Credit>,
    abort: Arc<Notify>,
}

impl End {
    /// `maxd` connected the stream, so this end may send.
    pub(super) fn opened(&self) {
        self.credit.grant(STREAM_CREDIT);
    }

    /// Takes bytes from `maxd`. False when they're over the credit this end
    /// granted, and the stream must be reset.
    pub(super) fn data(&self, bytes: &[u8]) -> bool {
        let len = bytes.len() as u64;
        if !update(&self.window, |left| left.checked_sub(len)) {
            return false;
        }
        let _ = self.events.send(Event::Data(bytes.to_vec()));
        true
    }

    /// `maxd` may take more.
    pub(super) fn credit(&self, granted: u64) {
        self.credit.grant(granted);
    }

    /// `maxd` closed its half, or reset the stream.
    pub(super) fn close(&self, how: TunnelEnd) {
        match how {
            TunnelEnd::Write => {
                let _ = self.events.send(Event::Shut);
            }
            TunnelEnd::Reset => self.abort.notify_one(),
        }
    }
}

impl Drop for End {
    fn drop(&mut self) {
        self.abort.notify_one();
    }
}

/// What a stream pumps: a connection accepted on a forward's port, or one
/// end of an in-memory pipe.
pub trait Socket: AsyncRead + AsyncWrite + Send + Unpin + 'static {
    /// Readies it for the pump.
    fn ready(&self) {}
    /// Makes the coming close a reset, where the socket has one.
    fn abort(&self) {}
}

impl Socket for TcpStream {
    fn ready(&self) {
        let _ = self.set_nodelay(true);
    }

    fn abort(&self) {
        // A reset, not a quiet close, so the program sees the abort.
        let _ = self.set_zero_linger();
    }
}

impl Socket for DuplexStream {}

/// Pumps `socket` for `stream` in a task of its own, until the stream
/// ends. `ended` runs once it's over, however it ends.
pub(super) fn start<S: Socket>(
    stream: StreamId,
    socket: S,
    link: Arc<dyn ForwardLink>,
    ended: Box<dyn FnOnce() + Send>,
) -> End {
    let (events, rx) = mpsc::unbounded_channel();
    let end = End {
        events,
        window: Arc::new(AtomicU64::new(STREAM_CREDIT)),
        credit: Arc::new(Credit {
            left: AtomicU64::new(0),
            more: Notify::new(),
        }),
        abort: Arc::new(Notify::new()),
    };
    let task = Task {
        stream,
        link,
        window: end.window.clone(),
        credit: end.credit.clone(),
    };
    let abort = end.abort.clone();
    tokio::spawn(async move {
        task.run(socket, rx, &abort).await;
        ended();
    });
    end
}

/// Why a pump stopped before both halves closed.
enum Abort {
    /// The socket failed here, so `maxd` must hear of it.
    Local,
    /// `maxd` reset the stream, or the link is gone.
    Peer,
}

struct Task {
    stream: StreamId,
    link: Arc<dyn ForwardLink>,
    window: Arc<AtomicU64>,
    credit: Arc<Credit>,
}

impl Task {
    async fn run<S: Socket>(
        &self,
        mut socket: S,
        events: mpsc::UnboundedReceiver<Event>,
        abort: &Notify,
    ) {
        socket.ready();
        let ended = tokio::select! {
            biased;
            () = abort.notified() => Err(Abort::Peer),
            ended = self.pump(&mut socket, events) => ended,
        };
        if let Err(abort) = ended {
            if matches!(abort, Abort::Local) {
                let _ = self.say(ClientMessage::TunnelClose {
                    stream: self.stream,
                    how: TunnelEnd::Reset,
                });
            }
            socket.abort();
        }
    }

    fn say(&self, message: ClientMessage) -> Result<(), Gone> {
        self.link.send(Out::Message(message))
    }

    async fn pump<S: Socket>(
        &self,
        socket: &mut S,
        events: mpsc::UnboundedReceiver<Event>,
    ) -> Result<(), Abort> {
        let (reader, writer) = tokio::io::split(socket);
        tokio::try_join!(self.reading(reader), self.writing(writer, events)).map(|_| ())
    }

    /// Socket to link, within the credit `maxd` granted.
    async fn reading(&self, mut reader: impl AsyncRead + Unpin) -> Result<(), Abort> {
        let mut buf = vec![0; MAX_TUNNEL_CHUNK];
        loop {
            let left = self.credit.wait().await;
            let room = usize::try_from(left).unwrap_or(usize::MAX).min(buf.len());
            let n = reader
                .read(&mut buf[..room])
                .await
                .map_err(|_| Abort::Local)?;
            if n == 0 {
                self.say(ClientMessage::TunnelClose {
                    stream: self.stream,
                    how: TunnelEnd::Write,
                })
                .map_err(|_| Abort::Peer)?;
                return Ok(());
            }
            self.credit.spend(n as u64);
            self.link
                .send(Out::Data {
                    stream: self.stream,
                    data: buf[..n].to_vec(),
                })
                .map_err(|_| Abort::Peer)?;
        }
    }

    /// Link to socket, granting `maxd` more as its bytes are written.
    async fn writing(
        &self,
        mut writer: impl AsyncWrite + Unpin,
        mut events: mpsc::UnboundedReceiver<Event>,
    ) -> Result<(), Abort> {
        let mut unacked = 0;
        loop {
            match events.recv().await {
                None => return Err(Abort::Peer),
                Some(Event::Data(bytes)) => {
                    writer.write_all(&bytes).await.map_err(|_| Abort::Local)?;
                    unacked += bytes.len() as u64;
                    if unacked >= GRANT_STEP {
                        // The window grows before `maxd` hears, so its next
                        // bytes always fit.
                        self.window.fetch_add(unacked, Ordering::AcqRel);
                        self.say(ClientMessage::TunnelCredit {
                            stream: self.stream,
                            granted: unacked,
                        })
                        .map_err(|_| Abort::Peer)?;
                        unacked = 0;
                    }
                }
                Some(Event::Shut) => {
                    let _ = writer.shutdown().await;
                    return Ok(());
                }
            }
        }
    }
}
