// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::io;
use std::path::Path;

use tokio::signal::unix::{signal, Signal, SignalKind};

use super::{Request, Stopped};

#[derive(Debug)]
pub struct Stops {
    terminate: Signal,
    interrupt: Signal,
}

pub fn listen(_socket: &Path) -> io::Result<Stops> {
    Ok(Stops {
        terminate: signal(SignalKind::terminate())?,
        interrupt: signal(SignalKind::interrupt())?,
    })
}

impl Stops {
    pub async fn next(&mut self) -> Stopped {
        tokio::select! {
            _ = self.terminate.recv() => Stopped::Asked,
            _ = self.interrupt.recv() => Stopped::Interrupted,
        }
    }
}

pub async fn ask(_socket: &Path, _request: Request) -> io::Result<u32> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "maxd takes stop requests from its service manager's signals here",
    ))
}
