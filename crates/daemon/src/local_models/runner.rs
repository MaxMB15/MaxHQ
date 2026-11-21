// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The managed server. Local agents reach it through a proxy on a port of
//! 127.0.0.1 that stays put across restarts, with a key of its own. Behind
//! it runs one llama-server per loaded model, each on a random port of
//! 127.0.0.1 with the same key, never in router mode. A request for a model
//! that isn't loaded loads it first, unloading the others nothing uses;
//! a model with no local agent turn for the idle setting unloads.
//!
//! The proxy keeps the tools and system prompt of the last full request
//! for each model, so a load you ask for replays them with a one-token
//! turn and the next turn finds them in the KV cache.

use std::collections::HashMap;
use std::convert::Infallible;
use std::io;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::header::{HeaderName, HeaderValue, AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, HOST};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use maxhq_protocol::local_models::{GpuMemory, KvCacheType, KvShape, ModelLoad};
use maxhq_protocol::secret::Secret;
use maxhq_protocol::time::Timestamp;
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

use super::hardware;

/// The largest request body the proxy takes: Claude Code's requests carry
/// the whole conversation.
const MAX_BODY: usize = 64 << 20;

/// How long a model may take to load.
const LOAD_FOR: Duration = Duration::from_secs(300);

/// How often a loading server is asked whether it's ready.
const POLL: Duration = Duration::from_millis(250);

/// The headers the proxy sets itself or that belong to one connection.
const DROPPED: [&str; 7] = [
    "host",
    "authorization",
    "x-api-key",
    "content-length",
    "connection",
    "keep-alive",
    "transfer-encoding",
];

/// How to load one model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    /// The model's ID, `<catalog id>@<quant>`, which the server reports.
    pub alias: String,
    /// The model file.
    pub path: PathBuf,
    /// The context size of each slot, in tokens.
    pub context: u32,
    /// How many requests it serves at once.
    pub slots: u8,
    /// How it stores its KV cache.
    pub kv: KvCacheType,
}

/// A running llama-server.
pub struct Backend {
    /// Its port on 127.0.0.1.
    pub port: u16,
    /// The process, stopped when this is dropped. None in tests.
    pub child: Option<tokio::process::Child>,
    /// The file that names its process, so a `maxd` that didn't stop it
    /// can stop it at the next start.
    pub pid_file: Option<PathBuf>,
}

impl Drop for Backend {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.start_kill();
        }
        if let Some(file) = &self.pid_file {
            let _ = std::fs::remove_file(file);
        }
    }
}

/// Takes how much of a model is in memory while it loads, in percent.
pub type LoadProgress = Arc<dyn Fn(u8) + Send + Sync>;

/// What the runner needs from the service that owns it.
pub trait Host: Send + Sync + 'static {
    /// How to load `model`, when it's a ready managed model.
    fn spec(&self, model: &str) -> Option<Spec>;
    /// The managed models ready to load.
    fn models(&self) -> Vec<String>;
    /// Starts llama-server for `spec`, answering once it serves, and tells
    /// `progress` how far it got meanwhile.
    fn start(
        &self,
        spec: Spec,
        progress: LoadProgress,
    ) -> BoxFuture<'static, Result<Backend, String>>;
    /// A model's load or speed changed.
    fn changed(&self);
    /// Whether a local agent's turn runs on the managed server now.
    fn busy(&self) -> bool;
    /// Minutes with no turn before a model unloads; 0 keeps it.
    fn idle_minutes(&self) -> u16;
}

/// How fast a model ran in its last turn, in tokens per second.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Speed {
    /// Reading the prompt.
    pub prompt: Option<f32>,
    /// Writing.
    pub decode: Option<f32>,
}

/// llama-server's running totals, from `/metrics`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Counters {
    prompt_tokens: f64,
    prompt_seconds: f64,
    decoded: f64,
    decode_seconds: f64,
}

/// One model the runner knows.
struct Slot {
    load: ModelLoad,
    backend: Option<Backend>,
    in_flight: usize,
    last_used: Instant,
    prefix: Option<Value>,
    speed: Speed,
    counters: Option<Counters>,
}

impl Slot {
    fn new() -> Self {
        Self {
            load: ModelLoad::Unloaded,
            backend: None,
            in_flight: 0,
            last_used: Instant::now(),
            prefix: None,
            speed: Speed::default(),
            counters: None,
        }
    }

    fn port(&self) -> Option<u16> {
        match (&self.load, &self.backend) {
            (ModelLoad::Loaded { .. }, Some(backend)) => Some(backend.port),
            _ => None,
        }
    }
}

/// The managed server's models and the proxy in front of them.
pub struct Runner {
    host: Arc<dyn Host>,
    key: Secret,
    slots: Mutex<HashMap<String, Slot>>,
    budget: Mutex<Option<GpuMemory>>,
    signal: watch::Sender<u64>,
}

