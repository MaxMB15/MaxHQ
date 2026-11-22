// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Framing, the same on every transport.
//!
//! ```text
//! u32 big-endian   length of the kind byte plus the body
//! u8               kind: 0 message, 1 terminal output, 2 terminal input,
//!                  3 transfer chunk, 4 tunnel data, 5 secret terminal
//!                  input
//! body             JSON for a message. For terminal output, the 16-byte
//!                  terminal ID, a u64 big-endian offset, then at most
//!                  64 KiB. For terminal input, the 16-byte terminal ID,
//!                  then at most 64 KiB. For a transfer chunk, the 16-byte
//!                  transfer ID, a u64 big-endian offset, then at most
//!                  256 KiB. For tunnel data, the 16-byte stream ID, then at
//!                  most 64 KiB. Secret terminal input is laid out as
//!                  terminal input.
//! ```
//!
//! Terminal, file and tunnel bytes never go through JSON, so they cost one
//! copy into the frame on the way out and none on the way in.
//!
//! A transfer's offset counts into its entries' bytes laid end to end in
//! manifest order, so one number says how far it got. A terminal output
//! frame's offset counts into everything that terminal wrote since it
//! started, so a client that comes back asks for what it missed. A kind 3
//! or 4 frame for an ID nobody opened gets dropped and counted, and a frame
//! over its limit closes the link.

use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

use crate::ids::{StreamId, TerminalId, TransferId};

/// The most a frame may declare. A peer that declares more is broken, and
/// the connection should close.
pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

const LEN_BYTES: usize = 4;
const KIND_BYTES: usize = 1;
const ID_BYTES: usize = 16;
const OFFSET_BYTES: usize = 8;

/// The most terminal bytes one frame carries, 64 KiB. Longer writes are
/// split, and a peer that sends more closes the link.
pub const MAX_TERMINAL_CHUNK: usize = 64 * 1024;

/// The most file bytes one transfer chunk frame carries, 256 KiB. Longer
/// writes are split, and a peer that sends more closes the link.
pub const MAX_TRANSFER_CHUNK: usize = 256 * 1024;

/// The most bytes one tunnel data frame carries, 64 KiB. Longer writes are
/// split, and a peer that sends more closes the link.
pub const MAX_TUNNEL_CHUNK: usize = 64 * 1024;

/// The kind byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
    /// A JSON [`crate::ClientMessage`] or [`crate::ServerMessage`].
    Message = 0,
    /// `[terminal: 16 bytes][offset: u64 BE][data, at most 64 KiB]`
    TerminalOutput = 1,
    /// `[terminal: 16 bytes][data, at most 64 KiB]`
    TerminalInput = 2,
    /// `[transfer: 16 bytes][offset: u64 BE][data, at most 256 KiB]`
    TransferChunk = 3,
    /// `[stream: 16 bytes][data, at most 64 KiB]`
    TunnelData = 4,
    /// `[terminal: 16 bytes][data, at most 64 KiB]`, input you sent from a
    /// secret field, such as a token at a prompt. The device records that a
    /// secret went, never the bytes. Since
    /// [`crate::message::SECRET_INPUT_SINCE`]: an older one drops the frame,
    /// so a client sends kind 2 there.
    TerminalSecret = 5,
}

