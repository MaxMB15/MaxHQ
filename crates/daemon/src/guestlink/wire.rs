// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What both ends of the guest link share: the outbox that queues frames
//! for one writer task, the reader that splits frames, the hold types on
//! the wire, and the token rate limit. What only the guest sends is in
//! `guest/wire.rs`, which builds on Unix alone, as the guest does.
//!
//! The link never ends a request by shutting its write side, since the
//! Incus proxy drops a half-close. Every message is a whole frame.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::time::{Duration, Instant};

use maxhq_hold as hold;
use maxhq_protocol::frame::EncodeError;
use maxhq_protocol::frame::{encode_message, encode_transfer_chunks, encode_tunnel_data};
use maxhq_protocol::guest::{
    ChildState, Ended, Framing, GuestDecoder, GuestFrame, GuestLinkError, HolderExit, HolderHello,
    HolderLine, WriteOutcome, MAX_TOKEN_REQUESTS_PER_MINUTE, MAX_TUNNELS,
};
use maxhq_protocol::ids::{SessionId, StreamId, TransferId};
use serde::Serialize;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// How many writes wait for the writer before a sender waits too.
const QUEUE: usize = 256;

/// How much one read takes from the socket.
const READ_SIZE: usize = 64 * 1024;

/// Why something didn't go out on the link.
#[derive(Debug, Error)]
pub enum SendError {
    /// The link is down.
    #[error("the guest link is down")]
    Down,
    /// The link already has [`MAX_TUNNELS`] tunnel streams open.
    #[error("the guest link already has {MAX_TUNNELS} tunnels open")]
    TooManyTunnels,
    /// The message is too big for a frame, or didn't serialize.
    #[error(transparent)]
    Encode(#[from] EncodeError),
}

/// Queues frames for the link's one writer, so frames never interleave.
/// Cloning it gives another sender on the same link.
#[derive(Clone, Debug)]
pub struct Outbox {
    tx: mpsc::Sender<Vec<u8>>,
}

impl Outbox {
    /// An outbox, and the queue its writer drains.
    pub(crate) fn new() -> (Self, mpsc::Receiver<Vec<u8>>) {
        let (tx, rx) = mpsc::channel(QUEUE);
        (Self { tx }, rx)
    }

    /// Sends one message, waiting while the queue is full.
    pub async fn send(&self, message: &impl Serialize) -> Result<(), SendError> {
        let mut frame = Vec::new();
        encode_message(&mut frame, message)?;
        self.frames(frame).await
    }

    /// Sends file bytes for a transfer, split into chunk frames.
    pub async fn transfer_chunks(
        &self,
        transfer: TransferId,
        offset: u64,
        bytes: &[u8],
    ) -> Result<(), SendError> {
        let mut frames = Vec::new();
        encode_transfer_chunks(&mut frames, transfer, offset, bytes);
        self.frames(frames).await
    }

    /// Sends bytes on a tunnel stream, split into data frames.
    pub async fn tunnel_data(&self, stream: StreamId, bytes: &[u8]) -> Result<(), SendError> {
        let mut frames = Vec::new();
        encode_tunnel_data(&mut frames, stream, bytes);
        self.frames(frames).await
    }

    /// Sends one message without waiting, for code that can't wait, such
    /// as a drop. With the queue full it goes out from a task of its own.
    pub fn post(&self, message: &impl Serialize) {
        let mut frame = Vec::new();
        if let Err(err) = encode_message(&mut frame, message) {
            tracing::warn!("couldn't encode a message for the guest link: {err}");
            return;
        }
        match self.tx.try_send(frame) {
            Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => {}
            Err(mpsc::error::TrySendError::Full(frame)) => {
                if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                    let tx = self.tx.clone();
                    runtime.spawn(async move {
                        let _ = tx.send(frame).await;
                    });
                }
            }
        }
    }

    async fn frames(&self, frames: Vec<u8>) -> Result<(), SendError> {
        self.tx.send(frames).await.map_err(|_| SendError::Down)
    }
}

/// Aborts a task when dropped, so a link's writer never outlives the link
/// and the other end sees the socket close.
pub(crate) struct Abort<T>(pub(crate) JoinHandle<T>);