impl Runner {
    /// A runner whose proxy and servers take `key`.
    pub fn new(host: Arc<dyn Host>, key: Secret) -> Arc<Self> {
        Arc::new(Self {
            host,
            key,
            slots: Mutex::default(),
            budget: Mutex::default(),
            signal: watch::Sender::new(0),
        })
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Slot>> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn wake(&self) {
        self.signal.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// Where `model` stands.
    pub fn load_of(&self, model: &str) -> ModelLoad {
        self.lock()
            .get(model)
            .map_or(ModelLoad::Unloaded, |slot| slot.load.clone())
    }

    /// How fast `model` ran in its last turn.
    pub fn speed_of(&self, model: &str) -> Speed {
        self.lock()
            .get(model)
            .map(|slot| slot.speed)
            .unwrap_or_default()
    }

    /// The GPU budget Metal reported, once llama-server listed it.
    pub fn budget(&self) -> Option<GpuMemory> {
        self.budget
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Keeps the GPU budget Metal reported.
    pub fn set_budget(&self, budget: GpuMemory) {
        *self.budget.lock().unwrap_or_else(PoisonError::into_inner) = Some(budget);
    }

    /// Shows how far `model` got loading, while it still loads.
    fn loading(&self, model: &str, percent: u8) {
        let moved = {
            let mut slots = self.lock();
            match slots.get_mut(model).map(|slot| &mut slot.load) {
                Some(ModelLoad::Loading { percent: now }) if *now != Some(percent) => {
                    *now = Some(percent);
                    true
                }
                _ => false,
            }
        };
        if moved {
            self.host.changed();
        }
    }

    /// The port `model` serves on, loading it first when it isn't loaded.
    pub async fn ensure(self: &Arc<Self>, model: &str) -> Result<u16, String> {
        let mut seen = self.signal.subscribe();
        // Set once a load this call waited on has had its chance, so a load
        // that fails isn't tried again and again.
        let mut waited = false;
        loop {
            let start = {
                let mut slots = self.lock();
                let slot = slots.entry(model.to_owned()).or_insert_with(Slot::new);
                if let Some(port) = slot.port() {
                    return Ok(port);
                }
                match &slot.load {
                    ModelLoad::Loading { .. } => {
                        waited = true;
                        false
                    }
                    ModelLoad::Failed { reason } if waited => return Err(reason.clone()),
                    ModelLoad::Unloaded if waited => {
                        return Err("The model was unloaded while it loaded.".to_owned())
                    }
                    _ => {
                        slot.load = ModelLoad::Loading { percent: None };
                        waited = true;
                        true
                    }
                }
            };
            if start {
                let runner = self.clone();
                let model = model.to_owned();
                tokio::spawn(async move { runner.load_now(model).await });
            }
            if seen.changed().await.is_err() {
                return Err("maxd is stopping.".to_owned());
            }
        }
    }

    /// Loads `model` now, then replays the last tools and system prompt it
    /// served so the next turn finds them cached.
    pub async fn load(self: &Arc<Self>, model: &str) -> Result<(), String> {
        let port = self.ensure(model).await?;
        let prefix = self.lock().get(model).and_then(|slot| slot.prefix.clone());
        if let Some(prefix) = prefix {
            let _busy = Busy::new(self, model);
            let started = Instant::now();
            match warm(port, &self.key, model, prefix).await {
                Ok(()) => {
                    tracing::info!(model, took = ?started.elapsed(), "warmed the model's prompt cache")
                }
                Err(err) => tracing::warn!(model, %err, "couldn't warm the model's prompt cache"),
            }
        }
        Ok(())
    }

    async fn load_now(self: Arc<Self>, model: String) {
        self.host.changed();
        let result = match self.host.spec(&model) {
            None => Err(format!("{model} isn't ready to load on this Mac.")),
            Some(spec) => {
                drop(self.make_room(&model));
                let progress: LoadProgress = {
                    let runner = Arc::downgrade(&self);
                    let model = model.clone();
                    Arc::new(move |percent| {
                        if let Some(runner) = runner.upgrade() {
                            runner.loading(&model, percent);
                        }
                    })
                };
                self.host
                    .start(spec.clone(), progress)
                    .await
                    .map(|backend| (spec, backend))
            }
        };
        let stale = {
            let mut slots = self.lock();
            let slot = slots.entry(model.clone()).or_insert_with(Slot::new);
            if matches!(slot.load, ModelLoad::Loading { .. }) {
                match result {
                    Ok((spec, backend)) => {
                        slot.load = ModelLoad::Loaded {
                            slots: spec.slots,
                            context: spec.context,
                            since: Timestamp::now(),
                        };
                        slot.backend = Some(backend);
                        slot.last_used = Instant::now();
                        slot.counters = None;
                        None
                    }
                    Err(reason) => {
                        tracing::warn!(model, %reason, "a model didn't load");
                        slot.load = ModelLoad::Failed { reason };
                        None
                    }
                }
            } else {
                // Unloaded while it loaded: the new server stops here.
                result.ok().map(|(_, backend)| backend)
            }
        };
        drop(stale);
        self.wake();
        self.host.changed();
    }

    /// Takes out the servers of the other loaded models nothing uses, so
    /// they stop when the caller drops them.
    fn make_room(&self, model: &str) -> Vec<Backend> {
        let mut stopped = Vec::new();
        for (name, slot) in self.lock().iter_mut() {
            if name != model && slot.in_flight == 0 && slot.port().is_some() {
                slot.load = ModelLoad::Unloaded;
                stopped.extend(slot.backend.take());
            }
        }
        stopped
    }

    /// Unloads `model`, ending what it serves.
    pub fn unload(&self, model: &str) {
        let backend = {
            let mut slots = self.lock();
            let Some(slot) = slots.get_mut(model) else {
                return;
            };
            slot.load = ModelLoad::Unloaded;
            slot.backend.take()
        };
        drop(backend);
        self.wake();
        self.host.changed();
    }

    /// Unloads every model that sat idle past the setting. A running turn
    /// keeps them all, and its end starts the count again.
    pub fn sweep(&self, now: Instant) -> bool {
        let minutes = self.host.idle_minutes();
        let busy = self.host.busy();
        let mut stopped = Vec::new();
        for slot in self.lock().values_mut() {
            if slot.port().is_none() {
                continue;
            }
            if busy {
                slot.last_used = now;
            } else if idle(minutes, slot.last_used, now, slot.in_flight) {
                slot.load = ModelLoad::Unloaded;
                stopped.extend(slot.backend.take());
            }
        }
        if stopped.is_empty() {
            return false;
        }
        tracing::info!(count = stopped.len(), "unloaded idle local models");
        drop(stopped);
        self.wake();
        self.host.changed();
        true
    }

    /// Marks each model whose llama-server stopped by itself as failed, so
    /// the next turn loads it again.
    fn reap(&self) {
        let mut stopped = Vec::new();
        for slot in self.lock().values_mut() {
            let Some(child) = slot
                .backend
                .as_mut()
                .and_then(|backend| backend.child.as_mut())
            else {
                continue;
            };
            if let Ok(Some(status)) = child.try_wait() {
                slot.load = ModelLoad::Failed {
                    reason: format!("llama-server stopped ({status})."),
                };
                stopped.extend(slot.backend.take());
            }
        }
        if stopped.is_empty() {
            return;
        }
        tracing::warn!(count = stopped.len(), "a llama-server stopped by itself");
        drop(stopped);
        self.wake();
        self.host.changed();
    }

    /// Reads each loaded model's speed and unloads idle ones.
    pub async fn tick(self: &Arc<Self>) {
        self.reap();
        let loaded: Vec<(String, u16)> = self
            .lock()
            .iter()
            .filter_map(|(model, slot)| slot.port().map(|port| (model.clone(), port)))
            .collect();
        let mut changed = false;
        for (model, port) in loaded {
            let Some(now) = metrics(port, &self.key).await else {
                continue;
            };
            let mut slots = self.lock();
            let Some(slot) = slots.get_mut(&model) else {
                continue;
            };
            if let Some(before) = slot.counters {
                let speed = rates(before, now, slot.speed);
                if speed != slot.speed {
                    slot.speed = speed;
                    changed = true;
                }
            }
            slot.counters = Some(now);
        }
        if !self.sweep(Instant::now()) && changed {
            self.host.changed();
        }
    }

    /// Serves the proxy on `listener` until `maxd` stops.
    pub async fn serve(self: Arc<Self>, listener: TcpListener) {
        loop {
            let stream = match listener.accept().await {
                Ok((stream, _)) => stream,
                Err(err) => {
                    tracing::warn!(%err, "the model proxy couldn't take a connection");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            let runner = self.clone();
            tokio::spawn(async move {
                let service = hyper::service::service_fn(move |request| {
                    let runner = runner.clone();
                    async move { Ok::<_, Infallible>(runner.handle(request).await) }
                });
                let served = hyper::server::conn::http1::Builder::new()
                    .timer(TokioTimer::new())
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
                if let Err(err) = served {
                    tracing::debug!(%err, "a model proxy connection ended early");
                }
            });
        }
    }

    /// Answers one request to the proxy.
    pub async fn handle(self: Arc<Self>, request: Request<Incoming>) -> Response<Body> {
        if !self.allowed(request.headers()) {
            return error(
                StatusCode::UNAUTHORIZED,
                "authentication_error",
                "This model server wants its key.",
            );
        }
        if request.method() == Method::GET {
            if request.uri().path() == "/v1/models" {
                return self.list();
            }
            return error(StatusCode::NOT_FOUND, "not_found_error", "Nothing is here.");
        }
        let (parts, body) = request.into_parts();
        let Ok(body) = Limited::new(body, MAX_BODY).collect().await else {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request_too_large",
                "The request is too large.",
            );
        };
        let body = body.to_bytes();
        let Some(model) = serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|json| {
                let model = json.get("model")?.as_str()?.to_owned();
                if parts.uri.path() == "/v1/messages" {
                    self.remember(&model, &json);
                }
                Some(model)
            })
        else {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "The request names no model.",
            );
        };
        if self.host.spec(&model).is_none() {
            return error(
                StatusCode::NOT_FOUND,
                "not_found_error",
                &format!("No model {model} is ready on this Mac."),
            );
        }
        let busy = Busy::new(&self, &model);
        let port = match self.ensure(&model).await {
            Ok(port) => port,
            Err(reason) => return error(StatusCode::SERVICE_UNAVAILABLE, "api_error", &reason),
        };
        match forward(port, &self.key, &parts, body).await {
            Ok(response) => response.map(|body| {
                body.map_frame(move |frame| {
                    busy.touch();
                    frame
                })
                .boxed_unsync()
            }),
            Err(err) => error(StatusCode::BAD_GATEWAY, "api_error", &err),
        }
    }

    /// Whether a request carries the key, as `x-api-key` or a bearer token.
    fn allowed(&self, headers: &hyper::HeaderMap) -> bool {
        let key = self.key.expose().as_bytes();
        let given = headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .or_else(|| {
                headers
                    .get(AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.strip_prefix("Bearer "))
                    .map(str::trim)
            });
        given.is_some_and(|given| crate::agents::same_hash(given.as_bytes(), key))
    }

    fn list(&self) -> Response<Body> {
        let data: Vec<Value> = self
            .host
            .models()
            .into_iter()
            .map(|id| json!({"id": id, "object": "model", "type": "model", "display_name": id}))
            .collect();
        reply(
            StatusCode::OK,
            &json!({"object": "list", "data": data, "has_more": false}),
        )
    }

    /// Keeps the tools and system prompt of a full request.
    fn remember(&self, model: &str, json: &Value) {
        let tools = json.get("tools").and_then(Value::as_array);
        if tools.is_none_or(Vec::is_empty) {
            return;
        }
        let prefix = json!({
            "system": json.get("system").cloned().unwrap_or(Value::Null),
            "tools": tools.cloned().unwrap_or_default(),
        });
        self.lock()
            .entry(model.to_owned())
            .or_insert_with(Slot::new)
            .prefix = Some(prefix);
    }
}

/// The proxy's answers.
pub type Body = UnsyncBoxBody<Bytes, hyper::Error>;

/// Counts a request to a model while it runs, and when it last did
/// anything.
struct Busy {
    runner: Arc<Runner>,
    model: String,
}

impl Busy {
    fn new(runner: &Arc<Runner>, model: &str) -> Self {
        let mut slots = runner.lock();
        let slot = slots.entry(model.to_owned()).or_insert_with(Slot::new);
        slot.in_flight += 1;
        slot.last_used = Instant::now();
        drop(slots);
        Self {
            runner: runner.clone(),
            model: model.to_owned(),
        }
    }

    fn touch(&self) {
        if let Some(slot) = self.runner.lock().get_mut(&self.model) {
            slot.last_used = Instant::now();
        }
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        if let Some(slot) = self.runner.lock().get_mut(&self.model) {
            slot.in_flight = slot.in_flight.saturating_sub(1);
            slot.last_used = Instant::now();
        }
    }
}

/// Whether a model that last served at `last_used` unloads at `now`.
pub fn idle(minutes: u16, last_used: Instant, now: Instant, in_flight: usize) -> bool {
    minutes > 0
        && in_flight == 0
        && now.saturating_duration_since(last_used) >= Duration::from_secs(u64::from(minutes) * 60)
}

/// The speeds between two readings of the totals, keeping `last` for what
/// didn't move.
pub fn rates(before: Counters, after: Counters, last: Speed) -> Speed {
    let rate = |tokens: f64, seconds: f64, kept: Option<f32>| {
        if tokens > 0.0 && seconds > 0.0 {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a speed far inside f32's range"
            )]
            Some((tokens / seconds) as f32)
        } else {
            kept
        }
    };
    Speed {
        prompt: rate(
            after.prompt_tokens - before.prompt_tokens,
            after.prompt_seconds - before.prompt_seconds,
            last.prompt,
        ),
        decode: rate(
            after.decoded - before.decoded,
            after.decode_seconds - before.decode_seconds,
            last.decode,
        ),
    }
}