/// A decoded frame, borrowed from the decoder's buffer.
#[derive(Debug, PartialEq, Eq)]
pub enum Frame<'a> {
    /// JSON for a message.
    Message(&'a [u8]),
    /// Bytes a terminal wrote.
    TerminalOutput {
        /// The terminal they came from.
        terminal: TerminalId,
        /// Where `bytes` start in everything the terminal wrote.
        offset: u64,
        /// At most [`MAX_TERMINAL_CHUNK`] bytes.
        bytes: &'a [u8],
    },
    /// Bytes for a terminal to read, such as keystrokes. Never logged or
    /// kept.
    TerminalInput {
        /// The terminal they go to.
        terminal: TerminalId,
        /// At most [`MAX_TERMINAL_CHUNK`] bytes.
        bytes: &'a [u8],
        /// Whether they came from a secret field, kind
        /// [`FrameKind::TerminalSecret`], so a record of them says only
        /// that a secret went.
        secret: bool,
    },
    /// Part of an upload or download.
    TransferChunk {
        /// The transfer they belong to.
        transfer: TransferId,
        /// Where `data` starts in the transfer's bytes.
        offset: u64,
        /// At most [`MAX_TRANSFER_CHUNK`] bytes.
        data: &'a [u8],
    },
    /// Bytes on one connection through a forward.
    TunnelData {
        /// The stream they belong to.
        stream: StreamId,
        /// At most [`MAX_TUNNEL_CHUNK`] bytes.
        data: &'a [u8],
    },
}

/// Why a message couldn't be framed.
#[derive(Debug, Error)]
pub enum EncodeError {
    /// The JSON is longer than [`MAX_FRAME_LEN`].
    #[error("message encodes to {0} bytes, more than the {MAX_FRAME_LEN} a frame allows")]
    TooLong(usize),
    /// The message didn't serialize.
    #[error("message doesn't serialize: {0}")]
    Json(#[from] serde_json::Error),
}

/// Why the incoming bytes aren't valid frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum DecodeError {
    /// The length is over [`MAX_FRAME_LEN`]. The stream can't be read past it.
    #[error("frame declares {0} bytes, more than the {MAX_FRAME_LEN} allowed")]
    TooLong(usize),
    /// The length is zero, so there's no kind byte. The stream can't be read
    /// past it.
    #[error("frame declares no bytes, so it has no kind")]
    Empty,
    /// The kind byte isn't one this build knows. The frame is consumed, so
    /// the next call reads the frame after it.
    #[error("unknown frame kind {0}")]
    UnknownKind(u8),
    /// A terminal, transfer or tunnel frame is too short to hold its ID,
    /// and its offset for terminal output or a transfer. The frame is
    /// consumed.
    #[error("kind {0} frame is shorter than its header")]
    ShortHeader(u8),
    /// A terminal, transfer or tunnel frame carries more than its kind
    /// allows. The frame is consumed, but the peer is broken and the
    /// connection should close.
    #[error("kind {kind} frame carries {len} bytes, more than the {limit} allowed")]
    ChunkTooLong {
        /// The kind byte, 1 to 5.
        kind: u8,
        /// The bytes after the header.
        len: usize,
        /// [`MAX_TERMINAL_CHUNK`], [`MAX_TRANSFER_CHUNK`] or
        /// [`MAX_TUNNEL_CHUNK`].
        limit: usize,
    },
}

/// Appends one message frame to `out`. On an error, `out` is left as it was.
pub fn encode_message(out: &mut Vec<u8>, message: &impl Serialize) -> Result<(), EncodeError> {
    let start = begin(out, FrameKind::Message);
    if let Err(err) = serde_json::to_writer(&mut *out, message) {
        out.truncate(start);
        return Err(err.into());
    }
    finish(out, start).map_err(|len| {
        out.truncate(start);
        EncodeError::TooLong(len)
    })
}

/// Appends terminal output frames to `out`, one per [`MAX_TERMINAL_CHUNK`]
/// bytes, for `bytes` that start at `offset` in what the terminal wrote.
/// Each frame's offset moves on by the bytes before it. Empty output
/// appends nothing.
pub fn encode_terminal_output(out: &mut Vec<u8>, terminal: TerminalId, offset: u64, bytes: &[u8]) {
    let mut offset = offset;
    for chunk in bytes.chunks(MAX_TERMINAL_CHUNK) {
        out.reserve(LEN_BYTES + KIND_BYTES + ID_BYTES + OFFSET_BYTES + chunk.len());
        let start = begin(out, FrameKind::TerminalOutput);
        out.extend_from_slice(terminal.0.as_bytes());
        out.extend_from_slice(&offset.to_be_bytes());
        out.extend_from_slice(chunk);
        // The chunk size keeps every frame within the limit.
        let _ = finish(out, start);
        offset += chunk.len() as u64;
    }
}

