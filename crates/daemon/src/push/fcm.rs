// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Sends a push to an Android phone through FCM's HTTP v1 API, with an
//! OAuth access token from the device's Firebase key. The message is data
//! only, at high priority so it wakes the phone: the app opens the sealed
//! part and shows it itself, as the iPhone's extension does. A request's
//! body holds the phone's token, so no error text here names it.

use std::time::Duration;

use maxhq_protocol::phones::{PushKind, PushToken};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::Secret;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::Sent;
use crate::secrets::fcm::TOKEN_URL;
use crate::secrets::http::{client, describe, form_post};
use crate::secrets::FcmKey;

/// FCM's host.
const HOST: &str = "https://fcm.googleapis.com";
/// The JWT bearer grant, form-encoded.
const GRANT: &str = "urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer";
/// How long FCM keeps a push for a phone that's off. A prompt an hour old
/// is stale, and the next state change pushes again.
pub const TTL: Duration = Duration::from_secs(60 * 60);

/// What an Android phone gets: FCM's data map, every value a string.
#[derive(Debug, Serialize)]
pub struct Data {
    /// The sealed part, base64, as APNs's `c`.
    pub c: String,
    /// The prompt it's about, as APNs's `e`, for a prompt only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub e: Option<String>,
    /// The agent, which groups its pushes, as APNs's `thread-id`.
    pub t: String,
    /// The push's kind, which picks the phone's channel.
    pub k: PushKind,
    /// `permission` for a request the push may deny, as APNs's category.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub a: Option<&'static str>,
    /// What the push is about, stable so a reminder replaces the first
    /// push, as APNs's collapse ID. It's also the message's collapse key.
    pub n: String,
}

/// The client.
pub struct Fcm {
    http: Client,
    /// Google's token endpoint, or the tests' fake.
    token_url: String,
    /// FCM's host, or the tests' fake.
    host: String,
}

/// The token endpoint's answer.
#[derive(Deserialize)]
struct Granted {
    access_token: Secret,
    expires_in: u64,
}

/// The token endpoint's error.
#[derive(Deserialize)]
struct Denied {
    error: String,
    #[serde(default)]
    error_description: String,
}

/// FCM's error body.
#[derive(Deserialize)]
struct Failure {
    error: FailureBody,
}

#[derive(Deserialize)]
struct FailureBody {
    #[serde(default)]
    status: String,
    #[serde(default)]
    details: Vec<Detail>,
}

#[derive(Deserialize)]
struct Detail {
    #[serde(default, rename = "errorCode")]
    error_code: String,
}

impl Fcm {
    /// A client for Google's hosts.
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            http: client(true)?,
            token_url: TOKEN_URL.to_owned(),
            host: HOST.to_owned(),
        })
    }

    /// A client for the fake at `host`, whose token endpoint is `/token`.
    #[cfg(test)]
    pub fn at(host: &str) -> Self {
        Self {
            http: client(false).unwrap(),
            token_url: format!("{host}/token"),
            host: host.to_owned(),
        }
    }

    /// Sends `data` to the phone `to` names, with an access token from
    /// `key` for `now`.
    pub async fn send(&self, key: &FcmKey, now: Timestamp, to: &PushToken, data: &Data) -> Sent {
        let access = match self.access(key, now).await {
            Ok(access) => access,
            Err(sent) => return sent,
        };
        let url = format!(
            "{}/v1/projects/{}/messages:send",
            self.host,
            key.project_id()
        );
        let sent = self
            .http
            .post(url)
            .header(AUTHORIZATION, format!("Bearer {}", access.expose()))
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json")
            .body(body(to, data))
            .send()
            .await;
        let response = match sent {
            Ok(response) => response,
            Err(err) => return Sent::Failed(describe(&err.without_url())),
        };
        let status = response.status();
        let bytes = response.bytes().await.unwrap_or_default();
        answered(status, &bytes)
    }

    /// The access token `key` holds, or a new one from Google.
    async fn access(&self, key: &FcmKey, now: Timestamp) -> Result<Secret, Sent> {
        if let Some(access) = key.access(now) {
            return Ok(access);
        }
        let assertion = key
            .assertion(now)
            .map_err(|reason| Sent::Failed(reason.to_owned()))?;
        let form = Secret::new(format!(
            "grant_type={GRANT}&assertion={}",
            assertion.expose()
        ));
        let sent = form_post(&self.http, &self.token_url, form.expose())
            .send()
            .await;
        let response = match sent {
            Ok(response) => response,
            Err(err) => return Err(Sent::Failed(describe(&err.without_url()))),
        };
        let status = response.status();
        let bytes = response.bytes().await.unwrap_or_default();
        if !status.is_success() {
            let reason = serde_json::from_slice::<Denied>(&bytes).map_or_else(
                |_| status.to_string(),
                |denied| format!("{}: {}", denied.error, denied.error_description),
            );
            return Err(Sent::Failed(format!(
                "Google refused the Firebase key: {reason}"
            )));
        }
        let Ok(granted) = serde_json::from_slice::<Granted>(&bytes) else {
            return Err(Sent::Failed(
                "Google's answer had no access token in it".into(),
            ));
        };
        key.keep(granted.access_token.clone(), now, granted.expires_in);
        Ok(granted.access_token)
    }
}

