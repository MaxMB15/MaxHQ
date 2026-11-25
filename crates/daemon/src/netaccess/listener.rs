// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The listener: HTTP/1.1 over TLS on the device's port, on each private
//! address, while the switch is on. It follows the settings, and looks at
//! the addresses again every 15 seconds, so a new network or a new port
//! needs no restart. It publishes how it stands as [`NetworkAccess`].

use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::header::{
    HeaderValue, AUTHORIZATION, CONNECTION, CONTENT_TYPE, HOST, SEC_WEBSOCKET_ACCEPT,
    SEC_WEBSOCKET_KEY, SEC_WEBSOCKET_VERSION, UPGRADE,
};
use hyper::upgrade::Upgraded;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use maxhq_os::ipc::LocalStream;
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::netaccess::{
    NetError, NetGrant, NetPair, NetPaired, NetRefusal, NetworkAccess, LINK_PATH, MAX_BODY,
    MAX_LINK_MESSAGE, PAIR_PATH, TOKEN_PATH,
};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::ClientMessage;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, watch, Semaphore};
use tokio::task::{JoinHandle, JoinSet};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use tracing::{debug, info};

use super::{tls, Access};
use crate::secrets::files::Folder;

/// How often the listener looks at the addresses again.
const RECHECK: Duration = Duration::from_secs(15);

/// How long a phone has for its TLS handshake, then its headers, then its
/// body, so a slow one can't hold a connection open.
const READ_FOR: Duration = Duration::from_secs(10);

/// How much of `maxd`'s bytes one WebSocket message carries at most.
const CHUNK: usize = 64 * 1024;

/// How long the listener waits after it couldn't take a connection, which
/// can last, such as while `maxd` is out of files.
const ACCEPT_PAUSE: Duration = Duration::from_millis(100);

/// How many connections a listener holds at once.
#[derive(Clone, Copy, Debug)]
pub struct Caps {
    /// Connections that aren't a link, which anyone can open without a token.
    pub unproven: usize,
    /// How many of those one address may hold.
    pub per_address: usize,
    /// Links, which only a phone with a live session opens. They count apart
    /// from the rest, so strangers can't crowd them out.
    pub links: usize,
}

impl Caps {
    /// Plenty for the phones of one person, and few enough to stay well inside
    /// the files `maxd` may open.
    pub const STANDARD: Self = Self {
        unproven: 128,
        per_address: 16,
        links: 256,
    };
}

/// The addresses to listen on now.
pub type Addresses = Arc<dyn Fn() -> Vec<IpAddr> + Send + Sync>;

/// What the listener needs.
pub struct Parts {
    /// The tokens and proofs.
    pub access: Access,
    /// The folder that keeps the certificate.
    pub folder: Folder,
    /// `maxd`'s own socket, which each link joins.
    pub socket: PathBuf,
    /// How many connections each listener holds at once.
    pub caps: Caps,
    /// Where to listen: [`super::addrs::private`] outside tests.
    pub addresses: Addresses,
}

/// A task that stops when dropped.
struct AbortOnDrop(JoinHandle<()>);

