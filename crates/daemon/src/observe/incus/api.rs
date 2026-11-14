// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The parts of the Incus REST API the observer reads, over the daemon's
//! Unix socket. Every call is a GET or a HEAD, so nothing here can change a
//! container.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use http_body_util::{BodyExt, Empty, Limited};
use hyper::body::Bytes;
use hyper::client::conn::http1::{self, SendRequest};
use hyper::header::{self, HeaderMap};
use hyper::upgrade::Upgraded;
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use maxhq_protocol::time::Timestamp;
use serde::Deserialize;
use thiserror::Error;
use tokio::net::UnixStream;

use super::time;

/// How long one request may take, so a wedged container can't hold up the
/// rest.
const TIMEOUT: Duration = Duration::from_secs(3);

/// The most one reply may hold. A container writes the files this reads, so
/// a huge one stops here and this device's memory holds.
const MAX_REPLY: usize = 32 * 1024 * 1024;

/// Why a call to Incus failed.
#[derive(Debug, Error)]
pub enum ApiError {
    /// The socket refused the connection, or the connection broke.
    #[error("can't reach Incus: {0}")]
    Io(#[from] std::io::Error),
    /// The HTTP exchange failed.
    #[error("Incus request failed: {0}")]
    Http(#[from] hyper::Error),
    /// The request couldn't be built, such as from an odd instance name.
    #[error("bad Incus request: {0}")]
    Request(#[from] hyper::http::Error),
    /// Incus took longer than [`TIMEOUT`].
    #[error("Incus didn't answer within {} seconds", TIMEOUT.as_secs())]
    Timeout,
    /// Incus answered with an error status.
    #[error("Incus answered {0}")]
    Status(StatusCode),
    /// Incus sent JSON that doesn't match what the observer expects.
    #[error("Incus sent JSON this can't read: {0}")]
    Json(#[from] serde_json::Error),
    /// Incus sent more than [`MAX_REPLY`].
    #[error("Incus sent more than {} MiB", MAX_REPLY / (1024 * 1024))]
    TooLarge,
}

/// Why a capped reply stopped: the connection broke, or it ran too long.
fn reply_error(error: Box<dyn std::error::Error + Send + std::marker::Sync>) -> ApiError {
    match error.downcast::<hyper::Error>() {
        Ok(error) => ApiError::Http(*error),
        Err(_) => ApiError::TooLarge,
    }
}

/// One instance, as `/1.0/instances?recursion=1` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Instance {
    /// The instance name, such as `ws-bravo`.
    pub name: String,
    /// `container` or `virtual-machine`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Such as `Running`, `Stopped` or `Frozen`.
    pub status: String,
    /// The instance's config merged over its profiles'.
    #[serde(default)]
    pub expanded_config: HashMap<String, String>,
    /// When the instance last started, or the zero time if it never has.
    #[serde(default)]
    pub last_used_at: String,
    /// When the instance was created.
    #[serde(default)]
    pub created_at: String,
    /// The profiles it takes keys and devices from.
    #[serde(default)]
    pub profiles: Vec<String>,
    /// Its devices merged over its profiles'.
    #[serde(default)]
    pub expanded_devices: HashMap<String, HashMap<String, String>>,
}

impl Instance {
    /// A config value, unless it's unset or empty.
    pub fn config(&self, key: &str) -> Option<&str> {
        self.expanded_config
            .get(key)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    }

    /// Whether the container runs now.
    pub fn is_running(&self) -> bool {
        self.status == "Running"
    }

    /// When the instance last started, if it ever has.
    pub fn started_at(&self) -> Option<Timestamp> {
        time::parse(&self.last_used_at)
    }
}

/// A file's version, from the headers the file API sends. Incus reports
/// the modification time to the second, so the size is part of it too.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    /// `X-Incus-Modified` and `Content-Length`, compared as they are.
    pub tag: String,
    /// The modification time, when the header reads as one.
    pub modified: Option<Timestamp>,
}

/// A file read from inside an instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    /// The version read.
    pub version: Version,
    /// The contents.
    pub body: Bytes,
}

/// The envelope around every synchronous Incus reply.
#[derive(Deserialize)]
struct Sync<T> {
    metadata: T,
}

/// One answer from Incus.
struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

/// A client for the Incus socket. It keeps one HTTP/1.1 connection open
/// and opens a new one when that breaks.
pub struct Client {
    socket: PathBuf,
    sender: Option<SendRequest<Empty<Bytes>>>,
}

impl Client {
    /// A client for the socket at `socket`. Nothing connects until the
    /// first call.
    pub fn new(socket: &Path) -> Self {
        Self {
            socket: socket.to_owned(),
            sender: None,
        }
    }

