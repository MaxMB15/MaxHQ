// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Network access end to end on 127.0.0.1: a phone pairs, trades a code,
//! links to `maxd` as itself, and is refused once its session ends.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use futures::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::StatusCode;
use hyper_util::rt::TokioIo;
use maxhq_core::dpop;
use maxhq_core::prove::Kind;
use maxhq_protocol::frame::{self, Frame, FrameDecoder};
use maxhq_protocol::ids::{ClientId, DeviceId, IdempotencyKey, PhoneId};
use maxhq_protocol::message::{ActionOutcome, ErrorCode};
use maxhq_protocol::netaccess::{
    NetError, NetGrant, NetPair, NetPaired, NetRefusal, NetToken, NetworkAccess, CODE_FOR,
    LINK_PATH, MAX_LINK_MESSAGE, PAIR_PATH, TOKEN_FOR, TOKEN_PATH,
};
use maxhq_protocol::phones::{PhoneBookEntry, PushEnvironment, PushService, PushToken};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::{ClientMessage, ProtocolRange, Secret, ServerMessage};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use serde::de::DeserializeOwned;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::WebSocketStream;

use super::listener::{self, Caps, Parts};
use super::Access;
use crate::app_keys::testing;
use crate::phones::openssh_for_tests as openssh;
use crate::secrets::files::Folder;
use crate::server::tests::{hello, serve, serve_on, Served};
use crate::session::clock::{Clock, FakeClock, SystemClock};
use crate::testing::TempDir;

const WAIT: Duration = Duration::from_secs(10);

/// The phone's own key, which signs its proofs.
const PHONE: [u8; 32] = [211; 32];

