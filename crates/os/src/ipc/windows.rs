// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Named pipes are stream A's to add here, at [`super::pipe_name`]. Until
//! then nothing can make a stream, so the types have no values.

use std::io;
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use super::{Found, Peer};

/// What `maxd` would serve on.
#[derive(Debug)]
pub enum Listener {}

/// One end of a pipe.
#[derive(Debug)]
pub enum Stream {}

pub type ReadHalf = tokio::io::ReadHalf<Stream>;
pub type WriteHalf = tokio::io::WriteHalf<Stream>;

const PIPES: &str = "Local sockets (named pipes)";

pub fn bind(_path: &Path) -> io::Result<Listener> {
    Err(crate::unsupported(PIPES))
}

pub async fn accept(listener: &Listener) -> io::Result<Stream> {
    match *listener {}
}

pub async fn connect(_path: &Path) -> io::Result<Stream> {
    Err(crate::unsupported(PIPES))
}

pub fn peer(stream: &Stream) -> io::Result<Peer> {
    match *stream {}
}

pub fn into_split(stream: Stream) -> (ReadHalf, WriteHalf) {
    tokio::io::split(stream)
}

pub fn pair() -> io::Result<(Stream, Stream)> {
    Err(crate::unsupported(PIPES))
}

pub fn probe(_path: &Path) -> io::Result<Found> {
    Err(crate::unsupported(PIPES))
}

impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match *self.get_mut() {}
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match *self.get_mut() {}
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match *self.get_mut() {}
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match *self.get_mut() {}
    }
}
