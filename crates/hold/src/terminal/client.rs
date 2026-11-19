// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! `maxd`'s side of a terminal holder's socket.

use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use maxhq_protocol::ids::TerminalId;
use maxhq_protocol::time::Timestamp;
use tokio::io::BufReader;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::marks::Mark;
use super::wire::{self, FromHolder, ToHolder, Typed};
use super::{TermExit, TermHello};
use crate::files::{self, HolderFile, EXIT, HOLDER, LOCK};

/// How long to wait for a holder that's starting.
const CONNECT_WAIT: Duration = Duration::from_secs(10);

/// How long one attempt may take to connect or trade hellos.
const ATTEMPT_WAIT: Duration = Duration::from_secs(5);

/// The pause between attempts.
const RETRY: Duration = Duration::from_millis(50);

/// How many events wait for `maxd` before the holder waits for it. A
/// connection that stays behind long enough loses output, which shows as a
/// jump in the offsets.
const EVENTS_QUEUE: usize = 64;

/// What [`super::Terminals::connect`] found.
pub enum Reached {
    /// The holder runs, or has ended its program and lingers.
    Live(TermLink),
    /// The holder has gone. Its exit comes when it saw the program end.
    Ended(Option<TermExit>),
    /// The terminal has no folder: it never ran here, or someone said
    /// goodbye.
    Missing,
}

/// A connection to a terminal's holder. Dropping it, and every copy of its
/// control, detaches, and the terminal runs on.
pub struct TermLink {
    /// What the holder said when it took the connection.
    pub hello: TermHello,
    /// Output from `hello.from` on with no gap, what changes, and the exit
    /// last. It ends after the exit, or when the holder goes without one.
    pub events: mpsc::Receiver<TermEvent>,
    /// What to tell the holder.
    pub control: TermControl,
}

/// Something that happened in a terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TermEvent {
    /// Output starting at `offset`. An offset past the end of the last
    /// chunk means this connection fell behind and lost the bytes between.
    Output {
        /// Where it starts in everything the terminal printed.
        offset: u64,
        /// The bytes, as the program wrote them.
        data: Vec<u8>,
    },
    /// The program started or stopped waiting for a password.
    Password(bool),
    /// The shell started or stopped running something.
    Busy(bool),
    /// The shell set the window title.
    Title(String),
    /// The shell said its working folder.
    Cwd(String),
    /// The shell marked its prompt or a command, which starts at `offset`
    /// in the output. A quiet link gets these, and so does one that asked
    /// for them with [`super::Terminals::connect_marked`].
    Mark {
        /// Where the mark is in everything the terminal printed.
        offset: u64,
        /// When the holder read it.
        at: Timestamp,
        /// What it marks.
        mark: Mark,
    },
    /// The program ended.
    Exit(TermExit),
}

/// Tells a terminal's holder what to do. Each call returns false once the
/// connection has gone.
#[derive(Clone, Debug)]
pub struct TermControl(mpsc::UnboundedSender<ToHolder>);

impl TermControl {
    /// Types `data`. The bytes are zeroed once written, and nothing keeps
    /// them.
    pub fn input(&self, data: Vec<u8>) -> bool {
        self.send(ToHolder::Input { data: Typed(data) })
    }

    /// Sets the window size.
    pub fn resize(&self, cols: u16, rows: u16) -> bool {
        self.send(ToHolder::Resize { cols, rows })
    }

    /// Types Ctrl-C.
    pub fn interrupt(&self) -> bool {
        self.send(ToHolder::Interrupt)
    }

    /// Hangs up the terminal's process groups, then kills them after
    /// `grace`. The exit comes as an event.
    pub fn kill(&self, grace: Duration) -> bool {
        let grace_ms = u64::try_from(grace.as_millis()).unwrap_or(u64::MAX);
        self.send(ToHolder::Kill { grace_ms })
    }

    /// Tells the holder its exit is safe with `maxd`, so it deletes its
    /// folder and ends. Before the exit, it does nothing.
    pub fn goodbye(&self) -> bool {
        self.send(ToHolder::Goodbye)
    }

    /// Lets go of the connection, and the terminal runs on.
    pub fn detach(&self) -> bool {
        self.send(ToHolder::Detach)
    }

    fn send(&self, request: ToHolder) -> bool {
        self.0.send(request).is_ok()
    }
}

