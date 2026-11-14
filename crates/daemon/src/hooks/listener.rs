// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The webhook listener: HTTP/1.1 on 127.0.0.1, open while any hook is
//! on or any task watches GitHub. The Funnel puts each hook's path in
//! front of it. A request comes to `/maxhq/<task>/<token>`, or to
//! `/<task>/<token>` when the Funnel drops the prefix, and GitHub's
//! repository webhooks come to the doorbell's path and token the same
//! way. It serves so many connections at once, and every answer names its
//! outcome in a header.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::io;
use std::net::Ipv4Addr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::header::{HeaderValue, CONTENT_LENGTH, CONTENT_TYPE};
use hyper::{Method, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use maxhq_protocol::ids::TaskId;
use maxhq_protocol::tasks::{DeliveryOutcome, Task, Trigger};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

use super::{Doorbell, Request, Webhooks};
use crate::agents::{hash, same_hash};

/// The file under `maxd`'s data folder that keeps the listener's port.
pub const PORT_FILE: &str = "hooks-port";

/// The largest body a request may have.
pub const MAX_BODY: usize = 1024 * 1024;

/// The header that names what came of a request.
pub const OUTCOME_HEADER: &str = "x-maxhq-hook-outcome";

/// How long the listener waits to try a port again that it couldn't
/// bind.
const RETRY: Duration = Duration::from_secs(30);

/// How long a sender has to send a request's headers, and then a task
/// hook's body, so a slow one can't hold a connection open.
pub(super) const READ_FOR: Duration = Duration::from_millis(if cfg!(test) { 100 } else { 10_000 });

/// How long GitHub has to send a delivery's body to the doorbell. It sends
/// the body at once, so whoever else holds the doorbell's path can't take
/// the connections by sending one slowly.
pub(super) const RING_FOR: Duration = Duration::from_millis(if cfg!(test) { 25 } else { 2_000 });

/// How many connections the listener serves at once. Any more wait to be
/// taken until one ends.
pub const MAX_CONNECTIONS: usize = 64;

/// The listener's port: the one kept in `data`, or a free one picked
/// now and kept there, so the Funnel's paths go on reaching it.
pub fn port(data: &Path) -> io::Result<u16> {
    let file = data.join(PORT_FILE);
    match std::fs::read_to_string(&file) {
        Ok(text) => {
            if let Ok(port @ 1..) = text.trim().parse::<u16>() {
                return Ok(port);
            }
        }
        Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
        Err(_) => {}
    }
    let port = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?
        .local_addr()?
        .port();
    std::fs::write(&file, format!("{port}\n"))?;
    Ok(port)
}

/// Whether any of `tasks` has its hook on, or watches GitHub, whose
/// repository webhooks ring the doorbell.
fn any_on(tasks: &[Task]) -> bool {
    tasks.iter().any(|task| match &task.trigger {
        Trigger::Hook(hook) => hook.on,
        Trigger::GithubEvents { .. } => task.enabled,
        _ => false,
    })
}

/// Where a request goes.
enum Target {
    Task(Box<Task>),
    Doorbell(Arc<dyn Doorbell>),
}

/// Listens on `port` while any hook is on, and closes it while none is.
pub(super) async fn listen(hooks: Webhooks, port: u16) {
    let mut tasks = hooks.inner.tasks.watch();
    loop {
        if tasks.wait_for(|tasks| any_on(tasks)).await.is_err() {
            return;
        }
        let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await {
            Ok(listener) => listener,
            Err(err) => {
                tracing::warn!(%err, port, "couldn't open the webhook listener");
                tokio::time::sleep(RETRY).await;
                continue;
            }
        };
        let serving = tokio::spawn(serve(hooks.clone(), listener));
        let ended = tasks.wait_for(|tasks| !any_on(tasks)).await.is_err();
        serving.abort();
        if ended {
            return;
        }
    }
}

/// Answers every connection `listener` takes, [`MAX_CONNECTIONS`] at a
/// time.
pub(super) async fn serve(hooks: Webhooks, listener: TcpListener) {
    let open = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        let Ok(permit) = open.clone().acquire_owned().await else {
            return;
        };
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(err) => {
                tracing::debug!(%err, "a webhook connection failed");
                continue;
            }
        };
        let hooks = hooks.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let service = hyper::service::service_fn(move |request| {
                let hooks = hooks.clone();
                async move { Ok::<_, Infallible>(handle(&hooks, request).await) }
            });
            // Senders open a connection for each delivery, so one that stays
            // would only keep a place from the next.
            let served = hyper::server::conn::http1::Builder::new()
                .keep_alive(false)
                .timer(TokioTimer::new())
                .header_read_timeout(READ_FOR)
                .serve_connection(TokioIo::new(stream), service)
                .await;
            if let Err(err) = served {
                tracing::debug!(%err, "a webhook connection ended early");
            }
        });
    }
}

