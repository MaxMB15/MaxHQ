// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! How local agents here use another device's managed model server, over
//! the `maxd` link to it. They talk to a listener on 127.0.0.1 that wants
//! this device's own linked key, which never leaves it. Each request opens
//! a model stream to the server's device and goes on over it with no key:
//! the link already proved who this device is, and that device's switch
//! says whether it may. Nothing binds to the network and nothing new signs
//! in.

use std::convert::Infallible;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::future::BoxFuture;
use http_body_util::{BodyExt, Limited};
use hyper::body::{Body as HttpBody, Bytes, Frame, Incoming, SizeHint};
use hyper::header::{HeaderValue, CONTENT_TYPE};
use hyper::{Request, Response, StatusCode, Uri};
use hyper_util::rt::{TokioIo, TokioTimer};
use maxhq_protocol::ids::ModelServerId;
use maxhq_protocol::secret::Secret;
use serde_json::json;
use tokio::io::DuplexStream;
use tokio::net::TcpListener;

use super::runner::{carries, error, send_on, Body, MAX_BODY};

/// The path a linked server's requests start with, before its id. A local
/// agent's base URL is this listener's address, this and the id.
pub const PREFIX: &str = "/linked/";

/// What a request for a server no linked device shares fails with.
const NOT_SHARED: &str = "No linked device shares that model server.";

/// What a request fails with on a `maxd` that has no links.
const NO_LINKS: &str = "This maxd links to no other devices.";

/// A stream opened to another device's model server.
pub struct Opened {
    /// This end of the stream.
    pub socket: DuplexStream,
    /// The device's name, for what a lost link says.
    pub name: String,
    /// What keeps the link up while the stream is in use.
    pub keep: Box<dyn Send>,
}

/// Opens streams to other devices' model servers.
pub trait Reach: Send + Sync + 'static {
    /// Opens a stream to `server`, or says why it can't.
    fn open(&self, server: ModelServerId) -> BoxFuture<'static, Result<Opened, String>>;
}

/// Opens model streams over the links between `maxd`s: those of `peers`,
/// else the installed ones.
pub struct OverPeers(pub Option<crate::peers::Peers>);

impl Reach for OverPeers {
    fn open(&self, server: ModelServerId) -> BoxFuture<'static, Result<Opened, String>> {
        let peers = self.0.clone().or_else(crate::peers::for_link);
        Box::pin(async move {
            let peers = peers.ok_or_else(|| NO_LINKS.to_owned())?;
            let shared = peers
                .shared_models()
                .borrow()
                .iter()
                .find(|shared| shared.models.server == server)
                .map(|shared| (shared.device, shared.name.clone()));
            let (device, name) = shared.ok_or_else(|| NOT_SHARED.to_owned())?;
            let (socket, open) = peers.model_stream(device, server).await?;
            Ok(Opened {
                socket,
                name,
                keep: Box::new(open),
            })
        })
    }
}

/// The listener local agents here reach linked servers through.
pub struct Linked {
    key: Secret,
    reach: Arc<dyn Reach>,
}

impl Linked {
    /// A listener that wants `key` and opens streams through `reach`.
    pub fn new(key: Secret, reach: Arc<dyn Reach>) -> Arc<Self> {
        Arc::new(Self { key, reach })
    }

