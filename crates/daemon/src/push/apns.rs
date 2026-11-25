// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Sends a push to APNs over HTTP/2, signed with the device's provider
//! token. A request's URL holds the phone's token, so no error text here
//! names it.

use maxhq_protocol::phones::{PushEnvironment, PushToken};
use maxhq_protocol::time::Timestamp;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Client, StatusCode};
use serde::Deserialize;

use super::Sent;
use crate::secrets::http::{describe, http2_client};
use crate::secrets::ApnsKey;

/// APNs for TestFlight and App Store builds.
const PRODUCTION: &str = "https://api.push.apple.com";
/// APNs for development builds.
const SANDBOX: &str = "https://api.sandbox.push.apple.com";

/// One push: what the phone gets, and its collapse ID.
pub struct Request<'a> {
    pub payload: &'a str,
    pub collapse: &'a str,
}

/// The client.
pub struct Apns {
    http: Client,
    /// The host every push goes to, for the tests' fake. Otherwise each
    /// token's environment picks Apple's host.
    host: Option<String>,
}

/// APNs's error body.
#[derive(Deserialize)]
struct Reason {
    reason: String,
}

impl Apns {
    /// A client for Apple's hosts.
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            http: http2_client(true)?,
            host: None,
        })
    }

    /// A client for the fake at `host`.
    #[cfg(test)]
    pub fn at(host: String) -> Self {
        Self {
            http: http2_client(false).unwrap(),
            host: Some(host),
        }
    }

    /// Sends `request` to the phone `to` names, with a provider token from
    /// `key` for `now`.
    pub async fn send(
        &self,
        key: &ApnsKey,
        now: Timestamp,
        to: &PushToken,
        request: Request<'_>,
    ) -> Sent {
        let jwt = match key.token(now) {
            Ok(jwt) => jwt,
            Err(reason) => return Sent::Failed(reason.to_owned()),
        };
        let host = self.host.as_deref().unwrap_or(match to.environment {
            PushEnvironment::Production => PRODUCTION,
            PushEnvironment::Sandbox => SANDBOX,
        });
        let url = format!("{host}/3/device/{}", to.token.expose());
        let sent = self
            .http
            .post(url)
            .header(AUTHORIZATION, format!("bearer {}", jwt.expose()))
            .header(CONTENT_TYPE, "application/json")
            .header("apns-topic", &to.topic)
            .header("apns-push-type", "alert")
            .header("apns-priority", "10")
            .header("apns-collapse-id", request.collapse)
            .body(request.payload.to_owned())
            .send()
            .await;
        let response = match sent {
            Ok(response) => response,
            Err(err) => return Sent::Failed(describe(&err.without_url())),
        };
        let status = response.status();
        if status.is_success() {
            return Sent::Done;
        }
        let body = response.bytes().await.unwrap_or_default();
        let reason = serde_json::from_slice::<Reason>(&body)
            .map_or_else(|_| status.to_string(), |r| r.reason);
        match (status, reason.as_str()) {
            (StatusCode::GONE, _) | (_, "Unregistered") => Sent::Gone,
            (StatusCode::FORBIDDEN, "ExpiredProviderToken") => Sent::Expired,
            _ => Sent::Failed(format!("APNs said {reason}")),
        }
    }
}
