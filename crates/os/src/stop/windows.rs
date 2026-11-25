// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The console's events and the stop endpoint. A closing console, a
//! logoff and a shutdown leave a few seconds at most, since Windows ends
//! the process once the handler returns.

use std::io;
use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::signal::windows::{
    ctrl_break, ctrl_c, ctrl_close, ctrl_logoff, ctrl_shutdown, CtrlBreak, CtrlC, CtrlClose,
    CtrlLogoff, CtrlShutdown,
};

use super::{Request, Stopped};
use crate::ipc::{LocalListener, LocalStream};

/// How long the endpoint pauses after a failed accept, so a broken one
/// doesn't spin.
const ACCEPT_PAUSE: Duration = Duration::from_millis(100);

#[derive(Debug)]
pub struct Stops {
    interrupt: CtrlC,
    brk: CtrlBreak,
    close: CtrlClose,
    logoff: CtrlLogoff,
    shutdown: CtrlShutdown,
    endpoint: LocalListener,
}

pub fn listen(socket: &Path) -> io::Result<Stops> {
    Ok(Stops {
        interrupt: ctrl_c()?,
        brk: ctrl_break()?,
        close: ctrl_close()?,
        logoff: ctrl_logoff()?,
        shutdown: ctrl_shutdown()?,
        endpoint: LocalListener::bind(&super::endpoint(socket))?,
    })
}

impl Stops {
    pub async fn next(&mut self) -> Stopped {
        loop {
            tokio::select! {
                _ = self.interrupt.recv() => return Stopped::Interrupted,
                _ = self.brk.recv() => return Stopped::Asked,
                _ = self.close.recv() => return Stopped::Asked,
                _ = self.logoff.recv() => return Stopped::Asked,
                _ = self.shutdown.recv() => return Stopped::Asked,
                accepted = self.endpoint.accept() => match accepted {
                    Ok(stream) => {
                        if let Some(stopped) = super::answer(stream).await {
                            return stopped;
                        }
                    }
                    Err(_) => tokio::time::sleep(ACCEPT_PAUSE).await,
                },
            }
        }
    }
}

pub async fn ask(socket: &Path, request: Request) -> io::Result<u32> {
    let path = super::endpoint(socket);
    // Connecting checks the endpoint's server runs as this user.
    let stream = LocalStream::connect(&path).await?;
    let pid = stream.peer()?.pid.ok_or_else(|| {
        io::Error::other("Windows didn't say which process serves maxd's stop endpoint")
    })?;
    let (read, mut write) = stream.into_split();
    write.write_all(request.line()).await?;
    write.flush().await?;
    let mut answer = String::new();
    let mut reader = BufReader::new(read);
    match tokio::time::timeout(super::REQUEST_WAIT, reader.read_line(&mut answer)).await {
        Ok(Ok(_)) if answer == super::TAKEN => Ok(pid),
        Ok(Ok(_)) => Err(io::Error::other(format!(
            "maxd (process {pid}) didn't take the request"
        ))),
        Ok(Err(err)) => Err(err),
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("maxd (process {pid}) didn't answer the request"),
        )),
    }
}