impl<T> Drop for Abort<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Writes queued frames until the queue closes or a write fails. It never
/// shuts the write side.
pub(crate) async fn write_frames(
    mut writer: impl AsyncWrite + Unpin,
    mut queue: mpsc::Receiver<Vec<u8>>,
) -> io::Result<()> {
    while let Some(frames) = queue.recv().await {
        writer.write_all(&frames).await?;
    }
    Ok(())
}

/// A frame read off the link, owned so it can outlive the decoder.
#[derive(Debug)]
pub(crate) enum Read {
    /// A message body, not parsed yet.
    Message(Vec<u8>),
    /// Part of an upload or download.
    Chunk {
        /// The transfer.
        transfer: TransferId,
        /// Where `data` starts.
        offset: u64,
        /// The bytes.
        data: Vec<u8>,
    },
    /// Bytes on a tunnel stream.
    Tunnel {
        /// The stream.
        stream: StreamId,
        /// The bytes.
        data: Vec<u8>,
    },
}

/// Why reading the link stopped.
#[derive(Debug, Error)]
pub(crate) enum ReadError {
    /// The other end closed the link.
    #[error("the link closed")]
    Closed,
    /// The socket failed.
    #[error("couldn't read the link: {0}")]
    Io(#[from] io::Error),
    /// The bytes broke the link's rules.
    #[error(transparent)]
    Link(#[from] GuestLinkError),
}

/// Reads frames off one side of the link.
pub(crate) struct Reader<R> {
    reader: R,
    decoder: GuestDecoder,
    buf: Vec<u8>,
}

impl<R: AsyncRead + Unpin> Reader<R> {
    /// A reader with nothing buffered.
    pub(crate) fn new(reader: R) -> Self {
        Self {
            reader,
            decoder: GuestDecoder::new(),
            buf: vec![0; READ_SIZE],
        }
    }

    /// The next frame. Any error means the link should drop.
    pub(crate) async fn next(&mut self) -> Result<Read, ReadError> {
        loop {
            if let Some(frame) = self.decoder.next_frame()? {
                return Ok(match frame {
                    GuestFrame::Message(body) => Read::Message(body.to_vec()),
                    GuestFrame::TransferChunk {
                        transfer,
                        offset,
                        data,
                    } => Read::Chunk {
                        transfer,
                        offset,
                        data: data.to_vec(),
                    },
                    GuestFrame::TunnelData { stream, data } => Read::Tunnel {
                        stream,
                        data: data.to_vec(),
                    },
                });
            }
            let read = self.reader.read(&mut self.buf).await?;
            if read == 0 {
                return Err(ReadError::Closed);
            }
            self.decoder.push(&self.buf[..read]);
        }
    }
}

/// How many token requests each session made in the last minute.
#[derive(Debug, Default)]
pub(crate) struct TokenRate {
    sessions: HashMap<SessionId, VecDeque<Instant>>,
}

impl TokenRate {
    /// The window the limit counts in.
    const WINDOW: Duration = Duration::from_secs(60);

    /// Counts a request, and says whether it's within
    /// [`MAX_TOKEN_REQUESTS_PER_MINUTE`].
    pub(crate) fn allow(&mut self, session: SessionId, now: Instant) -> bool {
        self.sessions.retain(|_, times| {
            times
                .back()
                .is_some_and(|last| now.duration_since(*last) < Self::WINDOW)
        });
        let times = self.sessions.entry(session).or_default();
        while times
            .front()
            .is_some_and(|first| now.duration_since(*first) >= Self::WINDOW)
        {
            times.pop_front();
        }
        if times.len() >= MAX_TOKEN_REQUESTS_PER_MINUTE as usize {
            return false;
        }
        times.push_back(now);
        true
    }
}

/// A holder line off the link.
pub(crate) fn line_in(line: HolderLine) -> hold::Line {
    hold::Line {
        seq: line.seq,
        at: line.at,
        text: line.text,
        replayed: line.replayed,
    }
}

/// A holder exit off the link.
pub(crate) fn exit_in(exit: HolderExit) -> hold::Exit {
    hold::Exit {
        code: exit.code,
        signal: exit.signal,
        at: exit.at,
        stderr_tail: exit.stderr_tail,
    }
}

/// A holder hello off the link.
pub(crate) fn hello_in(hello: HolderHello) -> hold::Hello {
    hold::Hello {
        holder_pid: hello.holder_pid,
        child: match hello.child {
            ChildState::Running { pid } => hold::ChildState::Running { pid },
            ChildState::Exited(exit) => hold::ChildState::Exited(exit_in(exit)),
        },
        first_seq: hello.first_seq,
        last_seq: hello.last_seq,
        pending: hello.pending.into_iter().map(line_in).collect(),
    }
}

/// A write outcome off the link.
pub(crate) fn outcome_in(outcome: WriteOutcome) -> hold::WriteOutcome {
    match outcome {
        WriteOutcome::Written => hold::WriteOutcome::Written,
        WriteOutcome::AlreadyAnswered => hold::WriteOutcome::AlreadyAnswered,
    }
}

/// An end's outcome off the link.
pub(crate) fn ended_in(ended: Ended) -> hold::Ended {
    match ended {
        Ended::Gone => hold::Ended::Gone,
        Ended::Unconfirmed { pid, reason } => hold::Ended::Unconfirmed { pid, reason },
    }
}

/// A framing as the link carries it.
pub(crate) fn framing_out(framing: hold::Framing) -> Framing {
    match framing {
        hold::Framing::ClaudeStreamJson => Framing::ClaudeStreamJson,
        hold::Framing::JsonRpc => Framing::JsonRpc,
    }
}

/// A write error from its text, as the guest sends it. Text that isn't
/// one of the hold crate's own becomes [`hold::WriteError::Io`].
pub(crate) fn write_error_in(text: String) -> hold::WriteError {
    use hold::WriteError as E;
    for known in [E::Finished, E::StillRunning, E::BadLine, E::Disconnected] {
        if known.to_string() == text {
            return known;
        }
    }
    E::Io(text)
}

/// A connect error from its text, as the guest sends it.
pub(crate) fn connect_error_in(text: String, session: SessionId) -> hold::ConnectError {
    use hold::ConnectError as E;
    if text == (E::NotFound { session }).to_string() {
        return E::NotFound { session };
    }
    if text == (E::NotStarted { session }).to_string() {
        return E::NotStarted { session };
    }
    let refused = E::Refused {
        message: String::new(),
    }
    .to_string();
    if let Some(message) = text.strip_prefix(refused.as_str()) {
        return E::Refused {
            message: message.to_owned(),
        };
    }
    let protocol = E::Protocol(String::new()).to_string();
    if let Some(message) = text.strip_prefix(protocol.as_str()) {
        return E::Protocol(message.to_owned());
    }
    E::Io(io::Error::other(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> SessionId {
        "0199a7c2-0000-7000-8000-0000000000f1".parse().unwrap()
    }

    #[test]
    fn errors_come_back_from_their_text() {
        use hold::WriteError as W;
        for err in [W::Finished, W::StillRunning, W::BadLine, W::Disconnected] {
            let text = err.to_string();
            assert_eq!(write_error_in(text.clone()).to_string(), text);
        }
        assert!(matches!(write_error_in("disk full".into()), W::Io(text) if text == "disk full"));

        use hold::ConnectError as C;
        let session = session();
        assert!(matches!(
            connect_error_in(C::NotFound { session }.to_string(), session),
            C::NotFound { .. }
        ));
        assert!(matches!(
            connect_error_in(C::NotStarted { session }.to_string(), session),
            C::NotStarted { .. }
        ));
        let refused = C::Refused {
            message: "busy".into(),
        };
        assert!(matches!(
            connect_error_in(refused.to_string(), session),
            C::Refused { message } if message == "busy"
        ));
        assert!(matches!(
            connect_error_in("socket gone".into(), session),
            C::Io(_)
        ));
    }

    #[test]
    fn a_session_gets_thirty_token_requests_a_minute() {
        let mut rate = TokenRate::default();
        let start = Instant::now();
        let session = session();
        let other: SessionId = "0199a7c2-0000-7000-8000-0000000000f2".parse().unwrap();
        for _ in 0..MAX_TOKEN_REQUESTS_PER_MINUTE {
            assert!(rate.allow(session, start));
        }
        assert!(!rate.allow(session, start + Duration::from_secs(59)));
        assert!(rate.allow(other, start));
        assert!(rate.allow(session, start + Duration::from_secs(61)));
    }
}