/// The totals in llama-server's Prometheus text.
pub fn parse_metrics(text: &str) -> Counters {
    let mut counters = Counters::default();
    for line in text.lines().filter(|line| !line.starts_with('#')) {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(value)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Ok(value) = value.parse::<f64>() else {
            continue;
        };
        match name {
            "llamacpp:prompt_tokens_total" => counters.prompt_tokens = value,
            "llamacpp:prompt_seconds_total" => counters.prompt_seconds = value,
            "llamacpp:tokens_predicted_total" => counters.decoded = value,
            "llamacpp:tokens_predicted_seconds_total" => counters.decode_seconds = value,
            _ => {}
        }
    }
    counters
}

/// How many requests a model serves at once: two when a second KV cache
/// still leaves room under the GPU budget, else one.
pub fn slots_for(bytes: u64, kv: &KvShape, context: u32, kv_type: KvCacheType, budget: u64) -> u8 {
    let two = bytes + kv_bytes(kv, context, kv_type) * 2 + OVERHEAD;
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a share of a memory size, rounded down"
    )]
    let roomy = (budget as f64 * 0.9) as u64;
    if two <= roomy {
        2
    } else {
        1
    }
}

/// The buffers a loaded model needs beside its weights and KV cache.
const OVERHEAD: u64 = 1 << 30;