    /// Serves on `listener` until `maxd` stops.
    pub async fn serve(self: Arc<Self>, listener: TcpListener) {
        loop {
            let stream = match listener.accept().await {
                Ok((stream, _)) => stream,
                Err(err) => {
                    tracing::warn!(%err, "the linked model listener couldn't take a connection");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            let linked = self.clone();
            tokio::spawn(async move {
                let service = hyper::service::service_fn(move |request| {
                    let linked = linked.clone();
                    async move { Ok::<_, Infallible>(linked.handle(request).await) }
                });
                let served = hyper::server::conn::http1::Builder::new()
                    .timer(TokioTimer::new())
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
                if let Err(err) = served {
                    tracing::debug!(%err, "a linked model connection ended early");
                }
            });
        }
    }

    /// Answers one request: with the key, it goes to the server its path
    /// names, over a stream of its own.
    pub async fn handle(&self, request: Request<Incoming>) -> Response<Body> {
        if !carries(request.headers(), &self.key) {
            return error(
                StatusCode::UNAUTHORIZED,
                "authentication_error",
                "This model server wants its key.",
            );
        }
        let Some((server, rest)) = target(request.uri()) else {
            return error(StatusCode::NOT_FOUND, "not_found_error", "Nothing is here.");
        };
        let (mut parts, body) = request.into_parts();
        let Ok(body) = Limited::new(body, MAX_BODY).collect().await else {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request_too_large",
                "The request is too large.",
            );
        };
        let opened = match self.reach.open(server).await {
            Ok(opened) => opened,
            Err(reason) => return final_error(&reason),
        };
        parts.uri = rest;
        let Opened { socket, name, keep } = opened;
        match send_on(socket, &name, "localhost", None, &parts, body.to_bytes()).await {
            Ok(response) => {
                let events = response
                    .headers()
                    .get(CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|value| value.starts_with("text/event-stream"));
                response.map(|body| {
                    Kept {
                        body: body.boxed_unsync(),
                        keep: Some(keep),
                        lost: events.then(|| lost_event(&name)),
                    }
                    .boxed_unsync()
                })
            }
            Err(_) => final_error(&lost(&name)),
        }
    }
}

/// The server and the rest of the path a request names, as
/// `/linked/<server>/v1/messages?beta=true`.
fn target(uri: &Uri) -> Option<(ModelServerId, Uri)> {
    let path = uri.path().strip_prefix(PREFIX)?;
    let (id, rest) = path.split_at(path.find('/').unwrap_or(path.len()));
    let server = ModelServerId::from_str(id).ok()?;
    let rest = if rest.is_empty() { "/" } else { rest };
    let rest = match uri.query() {
        Some(query) => format!("{rest}?{query}"),
        None => rest.to_owned(),
    };
    Some((server, rest.parse().ok()?))
}

/// What a request fails with when the link to its device goes.
fn lost(name: &str) -> String {
    format!("Lost the link to {name}.")
}

/// The event a streamed answer ends with when its link goes.
fn lost_event(name: &str) -> Bytes {
    let event = json!({
        "type": "error",
        "error": {"type": "api_error", "message": lost(name)},
    });
    Bytes::from(format!("event: error\ndata: {event}\n\n"))
}

/// An error Claude Code shows instead of trying again, since another try
/// fails the same way until the link or the switch changes.
fn final_error(reason: &str) -> Response<Body> {
    let mut response = error(StatusCode::BAD_GATEWAY, "api_error", reason);
    response
        .headers_mut()
        .insert("x-should-retry", HeaderValue::from_static("false"));
    response
}

/// An answer's body that keeps the link up until it ends, and ends a
/// streamed answer whose link went with an error event, so the turn says
/// why instead of hanging.
struct Kept {
    body: Body,
    keep: Option<Box<dyn Send>>,
    lost: Option<Bytes>,
}

impl HttpBody for Kept {
    type Data = Bytes;
    type Error = hyper::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, hyper::Error>>> {
        if self.keep.is_none() {
            return Poll::Ready(None);
        }
        match Pin::new(&mut self.body).poll_frame(cx) {
            Poll::Ready(Some(Err(err))) => {
                self.keep = None;
                match self.lost.take() {
                    Some(event) => Poll::Ready(Some(Ok(Frame::data(event)))),
                    None => Poll::Ready(Some(Err(err))),
                }
            }
            Poll::Ready(None) => {
                self.keep = None;
                Poll::Ready(None)
            }
            other => other,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.keep.is_none()
    }

    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use serde_json::Value;

    use super::*;
    use crate::local_models::tests::Fake;

    const KEY: &str = "linked-key-0123456789";

    /// Reaches a fake server on 127.0.0.1 through a pipe, as a link would.
    struct ToFake {
        fake: Fake,
        opened: AtomicUsize,
        refuse: Mutex<Option<String>>,
    }

    impl Reach for ToFake {
        fn open(&self, _server: ModelServerId) -> BoxFuture<'static, Result<Opened, String>> {
            self.opened.fetch_add(1, Ordering::SeqCst);
            let refuse = self.refuse.lock().unwrap().clone();
            let address = self.fake.base.trim_start_matches("http://").to_owned();
            Box::pin(async move {
                if let Some(reason) = refuse {
                    return Err(reason);
                }
                let (near, mut far) = tokio::io::duplex(64 * 1024);
                let mut tcp = tokio::net::TcpStream::connect(address).await.unwrap();
                tokio::spawn(async move {
                    let _ = tokio::io::copy_bidirectional(&mut far, &mut tcp).await;
                });
                Ok(Opened {
                    socket: near,
                    name: "Home Mac".into(),
                    keep: Box::new(()),
                })
            })
        }
    }

