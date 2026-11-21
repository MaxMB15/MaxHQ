// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Forwards and their streams: a client's local port reaching a port an
//! agent listens on, carried in tunnel frames with credit per stream.
//! Half-close is [`TunnelEnd::Write`] and abort is [`TunnelEnd::Reset`].
//!
//! [`Tunnels`] keeps every forward and stream. A stream to a container
//! workspace relays through that container's guest link, and the guest
//! connects, in `guest/tunnel.rs`. A stream to a host folder agent's port
//! is connected here. Each side sends through a [`Sink`], which queues
//! messages and stream bytes in order for its link's [`Writer`]. Each
//! client connection holds a [`ClientTunnels`], and each guest link a
//! [`GuestRelay`].
//!
//! [`TunnelEnd::Write`]: maxhq_protocol::message::TunnelEnd::Write
//! [`TunnelEnd::Reset`]: maxhq_protocol::message::TunnelEnd::Reset

mod pump;
mod registry;

use std::future::Future;

use maxhq_protocol::frame::{encode_message, encode_tunnel_data, EncodeError};
use maxhq_protocol::ids::StreamId;
use serde::Serialize;
use thiserror::Error;
use tokio::sync::mpsc;

pub(crate) use self::pump::{connect, open, targets, End, ToClient, Words};
#[cfg(test)]
pub(crate) use self::registry::tests as registry_tests;
pub use self::registry::{ClientTunnels, GuestRelay, Tunnels};

/// Credit each stream starts with, in bytes.
pub const STREAM_CREDIT: u64 = 256 * 1024;
/// The most credit a stream holds at once, in bytes. Grants pay back bytes
/// already sent, so real ones never come near it, and grants past it are
/// ignored.
pub const MOST_CREDIT: u64 = 4 * STREAM_CREDIT;
/// The most streams open through one forward.
pub const MAX_STREAMS_PER_FORWARD: usize = 256;
/// The most streams open for one client.
pub const MAX_STREAMS_PER_CLIENT: usize = 1024;

/// Something a tunnel sends on a link: a message, or bytes on a stream.
#[derive(Clone, Debug, PartialEq)]
pub enum Out<M> {
    /// A message, sent as a kind 0 frame.
    Message(M),
    /// Bytes on a stream, sent as kind 4 frames.
    Data {
        /// The stream.
        stream: StreamId,
        /// The bytes, which may be longer than one frame holds.
        data: Vec<u8>,
    },
}

impl<M: Serialize> Out<M> {
    /// Appends the frames for this to `out`. Bytes longer than one tunnel
    /// frame holds are split.
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        match self {
            Self::Message(message) => encode_message(out, message),
            Self::Data { stream, data } => {
                encode_tunnel_data(out, *stream, data);
                Ok(())
            }
        }
    }
}

/// The link a [`Sink`] wrote to is gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
#[error("the link is gone")]
pub struct Gone;

/// Where a tunnel sends to reach one link: a client's, or a container's
/// guest link. `M` is the message type that link carries.
///
/// A send never waits. It queues behind everything sent before it, so a
/// stream's close never passes its bytes. Credit bounds how much any
/// stream can queue.
pub trait Sink<M>: Send + Sync + 'static {
    /// Queues a message or stream bytes for the link's writer.
    fn send(&self, out: Out<M>) -> Result<(), Gone>;
}

/// A [`Sink`] that hands everything to a channel, for a link's writer task
/// to encode and write in order.
pub struct Queue<M> {
    tx: mpsc::UnboundedSender<Out<M>>,
}

impl<M> Queue<M> {
    /// A queue and the receiver its link's writer reads.
    pub fn new() -> (Self, mpsc::UnboundedReceiver<Out<M>>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }
}