/// The bytes a KV cache holds for `context` tokens, as the views count it.
pub fn kv_bytes(kv: &KvShape, context: u32, kv_type: KvCacheType) -> u64 {
    let heads = 2 * u64::from(kv.kv_heads) * u64::from(kv.head_dim);
    let tokens = u64::from(kv.full_layers) * u64::from(context)
        + u64::from(kv.swa_layers) * u64::from(kv.swa_window.min(context));
    match kv_type {
        KvCacheType::F16 => heads * 2 * tokens,
        // q8_0 packs 32 values in 34 bytes.
        KvCacheType::Q8_0 => heads * tokens * 34 / 32,
    }
}

/// The arguments llama-server runs with for `spec`.
pub fn args(spec: &Spec, port: u16, key_file: &Path) -> Vec<String> {
    let kv = match spec.kv {
        KvCacheType::F16 => "f16",
        KvCacheType::Q8_0 => "q8_0",
    };
    let slots = spec.slots.max(1);
    let mut args: Vec<String> = [
        "-m",
        &spec.path.to_string_lossy(),
        "--host",
        "127.0.0.1",
        "--port",
        &port.to_string(),
        "--api-key-file",
        &key_file.to_string_lossy(),
        // With a cache per slot, the context is split between them.
        "-c",
        &(spec.context * u32::from(slots)).to_string(),
        "-ctk",
        kv,
        "-ctv",
        kv,
        "--cache-reuse",
        "256",
        "--metrics",
        "--alias",
        &spec.alias,
        "--parallel",
        &slots.to_string(),
        "--jinja",
        "--no-webui",
    ]
    .iter()
    .map(|arg| (*arg).to_owned())
    .collect();
    if slots > 1 {
        args.push("--no-kv-unified".to_owned());
    }
    args
}