/// Appends terminal input frames to `out`, one per [`MAX_TERMINAL_CHUNK`]
/// bytes. Empty input appends nothing.
pub fn encode_terminal_input(out: &mut Vec<u8>, terminal: TerminalId, bytes: &[u8]) {
    encode_terminal_input_as(out, terminal, bytes, false);
}

/// Appends terminal input frames to `out` as [`encode_terminal_input`]
/// does, of kind [`FrameKind::TerminalSecret`] when `secret` says the
/// bytes came from a secret field. Send that kind only to a peer at
/// [`crate::message::SECRET_INPUT_SINCE`] or later.
pub fn encode_terminal_input_as(
    out: &mut Vec<u8>,
    terminal: TerminalId,
    bytes: &[u8],
    secret: bool,
) {
    let kind = if secret {
        FrameKind::TerminalSecret
    } else {
        FrameKind::TerminalInput
    };
    for chunk in bytes.chunks(MAX_TERMINAL_CHUNK) {
        out.reserve(LEN_BYTES + KIND_BYTES + ID_BYTES + chunk.len());
        let start = begin(out, kind);
        out.extend_from_slice(terminal.0.as_bytes());
        out.extend_from_slice(chunk);
        // The chunk size keeps every frame within the limit.
        let _ = finish(out, start);
    }
}

/// Appends transfer chunk frames to `out`, one per [`MAX_TRANSFER_CHUNK`]
/// bytes, for `bytes` that start at `offset` in the transfer. Each frame's
/// offset moves on by the bytes before it. Empty bytes append nothing.
pub fn encode_transfer_chunks(out: &mut Vec<u8>, transfer: TransferId, offset: u64, bytes: &[u8]) {
    let mut offset = offset;
    for chunk in bytes.chunks(MAX_TRANSFER_CHUNK) {
        out.reserve(LEN_BYTES + KIND_BYTES + ID_BYTES + OFFSET_BYTES + chunk.len());
        let start = begin(out, FrameKind::TransferChunk);
        out.extend_from_slice(transfer.0.as_bytes());
        out.extend_from_slice(&offset.to_be_bytes());
        out.extend_from_slice(chunk);
        // The chunk size keeps every frame within the limit.
        let _ = finish(out, start);
        offset += chunk.len() as u64;
    }
}

/// Appends tunnel data frames to `out`, one per [`MAX_TUNNEL_CHUNK`] bytes.
/// Empty bytes append nothing, since an end is [`crate::message::TunnelEnd`]
/// and not an empty frame.
pub fn encode_tunnel_data(out: &mut Vec<u8>, stream: StreamId, bytes: &[u8]) {
    for chunk in bytes.chunks(MAX_TUNNEL_CHUNK) {
        out.reserve(LEN_BYTES + KIND_BYTES + ID_BYTES + chunk.len());
        let start = begin(out, FrameKind::TunnelData);
        out.extend_from_slice(stream.0.as_bytes());
        out.extend_from_slice(chunk);
        // The chunk size keeps every frame within the limit.
        let _ = finish(out, start);
    }
}

/// Writes a placeholder length and the kind, and returns where the frame
/// starts.
fn begin(out: &mut Vec<u8>, kind: FrameKind) -> usize {
    let start = out.len();
    out.extend_from_slice(&[0; LEN_BYTES]);
    out.push(kind as u8);
    start
}

/// Fills in the length of the frame that starts at `start`, or returns the
/// length when it's over the limit.
fn finish(out: &mut [u8], start: usize) -> Result<(), usize> {
    let len = out.len() - start - LEN_BYTES;
    if len > MAX_FRAME_LEN {
        return Err(len);
    }
    // At most MAX_FRAME_LEN, so it fits in a u32.
    out[start..start + LEN_BYTES].copy_from_slice(&(len as u32).to_be_bytes());
    Ok(())
}