impl<M: Send + 'static> Queue<M> {
    /// A queue with its own task, which hands everything to `writer` one
    /// at a time and in order. The task ends once the queue is dropped and
    /// emptied, or as soon as a write fails. Sends after a failed write
    /// give [`Gone`].
    pub fn spawn(mut writer: impl Writer<M>) -> Self {
        let (queue, mut rx) = Self::new();
        tokio::spawn(async move {
            while let Some(out) = rx.recv().await {
                if writer.write(out).await.is_err() {
                    break;
                }
            }
        });
        queue
    }
}

impl<M: Send + 'static> Sink<M> for Queue<M> {
    fn send(&self, out: Out<M>) -> Result<(), Gone> {
        self.tx.send(out).map_err(|_| Gone)
    }
}

/// Writes what a [`Queue::spawn`] queue holds to its link. A write may
/// wait, since the queue's own task makes it.
pub trait Writer<M>: Send + 'static {
    /// Writes a message or stream bytes. [`Gone`] stops the queue.
    fn write(&mut self, out: Out<M>) -> impl Future<Output = Result<(), Gone>> + Send;
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use maxhq_protocol::frame::{Frame, FrameDecoder, MAX_TUNNEL_CHUNK};
    use maxhq_protocol::message::{ServerMessage, TunnelEnd};

    use super::*;

    #[test]
    fn long_bytes_are_split_into_tunnel_frames() {
        let stream = StreamId::generate();
        let data = vec![7; MAX_TUNNEL_CHUNK + 10];
        let mut out = Vec::new();
        Out::<ServerMessage>::Data {
            stream,
            data: data.clone(),
        }
        .encode(&mut out)
        .unwrap();
        Out::Message(ServerMessage::TunnelClose {
            stream,
            how: TunnelEnd::Write,
        })
        .encode(&mut out)
        .unwrap();

        let mut decoder = FrameDecoder::new();
        decoder.push(&out);
        let mut got = Vec::new();
        let mut kinds = Vec::new();
        while let Some(frame) = decoder.next_frame().unwrap() {
            match frame {
                Frame::TunnelData { stream: s, data } => {
                    assert_eq!(s, stream);
                    got.extend_from_slice(data);
                    kinds.push("data");
                }
                Frame::Message(_) => kinds.push("message"),
                other => panic!("unexpected frame {other:?}"),
            }
        }
        assert_eq!(got, data);
        assert_eq!(kinds, ["data", "data", "message"]);
    }

    #[tokio::test]
    async fn a_queue_keeps_order_and_reports_a_closed_writer() {
        let (queue, mut rx) = Queue::<u32>::new();
        let stream = StreamId::generate();
        queue.send(Out::Message(1)).unwrap();
        queue
            .send(Out::Data {
                stream,
                data: b"x".to_vec(),
            })
            .unwrap();
        assert_eq!(rx.recv().await, Some(Out::Message(1)));
        assert!(matches!(rx.recv().await, Some(Out::Data { .. })));
        drop(rx);
        assert_eq!(queue.send(Out::Message(2)), Err(Gone));
    }

    /// Writes into a channel after a wait, and fails on a message of 0.
    struct Slow(mpsc::UnboundedSender<Out<u32>>);

    impl Writer<u32> for Slow {
        async fn write(&mut self, out: Out<u32>) -> Result<(), Gone> {
            tokio::time::sleep(Duration::from_millis(1)).await;
            if out == Out::Message(0) {
                return Err(Gone);
            }
            self.0.send(out).map_err(|_| Gone)
        }
    }

    #[tokio::test]
    async fn a_spawned_queue_writes_in_order_until_a_write_fails() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let queue = Queue::spawn(Slow(tx));
        for n in 1..=20 {
            queue.send(Out::Message(n)).unwrap();
        }
        for n in 1..=20 {
            assert_eq!(rx.recv().await, Some(Out::Message(n)));
        }
        queue.send(Out::Message(0)).unwrap();
        assert_eq!(rx.recv().await, None);
        assert_eq!(queue.send(Out::Message(21)), Err(Gone));
    }
}
