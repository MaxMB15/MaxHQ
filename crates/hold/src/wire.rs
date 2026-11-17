// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What holders and clients say on the socket: one JSON object per line.
//!
//! Only this crate speaks it, and a holder and its clients are always the
//! same build or close to it, so it has no version of its own. A field
//! added later is left out when it's empty, and a holder from before it
//! skips it, so either side can be the older one.

use std::io;

use maxhq_protocol::ids::SessionId;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::secret::Secret;
use crate::{ChildState, Exit, Line, WriteOutcome};

/// The longest line of a CLI's stdout the holder keeps, in bytes. A prompt can
/// carry 20 MiB of attachments as base64 beside a frame of text, about 43 MiB
/// in one line, and a CLI can print that back.
pub(crate) const MAX_LINE: usize = 64 * 1024 * 1024;

/// The longest message read from the socket, in bytes: a line of [`MAX_LINE`]
/// with every byte escaped as JSON's `\u00XX`, and room for what's around it.
const MAX_MESSAGE: usize = 6 * MAX_LINE + 1024 * 1024;

/// From a client to the holder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ToHolder {
    /// The first message: which session the client wants, the last line it
    /// has and the holder's key from `holder.key`. A holder from before
    /// keys has no such file, so its clients send no key.
    Hello {
        session: SessionId,
        after: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<Secret>,
    },
    /// A line for the CLI's stdin.
    Write {
        id: u64,
        line: String,
        answers: Option<String>,
    },
    /// Ends the CLI: closes its stdin, then signals its process group.
    Stop { id: u64, grace_ms: u64 },
    /// Deletes a finished session's files.
    Forget { id: u64 },
    /// The client has seen the exit, so the holder can go.
    SawExit,
}

/// From the holder to a client.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum FromHolder {
    /// The answer to the client's hello.
    Hello {
        holder_pid: u32,
        child: ChildState,
        first_seq: u64,
        last_seq: u64,
        pending: Vec<Line>,
    },
    /// The hello didn't show the holder's key or named another session.
    /// The holder closes the connection after it.
    Refused { message: String },
    /// A line of output.
    Line(Line),
    /// The CLI ended.
    Exited(Exit),
    /// What happened to a write.
    Written { id: u64, outcome: WriteOutcome },
    /// The CLI has ended after a stop.
    Stopped { id: u64 },
    /// The files are gone.
    Forgotten { id: u64 },
    /// A request failed.
    Failed { id: u64, error: Failure },
}

/// Why a request failed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Failure {
    /// The CLI has ended or its stdin is closed.
    Finished,
    /// The CLI is still running.
    StillRunning,
    /// The line has a newline in it.
    BadLine,
    /// Something else, as text.
    Io { message: String },
}

impl From<Failure> for crate::WriteError {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Finished => Self::Finished,
            Failure::StillRunning => Self::StillRunning,
            Failure::BadLine => Self::BadLine,
            Failure::Io { message } => Self::Io(message),
        }
    }
}

/// Writes `message` and its newline.
pub(crate) async fn send<W, T>(writer: &mut W, message: &T) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let mut bytes = serde_json::to_vec(message).map_err(io::Error::other)?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await
}

/// Reads the next message, or `None` when the other side has closed.
pub(crate) async fn recv<R, T>(reader: &mut R, buf: &mut Vec<u8>) -> io::Result<Option<T>>
where
    R: AsyncBufRead + Unpin,
    T: DeserializeOwned,
{
    recv_within(reader, buf, MAX_MESSAGE).await
}

/// Reads the next message of up to `cap` bytes, with its newline. A longer one
/// is an error, which ends the connection.
pub(crate) async fn recv_within<R, T>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    cap: usize,
) -> io::Result<Option<T>>
where
    R: AsyncBufRead + Unpin,
    T: DeserializeOwned,
{
    buf.clear();
    let read = reader.take(cap as u64 + 1).read_until(b'\n', buf).await?;
    if read == 0 {
        return Ok(None);
    }
    if read > cap {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a message ran past {cap} bytes"),
        ));
    }
    serde_json::from_slice(buf)
        .map(Some)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::time::Timestamp;

    use super::*;

    #[tokio::test]
    async fn a_message_past_the_cap_ends_the_connection_without_being_kept() {
        let mut buf = Vec::new();
        let mut short: &[u8] = b"[1,2,3,4]\n";
        let read: Option<Vec<u8>> = recv_within(&mut short, &mut buf, 10).await.unwrap();
        assert_eq!(read, Some(vec![1, 2, 3, 4]));
        let mut long: &[u8] = b"[1,2,3,4,5]\n";
        let err = recv_within::<_, Vec<u8>>(&mut long, &mut buf, 10)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(buf.len() <= 11, "{}", buf.len());
        let mut endless = tokio::io::BufReader::new(tokio::io::repeat(b'x'));
        let err = recv_within::<_, Vec<u8>>(&mut endless, &mut buf, 1024)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(buf.len() <= 1025, "{}", buf.len());
    }

    /// `ToHolder` as holders before keys have it, word for word.
    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum ToHolderBeforeKeys {
        Hello {
            session: SessionId,
            after: Option<u64>,
        },
        Write {
            id: u64,
            line: String,
            answers: Option<String>,
        },
        Stop {
            id: u64,
            grace_ms: u64,
        },
        Forget {
            id: u64,
        },
        SawExit,
    }

    #[test]
    fn a_hello_reads_on_holders_from_before_keys_and_after() {
        let session = SessionId::generate();
        let old = ToHolderBeforeKeys::Hello {
            session,
            after: Some(4),
        };
        let open = ToHolder::Hello {
            session,
            after: Some(4),
            key: None,
        };
        let keyed = ToHolder::Hello {
            session,
            after: Some(4),
            key: Some(Secret::new("ab".repeat(32))),
        };
        // Without a key, the hello is the one clients sent before keys.
        let old_json = serde_json::to_string(&old).unwrap();
        assert_eq!(serde_json::to_string(&open).unwrap(), old_json);
        assert_eq!(serde_json::from_str::<ToHolder>(&old_json).unwrap(), open);
        // An older holder skips the key it doesn't know.
        let keyed_json = serde_json::to_string(&keyed).unwrap();
        assert!(keyed_json.contains(r#""key":"abab"#), "{keyed_json}");
        let read: ToHolderBeforeKeys = serde_json::from_str(&keyed_json).unwrap();
        assert_eq!(read, old);
        assert_eq!(
            serde_json::from_str::<ToHolder>(&keyed_json).unwrap(),
            keyed
        );
        assert!(!format!("{keyed:?}").contains("abab"));
    }

    #[test]
    fn lines_and_exits_are_flat_objects() {
        let line = FromHolder::Line(Line {
            seq: 3,
            at: Timestamp(9),
            text: "{}".into(),
            replayed: true,
        });
        assert_eq!(
            serde_json::to_string(&line).unwrap(),
            r#"{"type":"line","seq":3,"at":9,"text":"{}","replayed":true}"#
        );
        let hello = FromHolder::Hello {
            holder_pid: 1,
            child: ChildState::Exited(Exit {
                code: Some(0),
                signal: None,
                at: Timestamp(2),
                stderr_tail: String::new(),
            }),
            first_seq: 1,
            last_seq: 0,
            pending: Vec::new(),
        };
        let json = serde_json::to_string(&hello).unwrap();
        assert!(
            json.contains(r#""child":{"state":"exited","code":0"#),
            "{json}"
        );
        assert_eq!(serde_json::from_str::<FromHolder>(&json).unwrap(), hello);
    }
}