    async fn listener() -> (Arc<ToFake>, String) {
        let fake = Fake::start().await;
        fake.json(
            "POST /v1/messages?beta=true",
            200,
            &json!({"type": "message", "content": [{"type": "text", "text": "hi"}]}),
        );
        let reach = Arc::new(ToFake {
            fake,
            opened: AtomicUsize::new(0),
            refuse: Mutex::new(None),
        });
        let linked = Linked::new(Secret::new(KEY.into()), reach.clone());
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(linked.serve(listener));
        (reach, base)
    }

    fn server() -> ModelServerId {
        ModelServerId::derive("model-server:home-mac:managed")
    }

    #[tokio::test]
    async fn a_request_with_the_key_goes_on_with_no_key_of_its_own() {
        let (reach, base) = listener().await;
        let client = crate::secrets::http::client(false).unwrap();
        let url = format!("{base}{PREFIX}{}/v1/messages?beta=true", server());
        let without = client.post(&url).body("{}").send().await.unwrap();
        assert_eq!(without.status(), 401);
        let wrong = client
            .post(&url)
            .bearer_auth("nope")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(wrong.status(), 401);
        assert_eq!(reach.opened.load(Ordering::SeqCst), 0, "nothing opens");

        let answer = client
            .post(&url)
            .header("x-api-key", KEY)
            .header("anthropic-version", "2023-06-01")
            .body(r#"{"model":"m"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(answer.status(), 200);
        let answer: Value = serde_json::from_slice(&answer.bytes().await.unwrap()).unwrap();
        assert_eq!(answer["content"][0]["text"], "hi");
        let seen = reach.fake.seen();
        let sent = seen.last().unwrap();
        assert_eq!(sent.path, "/v1/messages?beta=true");
        assert_eq!(sent.header("anthropic-version"), Some("2023-06-01"));
        // The key here stays here.
        assert_eq!(sent.header("x-api-key"), None);
        assert_eq!(sent.header("authorization"), None);
        assert_eq!(sent.body, r#"{"model":"m"}"#);
    }

    #[tokio::test]
    async fn a_refusal_comes_back_with_its_reason_and_no_retry() {
        let (reach, base) = listener().await;
        *reach.refuse.lock().unwrap() = Some("Can't reach Home Mac.".into());
        let answer = crate::secrets::http::client(false)
            .unwrap()
            .post(format!("{base}{PREFIX}{}/v1/messages", server()))
            .bearer_auth(KEY)
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(answer.status(), 502);
        assert_eq!(answer.headers().get("x-should-retry").unwrap(), "false");
        let answer: Value = serde_json::from_slice(&answer.bytes().await.unwrap()).unwrap();
        assert_eq!(answer["error"]["message"], "Can't reach Home Mac.");
    }

    #[test]
    fn the_path_names_the_server_and_what_follows() {
        let id = server();
        let uri: Uri = format!("/linked/{id}/v1/messages?beta=true")
            .parse()
            .unwrap();
        let (got, rest) = target(&uri).unwrap();
        assert_eq!(got, id);
        assert_eq!(rest, "/v1/messages?beta=true");
        let bare: Uri = format!("/linked/{id}").parse().unwrap();
        assert_eq!(target(&bare).unwrap().1, "/");
        assert!(target(&"/v1/messages".parse().unwrap()).is_none());
        assert!(target(&"/linked/nope/v1/messages".parse().unwrap()).is_none());
    }

    #[test]
    fn a_lost_link_ends_a_stream_with_an_error_event() {
        let event = String::from_utf8(lost_event("Home Mac").to_vec()).unwrap();
        assert!(event.starts_with("event: error\ndata: "));
        let data: Value =
            serde_json::from_str(event.trim_start_matches("event: error\ndata: ").trim()).unwrap();
        assert_eq!(data["error"]["message"], "Lost the link to Home Mac.");
    }
}
