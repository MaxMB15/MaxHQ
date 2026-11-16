// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The phone's network access, for a device whose switch is on: a
//! WebSocket to its `maxd` over TLS that trusts only the certificate pinned
//! at pairing, whatever its names, issuer or dates. The link's bytes ride
//! in binary messages. The same TLS carries the JSON posts that pair the
//! phone and trade codes for tokens. Each request's headers come from the
//! caller, who signs a proof for its URL.

use std::fmt;
use std::io;
use std::net::IpAddr;
use std::pin::Pin as Pinned;
use std::sync::Arc;
use std::task::{ready, Context, Poll};
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use bytes::Bytes;
use futures::future::BoxFuture;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Full, Limited};
use hyper::header::{HeaderName, HeaderValue, CONTENT_TYPE, HOST};
use maxhq_protocol::ids::DeviceId;
use maxhq_protocol::link::Route;
use maxhq_protocol::netaccess::{NetError, NetRefusal, MAX_LINK_MESSAGE};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, SignatureScheme};
use subtle::ConstantTimeEq;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::WebSocketStream;

use super::reach::{self, Connect, Io, Logins, Reasons, STAGGER};
use super::{authority, Conn, DialError, Transport, LOGIN_TIMEOUT};
use crate::ssh_config::SshConfig;

/// The path of the link's WebSocket.
const LINK_PATH: &str = "/link";

/// The most a post's answer may hold.
const ANSWER_CAP: usize = 64 * 1024;

/// The most one message carries, so a large write goes as several.
const CHUNK: usize = 64 * 1024;

/// What network access says when it can't reach a port.
const REASONS: Reasons = Reasons {
    refused: "connection refused",
    timed_out: "the connection timed out",
    closed: "the connection closed",
};

/// One of a device's network routes: an address its `maxd` listens on, and
/// the certificate pin from pairing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WsRoute {
    /// A private address or name, such as `192.168.1.20` or
    /// `devbox.example.ts.net`.
    pub host: String,
    /// The listener's port.
    pub port: u16,
    /// The SHA-256 of the certificate's DER, as base64url with no padding.
    pub pin: String,
}

/// Gives the headers for a request, from its method and its full URL, such
/// as `GET` and `wss://192.168.1.20:7466/link`, so a proof can sign that
/// URL. An error ends the request with that reason.
pub type Headers = Arc<
    dyn Fn(&'static str, String) -> BoxFuture<'static, Result<Vec<(String, String)>, String>>
        + Send
        + Sync,
>;

/// Tells the caller that a device ended this phone's sign-in, with its
/// token expired or revoked, so the caller forgets the token.
pub type Ended = Arc<dyn Fn() + Send + Sync>;

/// Why a link fails once its device ended this phone's sign-in.
pub const SIGN_IN_ENDED: &str = "the device ended this phone's sign-in, so pair the phone again";

/// A post's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Posted {
    /// The HTTP status, such as 200.
    pub status: u16,
    /// The body, at most 64 KiB.
    pub body: Vec<u8>,
}

/// The phone's network access: the devices it may reach this way, each
/// route with its pin and the headers its requests carry. Clones share the
/// devices, so the caller can keep one to add devices as the book changes.
#[derive(Clone)]
pub struct PhoneWs {
    logins: Logins<Login>,
    connect: Connect,
}

/// One route's login.
#[derive(Clone)]
struct Login {
    route: WsRoute,
    headers: Headers,
    ended: Ended,
}

impl fmt::Debug for PhoneWs {
    /// Shows the routes it knows, never a header.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PhoneWs")
            .field("routes", &self.logins.names())
            .finish()
    }
}

impl Default for PhoneWs {
    fn default() -> Self {
        Self::new()
    }
}

impl PhoneWs {
    /// Knows no device yet.
    pub fn new() -> Self {
        Self {
            logins: Logins::default(),
            connect: reach::tcp(),
        }
    }

    /// Lets the phone reach `device` on `routes`, each trusting only its
    /// pin, with `headers` for every upgrade, and returns the routes in
    /// their order, for a [`crate::LinkSpec`]. `ended` hears when the
    /// device ends the phone's sign-in. A route trusted again replaces
    /// what it had.
    pub fn trust(
        &self,
        device: DeviceId,
        routes: &[WsRoute],
        headers: &Headers,
        ended: &Ended,
    ) -> Vec<Route> {
        self.logins.trust(
            device,
            routes.iter().map(|route| {
                let login = Login {
                    route: route.clone(),
                    headers: Arc::clone(headers),
                    ended: Arc::clone(ended),
                };
                (super::ws_route_host(&route.host, route.port), login)
            }),
        )
    }

    /// Whether `name` is one of `device`'s routes.
    #[cfg(feature = "russh")]
    pub(crate) fn knows(&self, device: Option<DeviceId>, name: &str) -> bool {
        self.logins.knows(device, name)
    }