/// Starts llama-server for `spec` and waits until it serves. Its output
/// goes to a log in `logs`, whose end a failure quotes.
pub async fn launch(
    program: &Path,
    key_file: &Path,
    logs: &Path,
    spec: &Spec,
    key: &Secret,
    progress: &(dyn Fn(u8) + Send + Sync),
) -> Result<Backend, String> {
    let bytes = std::fs::metadata(&spec.path).map_or(0, |meta| meta.len());
    std::fs::create_dir_all(logs)
        .map_err(|err| format!("couldn't make {}: {err}", logs.display()))?;
    let port = free_port().map_err(|err| format!("couldn't find a free port: {err}"))?;
    let name: String = spec
        .alias
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let log_path = logs.join(format!("{name}.log"));
    let log = std::fs::File::create(&log_path)
        .map_err(|err| format!("couldn't open {}: {err}", log_path.display()))?;
    let err_log = log
        .try_clone()
        .map_err(|err| format!("couldn't open {}: {err}", log_path.display()))?;
    let mut command = tokio::process::Command::new(program);
    command
        .args(args(spec, port, key_file))
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(err_log)
        .kill_on_drop(true);
    // Settings in the environment would override nothing given here, but
    // one could add a listener or a model folder.
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("LLAMA_") {
            command.env_remove(name);
        }
    }
    let mut child = command
        .spawn()
        .map_err(|err| format!("couldn't start llama-server: {err}"))?;
    let pid_file = child.id().and_then(|pid| {
        let file = logs.join(format!("{name}.pid"));
        std::fs::write(&file, pid.to_string()).ok().map(|()| file)
    });
    let mut backend = Backend {
        port,
        child: None,
        pid_file,
    };
    let started = Instant::now();
    loop {
        if let Some(resident) = child.id().and_then(resident) {
            progress(percent_of(resident, bytes));
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "llama-server stopped while it loaded the model ({status}). It said:\n{}",
                tail(&log_path)
            ));
        }
        if healthy(port, key).await {
            break;
        }
        if started.elapsed() > LOAD_FOR {
            return Err("llama-server didn't load the model in 5 minutes.".to_owned());
        }
        tokio::time::sleep(POLL).await;
    }
    backend.child = Some(child);
    Ok(backend)
}

/// The GPU budget Metal gives, as `llama-server --list-devices` lists it.
pub async fn listed_budget(program: &Path) -> Option<GpuMemory> {
    let listed = tokio::time::timeout(
        Duration::from_secs(20),
        tokio::process::Command::new(program)
            .arg("--list-devices")
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&listed.stdout),
        String::from_utf8_lossy(&listed.stderr)
    );
    text.lines().find_map(hardware::metal_budget)
}

/// How much of a model file of `bytes` is in memory, from what the process
/// holds, in percent. It stays below 100, since the server loads more than
/// the file: what's left shows once it serves.
fn percent_of(resident: u64, bytes: u64) -> u8 {
    if bytes == 0 {
        return 0;
    }
    let share = u128::from(resident) * 100 / u128::from(bytes);
    u8::try_from(share.min(99)).unwrap_or(99)
}

/// The memory a process holds, in bytes.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn resident(pid: u32) -> Option<u64> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_taskinfo>()).ok()?;
    let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::uninit();
    // SAFETY: `info` is a buffer of `size` bytes for `proc_taskinfo`, which
    // `proc_pidinfo` fills when it returns that size.
    let info = unsafe {
        if libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTASKINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        ) != size
        {
            return None;
        }
        info.assume_init()
    };
    Some(info.pti_resident_size)
}

/// The memory a process holds. Only a Mac runs a managed server.
#[cfg(not(target_os = "macos"))]
fn resident(_pid: u32) -> Option<u64> {
    None
}

/// Stops the llama-servers a `maxd` that ended without stopping them left
/// running, from the process files in `logs`.
pub fn stop_leftovers(logs: &Path) {
    let Ok(entries) = std::fs::read_dir(logs) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "pid") {
            continue;
        }
        let pid = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok());
        let _ = std::fs::remove_file(&path);
        let Some(pid) = pid else { continue };
        // Only a llama-server: the number may name another process now.
        let named = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "comm="])
            .output()
            .ok()
            .is_some_and(|out| {
                String::from_utf8_lossy(&out.stdout)
                    .trim()
                    .ends_with("llama-server")
            });
        if named {
            let _ = std::process::Command::new("kill")
                .arg(pid.to_string())
                .status();
        }
    }
}

fn free_port() -> io::Result<u16> {
    Ok(std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?
        .local_addr()?
        .port())
}

/// The last lines of a log.
fn tail(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(15)..].join("\n")
}

