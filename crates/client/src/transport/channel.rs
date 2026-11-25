// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What russh logins share: a session channel as the streams a [`Conn`]
//! takes, and what a russh failure means for the app. The phone's SSH
//! uses them, and so do the russh logins on Windows.

use russh::client::Msg;
use russh::{Channel, ChannelMsg, ChannelWriteHalf};
use tokio::io::AsyncWriteExt;
use tokio::task::JoinHandle;

use super::reach::{self, Reasons};
use super::{DialError, Reader, StderrTail, Writer};

/// How many bytes from `maxd` wait for the link to read them.
const PIPE: usize = 256 * 1024;

/// How many bytes of stderr wait for the tail to read them.
const STDERR_PIPE: usize = 8 * 1024;

/// What SSH says when it can't reach a port.
pub(crate) const REASONS: Reasons = Reasons {
    refused: "SSH connection refused",
    timed_out: "SSH timed out",
    closed: "the SSH connection closed",
};

/// A channel as streams: its data for the link and its stderr for the
/// tail, with the task that moves them and the half that closes it.
pub(crate) struct Piped {
    pub reader: Reader,
    pub writer: Writer,
    pub stderr: StderrTail,
    /// Moves the channel's messages. Aborting it stops reading.
    pub pump: JoinHandle<()>,
    /// Closes the channel, for a login that outlives it.
    pub closer: ChannelWriteHalf<Msg>,
}

/// Pipes a channel that runs a command.
pub(crate) fn pipe(channel: Channel<Msg>) -> Piped {
    let (mut read_half, closer) = channel.split();
    let writer = closer.make_writer();
    let (mut out, reader) = tokio::io::duplex(PIPE);
    let (mut err, stderr) = tokio::io::duplex(STDERR_PIPE);
    let pump = tokio::spawn(async move {
        while let Some(message) = read_half.wait().await {
            match message {
                ChannelMsg::Data { data } if out.write_all(&data).await.is_err() => break,
                ChannelMsg::ExtendedData { data, ext: 1 } => {
                    let _ = err.write_all(&data).await;
                }
                ChannelMsg::Eof | ChannelMsg::Close => break,
                _ => {}
            }
        }
    });
    Piped {
        reader: Box::new(reader),
        writer: Box::new(Box::pin(writer)),
        stderr: StderrTail::read(stderr),
        pump,
        closer,
    }
}

/// What a russh failure means for the app.
pub(crate) fn ssh_failure(err: &russh::Error) -> DialError {
    match err {
        russh::Error::UnknownKey => DialError::refused("the device showed a different host key"),
        russh::Error::IO(err) => reach::io_failure(err, &REASONS),
        _ => DialError::down("the SSH connection closed"),
    }
}
