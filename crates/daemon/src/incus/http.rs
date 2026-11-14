// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! HTTP over the Incus socket: one request, its whole reply, and the
//! envelope Incus wraps every reply in. Long calls come back as an
//! operation, which [`wait`] follows to its end.

use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Bytes;
use hyper::client::conn::http1;
use hyper::header::{self, HeaderName, HeaderValue};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use serde_json::Value;
use tokio::net::UnixStream;

use super::IncusError;

/// How long a plain request may take.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// How long one operation wait asks Incus to hold the request.
const WAIT_STEP: Duration = Duration::from_secs(30);

/// The most one reply may hold. An exec's log holds what a command in a
/// container printed, so a huge one stops here and this device's memory
/// holds.
pub(super) const MAX_REPLY: usize = 64 * 1024 * 1024;

/// One answer from Incus.
pub struct Reply {
    /// The HTTP status.
    pub status: StatusCode,
    /// The body, whole.
    pub body: Bytes,
}

/// The envelope around every Incus reply.
#[derive(Debug, Default, Deserialize)]
pub struct Envelope {
    /// `sync`, `async` or `error`.
    #[serde(rename = "type", default)]
    pub kind: String,
    /// For an async reply, the operation's path.
    #[serde(default)]
    pub operation: String,
    /// For an error, Incus's message.
    #[serde(default)]
    pub error: String,
    /// For an error, its HTTP status.
    #[serde(default)]
    pub error_code: u16,
    /// The payload.
    #[serde(default)]
    pub metadata: Value,
}

/// An operation, as Incus reports it.
#[derive(Debug, Default, Deserialize)]
pub struct Operation {
    /// The operation's ID.
    #[serde(default)]
    pub id: String,
    /// 103 while it runs, 200 once it succeeds, 400 or 401 once it fails.
    #[serde(default)]
    pub status_code: u16,
    /// What went wrong, for a failed one.
    #[serde(default)]
    pub err: String,
    /// The operation's own payload, such as the file descriptors' secrets
    /// for an exec, or its return code once it ends.
    #[serde(default)]
    pub metadata: Value,
}

impl Operation {
    /// True once the operation has ended, either way.
    pub fn done(&self) -> bool {
        self.status_code >= 200
    }

    /// The operation, or its error if it failed.
    pub fn succeeded(self) -> Result<Self, IncusError> {
        if self.status_code < 400 {
            return Ok(self);
        }
        let message = if self.err.is_empty() {
            "the operation failed".to_owned()
        } else {
            self.err
        };
        Err(IncusError::Api {
            status: self.status_code,
            message,
        })
    }
}

/// Sends one request with `body` and extra `headers`, and reads the whole
/// reply. Each request opens its own connection, which on a local socket
/// costs little and keeps a slow call from holding up the others.
pub async fn send(
    socket: &Path,
    method: Method,
    uri: &str,
    headers: &[(&str, String)],
    body: Bytes,
    timeout: Duration,
) -> Result<Reply, IncusError> {
    let exchange = async {
        let stream = UnixStream::connect(socket).await?;
        let (mut sender, connection) = http1::handshake(TokioIo::new(stream))
            .await
            .map_err(broken)?;
        tokio::spawn(async move {
            if let Err(error) = connection.await {
                tracing::debug!(%error, "Incus connection ended");
            }
        });
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, "incus");
        for (name, value) in headers {
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(broken)?;
            let value = HeaderValue::from_str(value).map_err(broken)?;
            request = request.header(name, value);
        }
        let request = request.body(Full::new(body)).map_err(broken)?;
        let response = sender.send_request(request).await.map_err(broken)?;
        let (parts, body) = response.into_parts();
        let body = Limited::new(body, MAX_REPLY)
            .collect()
            .await
            .map_err(broken)?
            .to_bytes();
        Ok(Reply {
            status: parts.status,
            body,
        })
    };
    match tokio::time::timeout(timeout, exchange).await {
        Ok(result) => result,
        Err(_) => Err(timed_out(timeout)),
    }
}

/// Sends a request with a JSON body, or none, and reads the envelope.
/// An error reply becomes [`IncusError::Api`].
pub async fn call(
    socket: &Path,
    method: Method,
    uri: &str,
    body: Option<&Value>,
) -> Result<Envelope, IncusError> {
    let (headers, body) = match body {
        Some(body) => (
            vec![("content-type", "application/json".to_owned())],
            Bytes::from(serde_json::to_vec(body).map_err(broken)?),
        ),
        None => (Vec::new(), Bytes::new()),
    };
    let reply = send(socket, method, uri, &headers, body, TIMEOUT).await?;
    envelope(&reply)
}

/// Reads a reply's envelope. An error reply, or any status of 400 or
/// more, becomes [`IncusError::Api`].
pub fn envelope(reply: &Reply) -> Result<Envelope, IncusError> {
    let envelope: Envelope = serde_json::from_slice(&reply.body).unwrap_or_default();
    if envelope.kind == "error" || reply.status.as_u16() >= 400 {
        let status = match envelope.error_code {
            0 => reply.status.as_u16(),
            code => code,
        };
        let message = if envelope.error.is_empty() {
            String::from_utf8_lossy(&reply.body).trim().to_owned()
        } else {
            envelope.error
        };
        return Err(IncusError::Api { status, message });
    }
    Ok(envelope)
}

