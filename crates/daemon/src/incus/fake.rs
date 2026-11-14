// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A stand-in for the Incus daemon in tests. It serves the parts of the
//! REST API the writable client uses over a Unix socket, replays the
//! replies recorded from a real Incus 7 daemon for the server, its
//! networks and its pools, and keeps instances, files and exec calls in
//! memory so a test can check what the client did.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, watch};

use super::ws::{opcode, server_frame, WebSocket};

/// The recorded reply to `GET /1.0`.
pub const SERVER: &str = include_str!("../../tests/fixtures/workspace/server.json");
/// The recorded reply to `GET /1.0/networks?recursion=1`.
pub const NETWORKS: &str = include_str!("../../tests/fixtures/workspace/networks.json");
/// The recorded reply to `GET /1.0/storage-pools?recursion=1`.
pub const POOLS: &str = include_str!("../../tests/fixtures/workspace/storage-pools.json");
/// A pool's usage, in the API's documented shape.
const RESOURCES: &str = include_str!("../../tests/fixtures/workspace/pool-resources.json");
/// What Incus says when it can't read a pool whose disk is gone.
const UNAVAILABLE: &str = include_str!("../../tests/fixtures/workspace/pool-unavailable.json");

/// One exec call, as the fake saw it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExecCall {
    /// The instance.
    pub instance: String,
    /// The command and its arguments.
    pub command: Vec<String>,
    /// The uid it ran as.
    pub user: u32,
    /// The environment it was given.
    pub environment: BTreeMap<String, String>,
    /// The folder it started in.
    pub cwd: String,
    /// What it read on stdin, for a streamed call.
    pub stdin: Vec<u8>,
    /// True when it came over WebSockets.
    pub streamed: bool,
}

/// What a faked command does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExecReply {
    /// Its exit status.
    pub status: i32,
    /// What it prints to stdout.
    pub stdout: Vec<u8>,
    /// What it prints to stderr.
    pub stderr: Vec<u8>,
}

impl ExecReply {
    /// A command that exits 0 and prints `stdout`.
    pub fn ok(stdout: &str) -> Self {
        Self {
            status: 0,
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    /// A command that exits with `status` and prints `stderr`.
    pub fn fail(status: i32, stderr: &str) -> Self {
        Self {
            status,
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }
}

/// Answers every exec call.
pub type ExecHandler = Arc<dyn Fn(&ExecCall) -> ExecReply + Send + Sync>;

/// A request the fake answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Logged {
    /// The method.
    pub method: String,
    /// The path and query.
    pub target: String,
    /// The body, as JSON when it was JSON.
    pub body: Value,
}

/// A file pushed into an instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pushed {
    /// The `X-Incus-type` header: `file` or `directory`.
    pub kind: String,
    /// The `X-Incus-mode` header.
    pub mode: String,
    /// The `X-Incus-uid` and `X-Incus-gid` headers.
    pub owner: (String, String),
    /// The bytes.
    pub bytes: Vec<u8>,
}

/// A snapshot the fake holds: what its instance had then.
#[derive(Clone, Debug, Default)]
pub struct Taken {
    /// The instance's files, by path.
    pub files: BTreeMap<String, Pushed>,
    /// The instance's own keys.
    pub config: Value,
    /// The instance's own devices.
    pub devices: Value,
    /// When Incus deletes it, in RFC 3339, the year 1 for never.
    pub expires_at: String,
    /// When it was taken, in RFC 3339.
    pub created_at: String,
}

/// What the fake daemon holds.
#[derive(Default)]
pub struct State {
    /// Instances by name, in the shape `GET /1.0/instances/<name>` sends.
    pub instances: BTreeMap<String, Value>,
    /// Pools whose usage Incus can't read, as when their disk is locked.
    pub locked_pools: BTreeSet<String>,
    /// Answers exec calls. Without one, every command exits 0 and prints
    /// nothing.
    pub exec: Option<ExecHandler>,
    /// Every exec call so far.
    pub execs: Vec<ExecCall>,
    /// Files pushed, by instance and path.
    pub files: BTreeMap<(String, String), Pushed>,
    /// Snapshots by instance and name.
    pub snapshots: BTreeMap<(String, String), Taken>,
    /// Every request so far.
    pub log: Vec<Logged>,
    /// Exec logs not yet deleted, by path.
    pub exec_logs: BTreeMap<String, Vec<u8>>,
    /// When set, `GET /1.0` and every other call fails with 500, as an
    /// Incus that's starting up.
    pub down: bool,
    /// When set, every pool reports this as its source folder, so a test
    /// can point the check at a folder that exists.
    pub pool_source: Option<String>,
    /// How long a start or a stop takes before Incus answers, as a
    /// container that takes its time to shut down.
    pub state_delay: Duration,
    /// The disk each instance's root uses, in bytes, as its state reports.
    pub disk_used: BTreeMap<String, u64>,
    /// Finished operations, by ID.
    operations: BTreeMap<String, Value>,
    /// Streamed execs waiting for their WebSockets, by operation ID.
    streams: BTreeMap<String, Arc<Stream>>,
    /// Numbers operations.
    next_operation: usize,
}

impl State {
    /// Adds an instance in the documented shape.
    pub fn add(&mut self, name: &str, status: &str, config: &[(&str, &str)]) {
        self.instances
            .insert(name.to_owned(), instance(name, status, config));
    }

