// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the discovery
// check, the token endpoint's errors and which of them end a session, and
// the revocation's retries, translated to Rust.

//! ChatGPT's OAuth server: its discovery document, the token endpoint for
//! a sign-in's code and each renewal, and revocation. Each call goes out
//! on the store's [`http`] client, and no error carries what was sent.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use maxhq_protocol::Secret;
use reqwest::header::ACCEPT;
use reqwest::{Client, RequestBuilder};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use url::Url;
use zeroize::Zeroizing;

use super::id_token::Jwks;
use crate::secrets::http::{self, describe, form_post};

/// OpenAI's sign-in server.
pub const OPENAI_ISSUER: &str = "https://auth.openai.com";
/// Where the issuer keeps its discovery document.
const DISCOVERY: &str = "/.well-known/openid-configuration";
/// The API the tokens are for.
pub const RESOURCE: &str = "https://api.openai.com/v1";
/// How long a renewal may take.
const REFRESH_TIMEOUT: Duration = Duration::from_secs(20);
/// How long each revocation attempt may take.
const REVOKE_TIMEOUT: Duration = Duration::from_secs(10);
/// The waits between revocation attempts, so three attempts in all.
const REVOKE_WAITS: [Duration; 2] = [Duration::from_millis(250), Duration::from_secs(1)];
/// The errors on a renewal that mean the session is gone for good.
const ENDED: [&str; 6] = [
    "invalid_grant",
    "invalid_refresh_token",
    "token_expired",
    "refresh_token_expired",
    "refresh_token_invalidated",
    "refresh_token_reused",
];

/// The issuer, and its endpoints once read.
pub struct Issuer {
    http: Result<Client, String>,
    issuer: String,
    endpoints: Mutex<Option<Arc<Endpoints>>>,
}

/// What the discovery document names.
#[derive(Deserialize)]
pub struct Endpoints {
    /// The issuer, which identity tokens name.
    pub issuer: String,
    /// The page a sign-in opens.
    pub authorization_endpoint: String,
    /// Where codes and refresh tokens go.
    pub token_endpoint: String,
    /// The keys identity tokens are signed with.
    pub jwks_uri: String,
    /// Where sign-out revokes a session.
    #[serde(default)]
    pub revocation_endpoint: Option<String>,
}

/// What the token endpoint gives.
#[derive(Deserialize)]
pub struct Tokens {
    pub access_token: Secret,
    #[serde(default)]
    pub refresh_token: Option<Secret>,
    #[serde(default)]
    pub id_token: Option<Secret>,
    pub token_type: String,
    /// Seconds until the access token stops working.
    pub expires_in: u64,
    /// The granted scopes, split by spaces.
    #[serde(default)]
    pub scope: String,
    /// The soonest a renewal is taken, in Unix seconds or ISO 8601.
    #[serde(default)]
    pub earliest_refresh_at: Option<Value>,
}

/// What goes to the token endpoint.
pub enum Grant<'a> {
    /// A sign-in's code, with the round's verifier and callback.
    Code {
        client_id: &'a str,
        code: &'a Secret,
        verifier: &'a Secret,
        redirect_uri: &'a str,
    },
    /// A renewal.
    Refresh {
        client_id: &'a str,
        refresh_token: &'a Secret,
    },
}

/// Why the token endpoint gave nothing.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TokenError {
    /// It couldn't be reached, or failed on its side.
    #[error("OpenAI's sign-in server couldn't be reached: {0}")]
    Unreachable(String),
    /// The session was revoked or ran out.
    #[error("the session ended")]
    Ended,
    /// The device's client registration was refused.
    #[error("the client registration was refused")]
    BadClient,
    /// The sign-in's code was used or ran out.
    #[error("the code ran out")]
    CodeExpired,
    /// You declined the sign-in.
    #[error("the sign-in was declined")]
    Declined,
    /// Any other OAuth error, by its code.
    #[error("the server refused with {0}")]
    Refused(String),
    /// What came back wasn't a token response.
    #[error("the server's reply wasn't a token response")]
    BadReply,
}

/// An OAuth error body.
#[derive(Deserialize)]
struct OAuthError {
    error: String,
}

impl Issuer {
    /// The issuer at `issuer`, through the proxy the environment names.
    pub fn new(issuer: &str) -> Self {
        Self::build(issuer, true)
    }

    /// A test server, reached directly.
    #[cfg(test)]
    pub fn local(issuer: &str) -> Self {
        Self::build(issuer, false)
    }

    fn build(issuer: &str, proxies: bool) -> Self {
        Self {
            http: http::client(proxies),
            issuer: issuer.trim_end_matches('/').to_owned(),
            endpoints: Mutex::new(None),
        }
    }

    fn http(&self) -> Result<&Client, String> {
        self.http.as_ref().map_err(Clone::clone)
    }