/// Splits a byte stream into frames, however the bytes arrive.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
    /// Where the first unread frame starts. Everything before it is spent.
    start: usize,
}

impl FrameDecoder {
    /// A decoder with nothing buffered.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds bytes read from the transport.
    pub fn push(&mut self, bytes: &[u8]) {
        self.compact();
        self.buf.extend_from_slice(bytes);
    }

    /// Bytes buffered that haven't been returned as frames yet.
    pub fn buffered(&self) -> usize {
        self.buf.len() - self.start
    }

    /// The next whole frame, or `None` until more bytes arrive. The frame
    /// borrows the buffer, so handle it before pushing more.
    pub fn next_frame(&mut self) -> Result<Option<Frame<'_>>, DecodeError> {
        let pending = &self.buf[self.start..];
        let Some(header) = pending.first_chunk::<LEN_BYTES>() else {
            return Ok(None);
        };
        let len = u32::from_be_bytes(*header) as usize;
        if len == 0 {
            return Err(DecodeError::Empty);
        }
        if len > MAX_FRAME_LEN {
            return Err(DecodeError::TooLong(len));
        }
        let Some(frame) = pending.get(LEN_BYTES..LEN_BYTES + len) else {
            return Ok(None);
        };
        self.start += LEN_BYTES + len;

        let (&kind, body) = frame.split_first().expect("len is at least 1");
        match kind {
            0 => Ok(Some(Frame::Message(body))),
            1 => Self::terminal_output(body).map(Some),
            2 => Self::terminal_input(body, false).map(Some),
            3 => Self::transfer(body).map(Some),
            4 => Self::tunnel(body).map(Some),
            5 => Self::terminal_input(body, true).map(Some),
            other => Err(DecodeError::UnknownKind(other)),
        }
    }

    /// Splits a transfer chunk's body into its transfer, offset and data.
    fn transfer(body: &[u8]) -> Result<Frame<'_>, DecodeError> {
        let kind = FrameKind::TransferChunk as u8;
        let (transfer, rest) = body
            .split_first_chunk::<ID_BYTES>()
            .ok_or(DecodeError::ShortHeader(kind))?;
        let (offset, data) = rest
            .split_first_chunk::<OFFSET_BYTES>()
            .ok_or(DecodeError::ShortHeader(kind))?;
        Ok(Frame::TransferChunk {
            transfer: TransferId(Uuid::from_bytes(*transfer)),
            offset: u64::from_be_bytes(*offset),
            data: within(kind, data, MAX_TRANSFER_CHUNK)?,
        })
    }

    /// Splits a tunnel frame's body into its stream and data.
    fn tunnel(body: &[u8]) -> Result<Frame<'_>, DecodeError> {
        let kind = FrameKind::TunnelData as u8;
        let (stream, data) = body
            .split_first_chunk::<ID_BYTES>()
            .ok_or(DecodeError::ShortHeader(kind))?;
        Ok(Frame::TunnelData {
            stream: StreamId(Uuid::from_bytes(*stream)),
            data: within(kind, data, MAX_TUNNEL_CHUNK)?,
        })
    }

    /// Splits a terminal output frame's body into its terminal, offset and
    /// bytes.
    fn terminal_output(body: &[u8]) -> Result<Frame<'_>, DecodeError> {
        let kind = FrameKind::TerminalOutput as u8;
        let (terminal, rest) = body
            .split_first_chunk::<ID_BYTES>()
            .ok_or(DecodeError::ShortHeader(kind))?;
        let (offset, bytes) = rest
            .split_first_chunk::<OFFSET_BYTES>()
            .ok_or(DecodeError::ShortHeader(kind))?;
        Ok(Frame::TerminalOutput {
            terminal: TerminalId(Uuid::from_bytes(*terminal)),
            offset: u64::from_be_bytes(*offset),
            bytes: within(kind, bytes, MAX_TERMINAL_CHUNK)?,
        })
    }

    /// Splits a terminal input frame's body into its terminal and bytes.
    fn terminal_input(body: &[u8], secret: bool) -> Result<Frame<'_>, DecodeError> {
        let kind = if secret {
            FrameKind::TerminalSecret
        } else {
            FrameKind::TerminalInput
        } as u8;
        let (terminal, bytes) = body
            .split_first_chunk::<ID_BYTES>()
            .ok_or(DecodeError::ShortHeader(kind))?;
        Ok(Frame::TerminalInput {
            terminal: TerminalId(Uuid::from_bytes(*terminal)),
            bytes: within(kind, bytes, MAX_TERMINAL_CHUNK)?,
            secret,
        })
    }

    /// Drops spent bytes once they're at least half the buffer, so each byte
    /// moves at most once on average.
    fn compact(&mut self) {
        if self.start == 0 {
            return;
        }
        if self.start == self.buf.len() {
            self.buf.clear();
        } else if self.start >= self.buf.len() / 2 {
            self.buf.drain(..self.start);
        } else {
            return;
        }
        self.start = 0;
    }
}