    /// The requests whose method isn't GET.
    pub fn writes(&self) -> Vec<&Logged> {
        self.log
            .iter()
            .filter(|l| l.method != "GET" && l.method != "HEAD")
            .collect()
    }

    /// The commands run so far, each joined with spaces.
    pub fn commands(&self) -> Vec<String> {
        self.execs.iter().map(|e| e.command.join(" ")).collect()
    }

    /// A new operation ID.
    fn operation_id(&mut self) -> String {
        self.next_operation += 1;
        format!("op-{}", self.next_operation)
    }

    /// Records a finished operation and its async reply.
    fn finish(&mut self, metadata: Value) -> (u16, Value) {
        let id = self.operation_id();
        self.operations.insert(
            id.clone(),
            json!({ "id": id, "status": "Success", "status_code": 200, "err": "", "metadata": metadata }),
        );
        async_reply(&id, json!({}))
    }

    /// Records a failed operation and its async reply.
    fn fail(&mut self, error: &str) -> (u16, Value) {
        let id = self.operation_id();
        self.operations.insert(
            id.clone(),
            json!({ "id": id, "status": "Failure", "status_code": 400, "err": error, "metadata": null }),
        );
        async_reply(&id, json!({}))
    }
}

/// An instance in the shape the API documents.
pub fn instance(name: &str, status: &str, config: &[(&str, &str)]) -> Value {
    let config: BTreeMap<&str, &str> = config.iter().copied().collect();
    json!({
        "name": name,
        "type": "container",
        "status": status,
        "status_code": if status == "Running" { 103 } else { 102 },
        "architecture": "x86_64",
        "config": config,
        "expanded_config": config,
        "devices": {},
        "profiles": [],
        "ephemeral": false,
        "stateful": false,
        "description": "",
        "created_at": "2026-10-24T09:00:00.000000000Z",
        "last_used_at": "1970-01-01T00:00:00Z",
        "location": "none",
        "project": "default",
    })
}

/// A streamed exec, from its POST until its streams close.
struct Stream {
    call: Mutex<ExecCall>,
    secrets: BTreeMap<String, String>,
    reply: watch::Sender<Option<ExecReply>>,
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
            std::env::temp_dir().join(format!("maxd-xc-incus-{}-{n}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let state = Arc::new(Mutex::new(state));
        let (events, _) = broadcast::channel(64);
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
        lock(&self.state)
    }

    /// Sends a lifecycle event to every listener.
    pub fn send_event(&self, instance: &str, action: &str) {
        let message = json!({
            "type": "lifecycle",
            "timestamp": "2026-10-24T09:00:00Z",
            "metadata": {
                "action": action,
                "source": format!("/1.0/instances/{instance}"),
                "name": instance,
                "project": "default",
            },
        });
        let _ = self.events.send(message.to_string().into_bytes());
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

/// Locks the state, through a poisoned lock too.
fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
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

/// One request's head and body.
struct Request {
    method: String,
    target: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

/// Reads one request. None once the client hangs up.
async fn read_request(stream: &mut BufReader<UnixStream>) -> Option<Request> {
    let mut head = Vec::new();
    loop {
        let mut line = String::new();
        match stream.read_line(&mut line).await {
            Ok(0) | Err(_) => return None,
            Ok(_) if line == "\r\n" => break,
            Ok(_) => head.push(line),
        }
    }
    let mut words = head.first()?.split_whitespace();
    let method = words.next()?.to_owned();
    let target = words.next()?.to_owned();
    let headers: BTreeMap<String, String> = head[1..]
        .iter()
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let length = headers
        .get("content-length")
        .and_then(|l| l.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0; length];
    stream.read_exact(&mut body).await.ok()?;
    Some(Request {
        method,
        target,
        headers,
        body,
    })
}

/// Answers requests on one connection, or plays a WebSocket once it asks
/// for one.
async fn connection(
    stream: UnixStream,
    state: Arc<Mutex<State>>,
    events: broadcast::Sender<Vec<u8>>,
) {
    let mut stream = BufReader::new(stream);
    while let Some(request) = read_request(&mut stream).await {
        let body = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
        lock(&state).log.push(Logged {
            method: request.method.clone(),
            target: request.target.clone(),
            body: body.clone(),
        });

        if request.target.starts_with("/1.0/events") {
            let mut events = events.subscribe();
            if upgrade(&mut stream).await.is_err() {
                return;
            }
            while let Ok(message) = events.recv().await {
                let frame = server_frame(true, opcode::TEXT, &message);
                if stream.write_all(&frame).await.is_err() {
                    return;
                }
            }
            return;
        }
        if let Some(rest) = request.target.strip_prefix("/1.0/operations/") {
            if let Some((id, query)) = rest.split_once("/websocket?secret=") {
                let stream_op = lock(&state).streams.get(id).cloned();
                if let Some(stream_op) = stream_op
                    && upgrade(&mut stream).await.is_ok()
                {
                    play_stream(&state, id, &stream_op, query, stream).await;
                }
                return;
            }
            if let Some(id) = rest.split('/').next().filter(|_| rest.contains("/wait")) {
                let reply = wait_operation(&state, id).await;
                if write_reply(&mut stream, reply.0, &reply.1).await.is_err() {
                    return;
                }
                continue;
            }
        }

        if request.method == "PUT" && request.target.ends_with("/state") {
            let delay = lock(&state).state_delay;
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
        }
        let reply = respond(&state, &events, &request, body);
        let written = match reply {
            Reply::Json(status, value) => write_reply(&mut stream, status, &value).await,
            Reply::Raw(bytes) => write_raw(&mut stream, &bytes).await,
            Reply::File { bytes, head } => write_file(&mut stream, &bytes, head).await,
        };
        if written.is_err() {
            return;
        }
    }
}

/// Switches the connection to a WebSocket.
async fn upgrade(stream: &mut BufReader<UnixStream>) -> std::io::Result<()> {
    let upgrade = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
                   Connection: Upgrade\r\nSec-WebSocket-Accept: x\r\n\r\n";
    stream.write_all(upgrade.as_bytes()).await
}

/// Writes a JSON reply.
async fn write_reply(
    stream: &mut BufReader<UnixStream>,
    status: u16,
    value: &Value,
) -> std::io::Result<()> {
    let body = value.to_string();
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        404 => "Not Found",
        _ => "Status",
    };
    let reply = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(reply.as_bytes()).await
}

/// Writes a plain 200 with raw bytes, as the exec log files come.
async fn write_raw(stream: &mut BufReader<UnixStream>, bytes: &[u8]) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\r\n",
        bytes.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(bytes).await
}

/// Writes a file as the file API sends it, with the headers its version
/// is read from. A file's modified time is a hash of its bytes, so a
/// change shows as one. A HEAD sends the headers only.
async fn write_file(
    stream: &mut BufReader<UnixStream>,
    bytes: &[u8],
    head: bool,
) -> std::io::Result<()> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    let reply = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nX-Incus-Type: file\r\n\
         X-Incus-Modified: {:016x}\r\nContent-Length: {}\r\n\r\n",
        hasher.finish(),
        bytes.len()
    );
    stream.write_all(reply.as_bytes()).await?;
    if !head {
        stream.write_all(bytes).await?;
    }
    Ok(())
}

/// What [`respond`] sends back.
enum Reply {
    Json(u16, Value),
    Raw(Vec<u8>),
    /// A file pulled, or only its headers for a HEAD.
    File {
        bytes: Vec<u8>,
        head: bool,
    },
}

/// A sync reply.
fn sync(metadata: Value) -> (u16, Value) {
    (
        200,
        json!({ "type": "sync", "status": "Success", "status_code": 200, "operation": "",
                "error_code": 0, "error": "", "metadata": metadata }),
    )
}

/// An async reply for operation `id`.
fn async_reply(id: &str, metadata: Value) -> (u16, Value) {
    (
        202,
        json!({ "type": "async", "status": "Operation created", "status_code": 100,
                "operation": format!("/1.0/operations/{id}"), "error_code": 0, "error": "",
                "metadata": { "id": id, "class": "task", "status": "Running",
                              "status_code": 103, "metadata": metadata } }),
    )
}

/// An error reply.
fn error(status: u16, message: &str) -> (u16, Value) {
    (
        status,
        json!({ "type": "error", "status": "", "status_code": 0, "operation": "",
                "error_code": status, "error": message, "metadata": null }),
    )
}

/// A recorded reply.
fn recorded(text: &str) -> (u16, Value) {
    let value: Value = serde_json::from_str(text).unwrap();
    let status = value["error_code"]
        .as_u64()
        .filter(|&c| c != 0)
        .unwrap_or(200);
    (status as u16, value)
}

/// The answer to one plain request.
fn respond(
    state: &Mutex<State>,
    events: &broadcast::Sender<Vec<u8>>,
    request: &Request,
    body: Value,
) -> Reply {
    let mut state = lock(state);
    let method = request.method.as_str();
    let target = request.target.as_str();
    if state.down {
        let (status, value) = error(500, "Incus is starting");
        return Reply::Json(status, value);
    }
    if let Some(bytes) = state.exec_logs.get(target).cloned() {
        return if method == "DELETE" {
            state.exec_logs.remove(target);
            let (status, value) = sync(Value::Null);
            Reply::Json(status, value)
        } else {
            Reply::Raw(bytes)
        };
    }
    let (status, value) = match (method, target) {
        ("GET", "/1.0") => recorded(SERVER),
        ("GET", "/1.0/networks?recursion=1") => recorded(NETWORKS),
        ("GET", "/1.0/storage-pools?recursion=1") => recorded(POOLS),
        ("GET", "/1.0/instances?recursion=1") => {
            sync(Value::Array(state.instances.values().cloned().collect()))
        }
        ("POST", "/1.0/instances") => create(&mut state, body),
        // The pools and networks stay the recorded ones; the log keeps
        // what was asked.
        ("POST", "/1.0/storage-pools" | "/1.0/networks") => sync(Value::Null),
        _ => {
            if let Some(pool) = target.strip_prefix("/1.0/storage-pools/") {
                storage_pool(&state, pool)
            } else if let Some(rest) = target.strip_prefix("/1.0/instances/")
                && let Some((name, path)) = rest.split_once("/files?path=")
                && matches!(method, "GET" | "HEAD")
            {
                return match state.files.get(&(name.to_owned(), path.to_owned())) {
                    Some(file) if file.kind != "directory" => Reply::File {
                        bytes: file.bytes.clone(),
                        head: method == "HEAD",
                    },
                    // What Incus says, not a 404, when a folder on the way is a file.
                    _ if through_a_file(&state, name, path) => {
                        let (status, value) = error(
                            500,
                            &format!("sftp: \"lstat {path}: not a directory\" (SSH_FX_FAILURE)"),
                        );
                        Reply::Json(status, value)
                    }
                    _ => {
                        let (status, value) = error(404, "Not Found");
                        Reply::Json(status, value)
                    }
                };
            } else if let Some(rest) = target.strip_prefix("/1.0/instances/") {
                instance_call(&mut state, events, method, rest, request, body)
            } else {
                error(404, "Not Found")
            }
        }
    };
    Reply::Json(status, value)
}

/// True when a folder on the way to `path` is a file.
fn through_a_file(state: &State, name: &str, path: &str) -> bool {
    path.match_indices('/').any(|(at, _)| {
        state
            .files
            .get(&(name.to_owned(), path[..at].to_owned()))
            .is_some_and(|file| file.kind != "directory")
    })
}

/// `/1.0/storage-pools/<pool>` and its `/resources`.
fn storage_pool(state: &State, rest: &str) -> (u16, Value) {
    let (name, resources) = match rest.strip_suffix("/resources") {
        Some(name) => (name, true),
        None => (rest, false),
    };
    let pools: Value = serde_json::from_str(POOLS).unwrap();
    let Some(pool) = pools["metadata"]
        .as_array()
        .and_then(|pools| pools.iter().find(|p| p["name"] == name))
    else {
        return error(404, "Storage pool not found");
    };
    if !resources {
        let mut pool = pool.clone();
        if let Some(source) = &state.pool_source {
            pool["config"]["source"] = json!(source);
        }
        return sync(pool);
    }
    if state.locked_pools.contains(name) {
        return recorded(UNAVAILABLE);
    }
    recorded(RESOURCES)
}

/// `POST /1.0/instances`.
fn create(state: &mut State, body: Value) -> (u16, Value) {
    let Some(name) = body["name"].as_str().map(str::to_owned) else {
        return error(400, "No name provided");
    };
    if state.instances.contains_key(&name) {
        return state.fail("This instance already exists");
    }
    if body["source"]["type"] == "copy" {
        return copy(state, name, &body);
    }
    let mut instance = instance(&name, "Stopped", &[]);
    instance["config"] = body["config"].clone();
    instance["expanded_config"] = body["config"].clone();
    instance["devices"] = body["devices"].clone();
    instance["profiles"] = body["profiles"].clone();
    state.instances.insert(name, instance);
    state.finish(json!({}))
}

/// A create that copies `source`, an instance or `<instance>/<snapshot>`,
/// with its files then. As in Incus, the body's keys go on top of the
/// source's, its devices replace any of the same name, and its profiles
/// replace the source's.
fn copy(state: &mut State, name: String, body: &Value) -> (u16, Value) {
    let source = body["source"]["source"].as_str().unwrap_or_default();
    let (from, snapshot) = match source.split_once('/') {
        Some((from, snapshot)) => (from, Some(snapshot)),
        None => (source, None),
    };
    let Some(original) = state.instances.get(from) else {
        return state.fail("Source instance not found");
    };
    let mut instance = original.clone();
    instance["name"] = json!(name);
    instance["status"] = json!("Stopped");
    instance["status_code"] = json!(102);
    let files = match snapshot {
        Some(snapshot) => match state.snapshots.get(&(from.to_owned(), snapshot.to_owned())) {
            Some(taken) => {
                set_own(&mut instance, &taken.config, &taken.devices);
                taken.files.clone()
            }
            None => return state.fail("Snapshot not found"),
        },
        None => files_of(state, from),
    };
    if let Some(keys) = body["config"].as_object() {
        for (key, value) in keys {
            instance["config"][key] = value.clone();
            instance["expanded_config"][key] = value.clone();
        }
    }
    if let Some(devices) = body["devices"].as_object() {
        for (device, keys) in devices {
            instance["devices"][device] = keys.clone();
        }
    }
    if body["profiles"].is_array() {
        instance["profiles"] = body["profiles"].clone();
    }
    for (path, pushed) in files {
        state.files.insert((name.clone(), path), pushed);
    }
    state.instances.insert(name, instance);
    state.finish(json!({}))
}

/// Calls under `/1.0/instances/<name>`.
fn instance_call(
    state: &mut State,
    events: &broadcast::Sender<Vec<u8>>,
    method: &str,
    rest: &str,
    request: &Request,
    body: Value,
) -> (u16, Value) {
    let (name, tail) = match rest.find(['/', '?']) {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, ""),
    };
    let name = name.to_owned();
    if !state.instances.contains_key(&name) {
        return error(404, "Instance not found");
    }
    let event = |action: &str| {
        let message = json!({ "type": "lifecycle", "metadata": {
            "action": action, "source": format!("/1.0/instances/{name}"), "name": name } });
        let _ = events.send(message.to_string().into_bytes());
    };
    match (method, tail) {
        ("GET", "") => sync(state.instances[&name].clone()),
        ("PATCH", "") => {
            let instance = state.instances.get_mut(&name).unwrap();
            if let Some(keys) = body["config"].as_object() {
                for (key, value) in keys {
                    instance["config"][key] = value.clone();
                    instance["expanded_config"][key] = value.clone();
                }
            }
            if let Some(devices) = body["devices"].as_object() {
                for (device, keys) in devices {
                    instance["devices"][device] = keys.clone();
                }
            }
            sync(Value::Null)
        }
        ("DELETE", "") => {
            if state.instances[&name]["status"] == "Running" {
                return state.fail("Instance is running");
            }
            state.instances.remove(&name);
            state.files.retain(|(instance, _), _| *instance != name);
            state.snapshots.retain(|(instance, _), _| *instance != name);
            event("instance-deleted");
            state.finish(json!({}))
        }
        ("PUT", "/state") => {
            let action = body["action"].as_str().unwrap_or_default();
            let status = match action {
                "start" | "restart" => "Running",
                "stop" => "Stopped",
                _ => return error(400, "Unknown action"),
            };
            state.instances.get_mut(&name).unwrap()["status"] = json!(status);
            event(match action {
                "start" => "instance-started",
                "restart" => "instance-restarted",
                _ => "instance-stopped",
            });
            state.finish(json!({}))
        }
        ("POST", "/exec") => exec(state, &name, body),
        ("GET", "/state") => {
            let usage = state.disk_used.get(&name).copied();
            let disk = match usage {
                Some(usage) => json!({ "root": { "usage": usage } }),
                None => json!({}),
            };
            sync(json!({ "status": state.instances[&name]["status"], "disk": disk }))
        }
        ("POST", tail) if tail.starts_with("/files?path=") => {
            let path = tail.trim_start_matches("/files?path=").to_owned();
            let header = |name: &str| request.headers.get(name).cloned().unwrap_or_default();
            let mut pushed = Pushed {
                kind: header("x-incus-type"),
                mode: header("x-incus-mode"),
                owner: (header("x-incus-uid"), header("x-incus-gid")),
                bytes: request.body.clone(),
            };
            let key = (name, path);
            if header("x-incus-write") == "append"
                && let Some(before) = state.files.get(&key)
            {
                pushed.bytes = [before.bytes.as_slice(), &request.body].concat();
            }
            state.files.insert(key, pushed);
            sync(Value::Null)
        }
        ("GET", "/snapshots?recursion=1") => {
            let listed = state
                .snapshots
                .iter()
                .filter(|((instance, _), _)| *instance == name)
                .map(|((_, snapshot), taken)| {
                    json!({ "name": snapshot, "created_at": taken.created_at,
                            "expires_at": taken.expires_at, "stateful": false })
                })
                .collect();
            sync(Value::Array(listed))
        }
        ("DELETE", tail) if tail.starts_with("/snapshots/") => {
            let snapshot = tail.trim_start_matches("/snapshots/").to_owned();
            match state.snapshots.remove(&(name, snapshot)) {
                Some(_) => state.finish(json!({})),
                None => error(404, "Snapshot not found"),
            }
        }
        ("POST", "/snapshots") => {
            let Some(snapshot) = body["name"].as_str().map(str::to_owned) else {
                return error(400, "No snapshot name provided");
            };
            // Incus applies the instance's expiry unless the body names
            // one, and the year 1 means never.
            let instance = &state.instances[&name];
            let expires_at = match body["expires_at"].as_str() {
                Some(at) => at.to_owned(),
                None if instance["config"]["snapshots.expiry"].is_string() => {
                    "2026-11-09T00:00:00Z".to_owned()
                }
                None => "0001-01-01T00:00:00Z".to_owned(),
            };
            let taken = Taken {
                files: files_of(state, &name),
                config: instance["config"].clone(),
                devices: instance["devices"].clone(),
                expires_at,
                created_at: format!("2026-11-02T00:{:02}:00Z", state.snapshots.len() % 60),
            };
            state.snapshots.insert((name, snapshot), taken);
            state.finish(json!({}))
        }
        ("PUT", "") if body["restore"].is_string() => {
            let snapshot = body["restore"].as_str().unwrap_or_default().to_owned();
            let Some(taken) = state.snapshots.get(&(name.clone(), snapshot)).cloned() else {
                return state.fail("Snapshot not found");
            };
            if state.instances[&name]["status"] == "Running" {
                return state.fail("Instance is running");
            }
            // A restore takes the keys and devices back too.
            set_own(
                state.instances.get_mut(&name).unwrap(),
                &taken.config,
                &taken.devices,
            );
            state.files.retain(|(instance, _), _| *instance != name);
            for (path, pushed) in taken.files {
                state.files.insert((name.clone(), path), pushed);
            }
            state.finish(json!({}))
        }
        // A full update: the body's keys, devices and profiles replace the
        // instance's.
        ("PUT", "") => {
            let instance = state.instances.get_mut(&name).unwrap();
            set_own(instance, &body["config"], &body["devices"]);
            instance["profiles"] = body["profiles"].clone();
            state.finish(json!({}))
        }
        _ => error(404, "Not Found"),
    }
}

