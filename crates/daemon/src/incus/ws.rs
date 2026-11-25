// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Just enough of the WebSocket protocol, RFC 6455, for the client side of
//! Incus's event stream and of an exec's stdin, stdout and stderr.

use std::io;
use std::path::Path;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use http_body_util::Empty;
use hyper::body::Bytes;
use hyper::client::conn::http1;
use hyper::header;
use hyper::upgrade::Upgraded;
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use maxhq_os::ipc::LocalStream;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use uuid::Uuid;

use super::http::{broken, TIMEOUT};
use super::IncusError;

/// The longest message read. Events are a few hundred bytes and exec
/// output comes in small frames, so anything near this is a broken stream.
const MAX_MESSAGE: usize = 8 << 20;

/// The most a single frame sent carries.
const MAX_FRAME: usize = 64 << 10;

/// The frame types in RFC 6455.
pub mod opcode {
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

/// One whole message.
#[derive(Debug, PartialEq, Eq)]
pub struct Message {
    /// True for a text message. Incus sends an empty one to mark the end
    /// of an exec stream.
    pub text: bool,
    /// The bytes.
    pub payload: Vec<u8>,
}

/// The client side of a WebSocket that has finished its opening handshake.
pub struct WebSocket<S> {
    stream: S,
}

/// A WebSocket over the Incus socket.
pub type IncusSocket = WebSocket<TokioIo<Upgraded>>;

/// Opens a WebSocket at `uri` on the Incus socket.
pub async fn open(socket: &Path, uri: &str) -> Result<IncusSocket, IncusError> {
    let opening = async {
        let stream = LocalStream::connect(socket).await?;
        let (mut sender, connection) = http1::handshake(TokioIo::new(stream))
            .await
            .map_err(broken)?;
        tokio::spawn(async move {
            if let Err(error) = connection.with_upgrades().await {
                tracing::debug!(%error, "Incus WebSocket connection ended");
            }
        });
        let request = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header(header::HOST, "incus")
            .header(header::CONNECTION, "Upgrade")
            .header(header::UPGRADE, "websocket")
            .header(header::SEC_WEBSOCKET_VERSION, "13")
            .header(header::SEC_WEBSOCKET_KEY, nonce())
            .body(Empty::<Bytes>::new())
            .map_err(broken)?;
        let response = sender.send_request(request).await.map_err(broken)?;
        if response.status() != StatusCode::SWITCHING_PROTOCOLS {
            return Err(IncusError::Api {
                status: response.status().as_u16(),
                message: format!("Incus wouldn't open a WebSocket at {uri}"),
            });
        }
        // The accept header isn't checked. It guards against caching
        // proxies, and there's none on a local socket.
        let upgraded = hyper::upgrade::on(response).await.map_err(broken)?;
        Ok(WebSocket::new(TokioIo::new(upgraded)))
    };
    match tokio::time::timeout(TIMEOUT, opening).await {
        Ok(result) => result,
        Err(_) => Err(IncusError::Io(io::Error::new(
            io::ErrorKind::TimedOut,
            "Incus didn't open the WebSocket in time",
        ))),
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> WebSocket<S> {
    /// Wraps a stream that has finished the opening handshake.
    pub fn new(stream: S) -> Self {
        Self { stream }
    }

    /// The next whole text or binary message, or None once the server
    /// closes the connection. Pings are answered on the way.
    pub async fn next(&mut self) -> io::Result<Option<Message>> {
        let mut message = Vec::new();
        let mut text = false;
        loop {
            let Some((fin, code, payload)) = self.frame().await? else {
                return Ok(None);
            };
            match code {
                opcode::PING => self.send(opcode::PONG, &payload).await?,
                opcode::PONG => {}
                opcode::CLOSE => return Ok(None),
                opcode::CONTINUATION | opcode::TEXT | opcode::BINARY => {
                    if code == opcode::TEXT {
                        text = true;
                    }
                    if message.len() + payload.len() > MAX_MESSAGE {
                        return Err(invalid("WebSocket message too long"));
                    }
                    message.extend_from_slice(&payload);
                    if fin {
                        return Ok(Some(Message {
                            text,
                            payload: message,
                        }));
                    }
                }
                _ => return Err(invalid("unknown WebSocket frame")),
            }
        }
    }

    /// Sends `payload` as one message, split into frames of at most
    /// [`MAX_FRAME`] bytes.
    pub async fn send_message(&mut self, code: u8, payload: &[u8]) -> io::Result<()> {
        if payload.is_empty() {
            return self.send(code, payload).await;
        }
        let chunks: Vec<&[u8]> = payload.chunks(MAX_FRAME).collect();
        let last = chunks.len() - 1;
        for (i, chunk) in chunks.into_iter().enumerate() {
            let code = if i == 0 { code } else { opcode::CONTINUATION };
            self.write_frame(i == last, code, chunk).await?;
        }
        self.stream.flush().await
    }

    /// Sends one frame with the final bit set.
    pub async fn send(&mut self, code: u8, payload: &[u8]) -> io::Result<()> {
        self.write_frame(true, code, payload).await?;
        self.stream.flush().await
    }

    /// Sends a close frame with no reason.
    pub async fn close(&mut self) -> io::Result<()> {
        self.send(opcode::CLOSE, &[]).await
    }

    /// The stream underneath, for a test that plays the server.
    #[cfg(all(test, unix))]
    pub fn into_inner(self) -> S {
        self.stream
    }

    /// Writes one frame. A client masks every frame it sends.
    async fn write_frame(&mut self, fin: bool, code: u8, payload: &[u8]) -> io::Result<()> {
        let mut frame = Vec::with_capacity(14 + payload.len());
        frame.push(if fin { 0x80 | code } else { code });
        match payload.len() {
            len @ 0..=125 => frame.push(0x80 | len as u8),
            len @ 126..=0xFFFF => {
                frame.push(0x80 | 126);
                frame.extend_from_slice(&(len as u16).to_be_bytes());
            }
            len => {
                frame.push(0x80 | 127);
                frame.extend_from_slice(&(len as u64).to_be_bytes());
            }
        }
        let mask = mask_key();
        frame.extend_from_slice(&mask);
        let start = frame.len();
        frame.extend_from_slice(payload);
        apply_mask(&mut frame[start..], mask);
        self.stream.write_all(&frame).await
    }

    /// Reads one frame. None when the stream ends between frames.
    async fn frame(&mut self) -> io::Result<Option<(bool, u8, Vec<u8>)>> {
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
            .ok_or_else(|| invalid("WebSocket frame too long"))?;
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
            apply_mask(&mut payload, mask);
        }
        Ok(Some((head[0] & 0x80 != 0, head[0] & 0x0F, payload)))
    }
}

/// A `Sec-WebSocket-Key`, which only has to differ between connections.
fn nonce() -> String {
    STANDARD.encode(Uuid::now_v7().as_bytes())
}

/// A frame mask. The last bytes of a version 7 UUID are random.
fn mask_key() -> [u8; 4] {
    Uuid::now_v7().as_bytes()[12..]
        .try_into()
        .unwrap_or_default()
}

/// Applies a frame mask, which undoes itself.
pub fn apply_mask(payload: &mut [u8], mask: [u8; 4]) {
    for (i, byte) in payload.iter_mut().enumerate() {
        *byte ^= mask[i % 4];
    }
}

/// The error for a stream that breaks the WebSocket rules.
fn invalid(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}

/// A server's unmasked frame, as Incus sends them. Tests use it to play
/// the server.
#[cfg(test)]
pub fn server_frame(fin: bool, code: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![if fin { 0x80 | code } else { code }];
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

    #[tokio::test]
    async fn sends_masked_frames_and_reads_messages() {
        let (client, server) = tokio::io::duplex(1 << 20);
        let mut client = WebSocket::new(client);
        // The server reads masked frames with the same reader.
        let mut server = WebSocket::new(server);

        let big = vec![7u8; MAX_FRAME + 10];
        client.send_message(opcode::BINARY, &big).await.unwrap();
        client.send_message(opcode::TEXT, b"").await.unwrap();
        client.close().await.unwrap();

        let first = server.next().await.unwrap().unwrap();
        assert!(!first.text);
        assert_eq!(first.payload, big);
        let barrier = server.next().await.unwrap().unwrap();
        assert!(barrier.text);
        assert!(barrier.payload.is_empty());
        assert_eq!(server.next().await.unwrap(), None);
    }

    #[tokio::test]
    async fn answers_pings_while_reading() {
        let (client, mut server) = tokio::io::duplex(4096);
        let mut client = WebSocket::new(client);
        server
            .write_all(&server_frame(true, opcode::PING, b"hi"))
            .await
            .unwrap();
        server
            .write_all(&server_frame(false, opcode::TEXT, b"{\"a\""))
            .await
            .unwrap();
        server
            .write_all(&server_frame(true, opcode::CONTINUATION, b":1}"))
            .await
            .unwrap();
        let message = client.next().await.unwrap().unwrap();
        assert_eq!(message.payload, b"{\"a\":1}");

        let mut pong = WebSocket::new(server);
        let (fin, code, payload) = pong.frame().await.unwrap().unwrap();
        assert!(fin);
        assert_eq!(code, opcode::PONG);
        assert_eq!(payload, b"hi");
    }

    #[test]
    fn nonces_are_base64_of_sixteen_bytes() {
        assert_eq!(nonce().len(), 24);
    }
}
