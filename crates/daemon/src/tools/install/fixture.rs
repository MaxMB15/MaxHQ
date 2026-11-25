// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A local HTTP server the download tests fetch from, on every system.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// How a fixture route answers.
#[derive(Clone)]
pub(crate) enum Answer {
    /// The whole body.
    Body(Vec<u8>),
    /// Says the body is longer than it is, then hangs up.
    Short(Vec<u8>),
    /// Sends part of the body, then waits forever.
    Stall(Vec<u8>),
}

/// A local HTTP server with fixed routes.
pub(crate) struct Fixture {
    pub(crate) base: String,
    routes: std::sync::Arc<Mutex<HashMap<String, Answer>>>,
    pub(crate) hits: std::sync::Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Fixture {
    pub(crate) async fn start() -> Fixture {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let routes: std::sync::Arc<Mutex<HashMap<String, Answer>>> = Default::default();
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        let task = tokio::spawn({
            let routes = routes.clone();
            let hits = hits.clone();
            async move {
                loop {
                    let Ok((mut socket, _)) = listener.accept().await else {
                        return;
                    };
                    let routes = routes.clone();
                    let hits = hits.clone();
                    tokio::spawn(async move {
                        let mut request = Vec::new();
                        let mut buffer = [0u8; 4096];
                        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                            match socket.read(&mut buffer).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => request.extend_from_slice(&buffer[..n]),
                            }
                        }
                        let text = String::from_utf8_lossy(&request);
                        let path = text.split_whitespace().nth(1).unwrap_or("/").to_owned();
                        hits.fetch_add(1, Ordering::SeqCst);
                        let answer = routes.lock().unwrap().get(&path).cloned();
                        let head = |length: usize| {
                            format!(
                                "HTTP/1.1 200 OK\r\ncontent-length: {length}\r\nconnection: close\r\n\r\n"
                            )
                        };
                        match answer {
                            None => {
                                let _ = socket
                                    .write_all(
                                        b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                                    )
                                    .await;
                            }
                            Some(Answer::Body(body)) => {
                                let _ = socket.write_all(head(body.len()).as_bytes()).await;
                                let _ = socket.write_all(&body).await;
                            }
                            Some(Answer::Short(body)) => {
                                let _ = socket.write_all(head(body.len() * 2).as_bytes()).await;
                                let _ = socket.write_all(&body).await;
                            }
                            Some(Answer::Stall(body)) => {
                                let _ = socket.write_all(head(body.len() * 2).as_bytes()).await;
                                let _ = socket.write_all(&body).await;
                                let _ = socket.flush().await;
                                std::future::pending::<()>().await;
                            }
                        }
                        let _ = socket.shutdown().await;
                    });
                }
            }
        });
        Fixture {
            base,
            routes,
            hits,
            task,
        }
    }

    /// Serves `answer` at `path` and gives its URL.
    pub(crate) fn serve(&self, path: &str, answer: Answer) -> String {
        self.routes.lock().unwrap().insert(path.to_owned(), answer);
        format!("{}{path}", self.base)
    }
}