/// `data`, unless it's longer than `limit`, which breaks a kind `kind`
/// frame.
fn within(kind: u8, data: &[u8], limit: usize) -> Result<&[u8], DecodeError> {
    if data.len() > limit {
        return Err(DecodeError::ChunkTooLong {
            kind,
            len: data.len(),
            limit,
        });
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ClientMessage;

    fn terminal() -> TerminalId {
        "0199a7c2-0000-7000-8000-0000000000c1".parse().unwrap()
    }

    #[test]
    fn messages_round_trip() {
        let mut out = Vec::new();
        let ping = ClientMessage::Ping { nonce: 9 };
        encode_message(&mut out, &ping).unwrap();
        // 25 bytes of JSON plus the kind byte.
        assert_eq!(
            serde_json::to_vec(&ping).unwrap(),
            br#"{"type":"ping","nonce":9}"#
        );
        assert_eq!(&out[..5], &[0, 0, 0, 26, 0]);

        let mut decoder = FrameDecoder::new();
        decoder.push(&out);
        let Some(Frame::Message(json)) = decoder.next_frame().unwrap() else {
            panic!("expected a message frame");
        };
        assert_eq!(serde_json::from_slice::<ClientMessage>(json).unwrap(), ping);
        assert_eq!(decoder.next_frame(), Ok(None));
    }

    #[test]
    fn frames_survive_any_split() {
        let mut out = Vec::new();
        encode_terminal_output(&mut out, terminal(), 40, b"\x1b[32mok\x1b[0m\r\n");
        encode_terminal_input(&mut out, terminal(), b"y");
        encode_message(&mut out, &ClientMessage::Ping { nonce: 1 }).unwrap();

        let mut decoder = FrameDecoder::new();
        let mut kinds = Vec::new();
        for byte in &out {
            decoder.push(std::slice::from_ref(byte));
            while let Some(frame) = decoder.next_frame().unwrap() {
                kinds.push(match frame {
                    Frame::TerminalOutput {
                        terminal: t,
                        offset,
                        bytes,
                    } => {
                        assert_eq!(
                            (t, offset, bytes),
                            (terminal(), 40, &b"\x1b[32mok\x1b[0m\r\n"[..])
                        );
                        1
                    }
                    Frame::TerminalInput {
                        terminal: t,
                        bytes,
                        secret,
                    } => {
                        assert_eq!((t, bytes, secret), (terminal(), &b"y"[..], false));
                        2
                    }
                    Frame::Message(_) => 0,
                    other => panic!("unexpected {other:?}"),
                });
            }
        }
        assert_eq!(kinds, [1, 2, 0]);
        assert_eq!(decoder.buffered(), 0);
    }

    #[test]
    fn long_output_splits_at_the_limit_and_moves_its_offset_on() {
        let mut out = Vec::new();
        let bytes = vec![b'y'; MAX_TERMINAL_CHUNK * 2 + 10];
        encode_terminal_output(&mut out, terminal(), 500, &bytes);

        let mut decoder = FrameDecoder::new();
        decoder.push(&out);
        let mut seen = Vec::new();
        while let Some(Frame::TerminalOutput { offset, bytes, .. }) = decoder.next_frame().unwrap()
        {
            seen.push((offset, bytes.len()));
        }
        let full = MAX_TERMINAL_CHUNK as u64;
        assert_eq!(
            seen,
            [
                (500, MAX_TERMINAL_CHUNK),
                (500 + full, MAX_TERMINAL_CHUNK),
                (500 + 2 * full, 10),
            ]
        );
        assert_eq!(MAX_TERMINAL_CHUNK, 64 * 1024);
    }

    #[test]
    fn long_input_splits_at_the_limit() {
        let mut out = Vec::new();
        encode_terminal_input(&mut out, terminal(), &vec![b'q'; MAX_TERMINAL_CHUNK + 1]);

        let mut decoder = FrameDecoder::new();
        decoder.push(&out);
        let mut lengths = Vec::new();
        while let Some(Frame::TerminalInput { bytes, .. }) = decoder.next_frame().unwrap() {
            lengths.push(bytes.len());
        }
        assert_eq!(lengths, [MAX_TERMINAL_CHUNK, 1]);
    }

    #[test]
    fn terminal_frames_lay_out_their_header() {
        let mut out = Vec::new();
        encode_terminal_output(&mut out, terminal(), 0x0102, b"ok");
        let mut body = terminal().0.as_bytes().to_vec();
        body.extend_from_slice(&0x0102u64.to_be_bytes());
        body.extend_from_slice(b"ok");
        assert_eq!(out, raw(1, &body));

        let mut out = Vec::new();
        encode_terminal_input(&mut out, terminal(), b"\x03");
        let mut body = terminal().0.as_bytes().to_vec();
        body.push(3);
        assert_eq!(out, raw(2, &body));
    }

    #[test]
    fn empty_terminal_bytes_write_nothing() {
        let mut out = Vec::new();
        encode_terminal_output(&mut out, terminal(), 7, b"");
        encode_terminal_input(&mut out, terminal(), b"");
        assert!(out.is_empty());
    }

    #[test]
    fn oversized_lengths_are_rejected_before_buffering() {
        let mut decoder = FrameDecoder::new();
        decoder.push(&(MAX_FRAME_LEN as u32 + 1).to_be_bytes());
        assert_eq!(
            decoder.next_frame(),
            Err(DecodeError::TooLong(MAX_FRAME_LEN + 1))
        );

        let mut decoder = FrameDecoder::new();
        decoder.push(&[0, 0, 0, 0]);
        assert_eq!(decoder.next_frame(), Err(DecodeError::Empty));
    }

    #[test]
    fn unknown_kinds_can_be_skipped() {
        let mut decoder = FrameDecoder::new();
        decoder.push(&[0, 0, 0, 2, 9, 0]);
        let mut out = Vec::new();
        encode_terminal_input(&mut out, terminal(), b"q");
        decoder.push(&out);
        assert_eq!(decoder.next_frame(), Err(DecodeError::UnknownKind(9)));
        assert!(matches!(
            decoder.next_frame(),
            Ok(Some(Frame::TerminalInput { .. }))
        ));
    }

    #[test]
    fn short_terminal_frames_are_rejected() {
        let mut decoder = FrameDecoder::new();
        decoder.push(&[0, 0, 0, 3, 1, 0xab, 0xcd]);
        // A terminal ID with no offset after it.
        decoder.push(&raw(1, terminal().0.as_bytes()));
        decoder.push(&raw(2, &[1, 2, 3]));
        assert_eq!(decoder.next_frame(), Err(DecodeError::ShortHeader(1)));
        assert_eq!(decoder.next_frame(), Err(DecodeError::ShortHeader(1)));
        assert_eq!(decoder.next_frame(), Err(DecodeError::ShortHeader(2)));
        assert_eq!(decoder.next_frame(), Ok(None));

        // Input needs no offset, so an ID alone is empty input.
        decoder.push(&raw(2, terminal().0.as_bytes()));
        assert_eq!(
            decoder.next_frame(),
            Ok(Some(Frame::TerminalInput {
                terminal: terminal(),
                bytes: &[],
                secret: false,
            }))
        );
    }

    #[test]
    fn secret_input_reads_as_input_that_says_so() {
        let mut out = Vec::new();
        encode_terminal_input_as(&mut out, terminal(), b"hunter2\r", true);
        encode_terminal_input_as(&mut out, terminal(), b"ls\r", false);
        assert_eq!(out[4], FrameKind::TerminalSecret as u8);
        let mut decoder = FrameDecoder::new();
        decoder.push(&out);
        assert_eq!(
            decoder.next_frame(),
            Ok(Some(Frame::TerminalInput {
                terminal: terminal(),
                bytes: b"hunter2\r",
                secret: true,
            }))
        );
        assert_eq!(
            decoder.next_frame(),
            Ok(Some(Frame::TerminalInput {
                terminal: terminal(),
                bytes: b"ls\r",
                secret: false,
            }))
        );
        // Laid out as input, with the same limit.
        decoder.push(&raw(5, &[1, 2]));
        assert_eq!(decoder.next_frame(), Err(DecodeError::ShortHeader(5)));
        let mut long = terminal().0.as_bytes().to_vec();
        long.resize(ID_BYTES + MAX_TERMINAL_CHUNK + 1, b'x');
        decoder.push(&raw(5, &long));
        assert!(matches!(
            decoder.next_frame(),
            Err(DecodeError::ChunkTooLong { kind: 5, .. })
        ));
    }

    fn transfer() -> TransferId {
        "0199a7c2-0000-7000-8000-0000000000d1".parse().unwrap()
    }

    fn stream() -> StreamId {
        "0199a7c2-0000-7000-8000-0000000000d2".parse().unwrap()
    }

    /// One frame of `kind` with `body`, as raw bytes.
    fn raw(kind: u8, body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + 1) as u32).to_be_bytes().to_vec();
        out.push(kind);
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn transfer_chunks_round_trip_at_their_limit() {
        let mut out = Vec::new();
        let bytes = vec![7u8; MAX_TRANSFER_CHUNK * 2 + 3];
        encode_transfer_chunks(&mut out, transfer(), 1000, &bytes);

        let mut decoder = FrameDecoder::new();
        decoder.push(&out);
        let mut seen = Vec::new();
        while let Some(frame) = decoder.next_frame().unwrap() {
            let Frame::TransferChunk {
                transfer: t,
                offset,
                data,
            } = frame
            else {
                panic!("expected a transfer chunk, got {frame:?}");
            };
            assert_eq!(t, transfer());
            seen.push((offset, data.len()));
        }
        let full = MAX_TRANSFER_CHUNK as u64;
        assert_eq!(
            seen,
            [
                (1000, MAX_TRANSFER_CHUNK),
                (1000 + full, MAX_TRANSFER_CHUNK),
                (1000 + 2 * full, 3),
            ]
        );
        assert_eq!(&out[4], &3);
    }

    #[test]
    fn tunnel_data_round_trips_at_its_limit() {
        let mut out = Vec::new();
        let bytes = vec![9u8; MAX_TUNNEL_CHUNK + 1];
        encode_tunnel_data(&mut out, stream(), &bytes);

        let mut decoder = FrameDecoder::new();
        decoder.push(&out);
        let mut lengths = Vec::new();
        while let Some(frame) = decoder.next_frame().unwrap() {
            let Frame::TunnelData { stream: s, data } = frame else {
                panic!("expected tunnel data, got {frame:?}");
            };
            assert_eq!(s, stream());
            lengths.push(data.len());
        }
        assert_eq!(lengths, [MAX_TUNNEL_CHUNK, 1]);

        let mut empty = Vec::new();
        encode_tunnel_data(&mut empty, stream(), b"");
        encode_transfer_chunks(&mut empty, transfer(), 0, b"");
        assert!(empty.is_empty());
    }

    #[test]
    fn chunks_over_their_limit_are_rejected() {
        let mut body = transfer().0.as_bytes().to_vec();
        body.extend_from_slice(&0u64.to_be_bytes());
        body.extend(std::iter::repeat_n(0u8, MAX_TRANSFER_CHUNK + 1));
        let mut decoder = FrameDecoder::new();
        decoder.push(&raw(3, &body));
        assert_eq!(
            decoder.next_frame(),
            Err(DecodeError::ChunkTooLong {
                kind: 3,
                len: MAX_TRANSFER_CHUNK + 1,
                limit: MAX_TRANSFER_CHUNK,
            })
        );

        let mut body = stream().0.as_bytes().to_vec();
        body.extend(std::iter::repeat_n(0u8, MAX_TUNNEL_CHUNK + 1));
        let mut decoder = FrameDecoder::new();
        decoder.push(&raw(4, &body));
        assert_eq!(
            decoder.next_frame(),
            Err(DecodeError::ChunkTooLong {
                kind: 4,
                len: MAX_TUNNEL_CHUNK + 1,
                limit: MAX_TUNNEL_CHUNK,
            })
        );

        let mut body = terminal().0.as_bytes().to_vec();
        body.extend_from_slice(&0u64.to_be_bytes());
        body.extend(std::iter::repeat_n(0u8, MAX_TERMINAL_CHUNK + 1));
        let mut decoder = FrameDecoder::new();
        decoder.push(&raw(1, &body));
        assert_eq!(
            decoder.next_frame(),
            Err(DecodeError::ChunkTooLong {
                kind: 1,
                len: MAX_TERMINAL_CHUNK + 1,
                limit: MAX_TERMINAL_CHUNK,
            })
        );

        let mut body = terminal().0.as_bytes().to_vec();
        body.extend(std::iter::repeat_n(0u8, MAX_TERMINAL_CHUNK + 1));
        let mut decoder = FrameDecoder::new();
        decoder.push(&raw(2, &body));
        assert_eq!(
            decoder.next_frame(),
            Err(DecodeError::ChunkTooLong {
                kind: 2,
                len: MAX_TERMINAL_CHUNK + 1,
                limit: MAX_TERMINAL_CHUNK,
            })
        );
    }

    #[test]
    fn short_chunk_headers_are_rejected() {
        let mut decoder = FrameDecoder::new();
        // A transfer ID with no offset after it.
        decoder.push(&raw(3, transfer().0.as_bytes()));
        decoder.push(&raw(4, &[1, 2, 3]));
        assert_eq!(decoder.next_frame(), Err(DecodeError::ShortHeader(3)));
        assert_eq!(decoder.next_frame(), Err(DecodeError::ShortHeader(4)));
        assert_eq!(decoder.next_frame(), Ok(None));
    }

    #[test]
    fn spent_bytes_are_dropped() {
        let mut one = Vec::new();
        encode_terminal_output(&mut one, terminal(), 0, &[b'x'; 1000]);
        let mut decoder = FrameDecoder::new();
        for _ in 0..1000 {
            decoder.push(&one);
            assert!(decoder.next_frame().unwrap().is_some());
        }
        assert!(decoder.buf.capacity() < 4 * one.len());
    }
}
