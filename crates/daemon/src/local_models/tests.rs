// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What the local model tests share: a scratch folder and a fake HTTP
//! server on 127.0.0.1 that plays a model server or Hugging Face.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A folder that's removed when the test ends.
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!("maxhq-local-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What the fake answers on one path.
#[derive(Clone)]
pub enum Route {
    /// A status and a JSON body.
    Json(u16, String),
    /// A file, served whole or from a `Range: bytes=N-`. With `cut`, the
    /// next answer stops after that many bytes and closes, once.
    File { body: Vec<u8>, cut: Option<usize> },
    /// A redirect to another URL.
    Redirect(String),
}

/// One request the fake took.
#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A fake HTTP server on 127.0.0.1.
#[derive(Clone)]
pub struct Fake {
    pub base: String,
    routes: Arc<Mutex<HashMap<String, Route>>>,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Fake {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let fake = Self {
            base,
            routes: Arc::default(),
            seen: Arc::default(),
        };
        let serving = fake.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let fake = serving.clone();
                tokio::spawn(async move { fake.answer(stream).await });
            }
        });
        fake
    }

    /// Answers `path` (with the method first for a POST, as `POST /x`, or
    /// `PLAIN /x` for a GET without `anthropic-version`).
    pub fn route(&self, path: &str, route: Route) {
        self.routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(path.to_owned(), route);
    }

    pub fn json(&self, path: &str, status: u16, body: &serde_json::Value) {
        self.route(path, Route::Json(status, body.to_string()));
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    async fn answer(&self, mut stream: tokio::net::TcpStream) {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        let head_end = loop {
            let Ok(n) = stream.read(&mut chunk).await else {
                return;
            };
            if n == 0 {
                return;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break at + 4;
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
        let mut lines = head.split("\r\n");
        let mut first = lines.next().unwrap_or_default().split(' ');
        let method = first.next().unwrap_or_default().to_owned();
        let path = first.next().unwrap_or_default().to_owned();
        let headers: Vec<(String, String)> = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(n, v)| (n.trim().to_owned(), v.trim().to_owned()))
            .collect();
        let length: usize = headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.parse().ok())
            .unwrap_or(0);
        while buf.len() < head_end + length {
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
        let body = String::from_utf8_lossy(&buf[head_end..]).to_string();
        let seen = Seen {
            method: method.clone(),
            path: path.clone(),
            headers,
            body,
        };
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(seen.clone());
        let key = if method == "GET" {
            path.clone()
        } else {
            format!("{method} {path}")
        };
        // A GET without `anthropic-version` takes `PLAIN <path>` when
        // there's one, as OpenRouter answers its OpenAI-style list then.
        let plain = (method == "GET" && seen.header("anthropic-version").is_none())
            .then(|| format!("PLAIN {path}"));
        let route = {
            let routes = self.routes.lock().unwrap_or_else(PoisonError::into_inner);
            plain
                .and_then(|plain| routes.get(&plain))
                .or_else(|| routes.get(&key))
                .cloned()
        };
        match route {
            None => {
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                    )
                    .await;
            }
            Some(Route::Json(status, body)) => {
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes()).await;
            }
            Some(Route::Redirect(to)) => {
                let reply = format!(
                    "HTTP/1.1 307 Temporary Redirect\r\nlocation: {to}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                );
                let _ = stream.write_all(reply.as_bytes()).await;
            }
            Some(Route::File { body, cut }) => {
                let from: usize = seen
                    .header("range")
                    .and_then(|r| r.strip_prefix("bytes="))
                    .and_then(|r| r.trim_end_matches('-').parse().ok())
                    .unwrap_or(0)
                    .min(body.len());
                let status = if from > 0 {
                    "206 Partial Content"
                } else {
                    "200 OK"
                };
                let range = if from > 0 {
                    format!(
                        "content-range: bytes {from}-{}/{}\r\n",
                        body.len() - 1,
                        body.len()
                    )
                } else {
                    String::new()
                };
                let rest = &body[from..];
                let head = format!(
                    "HTTP/1.1 {status}\r\ncontent-length: {}\r\n{range}connection: close\r\n\r\n",
                    rest.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let send = match cut {
                    Some(n) => {
                        self.route(
                            &key,
                            Route::File {
                                body: body.clone(),
                                cut: None,
                            },
                        );
                        &rest[..n.min(rest.len())]
                    }
                    None => rest,
                };
                let _ = stream.write_all(send).await;
                let _ = stream.flush().await;
            }
        }
    }
}