async fn healthy(port: u16, key: &Secret) -> bool {
    super::http()
        .get(format!("http://127.0.0.1:{port}/health"))
        .bearer_auth(key.expose())
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

async fn metrics(port: u16, key: &Secret) -> Option<Counters> {
    let response = super::http()
        .get(format!("http://127.0.0.1:{port}/metrics"))
        .bearer_auth(key.expose())
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    Some(parse_metrics(&response.text().await.ok()?))
}

/// Replays `prefix` with a one-token turn.
async fn warm(port: u16, key: &Secret, model: &str, prefix: Value) -> Result<(), String> {
    let mut body = json!({
        "model": model,
        "max_tokens": 1,
        "messages": [{"role": "user", "content": "Hi"}],
        "tools": prefix.get("tools").cloned().unwrap_or_default(),
    });
    if let Some(system) = prefix.get("system").filter(|system| !system.is_null()) {
        body["system"] = system.clone();
    }
    let response = super::http()
        .post(format!("http://127.0.0.1:{port}/v1/messages"))
        .bearer_auth(key.expose())
        .header(CONTENT_TYPE, "application/json")
        .body(body.to_string())
        .timeout(Duration::from_secs(300))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("it answered {}", response.status()))
    }
}

/// Sends a request on to the server on `port`, with the key.
async fn forward(
    port: u16,
    key: &Secret,
    parts: &hyper::http::request::Parts,
    body: Bytes,
) -> Result<Response<Incoming>, String> {
    let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        .await
        .map_err(|err| format!("couldn't reach llama-server: {err}"))?;
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|err| format!("couldn't reach llama-server: {err}"))?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let target = parts.uri.path_and_query().map_or("/", |path| path.as_str());
    let mut request = Request::builder().method(parts.method.clone()).uri(target);
    for (name, value) in &parts.headers {
        if !DROPPED.contains(&name.as_str()) {
            request = request.header(name, value);
        }
    }
    let request = request
        .header(HOST, format!("127.0.0.1:{port}"))
        .header(AUTHORIZATION, format!("Bearer {}", key.expose()))
        .header(CONTENT_LENGTH, body.len())
        .body(Full::new(body))
        .map_err(|err| err.to_string())?;
    let mut response = sender
        .send_request(request)
        .await
        .map_err(|err| format!("llama-server ended the request: {err}"))?;
    for name in ["connection", "keep-alive", "transfer-encoding"] {
        response.headers_mut().remove(HeaderName::from_static(name));
    }
    Ok(response)
}