    /// Every instance in the default project.
    pub async fn instances(&mut self) -> Result<Vec<Instance>, ApiError> {
        let reply = self.send(Method::GET, "/1.0/instances?recursion=1").await?;
        if reply.status != StatusCode::OK {
            return Err(ApiError::Status(reply.status));
        }
        let list: Sync<Vec<Instance>> = serde_json::from_slice(&reply.body)?;
        Ok(list.metadata)
    }

    /// One instance, or None when there's none by that name.
    pub async fn instance(&mut self, name: &str) -> Result<Option<Instance>, ApiError> {
        self.get_json(&format!("/1.0/instances/{name}")).await
    }

    /// The disk an instance's root uses, in bytes, when Incus reports it.
    pub async fn disk_used(&mut self, name: &str) -> Result<Option<u64>, ApiError> {
        let state: Option<serde_json::Value> = self
            .get_json(&format!("/1.0/instances/{name}/state"))
            .await?;
        Ok(state.and_then(|state| state["disk"]["root"]["usage"].as_u64()))
    }

    /// The names of an instance's snapshots, or None when it's not there.
    pub async fn snapshot_names(&mut self, name: &str) -> Result<Option<Vec<String>>, ApiError> {
        let listed: Option<Vec<serde_json::Value>> = self
            .get_json(&format!("/1.0/instances/{name}/snapshots?recursion=1"))
            .await?;
        Ok(listed.map(|all| {
            all.iter()
                .filter_map(|snapshot| snapshot["name"].as_str().map(str::to_owned))
                .collect()
        }))
    }

    /// A GET's metadata, or None on a 404.
    async fn get_json<T: serde::de::DeserializeOwned>(
        &mut self,
        uri: &str,
    ) -> Result<Option<T>, ApiError> {
        let reply = self.send(Method::GET, uri).await?;
        match reply.status {
            StatusCode::OK => {
                let found: Sync<T> = serde_json::from_slice(&reply.body)?;
                Ok(Some(found.metadata))
            }
            StatusCode::NOT_FOUND => Ok(None),
            status => Err(ApiError::Status(status)),
        }
    }

    /// A file's version, or None when it doesn't exist. This is a HEAD
    /// request, which unlike a GET doesn't put an `instance-file-retrieved`
    /// event on the Incus event bus, so polling with it stays quiet.
    pub async fn version(
        &mut self,
        instance: &str,
        path: &str,
    ) -> Result<Option<Version>, ApiError> {
        let reply = self.send(Method::HEAD, &file_uri(instance, path)).await?;
        match reply.status {
            StatusCode::OK => Ok(Some(version(&reply.headers))),
            StatusCode::NOT_FOUND => Ok(None),
            // A HEAD's error has no text, and only the text can say why.
            StatusCode::INTERNAL_SERVER_ERROR => {
                Ok(self.read(instance, path).await?.map(|file| file.version))
            }
            status => Err(ApiError::Status(status)),
        }
    }

