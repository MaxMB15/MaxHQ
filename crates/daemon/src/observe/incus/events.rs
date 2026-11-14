// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Lifecycle events from `/1.0/events`, so a container that starts or stops
//! shows up before the next poll. Incus serves events over a WebSocket, and
//! the observer only reads from it, so this holds just enough of the
//! protocol to read messages and answer the server's pings.

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use maxhq_protocol::time::Timestamp;
use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{api, time};

/// The pause before the first reconnect. It doubles up to [`LAST_RETRY`].
const FIRST_RETRY: Duration = Duration::from_secs(1);

/// The longest pause between reconnects.
const LAST_RETRY: Duration = Duration::from_secs(30);

/// The longest message read. Lifecycle events are a few hundred bytes, so
/// anything near this is a broken stream.
const MAX_MESSAGE: usize = 1 << 20;

/// The actions that change what a workspace container reports. Others,
/// such as `instance-file-retrieved`, which the observer's own reads cause,
/// or `instance-exec`, change nothing it reads.
const ACTIONS: [&str; 11] = [
    "instance-created",
    "instance-deleted",
    "instance-started",
    "instance-stopped",
    "instance-shutdown",
    "instance-restarted",
    "instance-paused",
    "instance-resumed",
    "instance-renamed",
    "instance-updated",
    "instance-restored",
];

/// The actions Incus sends once a container has stopped.
const STOPS: [&str; 2] = ["instance-stopped", "instance-shutdown"];

/// A lifecycle event that may change a workspace container.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lifecycle {
    /// The instance's name after the event.
    pub instance: String,
    /// When the container stopped, for an event that stops one.
    pub stopped_at: Option<Timestamp>,
}

/// One message from the event stream.
#[derive(Deserialize)]
struct Message {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    timestamp: String,
    metadata: Option<Metadata>,
}

/// A lifecycle event's details.
#[derive(Deserialize)]
struct Metadata {
    action: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    context: Option<Context>,
}

/// The extra fields some actions carry.
#[derive(Deserialize)]
struct Context {
    #[serde(default)]
    old_name: Option<String>,
}

/// Reads one event message. None unless it's a lifecycle event in
/// [`ACTIONS`] for a workspace container, which a rename counts as when
/// either name is one.
pub fn parse(message: &[u8]) -> Option<Lifecycle> {
    let message: Message = serde_json::from_slice(message).ok()?;
    let metadata = message.metadata?;
    if message.kind != "lifecycle" || !ACTIONS.contains(&metadata.action.as_str()) {
        return None;
    }
    let old_name = metadata.context.and_then(|c| c.old_name);
    let names_a_workspace = super::is_workspace_name(&metadata.name)
        || old_name.as_deref().is_some_and(super::is_workspace_name);
    if !names_a_workspace {
        return None;
    }
    let stopped_at = if STOPS.contains(&metadata.action.as_str()) {
        time::parse(&message.timestamp)
    } else {
        None
    };
    Some(Lifecycle {
        instance: metadata.name,
        stopped_at,
    })
}

/// Sends lifecycle events to `events` until its receiver closes. When the
/// stream drops, as it does when Incus restarts, this connects again after
/// a pause that grows while connections keep failing. The poll goes on
/// meanwhile, so a gap here only slows updates down.
pub async fn watch(socket: PathBuf, events: mpsc::Sender<Lifecycle>) {
    let mut pause = FIRST_RETRY;
    while !events.is_closed() {
        let opened = Instant::now();
        match api::open_events(&socket, &nonce()).await {
            Ok(stream) => {
                tracing::debug!("reading Incus lifecycle events");
                if !forward(Frames::new(stream), &events).await {
                    return;
                }
            }
            Err(error) => tracing::debug!(%error, "can't open the Incus event stream"),
        }
        if opened.elapsed() > LAST_RETRY {
            pause = FIRST_RETRY;
        }
        tokio::time::sleep(pause).await;
        pause = (pause * 2).min(LAST_RETRY);
    }
}

/// Passes events on until the stream ends. False once the receiver has
/// closed, so there's no one left to tell.
async fn forward<S>(mut frames: Frames<S>, events: &mpsc::Sender<Lifecycle>) -> bool
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        match frames.next().await {
            Ok(Some(message)) => {
                let Some(event) = parse(&message) else {
                    continue;
                };
                // A full queue already holds events that will start a pass,
                // so dropping this one loses at most a stop time.
                if let Err(mpsc::error::TrySendError::Closed(_)) = events.try_send(event) {
                    return false;
                }
            }
            Ok(None) => return true,
            Err(error) => {
                tracing::debug!(%error, "Incus event stream broke");
                return true;
            }
        }
    }
}

