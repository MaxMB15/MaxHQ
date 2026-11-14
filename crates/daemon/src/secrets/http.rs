// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The HTTP client the store calls GitHub and ChatGPT with. Rustls on ring
//! checks each server against the system's roots, redirects are never
//! followed, and nothing sent or got is logged, since reqwest's verbose
//! connection log stays off.

use std::error::Error as _;
use std::sync::Once;
use std::time::Duration;

use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::{Client, ClientBuilder, RequestBuilder};

/// How long a call may take in all.
const TIMEOUT: Duration = Duration::from_secs(30);
/// How long connecting may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// A client, through the proxy the environment names unless `proxies` is
/// false, as for a test server. The error says why it couldn't be built.
pub fn client(proxies: bool) -> Result<Client, String> {
    builder(proxies)
        .http1_only()
        .build()
        .map_err(|err| describe(&err))
}

/// A client that speaks only HTTP/2, as APNs does, through the proxy the
/// environment names unless `proxies` is false.
pub fn http2_client(proxies: bool) -> Result<Client, String> {
    builder(proxies)
        .http2_prior_knowledge()
        .build()
        .map_err(|err| describe(&err))
}

fn builder(proxies: bool) -> ClientBuilder {
    install_crypto();
    let mut builder = Client::builder()
        .user_agent(concat!("MaxHQ-maxd/", env!("CARGO_PKG_VERSION")))
        .timeout(TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none());
    if !proxies {
        builder = builder.no_proxy();
    }
    builder
}

/// Makes ring the process's TLS crypto, once. reqwest is built without a
/// crypto provider of its own, so no aws-lc build is needed.
fn install_crypto() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // Something else may have installed one first, which is fine.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// A reqwest error and each cause under it, since its own text names only
/// the URL.
pub fn describe(err: &reqwest::Error) -> String {
    let mut text = err.to_string();
    let mut cause = err.source();
    while let Some(next) = cause {
        text.push_str(": ");
        text.push_str(&next.to_string());
        cause = next.source();
    }
    text
}

/// A form POST of `body` to `url`, asking for JSON back.
pub fn form_post(http: &Client, url: &str, body: &str) -> RequestBuilder {
    http.post(url)
        .header(ACCEPT, "application/json")
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(body.to_owned())
}