    /// A file's contents, or None when it doesn't exist. This works on a
    /// stopped container too.
    pub async fn read(&mut self, instance: &str, path: &str) -> Result<Option<File>, ApiError> {
        let reply = self.send(Method::GET, &file_uri(instance, path)).await?;
        match reply.status {
            StatusCode::OK => Ok(Some(File {
                version: version(&reply.headers),
                body: reply.body,
            })),
            StatusCode::NOT_FOUND => Ok(None),
            StatusCode::INTERNAL_SERVER_ERROR if through_a_file(&reply.body) => Ok(None),
            status => Err(ApiError::Status(status)),
        }
    }

    /// Sends one request, and drops the connection after any failure so
    /// the next call starts clean.
    async fn send(&mut self, method: Method, uri: &str) -> Result<Reply, ApiError> {
        let result = match tokio::time::timeout(TIMEOUT, self.exchange(method, uri)).await {
            Ok(result) => result,
            Err(_) => Err(ApiError::Timeout),
        };
        if result.is_err() {
            self.sender = None;
        }
        result
    }

    /// One request and its whole answer, opening a connection first when
    /// there is none.
    async fn exchange(&mut self, method: Method, uri: &str) -> Result<Reply, ApiError> {
        if self.sender.as_ref().is_none_or(SendRequest::is_closed) {
            self.sender = Some(connect(&self.socket).await?);
        }
        let Some(sender) = self.sender.as_mut() else {
            unreachable!("connected above");
        };
        sender.ready().await?;
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, "incus")
            .body(Empty::new())?;
        let response = sender.send_request(request).await?;
        let (parts, body) = response.into_parts();
        let body = Limited::new(body, MAX_REPLY)
            .collect()
            .await
            .map_err(reply_error)?
            .to_bytes();
        Ok(Reply {
            status: parts.status,
            headers: parts.headers,
            body,
        })
    }
}

/// Incus answers 500, not 404, for a path through a file, such as `.git/HEAD`
/// in a linked checkout, where `.git` is a file. Nothing is at that path.
fn through_a_file(error: &[u8]) -> bool {
    String::from_utf8_lossy(error).contains("not a directory")
}

/// Opens `/1.0/events` for lifecycle events. Incus only serves events over
/// a WebSocket, so this upgrades the connection and hands back the raw
/// stream for [`super::events`] to read frames from. `key` is the
/// `Sec-WebSocket-Key` nonce.
pub async fn open_events(socket: &Path, key: &str) -> Result<TokioIo<Upgraded>, ApiError> {
    let opening = async {
        let stream = UnixStream::connect(socket).await?;
        let (mut sender, connection) = http1::handshake(TokioIo::new(stream)).await?;
        tokio::spawn(async move {
            if let Err(error) = connection.with_upgrades().await {
                tracing::debug!(%error, "Incus event connection ended");
            }
        });
        let request = Request::builder()
            .method(Method::GET)
            .uri("/1.0/events?type=lifecycle")
            .header(header::HOST, "incus")
            .header(header::CONNECTION, "Upgrade")
            .header(header::UPGRADE, "websocket")
            .header(header::SEC_WEBSOCKET_VERSION, "13")
            .header(header::SEC_WEBSOCKET_KEY, key)
            .body(Empty::<Bytes>::new())?;
        let response = sender.send_request(request).await?;
        if response.status() != StatusCode::SWITCHING_PROTOCOLS {
            return Err(ApiError::Status(response.status()));
        }
        // The accept header isn't checked. It guards against caching
        // proxies, and there's none on a local socket.
        Ok(TokioIo::new(hyper::upgrade::on(response).await?))
    };
    match tokio::time::timeout(TIMEOUT, opening).await {
        Ok(result) => result,
        Err(_) => Err(ApiError::Timeout),
    }
}

/// Opens an HTTP/1.1 connection to the socket and drives it in the
/// background.
async fn connect(socket: &Path) -> Result<SendRequest<Empty<Bytes>>, ApiError> {
    let stream = UnixStream::connect(socket).await?;
    let (sender, connection) = http1::handshake(TokioIo::new(stream)).await?;
    tokio::spawn(async move {
        if let Err(error) = connection.await {
            tracing::debug!(%error, "Incus connection ended");
        }
    });
    Ok(sender)
}