impl AbortOnDrop {
    /// Stops the task and waits until tokio has dropped it, so its socket
    /// is closed by the time this returns.
    async fn stop(mut self) {
        self.0.abort();
        let _ = (&mut self.0).await;
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// What each connection shares.
#[derive(Clone)]
struct Ctx {
    access: Access,
    socket: Arc<PathBuf>,
    /// This listener's certificate pin, which a phone's proof names.
    pin: Arc<str>,
    /// Closes when its listener stops, and its links with it.
    stop: watch::Receiver<()>,
    /// What's left of the listener's links.
    links: Arc<Semaphore>,
}

/// How many connections each address holds.
#[derive(Clone, Default)]
struct Seats(Arc<Mutex<HashMap<IpAddr, usize>>>);

impl Seats {
    /// A place for one more connection from `ip`, unless it has `most`.
    fn take(&self, ip: IpAddr, most: usize) -> Option<Seat> {
        let mut held = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let count = held.entry(ip).or_default();
        if *count >= most {
            return None;
        }
        *count += 1;
        Some(Seat {
            seats: self.clone(),
            ip,
        })
    }
}

/// One connection's place in [`Seats`], given back when it's dropped.
struct Seat {
    seats: Seats,
    ip: IpAddr,
}

impl Drop for Seat {
    fn drop(&mut self) {
        let mut held = self.seats.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = held.get_mut(&self.ip) {
            *count -= 1;
            if *count == 0 {
                held.remove(&self.ip);
            }
        }
    }
}

/// Listens while the switch is on, on the port and the addresses it's
/// given, and publishes how it stands.
pub async fn run(parts: Parts) {
    let access = parts.access;
    let hub = access.hub().clone();
    let me = hub.device().id;
    let mut deltas = hub.subscribe(None).deltas;
    let socket = Arc::new(parts.socket);
    let mut tls: Option<tls::Tls> = None;
    let mut listening: BTreeMap<SocketAddr, AbortOnDrop> = BTreeMap::new();
    let mut published: Option<NetworkAccess> = None;
    loop {
        let settings = access.settings();
        let mut problem = None;
        let wanted: Vec<SocketAddr> = if !settings.on {
            Vec::new()
        } else if settings.port == 0 {
            problem = Some("Pick a port from 1 to 65535.".to_owned());
            Vec::new()
        } else {
            (parts.addresses)()
                .into_iter()
                .map(|ip| SocketAddr::new(ip, settings.port))
                .collect()
        };
        // A listener stopped here is closed before the state below says
        // so, and before its port could be bound again.
        let gone: Vec<SocketAddr> = listening
            .keys()
            .filter(|addr| !wanted.contains(addr))
            .copied()
            .collect();
        for addr in gone {
            if let Some(task) = listening.remove(&addr) {
                task.stop().await;
            }
        }
        if settings.on && tls.is_none() {
            match tls::load(&parts.folder) {
                Ok(loaded) => tls = Some(loaded),
                Err(err) => {
                    problem = Some(format!("This device couldn't make its certificate: {err}"));
                }
            }
        }
        if let Some(tls) = &tls {
            for addr in &wanted {
                if listening.contains_key(addr) {
                    continue;
                }
                match TcpListener::bind(addr).await {
                    Ok(listener) => {
                        info!(%addr, "network access listening");
                        let ctx = (access.clone(), socket.clone(), Arc::from(tls.pin.as_str()));
                        let acceptor = TlsAcceptor::from(tls.config.clone());
                        let serving = serve(listener, acceptor, ctx, parts.caps);
                        listening.insert(*addr, AbortOnDrop(tokio::spawn(serving)));
                    }
                    Err(err) => {
                        debug!(%addr, %err, "network access couldn't listen");
                        problem.get_or_insert_with(|| bind_problem(settings.port, &err));
                    }
                }
            }
        }
        if settings.on && wanted.is_empty() && problem.is_none() {
            problem = Some(
                "This device has no private address to listen on. Join a local network first."
                    .to_owned(),
            );
        }
        let state = NetworkAccess {
            device: me,
            on: settings.on,
            port: settings.port,
            hosts: listening.keys().map(|addr| addr.ip().to_string()).collect(),
            pin: tls.as_ref().map(|tls| tls.pin.clone()),
            problem,
        };
        if published.as_ref() != Some(&state) {
            hub.apply([Change::Upsert {
                value: Entity::NetworkAccess(state.clone()),
            }]);
            published = Some(state);
        }

        let recheck = access.clock().sleep_until(access.clock().now() + RECHECK);
        tokio::pin!(recheck);
        loop {
            tokio::select! {
                () = &mut recheck => break,
                delta = deltas.recv() => match delta {
                    Ok(delta) if delta.changes.iter().any(|change| matches!(
                        change,
                        Change::Upsert { value } if value.key() == EntityKey::DeviceSettings(me)
                    )) => break,
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => break,
                    Err(broadcast::error::RecvError::Closed) => return,
                },
            }
        }
    }
}

/// The problem to show when the port couldn't be opened.
fn bind_problem(port: u16, err: &io::Error) -> String {
    match err.kind() {
        io::ErrorKind::AddrInUse => {
            format!("Port {port} is in use on this device. Pick another port.")
        }
        io::ErrorKind::PermissionDenied => {
            format!("This device doesn't let maxd use port {port}. Pick one above 1023.")
        }
        _ => format!("This device couldn't listen on port {port}: {err}"),
    }
}

/// Answers every connection `listener` takes, until it's dropped, which
/// closes them and their links too.
async fn serve(
    listener: TcpListener,
    tls: TlsAcceptor,
    (access, socket, pin): (Access, Arc<PathBuf>, Arc<str>),
    caps: Caps,
) {
    let (_stop, stop) = watch::channel(());
    let ctx = Ctx {
        access,
        socket,
        pin,
        stop,
        links: Arc::new(Semaphore::new(caps.links)),
    };
    let unproven = Arc::new(Semaphore::new(caps.unproven));
    let seats = Seats::default();
    let mut connections = JoinSet::new();
    loop {
        // Taken before accepting, so a connection past the limit waits in the
        // system's queue and costs `maxd` no file.
        let Ok(permit) = unproven.clone().acquire_owned().await else {
            return;
        };
        let (stream, from) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(err) => {
                debug!(%err, "a network connection failed");
                tokio::time::sleep(ACCEPT_PAUSE).await;
                continue;
            }
        };
        let Some(seat) = seats.take(from.ip().to_canonical(), caps.per_address) else {
            debug!(%from, "an address holds too many network connections");
            continue;
        };
        while connections.try_join_next().is_some() {}
        let (tls, ctx) = (tls.clone(), ctx.clone());
        connections.spawn(async move {
            // Held until this ends, which for a link is once it has upgraded.
            let _held = (permit, seat);
            let stream = match tokio::time::timeout(READ_FOR, tls.accept(stream)).await {
                Ok(Ok(stream)) => stream,
                Ok(Err(err)) => return debug!(%err, "a network connection's TLS failed"),
                Err(_) => return debug!("a network connection's TLS took too long"),
            };
            let service = hyper::service::service_fn(move |request| {
                let ctx = ctx.clone();
                async move { Ok::<_, Infallible>(handle(&ctx, request).await) }
            });
            let served = hyper::server::conn::http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(READ_FOR)
                .serve_connection(TokioIo::new(stream), service)
                .with_upgrades()
                .await;
            if let Err(err) = served {
                debug!(%err, "a network connection ended early");
            }
        });
    }
}

/// Answers one request. A refusal is logged without anything secret in it.
async fn handle(ctx: &Ctx, request: Request<Incoming>) -> Response<Full<Bytes>> {
    let path = request.uri().path().to_owned();
    let answer = match (request.method(), path.as_str()) {
        (&Method::POST, PAIR_PATH) => pair(ctx, request).await,
        (&Method::POST, TOKEN_PATH) => token(ctx, request).await,
        (&Method::GET, LINK_PATH) => link(ctx, request).await,
        _ => Err(NetError::NotFound),
    };
    let mut response = answer.unwrap_or_else(|error| {
        debug!(path, ?error, "refused a network request");
        let status = StatusCode::from_u16(error.status()).unwrap_or(StatusCode::BAD_REQUEST);
        json(status, &NetRefusal { error })
    });
    // One request a connection unless it becomes a link, so nobody holds one
    // open by repeating requests. The builder's own switch would also write
    // this over the header a link's answer needs.
    if response.status() != StatusCode::SWITCHING_PROTOCOLS {
        response
            .headers_mut()
            .insert(CONNECTION, HeaderValue::from_static("close"));
    }
    response
}

/// What a request's headers give: the URL its proof names, the proof, and
/// its token. A phone names `wss` for the link and `https` for the rest.
struct Signed {
    url: String,
    proof: Option<String>,
    token: Option<String>,
}

fn signed(request: &Request<Incoming>, scheme: &str) -> Result<Signed, NetError> {
    let header = |name: &str| {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
    };
    let host = header(HOST.as_str()).ok_or(NetError::BadRequest)?;
    Ok(Signed {
        url: format!("{scheme}://{host}{}", request.uri().path()),
        proof: header("dpop").map(str::to_owned),
        token: header(AUTHORIZATION.as_str())
            .and_then(|value| value.strip_prefix("DPoP "))
            .map(str::to_owned),
    })
}

/// The body, read in time and up to [`MAX_BODY`].
async fn body<T: DeserializeOwned>(request: Request<Incoming>) -> Result<T, NetError> {
    let body = Limited::new(request.into_body(), MAX_BODY).collect();
    let body = tokio::time::timeout(READ_FOR, body)
        .await
        .map_err(|_| NetError::BadRequest)?
        .map_err(|_| NetError::BadRequest)?;
    serde_json::from_slice(&body.to_bytes()).map_err(|_| NetError::BadRequest)
}

/// `POST /pair`: takes the pairing whose seed the body carries, pairs the
/// phone whose key made the proof, and gives it a token.
async fn pair(ctx: &Ctx, request: Request<Incoming>) -> Result<Response<Full<Bytes>>, NetError> {
    let signed = signed(&request, "https")?;
    let body: NetPair = body(request).await?;
    let phones = crate::phones::installed().ok_or(NetError::BadCode)?;
    // Only the seed lets a request use the pairing, so a wrong one leaves
    // it waiting.
    if !phones.seed_fits(body.pair, &body.seed) {
        return Err(NetError::BadCode);
    }
    let key = crate::sshkey::ed25519_key(&body.ssh_public).ok_or(NetError::BadRequest)?;
    ctx.access.proof(
        signed.proof.as_deref(),
        "POST",
        &signed.url,
        &key,
        None,
        &ctx.pin,
    )?;
    let paired = tokio::task::spawn_blocking(move || {
        let pairing = phones.take(body.pair).map_err(|_| NetError::BadCode)?;
        Ok(phones.pair(pairing, &body.name, &body.ssh_public, &body.push_public))
    })
    .await
    .map_err(|_| NetError::Unavailable)??;
    let ActionOutcome::PhonePaired { phone, book, mac } = paired else {
        return Err(NetError::BadRequest);
    };
    let token = ctx.access.start(phone)?;
    let paired = NetPaired {
        phone,
        book,
        token,
        mac,
    };
    Ok(json(StatusCode::OK, &paired))
}

/// `POST /token`: trades a code, or swaps a token for a new one.
async fn token(ctx: &Ctx, request: Request<Incoming>) -> Result<Response<Full<Bytes>>, NetError> {
    let signed = signed(&request, "https")?;
    let token = match body(request).await? {
        NetGrant::Code { code } => {
            ctx.access
                .trade(&code, signed.proof.as_deref(), &signed.url, &ctx.pin)
        }
        NetGrant::Refresh => {
            let token = signed.token.ok_or(NetError::Revoked)?;
            ctx.access
                .refresh(&token, signed.proof.as_deref(), &signed.url, &ctx.pin)
        }
    }?;
    Ok(json(StatusCode::OK, &token))
}

/// `GET /link`: upgrades to a WebSocket joined to `maxd`'s socket as the
/// token's phone.
async fn link(
    ctx: &Ctx,
    mut request: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, NetError> {
    let signed = signed(&request, "wss")?;
    let headers = request.headers();
    let has = |name, want: &str| {
        headers.get_all(name).iter().any(|value: &HeaderValue| {
            value.to_str().is_ok_and(|value| {
                value
                    .split(',')
                    .any(|part| part.trim().eq_ignore_ascii_case(want))
            })
        })
    };
    if !has(UPGRADE, "websocket")
        || !has(CONNECTION, "upgrade")
        || !has(SEC_WEBSOCKET_VERSION, "13")
    {
        return Err(NetError::BadRequest);
    }
    let accept = headers
        .get(SEC_WEBSOCKET_KEY)
        .map(|key| derive_accept_key(key.as_bytes()))
        .ok_or(NetError::BadRequest)?;
    // Listen for the session's end before checking it, so none is missed.
    let ended = ctx.access.ended();
    let token = signed.token.ok_or(NetError::Revoked)?;
    let session = ctx.access.session(
        &token,
        signed.proof.as_deref(),
        "GET",
        &signed.url,
        &ctx.pin,
    )?;
    let slot = ctx
        .links
        .clone()
        .try_acquire_owned()
        .map_err(|_| NetError::Unavailable)?;

    let first = crate::proxy::first_frame(&ClientMessage::Phone {
        phone: session.phone,
    })
    .map_err(|_| NetError::Unavailable)?;
    let mut maxd = LocalStream::connect(ctx.socket.as_path())
        .await
        .map_err(|_| NetError::Unavailable)?;
    maxd.write_all(&first)
        .await
        .map_err(|_| NetError::Unavailable)?;
    let upgrade = hyper::upgrade::on(&mut request);
    let (access, stop) = (ctx.access.clone(), ctx.stop.clone());
    tokio::spawn(async move {
        let _slot = slot;
        let upgraded = match upgrade.await {
            Ok(upgraded) => upgraded,
            Err(err) => return debug!(%err, "a phone's link didn't upgrade"),
        };
        let limits = WebSocketConfig::default()
            .max_message_size(Some(MAX_LINK_MESSAGE))
            .max_frame_size(Some(MAX_LINK_MESSAGE))
            .max_write_buffer_size(MAX_LINK_MESSAGE);
        let socket =
            WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, Some(limits))
                .await;
        join(socket, maxd, session.id, &access, ended, stop).await;
    });

    let mut response = Response::new(Full::new(Bytes::new()));
    *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    let headers = response.headers_mut();
    headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
    headers.insert(CONNECTION, HeaderValue::from_static("Upgrade"));
    if let Ok(accept) = HeaderValue::from_str(&accept) {
        headers.insert(SEC_WEBSOCKET_ACCEPT, accept);
    }
    Ok(response)
}

/// Carries bytes both ways, each WebSocket binary message as it came,
/// until either side closes, the session ends or the listener stops.
async fn join(
    socket: WebSocketStream<TokioIo<Upgraded>>,
    maxd: LocalStream,
    session: maxhq_protocol::ids::PhoneSessionId,
    access: &Access,
    mut ended: broadcast::Receiver<maxhq_protocol::ids::PhoneSessionId>,
    mut stop: watch::Receiver<()>,
) {
    let (mut to_phone, mut from_phone) = socket.split();
    let (mut from_maxd, mut to_maxd) = maxd.into_split();
    let mut buf = vec![0; CHUNK];
    loop {
        tokio::select! {
            read = from_maxd.read(&mut buf) => match read {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if to_phone.send(Message::binary(buf[..n].to_vec())).await.is_err() {
                        break;
                    }
                }
            },
            message = from_phone.next() => match message {
                Some(Ok(Message::Binary(data))) => {
                    if to_maxd.write_all(&data).await.is_err() {
                        break;
                    }
                }
                // The pong a ping asks for goes out on the next flush.
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {
                    let _ = to_phone.flush().await;
                }
                Some(Ok(_) | Err(_)) | None => break,
            },
            gone = ended.recv() => match gone {
                Ok(id) if id == session => break,
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    if !access.live(session) {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            _ = stop.changed() => break,
        }
    }
    let _ = to_phone.send(Message::Close(None)).await;
}

fn json(status: StatusCode, body: &impl Serialize) -> Response<Full<Bytes>> {
    let body = serde_json::to_vec(body).unwrap_or_default();
    let mut response = Response::new(Full::new(Bytes::from(body)));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}