fn reply(status: StatusCode, body: &Value) -> Response<Body> {
    let mut response = Response::new(
        Full::new(Bytes::from(body.to_string()))
            .map_err(|never| match never {})
            .boxed_unsync(),
    );
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

/// An error in the Messages API's shape.
fn error(status: StatusCode, kind: &str, message: &str) -> Response<Body> {
    reply(
        status,
        &json!({"type": "error", "error": {"type": kind, "message": message}}),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::local_models::tests::Fake;

    const KEY: &str = "test-key-0123456789";
    const MODEL: &str = "qwen3.6-35b-a3b@ud-q4_k_xl";

    /// A host whose servers are fakes on 127.0.0.1.
    struct Fakes {
        backend: Fake,
        starts: AtomicUsize,
        changes: AtomicUsize,
        busy: Mutex<bool>,
        fail: Mutex<Option<String>>,
        /// Stops at 40 percent this long before the load ends.
        halfway: Mutex<Option<Duration>>,
    }

    impl Host for Fakes {
        fn spec(&self, model: &str) -> Option<Spec> {
            (model == MODEL || model == "other@q4").then(|| Spec {
                alias: model.to_owned(),
                path: PathBuf::from("/models/m.gguf"),
                context: 32_768,
                slots: 1,
                kv: KvCacheType::F16,
            })
        }

        fn models(&self) -> Vec<String> {
            vec![MODEL.to_owned()]
        }

        fn start(
            &self,
            _spec: Spec,
            progress: LoadProgress,
        ) -> BoxFuture<'static, Result<Backend, String>> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            let port: u16 = self
                .backend
                .base
                .rsplit(':')
                .next()
                .unwrap()
                .parse()
                .unwrap();
            let fail = self.fail.lock().unwrap().clone();
            let halfway = *self.halfway.lock().unwrap();
            Box::pin(async move {
                if let Some(hold) = halfway {
                    progress(40);
                    tokio::time::sleep(hold).await;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
                match fail {
                    Some(reason) => Err(reason),
                    None => Ok(Backend {
                        port,
                        child: None,
                        pid_file: None,
                    }),
                }
            })
        }

        fn changed(&self) {
            self.changes.fetch_add(1, Ordering::SeqCst);
        }

        fn busy(&self) -> bool {
            *self.busy.lock().unwrap()
        }

        fn idle_minutes(&self) -> u16 {
            15
        }
    }

    async fn runner() -> (Arc<Runner>, Arc<Fakes>, String) {
        let backend = Fake::start().await;
        backend.json(
            "POST /v1/messages?beta=true",
            200,
            &json!({"type": "message", "content": [{"type": "text", "text": "hi"}]}),
        );
        let host = Arc::new(Fakes {
            backend,
            starts: AtomicUsize::new(0),
            changes: AtomicUsize::new(0),
            busy: Mutex::new(false),
            fail: Mutex::new(None),
            halfway: Mutex::new(None),
        });
        let runner = Runner::new(host.clone(), Secret::new(KEY.to_owned()));
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(runner.clone().serve(listener));
        (runner, host, base)
    }

    fn turn() -> String {
        json!({
            "model": MODEL,
            "max_tokens": 100,
            "system": [{"type": "text", "text": "You are Claude Code."}],
            "tools": [{"name": "Read", "input_schema": {"type": "object"}}],
            "messages": [{"role": "user", "content": "Hello"}],
        })
        .to_string()
    }

    #[tokio::test]
    async fn the_proxy_wants_its_key() {
        let (_runner, host, base) = runner().await;
        let client = reqwest::Client::new();
        let without = client
            .post(format!("{base}/v1/messages"))
            .body(turn())
            .send()
            .await
            .unwrap();
        assert_eq!(without.status(), 401);
        let wrong = client
            .post(format!("{base}/v1/messages"))
            .header("x-api-key", "nope")
            .body(turn())
            .send()
            .await
            .unwrap();
        assert_eq!(wrong.status(), 401);
        assert_eq!(host.starts.load(Ordering::SeqCst), 0, "nothing loads");
        let listed = client
            .get(format!("{base}/v1/models"))
            .bearer_auth(KEY)
            .send()
            .await
            .unwrap();
        let listed: Value = serde_json::from_str(&listed.text().await.unwrap()).unwrap();
        assert_eq!(listed["data"][0]["id"], MODEL);
    }

    #[tokio::test]
    async fn a_turn_loads_its_model_and_goes_through_with_the_key() {
        let (runner, host, base) = runner().await;
        let answer = reqwest::Client::new()
            .post(format!("{base}/v1/messages?beta=true"))
            .header("x-api-key", KEY)
            .header("anthropic-version", "2023-06-01")
            .body(turn())
            .send()
            .await
            .unwrap();
        assert_eq!(answer.status(), 200);
        let answer: Value = serde_json::from_str(&answer.text().await.unwrap()).unwrap();
        assert_eq!(answer["content"][0]["text"], "hi");
        assert_eq!(host.starts.load(Ordering::SeqCst), 1);
        assert!(matches!(
            runner.load_of(MODEL),
            ModelLoad::Loaded {
                slots: 1,
                context: 32_768,
                ..
            }
        ));
        let seen = host.backend.seen();
        let sent = seen.last().unwrap();
        assert_eq!(sent.path, "/v1/messages?beta=true");
        assert_eq!(
            sent.header("authorization"),
            Some(format!("Bearer {KEY}").as_str())
        );
        assert_eq!(sent.header("anthropic-version"), Some("2023-06-01"));
        assert_eq!(sent.header("x-api-key"), None);
        assert_eq!(sent.body, turn());

        // The next one finds it loaded.
        let again = reqwest::Client::new()
            .post(format!("{base}/v1/messages?beta=true"))
            .bearer_auth(KEY)
            .body(turn())
            .send()
            .await
            .unwrap();
        assert_eq!(again.status(), 200);
        assert_eq!(host.starts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn an_unknown_model_or_a_failed_load_says_why() {
        let (runner, host, base) = runner().await;
        let client = reqwest::Client::new();
        let unknown = client
            .post(format!("{base}/v1/messages"))
            .bearer_auth(KEY)
            .body(json!({"model": "nothing", "messages": []}).to_string())
            .send()
            .await
            .unwrap();
        assert_eq!(unknown.status(), 404);

        *host.fail.lock().unwrap() = Some("not enough memory".to_owned());
        let failed = client
            .post(format!("{base}/v1/messages?beta=true"))
            .bearer_auth(KEY)
            .body(turn())
            .send()
            .await
            .unwrap();
        assert_eq!(failed.status(), 503);
        assert!(failed.text().await.unwrap().contains("not enough memory"));
        assert_eq!(
            runner.load_of(MODEL),
            ModelLoad::Failed {
                reason: "not enough memory".to_owned()
            }
        );
        assert_eq!(host.starts.load(Ordering::SeqCst), 1, "tried once");
    }

    #[tokio::test]
    async fn a_load_you_ask_for_warms_the_last_prompt() {
        let (runner, host, base) = runner().await;
        host.backend.json(
            "POST /v1/messages",
            200,
            &json!({"type": "message", "content": []}),
        );
        reqwest::Client::new()
            .post(format!("{base}/v1/messages?beta=true"))
            .bearer_auth(KEY)
            .body(turn())
            .send()
            .await
            .unwrap();
        runner.unload(MODEL);
        assert_eq!(runner.load_of(MODEL), ModelLoad::Unloaded);
        runner.load(MODEL).await.unwrap();
        assert_eq!(host.starts.load(Ordering::SeqCst), 2);
        let seen = host.backend.seen();
        let warm = seen.last().unwrap();
        assert_eq!(warm.path, "/v1/messages");
        let warm: Value = serde_json::from_str(&warm.body).unwrap();
        assert_eq!(warm["max_tokens"], 1);
        assert_eq!(warm["tools"][0]["name"], "Read");
        assert_eq!(warm["system"][0]["text"], "You are Claude Code.");
    }

    #[tokio::test]
    async fn a_load_shows_how_far_it_got_until_it_serves() {
        let (runner, host, _base) = runner().await;
        *host.halfway.lock().unwrap() = Some(Duration::from_millis(400));
        let loading = tokio::spawn({
            let runner = runner.clone();
            async move { runner.ensure(MODEL).await }
        });
        let halfway = ModelLoad::Loading { percent: Some(40) };
        let deadline = Instant::now() + Duration::from_secs(5);
        while runner.load_of(MODEL) != halfway {
            assert!(Instant::now() < deadline, "never showed 40 percent");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let changes = host.changes.load(Ordering::SeqCst);
        // The same figure again changes nothing.
        runner.loading(MODEL, 40);
        assert_eq!(host.changes.load(Ordering::SeqCst), changes);
        loading.await.unwrap().unwrap();
        assert!(matches!(runner.load_of(MODEL), ModelLoad::Loaded { .. }));
        // Once it serves, a late figure doesn't take it back.
        runner.loading(MODEL, 70);
        assert!(matches!(runner.load_of(MODEL), ModelLoad::Loaded { .. }));
    }

    #[test]
    fn progress_is_the_share_of_the_file_in_memory_below_100() {
        assert_eq!(percent_of(0, 1_000), 0);
        assert_eq!(percent_of(500, 1_000), 50);
        assert_eq!(percent_of(1_000, 1_000), 99);
        assert_eq!(percent_of(5_000, 1_000), 99);
        assert_eq!(percent_of(500, 0), 0);
        assert_eq!(percent_of(u64::MAX, 1), 99);
    }

    #[tokio::test]
    async fn loading_one_model_unloads_the_idle_other() {
        let (runner, host, _base) = runner().await;
        runner.ensure("other@q4").await.unwrap();
        runner.ensure(MODEL).await.unwrap();
        assert_eq!(runner.load_of("other@q4"), ModelLoad::Unloaded);
        assert!(matches!(runner.load_of(MODEL), ModelLoad::Loaded { .. }));
        assert_eq!(host.starts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn an_idle_model_unloads_after_the_setting_unless_a_turn_runs() {
        let (runner, host, _base) = runner().await;
        runner.ensure(MODEL).await.unwrap();
        let start = Instant::now();
        let at = |minutes: u64| start + Duration::from_secs(minutes * 60);

        assert!(!runner.sweep(at(14)), "14 minutes stays");
        *host.busy.lock().unwrap() = true;
        assert!(!runner.sweep(at(16)), "a running turn keeps it");
        *host.busy.lock().unwrap() = false;
        assert!(
            !runner.sweep(at(30)),
            "the turn's end starts the count again"
        );
        assert!(runner.sweep(at(32)), "16 minutes after the turn it goes");
        assert_eq!(runner.load_of(MODEL), ModelLoad::Unloaded);
    }

    #[test]
    fn idle_counts_minutes_and_requests() {
        let then = Instant::now();
        let later = then + Duration::from_secs(16 * 60);
        assert!(idle(15, then, later, 0));
        assert!(!idle(15, then, later, 1), "a request in flight");
        assert!(!idle(0, then, later, 0), "0 keeps it loaded");
        assert!(!idle(15, then, then + Duration::from_secs(14 * 60), 0));
    }

    #[test]
    fn speeds_come_from_the_totals_that_moved() {
        let text = "# HELP x\nllamacpp:prompt_tokens_total 13016\nllamacpp:prompt_seconds_total 6.26\nllamacpp:tokens_predicted_total 100\nllamacpp:tokens_predicted_seconds_total 1.0\nllamacpp:requests_processing 0\n";
        let before = parse_metrics(text);
        assert!((before.prompt_seconds - 6.26).abs() < 1e-9);
        let after = Counters {
            prompt_tokens: before.prompt_tokens + 2_079.0,
            prompt_seconds: before.prompt_seconds + 1.0,
            ..before
        };
        let speed = rates(
            before,
            after,
            Speed {
                prompt: None,
                decode: Some(92.8),
            },
        );
        assert_eq!(speed.prompt, Some(2_079.0));
        assert_eq!(speed.decode, Some(92.8), "decode didn't move, so it stays");
    }

    #[test]
    fn llama_server_listens_on_loopback_with_the_settings() {
        let spec = Spec {
            alias: MODEL.to_owned(),
            path: PathBuf::from("/m/model.gguf"),
            context: 65_536,
            slots: 2,
            kv: KvCacheType::Q8_0,
        };
        let args = args(&spec, 4321, Path::new("/k/key"));
        let after = |flag: &str| {
            let at = args.iter().position(|arg| arg == flag).unwrap();
            args[at + 1].clone()
        };
        assert_eq!(after("--host"), "127.0.0.1");
        assert_eq!(after("--port"), "4321");
        assert_eq!(after("--api-key-file"), "/k/key");
        assert_eq!(after("-c"), "131072", "two slots of 64K");
        assert_eq!(after("-ctk"), "q8_0");
        assert_eq!(after("-ctv"), "q8_0");
        assert_eq!(after("--alias"), MODEL);
        assert_eq!(after("--parallel"), "2");
        assert!(args.iter().any(|arg| arg == "--metrics"));
        assert!(args.iter().any(|arg| arg == "--no-kv-unified"));
        assert!(!args
            .iter()
            .any(|arg| arg == "--models-dir" || arg == "--api-key"));

        let one = Spec { slots: 1, ..spec };
        let args = super::args(&one, 4321, Path::new("/k/key"));
        assert!(!args.iter().any(|arg| arg == "--no-kv-unified"));
    }

    #[test]
    fn a_second_slot_only_when_it_fits_an_m4_max_with_36_gb() {
        // Qwen3.6-35B-A3B: 10 full-attention layers of 2 KV heads by 256.
        let kv = KvShape {
            full_layers: 10,
            kv_heads: 2,
            head_dim: 256,
            swa_layers: 0,
            swa_window: 0,
        };
        let budget = hardware::budget(36 << 30, None).budget;
        assert_eq!(
            slots_for(22_000_000_000, &kv, 65_536, KvCacheType::F16, budget),
            2
        );
        assert_eq!(
            slots_for(25_500_000_000, &kv, 262_144, KvCacheType::F16, budget),
            1
        );
        assert_eq!(kv_bytes(&kv, 65_536, KvCacheType::F16), 1_342_177_280);
        assert_eq!(kv_bytes(&kv, 65_536, KvCacheType::Q8_0), 713_031_680);
    }
}