/// A `Sec-WebSocket-Key`, which only has to differ between connections.
fn nonce() -> String {
    base64(Uuid::now_v7().as_bytes())
}

/// Standard base64 with padding.
fn base64(bytes: &[u8]) -> String {
    /// The 64 digits, in order.
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | (u32::from(b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The frame types in RFC 6455.
mod opcode {
    /// The rest of a fragmented message.
    pub const CONTINUATION: u8 = 0x0;
    /// A UTF-8 message.
    pub const TEXT: u8 = 0x1;
    /// A binary message.
    pub const BINARY: u8 = 0x2;
    /// The peer is closing the connection.
    pub const CLOSE: u8 = 0x8;
    /// A heartbeat that wants a pong back.
    pub const PING: u8 = 0x9;
    /// The answer to a ping.
    pub const PONG: u8 = 0xA;
}

/// One WebSocket frame.
struct Frame {
    fin: bool,
    opcode: u8,
    payload: Vec<u8>,
}

/// Reads WebSocket messages on the client side of a connection, and
/// answers pings on the way, since Incus drops listeners that stop
/// answering its heartbeat.
pub struct Frames<S> {
    stream: S,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Frames<S> {
    /// Reads from `stream`, which has finished the opening handshake.
    pub fn new(stream: S) -> Self {
        Self { stream }
    }

    /// The next whole text or binary message, or None once the server
    /// closes the connection.
    pub async fn next(&mut self) -> io::Result<Option<Vec<u8>>> {
        let mut message = Vec::new();
        loop {
            let Some(frame) = self.frame().await? else {
                return Ok(None);
            };
            match frame.opcode {
                opcode::PING => self.control(opcode::PONG, &frame.payload).await?,
                opcode::PONG => {}
                opcode::CLOSE => return Ok(None),
                opcode::CONTINUATION | opcode::TEXT | opcode::BINARY => {
                    if message.len() + frame.payload.len() > MAX_MESSAGE {
                        return Err(broken("event message too long"));
                    }
                    message.extend_from_slice(&frame.payload);
                    if frame.fin {
                        return Ok(Some(message));
                    }
                }
                _ => return Err(broken("unknown WebSocket frame")),
            }
        }
    }

    /// Reads one frame. None when the stream ends between frames.
    async fn frame(&mut self) -> io::Result<Option<Frame>> {
        let mut head = [0; 2];
        match self.stream.read_exact(&mut head).await {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(error) => return Err(error),
        }
        let len = match head[1] & 0x7F {
            126 => u64::from(self.stream.read_u16().await?),
            127 => self.stream.read_u64().await?,
            len => u64::from(len),
        };
        let len = usize::try_from(len)
            .ok()
            .filter(|&len| len <= MAX_MESSAGE)
            .ok_or_else(|| broken("event frame too long"))?;
        let mask = if head[1] & 0x80 == 0 {
            None
        } else {
            let mut mask = [0; 4];
            self.stream.read_exact(&mut mask).await?;
            Some(mask)
        };
        let mut payload = vec![0; len];
        self.stream.read_exact(&mut payload).await?;
        if let Some(mask) = mask {
            unmask(&mut payload, mask);
        }
        Ok(Some(Frame {
            fin: head[0] & 0x80 != 0,
            opcode: head[0] & 0x0F,
            payload,
        }))
    }

    /// Sends a control frame. A client masks every frame it sends, and a
    /// control frame carries at most 125 bytes.
    async fn control(&mut self, opcode: u8, payload: &[u8]) -> io::Result<()> {
        let payload = &payload[..payload.len().min(125)];
        // The last bytes of a version 7 UUID are random.
        let mask: [u8; 4] = Uuid::now_v7().as_bytes()[12..]
            .try_into()
            .unwrap_or_default();
        let mut frame = Vec::with_capacity(6 + payload.len());
        frame.push(0x80 | opcode);
        frame.push(0x80 | payload.len() as u8);
        frame.extend_from_slice(&mask);
        let start = frame.len();
        frame.extend_from_slice(payload);
        unmask(&mut frame[start..], mask);
        self.stream.write_all(&frame).await?;
        self.stream.flush().await
    }
}

/// Applies a frame mask, which undoes itself.
fn unmask(payload: &mut [u8], mask: [u8; 4]) {
    for (i, byte) in payload.iter_mut().enumerate() {
        *byte ^= mask[i % 4];
    }
}

/// The error for a stream that breaks the WebSocket rules.
fn broken(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}

/// A server's unmasked frame, as Incus sends them. Tests use it to play
/// the server.
#[cfg(test)]
pub fn server_frame(fin: bool, opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![if fin { 0x80 | opcode } else { opcode }];
    match payload.len() {
        len @ 0..=125 => frame.push(len as u8),
        len @ 126..=0xFFFF => {
            frame.push(126);
            frame.extend_from_slice(&(len as u16).to_be_bytes());
        }
        len => {
            frame.push(127);
            frame.extend_from_slice(&(len as u64).to_be_bytes());
        }
    }
    frame.extend_from_slice(payload);
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events() -> Vec<&'static str> {
        include_str!("../../../tests/fixtures/incus/events.jsonl")
            .lines()
            .collect()
    }

    #[test]
    fn reads_lifecycle_events_for_workspaces() {
        let parsed: Vec<Option<Lifecycle>> = events().iter().map(|e| parse(e.as_bytes())).collect();
        assert_eq!(
            parsed,
            [
                // The observer's own file reads.
                None,
                Some(Lifecycle {
                    instance: "ws-harbor".into(),
                    stopped_at: Some(Timestamp(1_792_891_025_402)),
                }),
                Some(Lifecycle {
                    instance: "ws-tidepool".into(),
                    stopped_at: None,
                }),
                // Renamed away from a workspace name.
                Some(Lifecycle {
                    instance: "quill-archive".into(),
                    stopped_at: None,
                }),
                // Not a workspace.
                None,
            ]
        );
    }

    #[test]
    fn other_messages_are_skipped() {
        for message in [
            r#"{"type":"logging","timestamp":"","metadata":{"message":"x","level":"info"}}"#,
            r#"{"type":"lifecycle","metadata":null}"#,
            "not json",
        ] {
            assert_eq!(parse(message.as_bytes()), None, "{message}");
        }
    }

    #[test]
    fn base64_matches_rfc_6455() {
        // The sample nonce from RFC 6455 section 1.3.
        assert_eq!(base64(b"the sample nonce"), "dGhlIHNhbXBsZSBub25jZQ==");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
        assert_eq!(nonce().len(), 24);
    }

    #[tokio::test]
    async fn reads_messages_and_answers_pings() {
        let (client, mut server) = tokio::io::duplex(4096);
        let mut frames = Frames::new(client);
        let event = events()[1].as_bytes();
        let (first, rest) = event.split_at(100);

        let mut sent = server_frame(false, opcode::TEXT, first);
        sent.extend(server_frame(true, opcode::PING, b"beat"));
        sent.extend(server_frame(true, opcode::CONTINUATION, rest));
        sent.extend(server_frame(true, opcode::TEXT, b"{}"));
        sent.extend(server_frame(true, opcode::CLOSE, &[]));
        server.write_all(&sent).await.unwrap();

        assert_eq!(frames.next().await.unwrap().as_deref(), Some(event));
        let mut pong = [0; 10];
        server.read_exact(&mut pong).await.unwrap();
        assert_eq!(pong[..2], [0x8A, 0x84], "a masked pong with 4 bytes");
        let mut payload = pong[6..].to_vec();
        unmask(&mut payload, pong[2..6].try_into().unwrap());
        assert_eq!(payload, b"beat");

        assert_eq!(frames.next().await.unwrap().as_deref(), Some(&b"{}"[..]));
        assert_eq!(frames.next().await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_dropped_stream_ends_the_messages() {
        let (client, server) = tokio::io::duplex(64);
        drop(server);
        assert_eq!(Frames::new(client).next().await.unwrap(), None);
    }

    #[tokio::test]
    async fn forwards_events_until_the_stream_closes() {
        let (client, mut server) = tokio::io::duplex(8192);
        let mut sent = Vec::new();
        for event in events() {
            sent.extend(server_frame(true, opcode::TEXT, event.as_bytes()));
        }
        server.write_all(&sent).await.unwrap();
        drop(server);

        let (sender, mut receiver) = mpsc::channel(8);
        assert!(forward(Frames::new(client), &sender).await);
        let mut got = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            got.push(event.instance);
        }
        assert_eq!(got, ["ws-harbor", "ws-tidepool", "quill-archive"]);
    }
}
