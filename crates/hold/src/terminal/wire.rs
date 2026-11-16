// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What a terminal holder and `maxd` say on its socket: one JSON object per
//! line, with bytes as base64. It's apart from the agent holders' wire,
//! whose messages are lines of text.

use std::fmt;
use std::io;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use maxhq_protocol::ids::TerminalId;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tokio::io::{AsyncBufRead, AsyncWrite, AsyncWriteExt};

use super::{RunOutput, TermExit, TermHello};
use crate::secret::{zero, Secret};

/// The longest message read from the socket, in bytes. Output and input
/// come in chunks of 64 KiB at most, which base64 makes 88 KiB.
pub(crate) const MAX_MESSAGE: usize = 1024 * 1024;

/// The largest chunk of output or input in one message.
pub(crate) const CHUNK: usize = 64 * 1024;

/// From `maxd` to the holder.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ToHolder {
    /// The first message: the terminal, the holder's key, and the offset
    /// just past the last output byte `maxd` has.
    Hello {
        terminal: TerminalId,
        key: Secret,
        after: Option<u64>,
    },
    /// Bytes for the terminal to read.
    Input { data: Typed },
    /// A new window size.
    Resize { cols: u16, rows: u16 },
    /// Ctrl-C.
    Interrupt,
    /// Hangs up the terminal's process groups, then kills them after the
    /// grace.
    Kill { grace_ms: u64 },
    /// `maxd` is letting go of the connection, and the terminal runs on.
    Detach,
    /// `maxd` has read the exit, so the holder deletes its folder and ends.
    Goodbye,
}

/// From the holder to `maxd`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum FromHolder {
    /// The answer to the hello. Output from `from` on follows it.
    Hello(TermHello),
    /// The hello didn't show the key or named another terminal. The holder
    /// closes the connection after it.
    Refused { message: String },
    /// Output, starting at `offset` in everything the terminal printed.
    Output {
        offset: u64,
        #[serde(with = "base64_bytes")]
        data: Vec<u8>,
    },
    /// The terminal started or stopped waiting for a password.
    Password { on: bool },
    /// The terminal started or stopped running something other than its
    /// shell at a prompt.
    Busy { on: bool },
    /// The shell set the window title.
    Title { title: String },
    /// The shell set its working folder.
    Cwd { cwd: String },
    /// The terminal's process ended. Nothing but output before it comes
    /// after.
    Exit(TermExit),
}

/// Bytes someone typed. They print as nothing, not even their length, and
/// are zeroed when dropped.
pub(crate) struct Typed(pub Vec<u8>);

impl fmt::Debug for Typed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Typed")
    }
}

impl Drop for Typed {
    fn drop(&mut self) {
        zero(std::mem::take(&mut self.0));
    }
}

impl Serialize for Typed {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut text = STANDARD.encode(&self.0);
        let done = serializer.serialize_str(&text);
        zero(std::mem::take(&mut text).into_bytes());
        done
    }
}

impl<'de> Deserialize<'de> for Typed {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = crate::secret::Zeroed(String::deserialize(deserializer)?.into_bytes());
        STANDARD
            .decode(&text.0)
            .map(Typed)
            .map_err(serde::de::Error::custom)
    }
}

/// Writes `message` and its newline, then zeroes the bytes, which may
/// carry input.
pub(crate) async fn send<W, T>(writer: &mut W, message: &T) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let mut bytes = serde_json::to_vec(message).map_err(io::Error::other)?;
    bytes.push(b'\n');
    let sent = writer.write_all(&bytes).await;
    zero(bytes);
    sent
}

/// Reads the next message, or `None` when the other side has closed, then
/// zeroes the line it came on, which may carry input.
pub(crate) async fn recv<R, T>(reader: &mut R, buf: &mut Vec<u8>) -> io::Result<Option<T>>
where
    R: AsyncBufRead + Unpin,
    T: DeserializeOwned,
{
    let read = crate::wire::recv_within(reader, buf, MAX_MESSAGE).await;
    buf.fill(0);
    read
}

/// Bytes as a base64 string.
mod base64_bytes {
    use super::*;

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        STANDARD.decode(text).map_err(serde::de::Error::custom)
    }
}

impl RunOutput {
    /// The output from the emulator's transcript.
    pub(crate) fn from_transcript(transcript: maxhq_term::Transcript) -> Self {
        Self {
            text: transcript.text,
            lines: transcript.lines,
            truncated: transcript.truncated,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn bytes_go_as_base64_and_typed_ones_never_print() {
        let output = FromHolder::Output {
            offset: 7,
            data: b"\x1b[1mhi\r\n".to_vec(),
        };
        let json = serde_json::to_value(&output).unwrap();
        assert_eq!(
            json,
            json!({ "type": "output", "offset": 7, "data": "G1sxbWhpDQo=" })
        );
        assert_eq!(serde_json::from_value::<FromHolder>(json).unwrap(), output);

        let input = ToHolder::Input {
            data: Typed(b"hunter2\n".to_vec()),
        };
        let line = serde_json::to_string(&input).unwrap();
        assert_eq!(line, r#"{"type":"input","data":"aHVudGVyMgo="}"#);
        let ToHolder::Input { data } = serde_json::from_str(&line).unwrap() else {
            panic!("not input: {line}");
        };
        assert_eq!(data.0, b"hunter2\n");
        assert_eq!(format!("{input:?}"), "Input { data: Typed }");
    }
}