    /// The issuer's endpoints, read once. A document that names another
    /// issuer, or an endpoint off the issuer's origin, is refused.
    pub async fn endpoints(&self) -> Result<Arc<Endpoints>, String> {
        if let Some(endpoints) = self.cached() {
            return Ok(endpoints);
        }
        let request = self.http()?.get(format!("{}{DISCOVERY}", self.issuer));
        let endpoints: Endpoints = get_json(request).await?;
        let mut urls = vec![
            &endpoints.authorization_endpoint,
            &endpoints.token_endpoint,
            &endpoints.jwks_uri,
        ];
        urls.extend(&endpoints.revocation_endpoint);
        let on_issuer = |url: &String| {
            Url::parse(url).is_ok_and(|url| url.origin().ascii_serialization() == self.issuer)
        };
        if endpoints.issuer != self.issuer || !urls.into_iter().all(on_issuer) {
            return Err("its discovery document names another server".to_owned());
        }
        let endpoints = Arc::new(endpoints);
        *self
            .endpoints
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(endpoints.clone());
        Ok(endpoints)
    }

    fn cached(&self) -> Option<Arc<Endpoints>> {
        self.endpoints
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The keys identity tokens are signed with.
    pub async fn keys(&self, endpoints: &Endpoints) -> Result<Jwks, String> {
        get_json(self.http()?.get(&endpoints.jwks_uri)).await
    }

    /// Trades a code or a refresh token for tokens.
    pub async fn tokens(
        &self,
        endpoints: &Endpoints,
        grant: Grant<'_>,
    ) -> Result<Tokens, TokenError> {
        let refresh = matches!(grant, Grant::Refresh { .. });
        let body = Zeroizing::new(form_body(grant));
        let http = self.http().map_err(TokenError::Unreachable)?;
        let mut request = form_post(http, &endpoints.token_endpoint, &body);
        if refresh {
            request = request.timeout(REFRESH_TIMEOUT);
        }
        let response = request
            .send()
            .await
            .map_err(|err| TokenError::Unreachable(describe(&err)))?;
        let status = response.status();
        let reply = response
            .bytes()
            .await
            .map_err(|err| TokenError::Unreachable(describe(&err)))?;
        if status.is_server_error() {
            return Err(TokenError::Unreachable(format!("it answered {status}")));
        }
        if !status.is_success() {
            let error = serde_json::from_slice::<OAuthError>(&reply)
                .map_err(|_| TokenError::BadReply)?
                .error;
            return Err(match error.as_str() {
                code if refresh && ENDED.contains(&code) => TokenError::Ended,
                "invalid_client" => TokenError::BadClient,
                "invalid_grant" => TokenError::CodeExpired,
                "access_denied" => TokenError::Declined,
                _ => TokenError::Refused(error.chars().take(60).collect()),
            });
        }
        serde_json::from_slice(&reply).map_err(|_| TokenError::BadReply)
    }

    /// Revokes a refresh token, trying three times while the server can't
    /// be reached or fails on its side. Returns whether it confirmed.
    pub async fn revoke(&self, client_id: &str, refresh_token: &Secret) -> bool {
        let Ok(endpoints) = self.endpoints().await else {
            return false;
        };
        let (Some(endpoint), Ok(http)) = (&endpoints.revocation_endpoint, self.http()) else {
            return false;
        };
        let body = Zeroizing::new(
            url::form_urlencoded::Serializer::new(String::new())
                .append_pair("token", refresh_token.expose())
                .append_pair("token_type_hint", "refresh_token")
                .append_pair("client_id", client_id)
                .finish(),
        );
        for attempt in 0..=REVOKE_WAITS.len() {
            let sent = form_post(http, endpoint, &body)
                .timeout(REVOKE_TIMEOUT)
                .send()
                .await;
            let retry = match sent {
                Ok(response) if response.status().as_u16() == 200 => return true,
                Ok(response) => response.status().is_server_error(),
                Err(_) => true,
            };
            match REVOKE_WAITS.get(attempt) {
                Some(wait) if retry => tokio::time::sleep(*wait).await,
                _ => break,
            }
        }
        false
    }
}

/// The token endpoint's form for `grant`.
fn form_body(grant: Grant<'_>) -> String {
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    match grant {
        Grant::Code {
            client_id,
            code,
            verifier,
            redirect_uri,
        } => form
            .append_pair("grant_type", "authorization_code")
            .append_pair("client_id", client_id)
            .append_pair("code", code.expose())
            .append_pair("code_verifier", verifier.expose())
            .append_pair("redirect_uri", redirect_uri),
        Grant::Refresh {
            client_id,
            refresh_token,
        } => form
            .append_pair("grant_type", "refresh_token")
            .append_pair("client_id", client_id)
            .append_pair("refresh_token", refresh_token.expose()),
    };
    form.append_pair("resource", RESOURCE).finish()
}

/// GETs JSON. The error says why, without the reply.
async fn get_json<T: DeserializeOwned>(request: RequestBuilder) -> Result<T, String> {
    let response = request
        .header(ACCEPT, "application/json")
        .send()
        .await
        .map_err(|err| describe(&err))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("it answered {status}"));
    }
    let body = response.bytes().await.map_err(|err| describe(&err))?;
    serde_json::from_slice(&body).map_err(|_| "its reply wasn't what was asked for".to_owned())
}