    /// Posts `body` as JSON to `path` on `route`, such as `/pair` or
    /// `/token`, with the headers `headers` gives, and returns the answer.
    /// The error is a reason the app can show.
    pub async fn post(
        &self,
        route: &WsRoute,
        path: &str,
        headers: &Headers,
        body: Vec<u8>,
    ) -> Result<Posted, String> {
        let posting = async {
            let host = authority(&route.host, route.port);
            let stream = self.tls(route).await.map_err(|err| err.reason)?;
            let extra = headers("POST", format!("https://{host}{path}")).await?;
            let (mut sender, connection) =
                hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream))
                    .await
                    .map_err(|_| REASONS.closed.to_owned())?;
            let driving = tokio::spawn(connection);
            let mut request = hyper::Request::post(path)
                .header(HOST, host)
                .header(CONTENT_TYPE, "application/json");
            for (name, value) in header_pairs(extra)? {
                request = request.header(name, value);
            }
            let request = request
                .body(Full::new(Bytes::from(body)))
                .map_err(|_| "that request doesn't make sense".to_owned())?;
            let answer = sender.send_request(request).await;
            let posted = match answer {
                Ok(answer) => {
                    let status = answer.status().as_u16();
                    match Limited::new(answer.into_body(), ANSWER_CAP).collect().await {
                        Ok(body) => Ok(Posted {
                            status,
                            body: body.to_bytes().to_vec(),
                        }),
                        Err(_) => Err("the device's answer was too long".to_owned()),
                    }
                }
                Err(_) => Err(REASONS.closed.to_owned()),
            };
            driving.abort();
            posted
        };
        match tokio::time::timeout(LOGIN_TIMEOUT, posting).await {
            Ok(posted) => posted,
            Err(_) => Err(REASONS.timed_out.to_owned()),
        }
    }

    /// A TLS stream to `route` that trusts only its pin.
    pub(crate) async fn tls(&self, route: &WsRoute) -> Result<TlsStream<Box<dyn Io>>, DialError> {
        let config = pinned_config(&route.pin)?;
        let name = server_name(&route.host)?;
        let stream = reach::open(&self.connect, &route.host, route.port, &REASONS).await?;
        TlsConnector::from(config)
            .connect(name, stream)
            .await
            .map_err(|err| tls_failure(&err))
    }

    /// Opens the link's WebSocket on the route `name` to `device`.
    async fn open(&self, device: Option<DeviceId>, name: &str) -> Result<Conn, DialError> {
        let login = self.logins.get(device, name)?;
        let opening = async {
            let url = format!(
                "wss://{}{LINK_PATH}",
                authority(&login.route.host, login.route.port)
            );
            let stream = self.tls(&login.route).await?;
            // Signed once the device answers, so a dead route costs no proof.
            let extra = (login.headers)("GET", url.clone())
                .await
                .map_err(DialError::refused)?;
            let mut request = url
                .into_client_request()
                .map_err(|_| DialError::refused("that route doesn't make a URL"))?;
            for (name, value) in header_pairs(extra).map_err(DialError::refused)? {
                request.headers_mut().append(name, value);
            }
            let limits = WebSocketConfig::default()
                .max_message_size(Some(MAX_LINK_MESSAGE))
                .max_frame_size(Some(MAX_LINK_MESSAGE))
                .max_write_buffer_size(MAX_LINK_MESSAGE);
            let (socket, _) =
                tokio_tungstenite::client_async_with_config(request, stream, Some(limits))
                    .await
                    .map_err(|err| upgrade_failure(&err, &login.ended))?;
            Ok(conn(socket))
        };
        match tokio::time::timeout(LOGIN_TIMEOUT, opening).await {
            Ok(conn) => conn,
            Err(_) => Err(DialError::refused(
                "the device didn't finish opening the link",
            )),
        }
    }
}

impl Transport for PhoneWs {
    fn resolve(&self, _destination: &str) -> BoxFuture<'static, io::Result<SshConfig>> {
        Box::pin(async { Err(io::Error::other("network access reads no ssh config")) })
    }

    fn lookup(&self, _host: &str) -> BoxFuture<'static, Vec<IpAddr>> {
        Box::pin(async { Vec::new() })
    }

    fn dial<'a>(
        &'a self,
        route: &'a Route,
        _config: Option<&'a SshConfig>,
        device: Option<DeviceId>,
    ) -> BoxFuture<'a, Result<Conn, DialError>> {
        Box::pin(async move {
            let Route::Ssh { host } = route else {
                return Err(DialError::refused(
                    "the phone reaches devices over the network",
                ));
            };
            self.open(device, host).await
        })
    }

    /// Each dial opens afresh, so there's nothing to drop.
    fn forget<'a>(&'a self, _host: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    fn stagger(&self) -> Option<Duration> {
        Some(STAGGER)
    }
}

/// The caller's headers, checked.
fn header_pairs(pairs: Vec<(String, String)>) -> Result<Vec<(HeaderName, HeaderValue)>, String> {
    pairs
        .into_iter()
        .map(|(name, value)| {
            let name = HeaderName::from_bytes(name.as_bytes());
            let value = HeaderValue::from_str(&value);
            match (name, value) {
                (Ok(name), Ok(mut value)) => {
                    value.set_sensitive(true);
                    Ok((name, value))
                }
                _ => Err("a request header doesn't read".to_owned()),
            }
        })
        .collect()
}