/// Answers one request.
async fn handle(hooks: &Webhooks, request: hyper::Request<Incoming>) -> Response<Full<Bytes>> {
    if request.method() != Method::POST {
        return refuse(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed");
    }
    let too_large = || refuse(StatusCode::PAYLOAD_TOO_LARGE, "body_too_large");
    let length = request.headers().get(CONTENT_LENGTH);
    if length.is_some_and(|length| {
        length
            .to_str()
            .ok()
            .and_then(|length| length.parse::<usize>().ok())
            .is_none_or(|length| length > MAX_BODY)
    }) {
        return too_large();
    }
    let path = request.uri().path();
    let target = match hooks.doorbell().filter(|bell| bell.rings(path)) {
        Some(bell) => Target::Doorbell(bell),
        None => match find(hooks, path) {
            Some(task) => Target::Task(Box::new(task)),
            None => return refuse(StatusCode::NOT_FOUND, "not_found"),
        },
    };
    let query = request.uri().query().unwrap_or_default().to_owned();
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    for (name, value) in request.headers() {
        let Ok(value) = value.to_str() else { continue };
        headers
            .entry(name.as_str().to_owned())
            .and_modify(|joined| {
                joined.push_str(", ");
                joined.push_str(value);
            })
            .or_insert_with(|| value.to_owned());
    }
    if let Target::Doorbell(bell) = &target
        && let Err((status, outcome)) = bell.admit(&headers)
    {
        return refuse(status, outcome);
    }
    // Chunked bodies have no length, so the read is capped too.
    let body = Limited::new(request.into_body(), MAX_BODY).collect();
    let read_for = match target {
        Target::Task(_) => READ_FOR,
        Target::Doorbell(_) => RING_FOR,
    };
    let body = match tokio::time::timeout(read_for, body).await {
        Ok(Ok(body)) => body,
        Ok(Err(_)) => return too_large(),
        Err(_) => return refuse(StatusCode::REQUEST_TIMEOUT, "request_timeout"),
    };
    let body = body.to_bytes().to_vec();
    let task = match target {
        Target::Task(task) => *task,
        Target::Doorbell(bell) => {
            let (status, outcome) = bell.ring(headers, body).await;
            if status.is_success() {
                return reply(status, serde_json::json!({}), outcome);
            }
            return refuse(status, outcome);
        }
    };
    let request = Request {
        delivery: uuid::Uuid::now_v7().to_string(),
        received: hooks.now(),
        headers,
        query,
        body,
    };
    let delivery = request.delivery.clone();
    let outcome = hooks.check(&task, request).await;
    answer(outcome, &delivery)
}

/// The task a path names, when its hook is on and the path holds its
/// token. Anything else is the same not found.
fn find(hooks: &Webhooks, path: &str) -> Option<Task> {
    let path = path
        .strip_prefix("/maxhq")
        .filter(|rest| rest.starts_with('/'))
        .unwrap_or(path);
    let (task, token) = path.strip_prefix('/')?.split_once('/')?;
    if token.is_empty() || token.contains('/') {
        return None;
    }
    let id: TaskId = task.parse().ok()?;
    let task = hooks
        .task(id)
        .filter(|task| matches!(&task.trigger, Trigger::Hook(hook) if hook.on))?;
    let stored = hooks.inner.db.hook_token(id).ok()??;
    same_hash(&hash(token), &stored).then_some(task)
}

/// The answer to a request that reached its task.
fn answer(outcome: DeliveryOutcome, delivery: &str) -> Response<Full<Bytes>> {
    let name = serde_json::to_value(outcome)
        .ok()
        .and_then(|name| name.as_str().map(str::to_owned))
        .unwrap_or_default();
    let (status, error) = match outcome {
        DeliveryOutcome::Accepted
        | DeliveryOutcome::Duplicate
        | DeliveryOutcome::Coalesced
        | DeliveryOutcome::Filtered
        | DeliveryOutcome::PromptTooLong
        | DeliveryOutcome::Held => {
            let body = serde_json::json!({ "deliveryId": delivery });
            return reply(StatusCode::ACCEPTED, body, &name);
        }
        DeliveryOutcome::RejectedSignature => (StatusCode::UNAUTHORIZED, "invalid_signature"),
        DeliveryOutcome::Disabled => (StatusCode::CONFLICT, "hook_disabled"),
        DeliveryOutcome::Expired => (StatusCode::GONE, "delivery_too_old"),
        DeliveryOutcome::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
        DeliveryOutcome::QueueFull => (StatusCode::TOO_MANY_REQUESTS, "queue_full"),
        DeliveryOutcome::TargetOffline => (StatusCode::SERVICE_UNAVAILABLE, "target_offline"),
        DeliveryOutcome::DispatchFailed => (StatusCode::INTERNAL_SERVER_ERROR, "dispatch_failed"),
    };
    reply(status, serde_json::json!({ "error": error }), &name)
}

/// An answer to a request that didn't reach a task, named `outcome`.
fn refuse(status: StatusCode, outcome: &str) -> Response<Full<Bytes>> {
    reply(status, serde_json::json!({ "error": outcome }), outcome)
}

fn reply(status: StatusCode, body: serde_json::Value, outcome: &str) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(body.to_string())));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    if let Ok(outcome) = HeaderValue::from_str(outcome) {
        headers.insert(OUTCOME_HEADER, outcome);
    }
    response
}