/// The request FCM takes: the message to `to`, data only, high priority,
/// collapsing on what it's about and kept for [`TTL`].
pub fn body(to: &PushToken, data: &Data) -> String {
    json!({
        "message": {
            "token": to.token.expose(),
            "data": data,
            "android": {
                "priority": "high",
                "collapse_key": data.n,
                "ttl": format!("{}s", TTL.as_secs()),
            },
        },
    })
    .to_string()
}

/// What FCM's answer means. A token it doesn't know any more is gone, and
/// a refused access token gets one new one.
fn answered(status: StatusCode, body: &[u8]) -> Sent {
    if status.is_success() {
        return Sent::Done;
    }
    let failure = serde_json::from_slice::<Failure>(body).ok();
    let code = failure
        .as_ref()
        .and_then(|failure| {
            failure
                .error
                .details
                .iter()
                .find(|d| !d.error_code.is_empty())
        })
        .map(|detail| detail.error_code.as_str());
    let named = failure
        .as_ref()
        .map(|failure| failure.error.status.as_str());
    match (status, code, named) {
        (_, Some("UNREGISTERED"), _) | (_, _, Some("NOT_FOUND")) | (StatusCode::NOT_FOUND, ..) => {
            Sent::Gone
        }
        (StatusCode::UNAUTHORIZED, ..) | (_, _, Some("UNAUTHENTICATED")) => Sent::Expired,
        (_, Some(code), _) => Sent::Failed(format!("FCM said {code}")),
        (_, None, Some(named)) if !named.is_empty() => Sent::Failed(format!("FCM said {named}")),
        _ => Sent::Failed(format!("FCM said {status}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(status: &str, code: Option<&str>) -> Vec<u8> {
        let details = code.map_or_else(Vec::new, |code| {
            vec![json!({
                "@type": "type.googleapis.com/google.firebase.fcm.v1.FcmError",
                "errorCode": code,
            })]
        });
        json!({ "error": { "code": 0, "message": "test", "status": status, "details": details } })
            .to_string()
            .into_bytes()
    }

    #[test]
    fn fcm_answers_map_to_what_the_send_path_does() {
        assert_eq!(answered(StatusCode::OK, b"{}"), Sent::Done);
        assert_eq!(
            answered(
                StatusCode::NOT_FOUND,
                &failure("NOT_FOUND", Some("UNREGISTERED"))
            ),
            Sent::Gone
        );
        assert_eq!(
            answered(StatusCode::NOT_FOUND, &failure("NOT_FOUND", None)),
            Sent::Gone
        );
        assert_eq!(answered(StatusCode::NOT_FOUND, b""), Sent::Gone);
        assert_eq!(
            answered(
                StatusCode::BAD_REQUEST,
                &failure("INVALID_ARGUMENT", Some("UNREGISTERED"))
            ),
            Sent::Gone,
            "the code counts whatever the status"
        );
        assert_eq!(
            answered(StatusCode::UNAUTHORIZED, &failure("UNAUTHENTICATED", None)),
            Sent::Expired
        );
        assert_eq!(answered(StatusCode::UNAUTHORIZED, b""), Sent::Expired);
        assert_eq!(
            answered(
                StatusCode::FORBIDDEN,
                &failure("PERMISSION_DENIED", Some("SENDER_ID_MISMATCH"))
            ),
            Sent::Failed("FCM said SENDER_ID_MISMATCH".into())
        );
        assert_eq!(
            answered(
                StatusCode::TOO_MANY_REQUESTS,
                &failure("RESOURCE_EXHAUSTED", None)
            ),
            Sent::Failed("FCM said RESOURCE_EXHAUSTED".into())
        );
        assert_eq!(
            answered(StatusCode::SERVICE_UNAVAILABLE, b"<html>"),
            Sent::Failed("FCM said 503 Service Unavailable".into())
        );
    }

    #[test]
    fn a_message_is_data_only_at_high_priority() {
        let to = PushToken {
            token: Secret::new("test-not-a-real:token".into()),
            topic: "com.example.maxhq".into(),
            environment: maxhq_protocol::phones::PushEnvironment::Production,
            kinds: vec![PushKind::NeedsInput],
            service: maxhq_protocol::phones::PushService::Fcm,
        };
        let data = Data {
            c: "c2VhbGVk".into(),
            e: Some("prompt".into()),
            t: "agent".into(),
            k: PushKind::NeedsInput,
            a: Some("permission"),
            n: "needs-input:prompt".into(),
        };
        let value: serde_json::Value = serde_json::from_str(&body(&to, &data)).unwrap();
        assert_eq!(
            value,
            json!({
                "message": {
                    "token": "test-not-a-real:token",
                    "data": {
                        "c": "c2VhbGVk",
                        "e": "prompt",
                        "t": "agent",
                        "k": "needs_input",
                        "a": "permission",
                        "n": "needs-input:prompt",
                    },
                    "android": {
                        "priority": "high",
                        "collapse_key": "needs-input:prompt",
                        "ttl": "3600s",
                    },
                },
            })
        );
        let plain = Data {
            e: None,
            a: None,
            k: PushKind::Finished,
            ..data
        };
        let value: serde_json::Value = serde_json::from_str(&body(&to, &plain)).unwrap();
        let data = value["message"]["data"].as_object().unwrap();
        assert!(!data.contains_key("e") && !data.contains_key("a"));
        assert_eq!(data["k"], json!("finished"));
    }
}