/// The name TLS sends, an IP address sending none.
fn server_name(host: &str) -> Result<ServerName<'static>, DialError> {
    ServerName::try_from(host.to_owned())
        .map_err(|_| DialError::refused("that route's host isn't a name or an address"))
}

/// The SHA-256 a pin names.
fn read_pin(pin: &str) -> Option<[u8; 32]> {
    URL_SAFE_NO_PAD.decode(pin).ok()?.try_into().ok()
}

/// A client config that trusts only the certificate `pin` names.
fn pinned_config(pin: &str) -> Result<Arc<ClientConfig>, DialError> {
    let sha256 = read_pin(pin)
        .ok_or_else(|| DialError::refused("the device's certificate pin doesn't read"))?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let pin = Pin {
        sha256,
        algorithms: provider.signature_verification_algorithms,
    };
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| DialError::refused("TLS isn't set up in this build"))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(pin))
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// Accepts only the pinned certificate, and still checks that the server
/// holds its key.
#[derive(Debug)]
struct Pin {
    sha256: [u8; 32],
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for Pin {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let digest = ring::digest::digest(&ring::digest::SHA256, end_entity);
        if bool::from(digest.as_ref().ct_eq(&self.sha256)) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

/// What a failed TLS handshake means for the app. A device that showed
/// another certificate answered, so it's refused, not down.
fn tls_failure(err: &io::Error) -> DialError {
    let tls = err
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustls::Error>());
    match tls {
        Some(rustls::Error::InvalidCertificate(_)) => {
            DialError::refused("the device showed a different certificate")
        }
        Some(_) => DialError::refused("the TLS handshake failed"),
        None => reach::io_failure(err, &REASONS),
    }
}

/// What a failed upgrade means for the app. A token the device ended goes
/// to `ended`.
fn upgrade_failure(err: &tungstenite::Error, ended: &Ended) -> DialError {
    match err {
        tungstenite::Error::Http(answer) => {
            let refusal = answer
                .body()
                .as_deref()
                .and_then(|body| serde_json::from_slice::<NetRefusal>(body).ok());
            match (
                answer.status().as_u16(),
                refusal.map(|refusal| refusal.error),
            ) {
                (_, Some(NetError::Expired | NetError::Revoked)) => {
                    ended();
                    DialError::refused(SIGN_IN_ENDED)
                }
                (401 | 403, _) => DialError::refused("the device didn't take this phone's sign-in"),
                (404, _) => DialError::refused("that device has no network access"),
                (status, _) => DialError::refused(format!("the device answered {status}")),
            }
        }
        tungstenite::Error::Io(err) => reach::io_failure(err, &REASONS),
        _ => DialError::refused("the device didn't open the link"),
    }
}

/// A WebSocket as the link's stream.
pub(crate) fn conn<S>(socket: WebSocketStream<S>) -> Conn
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (sink, stream) = socket.split();
    Conn::new(
        Box::new(Reader {
            stream,
            held: Bytes::new(),
            ended: false,
        }),
        Box::new(Writer { sink }),
    )
}

/// The link's bytes out of binary messages. Pings get their pong as the
/// socket reads, and a close or a text message ends the stream.
struct Reader<S> {
    stream: SplitStream<WebSocketStream<S>>,
    /// What's left of the last message.
    held: Bytes,
    ended: bool,
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncRead for Reader<S> {
    fn poll_read(
        mut self: Pinned<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            if !self.held.is_empty() {
                let n = self.held.len().min(buf.remaining());
                buf.put_slice(&self.held.split_to(n));
                return Poll::Ready(Ok(()));
            }
            if self.ended {
                return Poll::Ready(Ok(()));
            }
            match ready!(self.stream.poll_next_unpin(cx)) {
                Some(Ok(Message::Binary(data))) => self.held = data,
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
                Some(Ok(Message::Text(_) | Message::Close(_))) | None => self.ended = true,
                Some(Err(err)) => {
                    self.ended = true;
                    return Poll::Ready(Err(io::Error::new(io::ErrorKind::ConnectionReset, err)));
                }
            }
        }
    }
}

/// The link's bytes into binary messages, at most [`CHUNK`] each.
struct Writer<S> {
    sink: SplitSink<WebSocketStream<S>, Message>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncWrite for Writer<S> {
    fn poll_write(
        mut self: Pinned<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        ready!(self.sink.poll_ready_unpin(cx)).map_err(broken)?;
        let n = buf.len().min(CHUNK);
        self.sink
            .start_send_unpin(Message::Binary(Bytes::copy_from_slice(&buf[..n])))
            .map_err(broken)?;
        Poll::Ready(Ok(n))
    }

    fn poll_flush(mut self: Pinned<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.sink.poll_flush_unpin(cx).map_err(broken)
    }

    fn poll_shutdown(mut self: Pinned<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.sink.poll_close_unpin(cx).map_err(broken)
    }
}

/// A WebSocket error as the stream's.
fn broken(err: tungstenite::Error) -> io::Error {
    match err {
        tungstenite::Error::Io(err) => err,
        err => io::Error::new(io::ErrorKind::BrokenPipe, err),
    }
}