/// The operation an async reply started, as it stood when Incus answered.
pub fn started(envelope: &Envelope) -> Result<Operation, IncusError> {
    if envelope.kind != "async" {
        return Err(broken("Incus answered at once where it should start an operation").into());
    }
    let mut operation: Operation =
        serde_json::from_value(envelope.metadata.clone()).unwrap_or_default();
    if operation.id.is_empty() {
        operation.id = operation_id(&envelope.operation).to_owned();
    }
    if operation.id.is_empty() {
        return Err(broken("Incus started an operation without an ID").into());
    }
    Ok(operation)
}

/// Waits for an operation to end, up to `limit`, and fails if it failed.
pub async fn wait(socket: &Path, id: &str, limit: Duration) -> Result<Operation, IncusError> {
    finished(socket, id, limit).await?.succeeded()
}

/// Waits for an operation to end, up to `limit`, either way.
pub async fn finished(socket: &Path, id: &str, limit: Duration) -> Result<Operation, IncusError> {
    let deadline = Instant::now() + limit;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(timed_out(limit));
        }
        let step = left.min(WAIT_STEP);
        let uri = format!(
            "/1.0/operations/{}/wait?timeout={}",
            escape(id),
            step.as_secs().max(1)
        );
        let reply = send(socket, Method::GET, &uri, &[], Bytes::new(), step + TIMEOUT).await?;
        let envelope = envelope(&reply)?;
        let operation: Operation = serde_json::from_value(envelope.metadata).map_err(broken)?;
        if operation.done() {
            return Ok(operation);
        }
    }
}

/// Sends a request that may start an operation and waits for it, up to
/// `limit`. A sync reply is done already.
pub async fn run(
    socket: &Path,
    method: Method,
    uri: &str,
    body: Option<&Value>,
    limit: Duration,
) -> Result<Value, IncusError> {
    let envelope = call(socket, method, uri, body).await?;
    if envelope.kind != "async" {
        return Ok(envelope.metadata);
    }
    let operation = started(&envelope)?;
    Ok(wait(socket, &operation.id, limit).await?.metadata)
}

/// The ID at the end of an operation's path, such as
/// `/1.0/operations/<id>`.
fn operation_id(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or_default()
}

/// Escapes a path segment or query value. Instance names and the paths
/// maxd writes are plain already, so this only guards against surprises.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The error for a call that ran past its time.
fn timed_out(limit: Duration) -> IncusError {
    IncusError::Io(io::Error::new(
        io::ErrorKind::TimedOut,
        format!("Incus didn't answer within {} seconds", limit.as_secs()),
    ))
}

/// Wraps an error that isn't a plain socket error as one, so
/// [`IncusError`] keeps its four kinds.
pub fn broken(error: impl ToString) -> io::Error {
    io::Error::other(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(status: u16, body: &str) -> Reply {
        Reply {
            status: StatusCode::from_u16(status).unwrap(),
            body: Bytes::from(body.to_owned()),
        }
    }

    #[test]
    fn reads_sync_async_and_error_replies() {
        let sync = envelope(&reply(
            200,
            r#"{"type":"sync","status_code":200,"metadata":{"a":1}}"#,
        ))
        .unwrap();
        assert_eq!(sync.metadata["a"], 1);

        let started_op = envelope(&reply(
            202,
            r#"{"type":"async","status_code":100,"operation":"/1.0/operations/abc",
               "metadata":{"id":"abc","status_code":103,"metadata":{"fds":{"0":"s0"}}}}"#,
        ))
        .unwrap();
        let operation = started(&started_op).unwrap();
        assert_eq!(operation.id, "abc");
        assert!(!operation.done());
        assert_eq!(operation.metadata["fds"]["0"], "s0");

        let error = envelope(&reply(
            404,
            r#"{"type":"error","error":"Instance not found","error_code":404}"#,
        ))
        .unwrap_err();
        assert!(matches!(
            error,
            IncusError::Api { status: 404, ref message } if message == "Instance not found"
        ));

        let plain = envelope(&reply(500, "boom")).unwrap_err();
        assert!(matches!(
            plain,
            IncusError::Api { status: 500, ref message } if message == "boom"
        ));
    }

    #[test]
    fn an_operation_without_metadata_takes_its_id_from_the_path() {
        let envelope = Envelope {
            kind: "async".into(),
            operation: "/1.0/operations/0193-x".into(),
            ..Envelope::default()
        };
        assert_eq!(started(&envelope).unwrap().id, "0193-x");
        let sync = Envelope {
            kind: "sync".into(),
            ..Envelope::default()
        };
        assert!(started(&sync).is_err());
    }

    #[test]
    fn escapes_what_a_uri_cant_carry() {
        assert_eq!(escape("/usr/local/lib/maxd"), "/usr/local/lib/maxd");
        assert_eq!(escape("a b&c"), "a%20b%26c");
    }
}
