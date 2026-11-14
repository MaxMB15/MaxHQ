// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The JSON-RPC lines `codex app-server` speaks on stdio. It leaves out the
//! `"jsonrpc"` field, and so does the driver.

use maxhq_protocol::time::Timestamp;
use serde_json::{json, Map, Value};

/// The oldest app-server the driver speaks to, as major, minor and patch.
pub const MIN_VERSION: (u64, u64, u64) = (0, 164, 0);

/// The error code for a method the driver doesn't handle.
pub const METHOD_NOT_FOUND: i64 = -32601;

/// One line from the app-server, sorted by what it is.
#[derive(Debug)]
pub enum Incoming {
    /// The answer to one of the driver's requests.
    Response {
        /// The request's ID, as the driver sent it.
        id: Value,
        /// The result, or the error.
        outcome: Result<Value, RpcError>,
    },
    /// A request the driver must answer.
    Request {
        /// The request's ID, to echo in the answer.
        id: Value,
        /// The method, such as `item/commandExecution/requestApproval`.
        method: String,
        /// Its parameters.
        params: Value,
    },
    /// A notification, which needs no answer.
    Notification {
        /// The method, such as `item/started`.
        method: String,
        /// Its parameters.
        params: Value,
        /// When the app-server sent it, from `emittedAtMs`.
        emitted: Option<Timestamp>,
    },
}

/// An error the app-server answered with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RpcError {
    /// The JSON-RPC code, such as -32600.
    pub code: i64,
    /// The message, for a person to read.
    pub message: String,
}

/// Sorts one line, or gives `None` for a line that isn't JSON-RPC.
pub fn parse(text: &str) -> Option<Incoming> {
    let mut value: Value = serde_json::from_str(text).ok()?;
    let message = value.as_object_mut()?;
    let id = message.remove("id");
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let params = message.remove("params").unwrap_or(Value::Null);
    match (id, method) {
        (Some(id), Some(method)) => Some(Incoming::Request { id, method, params }),
        (None, Some(method)) => Some(Incoming::Notification {
            method,
            params,
            emitted: message
                .get("emittedAtMs")
                .and_then(Value::as_u64)
                .map(Timestamp),
        }),
        (Some(id), None) => {
            let outcome = match message.remove("error") {
                Some(error) => Err(RpcError {
                    code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                    message: error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                }),
                None => Ok(message.remove("result").unwrap_or(Value::Null)),
            };
            Some(Incoming::Response { id, outcome })
        }
        (None, None) => None,
    }
}

/// How the holder keys a request: a number as its decimal digits, a string
/// as itself.
pub fn key(id: &Value) -> Option<String> {
    match id {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// The version in an `initialize` result's `userAgent`, which reads like
/// `maxhq/0.164.0 (Mac OS 26.5.1; arm64) vscode/3.22.7`. A suffix such as
/// `-alpha.1` is dropped.
pub fn version(user_agent: &str) -> Option<(u64, u64, u64)> {
    let (_, rest) = user_agent.split_once('/')?;
    let version = rest.split_whitespace().next()?;
    let core = version.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

/// Request IDs for one run of `maxd`. Each is the run's nonce, a dash and a
/// count, so a reply to a request from before a restart never matches a
/// new one.
#[derive(Debug)]
pub struct Ids {
    nonce: String,
    next: u64,
}

impl Ids {
    /// IDs under a nonce made from random bits.
    pub fn new() -> Self {
        let uuid = uuid::Uuid::now_v7().simple().to_string();
        // The last 12 digits of a v7 UUID are random.
        Self::with_nonce(&uuid[20..])
    }

    /// IDs under the given nonce.
    pub fn with_nonce(nonce: &str) -> Self {
        Self {
            nonce: nonce.to_owned(),
            next: 0,
        }
    }

    /// The next ID.
    pub fn next(&mut self) -> String {
        let id = format!("{}-{}", self.nonce, self.next);
        self.next += 1;
        id
    }

    /// Whether `id` came from this run.
    pub fn ours(&self, id: &str) -> bool {
        id.strip_prefix(&self.nonce)
            .and_then(|rest| rest.strip_prefix('-'))
            .is_some_and(|count| count.parse::<u64>().is_ok())
    }
}

/// A request line.
pub fn request(id: &str, method: &str, params: Value) -> String {
    json!({ "id": id, "method": method, "params": params }).to_string()
}

/// A notification line.
pub fn notification(method: &str) -> String {
    json!({ "method": method }).to_string()
}

/// A successful answer to a request.
pub fn response(id: &Value, result: Value) -> String {
    json!({ "id": id, "result": result }).to_string()
}

/// An error answer to a request.
pub fn error(id: &Value, code: i64, message: &str) -> String {
    json!({ "id": id, "error": { "code": code, "message": message } }).to_string()
}

/// A JSON object from pairs, leaving out the ones whose value is null.
pub fn object<const N: usize>(pairs: [(&str, Value); N]) -> Value {
    let map: Map<String, Value> = pairs
        .into_iter()
        .filter(|(_, value)| !value.is_null())
        .map(|(key, value)| (key.to_owned(), value))
        .collect();
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_sort_into_requests_notifications_and_responses() {
        let request = parse(r#"{"id":0,"method":"item/tool/call","params":{"a":1}}"#);
        assert!(
            matches!(request, Some(Incoming::Request { ref method, .. }) if method == "item/tool/call")
        );
        let note = parse(r#"{"method":"turn/started","params":{},"emittedAtMs":5}"#);
        assert!(matches!(
            note,
            Some(Incoming::Notification {
                emitted: Some(Timestamp(5)),
                ..
            })
        ));
        let failed = parse(r#"{"id":"a-1","error":{"code":-32600,"message":"no"}}"#);
        let Some(Incoming::Response { outcome, .. }) = failed else {
            panic!("not a response");
        };
        assert_eq!(
            outcome,
            Err(RpcError {
                code: -32600,
                message: "no".into()
            })
        );
        assert!(parse("not json").is_none());
        assert!(parse("{}").is_none());
    }

    #[test]
    fn keys_match_the_holder() {
        assert_eq!(key(&json!(7)).as_deref(), Some("7"));
        assert_eq!(key(&json!("abc")).as_deref(), Some("abc"));
        assert_eq!(key(&json!(null)), None);
    }

    #[test]
    fn versions_come_from_the_user_agent() {
        let agent = "maxhq/0.164.0 (Mac OS 26.5.1; arm64) vscode/3.22.7 (maxhq; 0.0.0)";
        assert_eq!(version(agent), Some((0, 164, 0)));
        assert_eq!(version("maxhq/0.165.2-alpha.1 (Linux)"), Some((0, 165, 2)));
        assert_eq!(version("maxhq/dev (Linux)"), None);
        assert_eq!(version("maxhq"), None);
    }

    #[test]
    fn ids_from_another_run_are_not_ours() {
        let mut ids = Ids::with_nonce("aaaa");
        assert_eq!(ids.next(), "aaaa-0");
        assert!(ids.ours("aaaa-0"));
        assert!(!ids.ours("bbbb-0"));
        assert!(!ids.ours("aaaa-x"));
        let fresh = Ids::new();
        assert_eq!(fresh.nonce.len(), 12);
        assert!(!fresh.ours("aaaa-0"));
    }
}