/// Sets an instance's own keys and devices, as a restore or a copy from a
/// snapshot does.
fn set_own(instance: &mut Value, config: &Value, devices: &Value) {
    instance["config"] = config.clone();
    instance["expanded_config"] = config.clone();
    instance["devices"] = devices.clone();
}

/// The files pushed into `name`, by path.
fn files_of(state: &State, name: &str) -> BTreeMap<String, Pushed> {
    state
        .files
        .iter()
        .filter(|((instance, _), _)| instance == name)
        .map(|((_, path), pushed)| (path.clone(), pushed.clone()))
        .collect()
}

/// `POST /1.0/instances/<name>/exec`.
fn exec(state: &mut State, name: &str, body: Value) -> (u16, Value) {
    if state.instances[name]["status"] != "Running" {
        return error(400, "Instance is not running");
    }
    let call = ExecCall {
        instance: name.to_owned(),
        command: body["command"]
            .as_array()
            .map(|c| {
                c.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        user: body["user"].as_u64().unwrap_or(0) as u32,
        environment: serde_json::from_value(body["environment"].clone()).unwrap_or_default(),
        cwd: body["cwd"].as_str().unwrap_or_default().to_owned(),
        stdin: Vec::new(),
        streamed: body["wait-for-websocket"] == json!(true),
    };
    if call.streamed {
        let id = state.operation_id();
        let secrets: BTreeMap<String, String> = ["0", "1", "2", "control"]
            .iter()
            .map(|fd| ((*fd).to_owned(), format!("secret-{id}-{fd}")))
            .collect();
        let (reply, _) = watch::channel(None);
        state.streams.insert(
            id.clone(),
            Arc::new(Stream {
                call: Mutex::new(call),
                secrets: secrets.clone(),
                reply,
            }),
        );
        return async_reply(&id, json!({ "fds": secrets }));
    }
    let reply = run(state, call);
    let id = state.next_operation + 1;
    let base = format!("/1.0/instances/{name}/logs/exec-output/exec_{id}");
    let stdout = format!("{base}.stdout");
    let stderr = format!("{base}.stderr");
    state.exec_logs.insert(stdout.clone(), reply.stdout);
    state.exec_logs.insert(stderr.clone(), reply.stderr);
    let id = state.operation_id();
    let metadata = json!({ "return": reply.status, "output": { "1": stdout, "2": stderr } });
    state
        .operations
        .insert(id.clone(), exec_operation(&id, reply.status, metadata));
    async_reply(&id, json!({}))
}

/// An ended exec's operation. Like Incus, it marks exits 126 and 127 as
/// failed while keeping the code and the output.
fn exec_operation(id: &str, status: i32, metadata: Value) -> Value {
    let err = match status {
        126 => "Command not executable",
        127 => "Command not found",
        _ => {
            return json!({ "id": id, "status": "Success", "status_code": 200, "err": "",
                           "metadata": metadata });
        }
    };
    json!({ "id": id, "status": "Failure", "status_code": 400, "err": err, "metadata": metadata })
}

/// Records a call and runs the handler.
fn run(state: &mut State, call: ExecCall) -> ExecReply {
    let handler = state.exec.clone();
    let reply = handler.map(|h| h(&call)).unwrap_or_default();
    state.execs.push(call);
    reply
}

/// Plays one of a streamed exec's WebSockets.
async fn play_stream(
    state: &Mutex<State>,
    id: &str,
    stream_op: &Stream,
    secret: &str,
    stream: BufReader<UnixStream>,
) {
    let Some(fd) = stream_op
        .secrets
        .iter()
        .find(|(_, s)| s.as_str() == secret)
        .map(|(fd, _)| fd.clone())
    else {
        return;
    };
    let mut socket = WebSocket::new(stream);
    match fd.as_str() {
        "0" => {
            let mut stdin = Vec::new();
            while let Ok(Some(message)) = socket.next().await {
                if message.text {
                    break;
                }
                stdin.extend_from_slice(&message.payload);
            }
            let call = {
                let mut call = stream_op
                    .call
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                call.stdin = stdin;
                call.clone()
            };
            let reply = {
                let mut state = lock(state);
                let reply = run(&mut state, call);
                state.operations.insert(
                    id.to_owned(),
                    exec_operation(id, reply.status, json!({ "return": reply.status })),
                );
                reply
            };
            stream_op.reply.send_replace(Some(reply));
        }
        "1" | "2" => {
            let mut reply = stream_op.reply.subscribe();
            let done = match reply.wait_for(Option::is_some).await {
                Ok(done) => done.clone(),
                Err(_) => return,
            };
            let Some(done) = done else {
                return;
            };
            let bytes = if fd == "1" { done.stdout } else { done.stderr };
            let mut stream = socket.into_inner();
            let _ = stream
                .write_all(&server_frame(true, opcode::BINARY, &bytes))
                .await;
            let _ = stream
                .write_all(&server_frame(true, opcode::TEXT, b""))
                .await;
            let _ = stream
                .write_all(&server_frame(true, opcode::CLOSE, b""))
                .await;
        }
        _ => {
            // The control stream stays open until the client hangs up.
            while let Ok(Some(_)) = socket.next().await {}
        }
    }
}

/// `GET /1.0/operations/<id>/wait`. A streamed exec's operation ends once
/// its stdin has closed.
async fn wait_operation(state: &Mutex<State>, id: &str) -> (u16, Value) {
    let stream_op = lock(state).streams.get(id).cloned();
    if let Some(stream_op) = stream_op {
        let mut reply = stream_op.reply.subscribe();
        let _ = reply.wait_for(Option::is_some).await;
    }
    let state = lock(state);
    match state.operations.get(id) {
        Some(operation) => sync(operation.clone()),
        None => error(404, "Operation not found"),
    }
}