/// Reaches the holder for `terminal` in `dir`, waiting for one that's
/// starting. A quiet link gets everything but the output. A link with
/// `marks` gets the shell's marks too.
pub(super) async fn connect(
    dir: &Path,
    terminal: TerminalId,
    after: Option<u64>,
    quiet: bool,
    marks: bool,
) -> io::Result<Reached> {
    let deadline = Instant::now() + CONNECT_WAIT;
    loop {
        match fs::metadata(dir) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Ok(Reached::Missing),
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Reached::Missing),
            Err(err) => return Err(err),
        }
        let holder: Option<HolderFile> = files::read_json(&dir.join(HOLDER))?;
        let alive = files::holder_alive(dir)?;
        let last_try = Instant::now() >= deadline;
        match holder {
            // A holder that took its lock and went without a word didn't
            // start, and one with a lock still to take hasn't yet.
            _ if !alive && (holder.is_some() || dir.join(LOCK).exists() || last_try) => {
                return Ok(Reached::Ended(files::read_json(&dir.join(EXIT))?));
            }
            Some(holder) => {
                match attempt(dir, &holder.socket(), terminal, after, quiet, marks).await {
                    Ok(link) => return Ok(Reached::Live(link)),
                    Err(err) if last_try || err.kind() == io::ErrorKind::PermissionDenied => {
                        return Err(err);
                    }
                    Err(_) => {}
                }
            }
            // The holder hasn't written `holder.json` yet.
            None if last_try => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the terminal's holder didn't start in time",
                ));
            }
            None => {}
        }
        tokio::time::sleep(RETRY).await;
    }
}

/// Connects to the socket at `path` and trades hellos with the key from
/// `dir`.
async fn attempt(
    dir: &Path,
    path: &Path,
    terminal: TerminalId,
    after: Option<u64>,
    quiet: bool,
    marks: bool,
) -> io::Result<TermLink> {
    let key = files::read_key(dir)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the terminal's folder has no key",
        )
    })?;
    let timed_out = |what: &str| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!("the holder didn't {what} in time"),
        )
    };
    let stream = tokio::time::timeout(ATTEMPT_WAIT, UnixStream::connect(path))
        .await
        .map_err(|_| timed_out("take the connection"))??;
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    let hello = ToHolder::Hello {
        terminal,
        key,
        after,
        quiet,
        marks,
    };
    wire::send(&mut write, &hello).await?;
    drop(hello);
    let mut buf = Vec::new();
    let reply = tokio::time::timeout(ATTEMPT_WAIT, wire::recv(&mut read, &mut buf))
        .await
        .map_err(|_| timed_out("say hello"))??;
    let hello = match reply {
        Some(FromHolder::Hello(hello)) => hello,
        Some(FromHolder::Refused { message }) => {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, message));
        }
        Some(other) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected a hello, got {other:?}"),
            ));
        }
        None => {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the holder closed the connection",
            ));
        }
    };
    let (events_tx, events) = mpsc::channel(EVENTS_QUEUE);
    let (control, requests) = mpsc::unbounded_channel();
    tokio::spawn(read_holder(read, events_tx));
    tokio::spawn(send_requests(write, requests));
    Ok(TermLink {
        hello,
        events,
        control: TermControl(control),
    })
}

/// Passes on what the holder says until it closes or `maxd` stops
/// listening.
async fn read_holder(mut read: BufReader<OwnedReadHalf>, events: mpsc::Sender<TermEvent>) {
    let mut buf = Vec::new();
    while let Ok(Some(message)) = wire::recv::<_, FromHolder>(&mut read, &mut buf).await {
        let event = match message {
            FromHolder::Output { offset, data } => TermEvent::Output { offset, data },
            FromHolder::Password { on } => TermEvent::Password(on),
            FromHolder::Busy { on } => TermEvent::Busy(on),
            FromHolder::Title { title } => TermEvent::Title(title),
            FromHolder::Cwd { cwd } => TermEvent::Cwd(cwd),
            FromHolder::Mark { offset, at, mark } => TermEvent::Mark { offset, at, mark },
            FromHolder::Exit(exit) => TermEvent::Exit(exit),
            FromHolder::Hello(_) | FromHolder::Refused { .. } => continue,
        };
        if events.send(event).await.is_err() {
            return;
        }
    }
}

/// Sends requests in order until `maxd` drops every control or detaches.
async fn send_requests(mut write: OwnedWriteHalf, mut requests: mpsc::UnboundedReceiver<ToHolder>) {
    while let Some(request) = requests.recv().await {
        let detach = matches!(request, ToHolder::Detach);
        if wire::send(&mut write, &request).await.is_err() || detach {
            return;
        }
    }
    let _ = wire::send(&mut write, &ToHolder::Detach).await;
}
