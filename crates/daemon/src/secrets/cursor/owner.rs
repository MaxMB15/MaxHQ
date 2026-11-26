// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Whose a Cursor API key is, from Cursor's API. The CLI can't say:
//! `agent status` and `agent about` report only the login the CLI saved and
//! ignore `CURSOR_API_KEY`, and the commands that do sign in with the key
//! save it over that login. Cursor's `GET /v1/me` names the user a key
//! belongs to, though a service account's key names nobody. Nothing here
//! logs the key or what Cursor sends.

use std::future::Future;
use std::time::Duration;

use maxhq_protocol::Secret;
use reqwest::header::ACCEPT;
use reqwest::{Client, StatusCode};
use serde::Deserialize;

use crate::secrets::http;

/// Where Cursor says whose a key is.
pub const ENDPOINT: &str = "https://api.cursor.com/v1/me";

/// How long the call may take.
const TIMEOUT: Duration = Duration::from_secs(20);

/// Who a key belongs to, as far as Cursor says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Owner {
    /// The user's email, or none for a service account's key.
    pub email: Option<String>,
}

/// Where the owner comes from, so tests stand in for Cursor.
pub trait Ask: Send + 'static {
    /// Whose `key` is, or why Cursor couldn't say, in a few words that
    /// quote neither the key nor Cursor's answer.
    fn owner(&mut self, key: &Secret) -> impl Future<Output = Result<Owner, String>> + Send;
}

/// Cursor itself.
#[derive(Default)]
pub struct Live {
    client: Option<Client>,
}

impl Ask for Live {
    async fn owner(&mut self, key: &Secret) -> Result<Owner, String> {
        let client = match &self.client {
            Some(client) => client,
            None => self
                .client
                .insert(http::client(true).map_err(|_| "couldn't set up a connection".to_owned())?),
        };
        fetch(client, ENDPOINT, key).await
    }
}

/// The fields of Cursor's answer the owner needs.
#[derive(Deserialize)]
struct Answer {
    #[serde(rename = "userEmail")]
    user_email: Option<String>,
}

/// Asks `url` whose `key` is, with the key as the basic-auth user name, as
/// Cursor's API takes it.
pub async fn fetch(client: &Client, url: &str, key: &Secret) -> Result<Owner, String> {
    let response = client
        .get(url)
        .basic_auth(key.expose(), None::<&str>)
        .header(ACCEPT, "application/json")
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(|err| {
            if err.is_timeout() {
                "Cursor took too long to answer".to_owned()
            } else {
                "couldn't reach Cursor".to_owned()
            }
        })?;
    match response.status() {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            return Err("Cursor turned the key down".into());
        }
        status if !status.is_success() => {
            return Err(format!("Cursor answered {}", status.as_u16()));
        }
        _ => {}
    }
    let body = response
        .bytes()
        .await
        .map_err(|_| "Cursor's answer broke off".to_owned())?;
    let answer: Answer =
        serde_json::from_slice(&body).map_err(|_| "Cursor's answer isn't JSON".to_owned())?;
    let email = answer
        .user_email
        .map(|email| email.trim().to_owned())
        .filter(|email| !email.is_empty());
    Ok(Owner { email })
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;

    use super::*;

    /// A key's shape, with nothing real in it.
    const KEY: &str = "key_test-not-a-real-cursor-key";

    /// A server on 127.0.0.1 that answers one request with `status` and
    /// `body`, and hands back the request it got.
    fn serve(status: &str, body: &'static str) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1/me", listener.local_addr().unwrap());
        let status = status.to_owned();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut got = Vec::new();
            let mut chunk = [0; 4096];
            // The request has no body, so it ends with its headers.
            while !got.ends_with(b"\r\n\r\n") {
                let read = stream.read(&mut chunk).unwrap();
                assert!(read > 0, "the request ended early");
                got.extend_from_slice(&chunk[..read]);
            }
            let reply = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(reply.as_bytes()).unwrap();
            String::from_utf8(got).unwrap()
        });
        (url, handle)
    }

    async fn ask(status: &str, body: &'static str) -> (Result<Owner, String>, String) {
        let (url, server) = serve(status, body);
        let client = http::client(false).unwrap();
        let owner = fetch(&client, &url, &Secret::new(KEY.to_owned())).await;
        (owner, server.join().unwrap())
    }

    #[tokio::test]
    async fn the_key_goes_as_the_user_name_and_the_email_comes_back() {
        let (owner, request) = ask(
            "200 OK",
            r#"{"apiKeyName":"Laptop","createdAt":"2026-01-01T00:00:00Z","userId":7,"userEmail":" someone@example.com ","userFirstName":"Some","userLastName":"One"}"#,
        )
        .await;
        assert_eq!(
            owner,
            Ok(Owner {
                email: Some("someone@example.com".into())
            })
        );
        let request = request.to_ascii_lowercase();
        assert!(request.starts_with("get /v1/me "), "{request}");
        // The key, then an empty password.
        let basic = STANDARD.encode(format!("{KEY}:")).to_ascii_lowercase();
        assert!(
            request.contains(&format!("authorization: basic {basic}\r\n")),
            "{request}"
        );
        assert!(request.contains("accept: application/json\r\n"));
    }

    #[tokio::test]
    async fn a_service_accounts_key_names_nobody() {
        let (owner, _) = ask(
            "200 OK",
            r#"{"apiKeyName":"CI","createdAt":"2026-01-01T00:00:00Z"}"#,
        )
        .await;
        assert_eq!(owner, Ok(Owner { email: None }));
        let (owner, _) = ask("200 OK", r#"{"userEmail":"  "}"#).await;
        assert_eq!(owner, Ok(Owner { email: None }));
    }

    #[tokio::test]
    async fn failures_say_why_without_cursors_answer() {
        let (owner, _) = ask("401 Unauthorized", r#"{"error":"private answer"}"#).await;
        assert_eq!(owner, Err("Cursor turned the key down".into()));
        let (owner, _) = ask("500 Internal Server Error", "private answer").await;
        assert_eq!(owner, Err("Cursor answered 500".into()));
        let (owner, _) = ask("200 OK", "private answer").await;
        let reason = owner.unwrap_err();
        assert!(!reason.contains("private"), "{reason}");
    }
}