/// The file API's URI for a path inside an instance. Instance names are
/// letters, digits and dashes. A path can come from what a container wrote,
/// so it's escaped.
fn file_uri(instance: &str, path: &str) -> String {
    format!(
        "/1.0/instances/{instance}/files?path={}",
        crate::incus::http::escape(path)
    )
}

/// A file's version from the file API's reply headers.
fn version(headers: &HeaderMap) -> Version {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
    };
    let modified = header("x-incus-modified");
    Version {
        tag: format!("{modified} {}", header("content-length")),
        modified: time::parse(modified),
    }
}

#[cfg(test)]
mod tests {
    use super::super::fake::{FakeFile, FakeIncus, State};
    use super::*;

    #[test]
    fn a_file_path_stays_one_query_value() {
        assert_eq!(
            file_uri("a", "/x y?z&path=/etc/shadow#"),
            "/1.0/instances/a/files?path=/x%20y%3Fz%26path%3D/etc/shadow%23"
        );
    }

    #[tokio::test]
    async fn a_huge_file_is_refused_before_it_fills_memory() {
        let mut state = State::default();
        for (path, size) in [("/big", MAX_REPLY + 1), ("/fine", 2)] {
            state.files.insert(
                ("a".to_owned(), path.to_owned()),
                FakeFile {
                    modified: "Sat, 01 Aug 2026 10:00:00 GMT".to_owned(),
                    body: vec![b'x'; size],
                },
            );
        }
        let incus = FakeIncus::start(state);
        let mut client = Client::new(&incus.socket);
        let fine = client.read("a", "/fine").await.unwrap().unwrap();
        assert_eq!(&fine.body[..], b"xx");
        let big = client.read("a", "/big").await.unwrap_err();
        assert!(matches!(big, ApiError::TooLarge), "{big:?}");
    }

    #[test]
    fn only_a_path_through_a_file_counts_as_not_there() {
        assert!(through_a_file(
            br#"{"error":"sftp: \"lstat /a/b: not a directory\" (SSH_FX_FAILURE)"}"#
        ));
        assert!(!through_a_file(br#"{"error":"sftp: permission denied"}"#));
    }

    #[test]
    fn reads_the_instance_list() {
        let reply = include_str!("../../../tests/fixtures/incus/instances.json");
        let list: Sync<Vec<Instance>> = serde_json::from_str(reply).unwrap();
        let names: Vec<(&str, &str, &str)> = list
            .metadata
            .iter()
            .map(|i| (i.name.as_str(), i.kind.as_str(), i.status.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("ledger", "virtual-machine", "Running"),
                ("vault", "container", "Stopped"),
                ("ws-harbor", "container", "Running"),
                ("ws-lantern", "container", "Running"),
                ("ws-quill", "container", "Running"),
                ("ws-tidepool", "container", "Stopped"),
            ]
        );

        let lantern = &list.metadata[3];
        assert_eq!(lantern.config("user.repo"), Some("example-org/Lantern"));
        assert_eq!(lantern.config("user.title"), Some("lantern"));
        assert_eq!(lantern.config("user.agent"), Some("claude"));
        assert_eq!(
            lantern.config("user.created"),
            Some("2026-10-18T02:34:46+02:00")
        );
        assert_eq!(lantern.started_at(), Some(Timestamp(1_792_728_193_744)));
        assert!(lantern.is_running());
        assert_eq!(list.metadata[1].config("user.repo"), None);
        assert!(!list.metadata[5].is_running());
    }

    #[test]
    fn versions_come_from_the_file_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-incus-modified",
            "2026-10-24 09:01:03 +0000 UTC".parse().unwrap(),
        );
        headers.insert("content-length", "125".parse().unwrap());
        let read = version(&headers);
        assert_eq!(read.tag, "2026-10-24 09:01:03 +0000 UTC 125");
        assert_eq!(read.modified, Some(Timestamp(1_792_832_463_000)));
    }
}
