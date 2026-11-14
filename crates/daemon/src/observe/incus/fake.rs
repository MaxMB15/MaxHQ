// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A stand-in for the Incus daemon in tests. It serves the instance list,
//! the file API and the event stream over a Unix socket the way Incus
//! does, and counts the file reads.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;

use super::events::server_frame;

/// One file inside a container.
#[derive(Clone)]
pub struct FakeFile {
    /// The `X-Incus-Modified` header.
    pub modified: String,
    /// The contents.
    pub body: Vec<u8>,
}

/// What the fake daemon holds.
#[derive(Default)]
pub struct State {
    /// The instance list reply.
    pub instances: Value,
    /// Files by instance and path.
    pub files: HashMap<(String, String), FakeFile>,
    /// File GETs so far, by instance and path.
    pub gets: HashMap<(String, String), usize>,
    /// File HEADs so far.
    pub heads: usize,
    /// Files whose reads fail, by instance and path.
    pub broken: HashSet<(String, String)>,
}

/// A fake Incus daemon. The socket goes away when this drops.
pub struct FakeIncus {
    /// Where it listens.
    pub socket: PathBuf,
    state: Arc<Mutex<State>>,
    events: broadcast::Sender<Vec<u8>>,
    server: tokio::task::JoinHandle<()>,
}

impl FakeIncus {
    /// Starts serving `state` on a new socket in the temp folder.
    pub fn start(state: State) -> Self {
        /// Numbers the sockets so tests running at once don't share one.
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let socket =
            std::env::temp_dir().join(format!("maxd-incus-{}-{n}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let state = Arc::new(Mutex::new(state));
        let (events, _) = broadcast::channel(16);
        let server = tokio::spawn(serve(listener, state.clone(), events.clone()));
        Self {
            socket,
            state,
            events,
            server,
        }
    }

    /// The state, to change or check.
    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sends an event message to every listener.
    pub fn send_event(&self, message: &str) {
        let _ = self.events.send(message.as_bytes().to_vec());
    }

    /// How many listeners are on the event stream.
    pub fn listeners(&self) -> usize {
        self.events.receiver_count()
    }
}

impl Drop for FakeIncus {
    fn drop(&mut self) {
        self.server.abort();
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// Accepts connections until the test ends.
async fn serve(
    listener: UnixListener,
    state: Arc<Mutex<State>>,
    events: broadcast::Sender<Vec<u8>>,
) {
    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(connection(stream, state.clone(), events.clone()));
    }
}

/// Answers requests on one connection, or streams events once it asks
/// for them.
async fn connection(
    stream: UnixStream,
    state: Arc<Mutex<State>>,
    events: broadcast::Sender<Vec<u8>>,
) {
    let mut stream = BufReader::new(stream);
    loop {
        let mut head = Vec::new();
        loop {
            let mut line = String::new();
            match stream.read_line(&mut line).await {
                Ok(0) | Err(_) => return,
                Ok(_) if line == "\r\n" => break,
                Ok(_) => head.push(line),
            }
        }
        let Some(request) = head.first() else {
            return;
        };
        let mut words = request.split_whitespace();
        let method = words.next().unwrap_or_default().to_owned();
        let target = words.next().unwrap_or_default().to_owned();

        if target.starts_with("/1.0/events") {
            let mut events = events.subscribe();
            let upgrade = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
                           Connection: Upgrade\r\nSec-WebSocket-Accept: x\r\n\r\n";
            if stream.write_all(upgrade.as_bytes()).await.is_err() {
                return;
            }
            while let Ok(message) = events.recv().await {
                let frame = server_frame(true, 0x1, &message);
                if stream.write_all(&frame).await.is_err() {
                    return;
                }
            }
            return;
        }

        let reply = respond(&state, &method, &target);
        if stream.write_all(&reply).await.is_err() {
            return;
        }
    }
}

/// The whole reply to one plain request.
fn respond(state: &Mutex<State>, method: &str, target: &str) -> Vec<u8> {
    let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
    let not_found = include_str!("../../../tests/fixtures/incus/not-found.json");
    let (status, headers, body) = if target == "/1.0/instances?recursion=1" {
        (
            "200 OK",
            String::new(),
            state.instances.to_string().into_bytes(),
        )
    } else if let Some((instance, path)) = file_target(target) {
        let key = (instance, path);
        match state.files.get(&key).cloned() {
            _ if state.broken.contains(&key) => {
                ("500 Internal Server Error", String::new(), Vec::new())
            }
            Some(file) => {
                if method == "GET" {
                    *state.gets.entry(key).or_default() += 1;
                } else {
                    state.heads += 1;
                }
                let headers = format!("X-Incus-Modified: {}\r\n", file.modified);
                ("200 OK", headers, file.body)
            }
            None => (
                "404 Not Found",
                String::new(),
                not_found.as_bytes().to_vec(),
            ),
        }
    } else {
        (
            "404 Not Found",
            String::new(),
            not_found.as_bytes().to_vec(),
        )
    };
    let mut reply = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{headers}\r\n",
        body.len()
    )
    .into_bytes();
    if method != "HEAD" {
        reply.extend(body);
    }
    reply
}

/// The instance and path a file API request names.
fn file_target(target: &str) -> Option<(String, String)> {
    let rest = target.strip_prefix("/1.0/instances/")?;
    let (instance, path) = rest.split_once("/files?path=")?;
    Some((instance.to_owned(), path.to_owned()))
}