/// Trusts only the certificate with the pin, as a phone does.
#[derive(Debug)]
struct Pinned {
    pin: String,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        cert: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if super::tls::pin(cert) == self.pin {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("not the pinned certificate".into()))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let algorithms = &self.provider.signature_verification_algorithms;
        verify_tls12_signature(message, cert, dss, algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let algorithms = &self.provider.signature_verification_algorithms;
        verify_tls13_signature(message, cert, dss, algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// A phone that reaches the listener on `port` and pins `pin`.
struct Phone {
    port: u16,
    tls: TlsConnector,
    jti: u32,
}

impl Phone {
    fn new(port: u16, pin: &str) -> Self {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = Pinned {
            pin: pin.to_owned(),
            provider: provider.clone(),
        };
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth();
        Self {
            port,
            tls: TlsConnector::from(Arc::new(config)),
            jti: 0,
        }
    }

    /// The URL a proof names, as the phone writes it.
    fn url(&self, method: &str, path: &str) -> String {
        let scheme = if method == "GET" { "wss" } else { "https" };
        format!("{scheme}://127.0.0.1:{}{path}", self.port)
    }

    /// A new proof for `method` on `path`.
    fn proof(&mut self, method: &str, path: &str, token: Option<&str>) -> String {
        self.proof_for(method, path, token, None)
    }

    /// A new proof for `method` on `path` that names the certificate `pin`.
    fn proof_for(
        &mut self,
        method: &str,
        path: &str,
        token: Option<&str>,
        pin: Option<&str>,
    ) -> String {
        self.jti += 1;
        let now = i64::try_from(SystemClock.wall().0 / 1000).unwrap();
        dpop::sign_for_pin(
            &PHONE,
            method,
            &self.url(method, path),
            token,
            pin,
            &self.jti.to_string(),
            now,
        )
        .unwrap()
    }

    async fn connect(&self) -> TlsStream<TcpStream> {
        self.try_connect().await.unwrap()
    }

    /// A TLS connection, or the error from a listener that turned it away.
    async fn try_connect(&self) -> std::io::Result<TlsStream<TcpStream>> {
        let tcp = TcpStream::connect((Ipv4Addr::LOCALHOST, self.port)).await?;
        let name = ServerName::try_from("maxd").unwrap();
        self.tls.connect(name, tcp).await
    }

    /// Posts `body` to `path` with `proof`, and the answer.
    async fn post(
        &self,
        path: &str,
        token: Option<&str>,
        proof: &str,
        body: &impl serde::Serialize,
    ) -> (StatusCode, Bytes) {
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake(TokioIo::new(self.connect().await))
                .await
                .unwrap();
        tokio::spawn(connection);
        let mut request = hyper::Request::post(path)
            .header("host", format!("127.0.0.1:{}", self.port))
            .header("content-type", "application/json")
            .header("dpop", proof);
        if let Some(token) = token {
            request = request.header("authorization", format!("DPoP {token}"));
        }
        let request = request
            .body(Full::new(Bytes::from(serde_json::to_vec(body).unwrap())))
            .unwrap();
        let response = timeout(WAIT, sender.send_request(request))
            .await
            .unwrap()
            .unwrap();
        let status = response.status();
        (
            status,
            response.into_body().collect().await.unwrap().to_bytes(),
        )
    }

    /// Opens the link with `token` and `proof`.
    async fn link(
        &self,
        token: &str,
        proof: &str,
    ) -> Result<WebSocketStream<TlsStream<TcpStream>>, (StatusCode, String)> {
        let mut request = format!("wss://127.0.0.1:{}{LINK_PATH}", self.port)
            .into_client_request()
            .unwrap();
        let headers = request.headers_mut();
        headers.insert("authorization", format!("DPoP {token}").parse().unwrap());
        headers.insert("dpop", proof.parse().unwrap());
        match tokio_tungstenite::client_async(request, self.connect().await).await {
            Ok((socket, _)) => Ok(socket),
            Err(tungstenite::Error::Http(response)) => {
                let body = response.body().clone().unwrap_or_default();
                Err((response.status(), String::from_utf8(body).unwrap()))
            }
            Err(err) => panic!("{err}"),
        }
    }
}

fn ok<T: DeserializeOwned>((status, body): (StatusCode, Bytes)) -> T {
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice(&body).unwrap()
}

fn refusal((status, body): (StatusCode, Bytes)) -> (StatusCode, String) {
    let refusal: NetRefusal = serde_json::from_slice(&body).unwrap();
    (
        status,
        serde_json::to_value(refusal.error)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned(),
    )
}

async fn send(socket: &mut WebSocketStream<TlsStream<TcpStream>>, message: &ClientMessage) {
    let mut out = Vec::new();
    frame::encode_message(&mut out, message).unwrap();
    socket.send(Message::binary(out)).await.unwrap();
}

/// The next message `maxd` sends over the link.
async fn recv(
    socket: &mut WebSocketStream<TlsStream<TcpStream>>,
    decoder: &mut FrameDecoder,
) -> ServerMessage {
    loop {
        if let Some(Frame::Message(json)) = decoder.next_frame().unwrap() {
            return serde_json::from_slice(json).unwrap();
        }
        match timeout(WAIT, socket.next()).await.unwrap() {
            Some(Ok(Message::Binary(data))) => decoder.push(&data),
            other => panic!("expected a frame, got {other:?}"),
        }
    }
}

/// `maxd` on a socket, with network access on and its listener on
/// 127.0.0.1, and the listener's state once it listens.
async fn listening(served: &Served, dir: &TempDir) -> (Access, NetworkAccess) {
    listening_with(served, dir, Caps::STANDARD).await
}

/// The same, with the listener holding at most `caps` connections.
async fn listening_with(served: &Served, dir: &TempDir, caps: Caps) -> (Access, NetworkAccess) {
    let port = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let hub = served.hub.clone();
    let me = hub.device().id;
    let Some(Entity::DeviceSettings(mut settings)) = hub.get(&EntityKey::DeviceSettings(me)) else {
        panic!("the device has settings");
    };
    settings.network.on = true;
    settings.network.port = port;
    hub.apply([Change::Upsert {
        value: Entity::DeviceSettings(settings),
    }]);
    let access = Access::open(hub.clone(), served.db.clone(), Arc::new(SystemClock));
    let mut deltas = hub.subscribe(None).deltas;
    tokio::spawn(listener::run(Parts {
        access: access.clone(),
        folder: Folder::open(dir.path().join(super::DIR)).unwrap(),
        socket: served.socket.clone(),
        caps,
        addresses: Arc::new(|| vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]),
    }));
    let state = timeout(WAIT, async {
        loop {
            if let Some(Entity::NetworkAccess(state)) = hub.get(&EntityKey::NetworkAccess(me)) {
                return state;
            }
            let _ = deltas.recv().await;
        }
    })
    .await
    .unwrap();
    (access, state)
}

/// A fake device that shows its own certificate can carry a phone's
/// request to the real one and set any `Host` it likes. The proof names the
/// certificate the phone checked, so the real listener turns it away on
/// every route, while a phone that checked the real one gets through.
#[tokio::test]
async fn a_proof_naming_another_certificate_opens_nothing_here() {
    const ELSEWHERE: Option<&str> = Some("the fake device's pin");
    let phones = crate::phones::for_tests();
    let served = serve();
    let dir = TempDir::new();
    let (access, state) = listening(&served, &dir).await;
    let pin = state.pin.clone().unwrap();
    let mut phone = Phone::new(state.port, &pin);
    let book = crate::phones::book_for_tests();
    let ActionOutcome::PhonePair { pair, seed, .. } = phones.begin(book, None) else {
        panic!("expected a pairing");
    };
    let pairing = NetPair {
        pair,
        seed,
        name: "iPhone".into(),
        ssh_public: openssh(&PHONE),
        push_public: STANDARD.encode([9u8; 32]),
    };
    let bad_proof = (StatusCode::UNAUTHORIZED, "bad_proof".to_owned());

    // Pairing.
    let proof = phone.proof_for("POST", PAIR_PATH, None, ELSEWHERE);
    let refused = phone.post(PAIR_PATH, None, &proof, &pairing).await;
    assert_eq!(refusal(refused), bad_proof);
    let proof = phone.proof_for("POST", PAIR_PATH, None, Some(&pin));
    let paired: NetPaired = ok(phone.post(PAIR_PATH, None, &proof, &pairing).await);
    let token = paired.token.token.expose().to_owned();

    // Trading a code, and refreshing a token.
    let ActionOutcome::PhoneAdded { code } = access.added(paired.phone) else {
        panic!("expected a code");
    };
    let grant = NetGrant::Code { code };
    let proof = phone.proof_for("POST", TOKEN_PATH, None, ELSEWHERE);
    let refused = phone.post(TOKEN_PATH, None, &proof, &grant).await;
    assert_eq!(refusal(refused), bad_proof);
    let proof = phone.proof_for("POST", TOKEN_PATH, Some(&token), ELSEWHERE);
    let refused = phone
        .post(TOKEN_PATH, Some(&token), &proof, &NetGrant::Refresh)
        .await;
    assert_eq!(refusal(refused), bad_proof);

    // The link.
    let proof = phone.proof_for("GET", LINK_PATH, Some(&token), ELSEWHERE);
    assert!(phone.link(&token, &proof).await.is_err());

    // A phone from before the claim, and one that names this listener.
    for named in [None, Some(pin.as_str())] {
        let proof = phone.proof_for("GET", LINK_PATH, Some(&token), named);
        let mut link = phone.link(&token, &proof).await.unwrap();
        let mut decoder = FrameDecoder::new();
        send(&mut link, &hello(ProtocolRange::OURS)).await;
        assert!(matches!(
            recv(&mut link, &mut decoder).await,
            ServerMessage::Hello(_)
        ));
    }
}

/// Pairs a phone over `phone`, and gives its token.
async fn paired_token(phone: &mut Phone) -> String {
    let phones = crate::phones::installed().unwrap();
    let ActionOutcome::PhonePair { pair, seed, .. } =
        phones.begin(crate::phones::book_for_tests(), None)
    else {
        panic!("expected a pairing");
    };
    let proof = phone.proof("POST", PAIR_PATH, None);
    let pairing = NetPair {
        pair,
        seed,
        name: "iPhone".into(),
        ssh_public: openssh(&PHONE),
        push_public: STANDARD.encode([9u8; 32]),
    };
    let paired: NetPaired = ok(phone.post(PAIR_PATH, None, &proof, &pairing).await);
    paired.token.token.expose().to_owned()
}

/// A request a connection keeps open by repeating it would hold a file for
/// as long as it liked, with no token.
#[tokio::test]
async fn a_connection_gets_one_request() {
    let served = serve();
    let dir = TempDir::new();
    let (_access, state) = listening(&served, &dir).await;
    let phone = Phone::new(state.port, state.pin.as_deref().unwrap());

    let mut connection = phone.connect().await;
    connection
        .write_all(b"GET /nope HTTP/1.1\r\nhost: x\r\n\r\n")
        .await
        .unwrap();
    let mut answer = Vec::new();
    let read = timeout(WAIT, connection.read_to_end(&mut answer)).await;
    assert!(read.is_ok(), "the connection stayed open");
    assert!(answer.starts_with(b"HTTP/1.1 404"), "{answer:?}");
}

#[tokio::test]
async fn one_address_cant_hold_more_than_its_share() {
    let served = serve();
    let dir = TempDir::new();
    let caps = Caps {
        unproven: 8,
        per_address: 2,
        links: 8,
    };
    let (_access, state) = listening_with(&served, &dir, caps).await;
    let phone = Phone::new(state.port, state.pin.as_deref().unwrap());
    let stranger = || TcpStream::connect((Ipv4Addr::LOCALHOST, state.port));

    let (first, second) = (stranger().await.unwrap(), stranger().await.unwrap());
    // The listener would give a silent connection 10 seconds.
    let mut third = stranger().await.unwrap();
    let mut byte = [0u8; 1];
    let closed = timeout(Duration::from_secs(5), third.read(&mut byte)).await;
    assert!(matches!(closed, Ok(Ok(0) | Err(_))), "{closed:?}");

    // Its share comes back when those go. Until the listener sees them go,
    // it still turns this address away, at times during the handshake.
    drop((first, second));
    timeout(WAIT, async {
        loop {
            if let Ok(mut connection) = phone.try_connect().await {
                let asked = connection
                    .write_all(b"GET /nope HTTP/1.1\r\nhost: x\r\n\r\n")
                    .await;
                let mut answer = Vec::new();
                let _ = connection.read_to_end(&mut answer).await;
                if asked.is_ok() && answer.starts_with(b"HTTP/1.1 404") {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_connection_past_the_limit_waits_outside_maxd() {
    let served = serve();
    let dir = TempDir::new();
    let caps = Caps {
        unproven: 2,
        per_address: 8,
        links: 8,
    };
    let (_access, state) = listening_with(&served, &dir, caps).await;
    let phone = Phone::new(state.port, state.pin.as_deref().unwrap());
    let stranger = || TcpStream::connect((Ipv4Addr::LOCALHOST, state.port));

    let (first, _second) = (stranger().await.unwrap(), stranger().await.unwrap());
    let mut waiting = Box::pin(phone.connect());
    assert!(
        timeout(Duration::from_millis(500), &mut waiting)
            .await
            .is_err(),
        "the listener took a connection past its limit"
    );
    drop(first);
    assert!(timeout(WAIT, waiting).await.is_ok());
}

/// A link gives its place back once it has upgraded, so strangers filling the
/// listener leave it alone and it doesn't leave them short. Links have a limit
/// of their own.
#[tokio::test]
async fn links_count_apart_from_connections_and_have_their_own_limit() {
    crate::phones::for_tests();
    let served = serve();
    let dir = TempDir::new();
    let caps = Caps {
        unproven: 1,
        per_address: 8,
        links: 1,
    };
    let (_access, state) = listening_with(&served, &dir, caps).await;
    let mut phone = Phone::new(state.port, state.pin.as_deref().unwrap());
    let token = paired_token(&mut phone).await;

    let proof = phone.proof("GET", LINK_PATH, Some(&token));
    let mut link = phone.link(&token, &proof).await.unwrap();

    // A stranger takes the one place, and the link goes on.
    let stranger = TcpStream::connect((Ipv4Addr::LOCALHOST, state.port))
        .await
        .unwrap();
    let mut decoder = FrameDecoder::new();
    send(&mut link, &hello(ProtocolRange::OURS)).await;
    assert!(matches!(
        recv(&mut link, &mut decoder).await,
        ServerMessage::Hello(_)
    ));
    drop(stranger);

    // A second link is one too many, until the first has closed.
    let proof = phone.proof("GET", LINK_PATH, Some(&token));
    let refused = timeout(WAIT, phone.link(&token, &proof)).await.unwrap();
    let Err((status, _)) = refused else {
        panic!("a link past the limit opened");
    };
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    link.close(None).await.unwrap();
    timeout(WAIT, async {
        loop {
            let proof = phone.proof("GET", LINK_PATH, Some(&token));
            if phone.link(&token, &proof).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_phone_pairs_trades_a_code_and_links_until_its_session_ends() {
    let phones = crate::phones::for_tests();
    let served = serve();
    let dir = TempDir::new();
    let (access, state) = listening(&served, &dir).await;
    assert_eq!(state.hosts, ["127.0.0.1"]);
    assert_eq!(state.problem, None);
    let mut phone = Phone::new(state.port, state.pin.as_deref().unwrap());

    // Pair with the seed of a real pairing.
    let book = crate::phones::book_for_tests();
    let ActionOutcome::PhonePair { pair, seed, .. } = phones.begin(book.clone(), None) else {
        panic!("expected a pairing");
    };
    let pairing = |seed: Secret| NetPair {
        pair,
        seed,
        name: "iPhone".into(),
        ssh_public: openssh(&PHONE),
        push_public: STANDARD.encode([9u8; 32]),
    };
    let proof = phone.proof("POST", PAIR_PATH, None);
    let wrong = Secret::new(STANDARD.encode([1u8; 32]));
    let refused = phone.post(PAIR_PATH, None, &proof, &pairing(wrong)).await;
    assert_eq!(refusal(refused), (StatusCode::FORBIDDEN, "bad_code".into()));
    let proof = phone.proof("POST", PAIR_PATH, None);
    let paired: NetPaired = ok(phone.post(PAIR_PATH, None, &proof, &pairing(seed)).await);
    assert_eq!(paired.phone, PhoneId::derive(&openssh(&PHONE)));
    assert_eq!(paired.book, book);
    assert!(phones.knows(paired.phone));

    // Refresh swaps the token, and the old one stops working.
    let old = paired.token.token.expose().to_owned();
    let proof = phone.proof("POST", TOKEN_PATH, Some(&old));
    let new: NetToken = ok(phone
        .post(TOKEN_PATH, Some(&old), &proof, &NetGrant::Refresh)
        .await);
    assert_eq!(new.session, paired.token.session);
    let proof = phone.proof("POST", TOKEN_PATH, Some(&old));
    let again = phone
        .post(TOKEN_PATH, Some(&old), &proof, &NetGrant::Refresh)
        .await;
    assert_eq!(refusal(again), (StatusCode::UNAUTHORIZED, "revoked".into()));

    // A code from the Mac works once, and a proof works once.
    let ActionOutcome::PhoneAdded { code } = access.added(paired.phone) else {
        panic!("expected a code");
    };
    let proof = phone.proof("POST", TOKEN_PATH, None);
    let grant = NetGrant::Code { code: code.clone() };
    let traded: NetToken = ok(phone.post(TOKEN_PATH, None, &proof, &grant).await);
    let ActionOutcome::PhoneAdded { code: second } = access.added(paired.phone) else {
        panic!("expected a code");
    };
    let replayed = NetGrant::Code { code: second };
    let refused = phone.post(TOKEN_PATH, None, &proof, &replayed).await;
    assert_eq!(
        refusal(refused),
        (StatusCode::UNAUTHORIZED, "bad_proof".into())
    );
    let proof = phone.proof("POST", TOKEN_PATH, None);
    let refused = phone.post(TOKEN_PATH, None, &proof, &grant).await;
    assert_eq!(refusal(refused), (StatusCode::FORBIDDEN, "bad_code".into()));

    // The link reaches maxd as the phone, which proves it holds the
    // phone's key like any other connection.
    let token = traded.token.expose().to_owned();
    let proof = phone.proof("GET", LINK_PATH, Some(&token));
    let mut link = phone.link(&token, &proof).await.unwrap();
    let mut decoder = FrameDecoder::new();
    send(&mut link, &hello(ProtocolRange::OURS)).await;
    let ServerMessage::Hello(server) = recv(&mut link, &mut decoder).await else {
        panic!("expected a hello");
    };
    send(&mut link, &ClientMessage::Sync { since: None }).await;
    assert!(matches!(
        recv(&mut link, &mut decoder).await,
        ServerMessage::Error {
            code: ErrorCode::NotAllowed,
            ..
        }
    ));
    let me = ClientId::derive("test");
    let proved = testing::prove_as(Kind::Phone, &PHONE, &server, me);
    send(&mut link, &proved).await;
    assert!(matches!(
        recv(&mut link, &mut decoder).await,
        ServerMessage::ActionResult {
            outcome: ActionOutcome::Done,
            ..
        }
    ));
    send(&mut link, &ClientMessage::Presence { agent: None }).await;
    send(
        &mut link,
        &ClientMessage::SetPushToken {
            key: IdempotencyKey::derive("net-token"),
            push: PushToken {
                token: Secret::new("00ff".repeat(8)),
                topic: "com.example.maxhq".into(),
                environment: PushEnvironment::Sandbox,
                kinds: Vec::new(),
                service: PushService::Apns,
            },
        },
    )
    .await;
    // Only a phone's own connection gets past that check.
    let ServerMessage::ActionResult {
        outcome: ActionOutcome::Refused { reason },
        ..
    } = recv(&mut link, &mut decoder).await
    else {
        panic!("expected an answer");
    };
    assert_ne!(reason, crate::push::ONLY_PHONES);
    let refused = phone.link(&token, &proof).await.unwrap_err();
    assert_eq!(refused.0, StatusCode::UNAUTHORIZED);
    assert!(refused.1.contains("bad_proof"), "{}", refused.1);

    // Revoking the session closes the link, and refuses the next.
    assert_eq!(access.revoke(traded.session), ActionOutcome::Done);
    let closed = timeout(WAIT, async {
        loop {
            match link.next().await {
                Some(Ok(Message::Binary(_))) => {}
                _ => return,
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "the link closes");
    let proof = phone.proof("GET", LINK_PATH, Some(&token));
    let refused = phone.link(&token, &proof).await.unwrap_err();
    assert_eq!(refused.0, StatusCode::UNAUTHORIZED);
    assert!(refused.1.contains("revoked"), "{}", refused.1);
}

/// A code from the Mac works for ten minutes, while the token a phone
/// trades it for still works for thirty days.
#[tokio::test]
async fn a_code_is_refused_after_ten_minutes() {
    assert_eq!(CODE_FOR, Duration::from_secs(10 * 60));
    assert_eq!(TOKEN_FOR, Duration::from_secs(30 * 24 * 60 * 60));
    let served = serve();
    let hub = served.hub.clone();
    let me = hub.device().id;
    let Some(Entity::DeviceSettings(mut settings)) = hub.get(&EntityKey::DeviceSettings(me)) else {
        panic!("the device has settings");
    };
    settings.network.on = true;
    hub.apply([Change::Upsert {
        value: Entity::DeviceSettings(settings),
    }]);
    let clock = Arc::new(FakeClock::new());
    let access = Access::open(hub, served.db.clone(), clock.clone());
    let phone = PhoneId::derive(&openssh(&PHONE));
    let ActionOutcome::PhoneAdded { code } = access.added(phone) else {
        panic!("expected a code");
    };

    // Just before, it's still good: only the missing proof keeps it back.
    clock.advance(CODE_FOR - Duration::from_secs(1));
    assert_ne!(access.trade(&code, None, "", ""), Err(NetError::Expired));
    clock.advance(Duration::from_secs(1));
    assert_eq!(access.trade(&code, None, "", ""), Err(NetError::Expired));
    assert_eq!(NetError::Expired.status(), 401);
}

#[tokio::test]
async fn a_message_past_the_limit_ends_the_link() {
    let phones = crate::phones::for_tests();
    let served = serve();
    let dir = TempDir::new();
    let (_access, state) = listening(&served, &dir).await;
    let mut phone = Phone::new(state.port, state.pin.as_deref().unwrap());
    let book = crate::phones::book_for_tests();
    let ActionOutcome::PhonePair { pair, seed, .. } = phones.begin(book, None) else {
        panic!("expected a pairing");
    };
    let pairing = NetPair {
        pair,
        seed,
        name: "iPhone".into(),
        ssh_public: openssh(&PHONE),
        push_public: STANDARD.encode([9u8; 32]),
    };
    let proof = phone.proof("POST", PAIR_PATH, None);
    let paired: NetPaired = ok(phone.post(PAIR_PATH, None, &proof, &pairing).await);
    let token = paired.token.token.expose().to_owned();
    let proof = phone.proof("GET", LINK_PATH, Some(&token));
    let mut link = phone.link(&token, &proof).await.unwrap();
    let mut decoder = FrameDecoder::new();
    send(&mut link, &hello(ProtocolRange::OURS)).await;
    assert!(matches!(
        recv(&mut link, &mut decoder).await,
        ServerMessage::Hello(_)
    ));

    // Pings in one message, up to a given size, each a frame maxd answers.
    let pings = |size: usize| {
        let mut out = Vec::new();
        while out.len() < size {
            frame::encode_message(&mut out, &ClientMessage::Ping { nonce: 7 }).unwrap();
        }
        out
    };

    // The most either end sends at a time goes through.
    link.send(Message::binary(pings(64 * 1024))).await.unwrap();
    while !matches!(
        recv(&mut link, &mut decoder).await,
        ServerMessage::Pong { nonce: 7 }
    ) {}

    // One past the limit closes the link, so maxd never answers its frames.
    // The send itself can fail, once the link closes partway through it.
    let _ = link
        .send(Message::binary(pings(MAX_LINK_MESSAGE + 1)))
        .await;
    let closed = timeout(WAIT, async {
        loop {
            match link.next().await {
                Some(Ok(Message::Binary(_))) => {}
                _ => return,
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "the link closes");
}

#[tokio::test]
async fn turning_the_switch_off_stops_the_listener() {
    let served = serve();
    let dir = TempDir::new();
    let (_access, state) = listening(&served, &dir).await;
    let hub = served.hub.clone();
    let me = hub.device().id;
    let mut deltas = hub.subscribe(None).deltas;
    let Some(Entity::DeviceSettings(mut settings)) = hub.get(&EntityKey::DeviceSettings(me)) else {
        panic!("the device has settings");
    };
    settings.network.on = false;
    hub.apply([Change::Upsert {
        value: Entity::DeviceSettings(settings),
    }]);
    timeout(WAIT, async {
        loop {
            if let Some(Entity::NetworkAccess(now)) = hub.get(&EntityKey::NetworkAccess(me))
                && !now.on
                && now.hosts.is_empty()
            {
                return;
            }
            let _ = deltas.recv().await;
        }
    })
    .await
    .unwrap();
    // The listener closes before the state says so. Another test running now
    // can take the freed port, so a connection there must not reach this
    // listener's certificate.
    let Ok(tcp) = TcpStream::connect((Ipv4Addr::LOCALHOST, state.port)).await else {
        return;
    };
    let phone = Phone::new(state.port, state.pin.as_deref().unwrap());
    let name = ServerName::try_from("maxd").unwrap();
    let ours = timeout(WAIT, phone.tls.connect(name, tcp)).await;
    assert!(!matches!(ours, Ok(Ok(_))), "the listener still answers");
}

/// The phone's core paired with `maxd` over its network access, through
/// the code the Mac would show, with its keys in memory.
mod phone_core {
    use std::future::Future;

    use maxhq_app::pair_code::PairCode;
    use maxhq_app::phone::book_link;
    use maxhq_app::request::{Request, Response};
    use maxhq_mobile::{Core, KeyStore, MemoryStore, Platform};
    use maxhq_protocol::link::LinkState;
    use maxhq_protocol::netaccess::{PhoneNetwork, PhoneSession};
    use zeroize::Zeroizing;

    use super::*;

    /// Waits until the core's view has an entity `wanted` takes.
    async fn view_until(core: &Core, wanted: impl Fn(&Entity) -> bool) {
        timeout(WAIT, async {
            loop {
                let Ok(Response::ViewSnapshot(snapshot)) =
                    core.request(Request::ViewSnapshot).await
                else {
                    panic!("expected the view");
                };
                if snapshot.entities.iter().any(&wanted) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
    }

    /// Waits until the core's link to `device` is in a state `wanted`
    /// takes.
    async fn link_until(core: &Core, device: DeviceId, wanted: impl Fn(&LinkState) -> bool) {
        let link = book_link(device);
        view_until(core, |entity| {
            matches!(entity, Entity::DeviceLink(each) if each.id == link && wanted(&each.state))
        })
        .await;
    }

    /// The session `maxd` holds for `phone` once its link has opened.
    async fn used_session(served: &Served, phone: PhoneId) -> PhoneSession {
        let hub = served.hub.clone();
        let mut deltas = hub.subscribe(None).deltas;
        timeout(WAIT, async {
            loop {
                let used = hub
                    .snapshot()
                    .entities
                    .into_iter()
                    .find_map(|entity| match entity {
                        Entity::PhoneSession(session)
                            if session.phone == phone && session.last_used.is_some() =>
                        {
                            Some(session)
                        }
                        _ => None,
                    });
                if let Some(session) = used {
                    return session;
                }
                let _ = deltas.recv().await;
            }
        })
        .await
        .unwrap()
    }

    /// Drops the core off the async runtime, since it holds a runtime of
    /// its own.
    fn dropped(core: Core) -> impl Future<Output = ()> {
        let dropping = tokio::task::spawn_blocking(move || drop(core));
        async move { dropping.await.unwrap() }
    }

    #[tokio::test]
    async fn the_phone_pairs_over_the_network_links_as_itself_and_a_revoke_ends_it() {
        let phones = crate::phones::for_tests();
        // The phones service pairs for devbox, so the listener is devbox too.
        // The phone's link takes a hello from the device its book names alone.
        let served = serve_on(crate::secrets::testing::hub_for("devbox"));
        let dir = TempDir::new();
        let (access, state) = listening(&served, &dir).await;
        let devbox = DeviceId::derive("devbox");
        let network = PhoneNetwork {
            hosts: state.hosts.clone(),
            port: state.port,
            pin: state.pin.clone().unwrap(),
            code: None,
        };
        // The Mac's book: devbox, with its network access alone.
        let book = vec![PhoneBookEntry {
            device: devbox,
            name: "devbox".into(),
            routes: Vec::new(),
            host_key: String::new(),
            network: Some(network.clone()),
        }];
        let ActionOutcome::PhonePair { pair, seed, .. } = phones.begin(book.clone(), None) else {
            panic!("expected a pairing");
        };
        let seed: [u8; 32] = STANDARD.decode(seed.expose()).unwrap().try_into().unwrap();
        let code = PairCode {
            pair,
            seed: Zeroizing::new(seed),
            device: devbox,
            host_key: String::new(),
            routes: Vec::new(),
            network: Some(network),
        };
        let code = code.encode().unwrap();

        let store: Arc<dyn KeyStore> = Arc::new(MemoryStore::default());
        let data = TempDir::new();
        let core = Core::open(
            data.path(),
            &data.path().join("cache"),
            Platform::Ios,
            Arc::clone(&store),
        )
        .unwrap();
        let paired = core.pair(&code, "iPhone").await.unwrap();
        let phone = core.key().phone;
        assert_eq!(paired.phone, phone);
        assert_eq!(paired.devices, book);
        assert!(phones.knows(phone));
        assert!(maxhq_mobile::token(&*store, devbox).unwrap().is_some());

        // The book's link opens over the network, as the phone, which
        // proves its key and so reads what devbox holds.
        link_until(&core, devbox, |state| {
            matches!(state, LinkState::Connected { .. })
        })
        .await;
        let itself = served.hub.device().clone();
        view_until(
            &core,
            |entity| matches!(entity, Entity::Device(device) if *device == itself),
        )
        .await;
        let session = used_session(&served, phone).await;

        // Revoking the session drops the link, forgets the token, and the
        // phone says to pair again.
        assert_eq!(access.revoke(session.id), ActionOutcome::Done);
        link_until(&core, devbox, |state| {
            matches!(state, LinkState::Unreachable { reason, .. } if reason == maxhq_client::SIGN_IN_ENDED)
        })
        .await;
        assert!(maxhq_mobile::token(&*store, devbox).unwrap().is_none());
        dropped(core).await;
    }
}
