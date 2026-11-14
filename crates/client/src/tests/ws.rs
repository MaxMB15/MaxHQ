// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The phone's network access against TLS servers on 127.0.0.1, each with
//! its own self-signed certificate.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use futures::{SinkExt, StreamExt};
use maxhq_protocol::netaccess::MAX_LINK_MESSAGE;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::server::TlsStream;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::Message;

use super::*;
use crate::transport::ws::conn;
use crate::transport::{Ended, Headers, PhoneWs, WsRoute, SIGN_IN_ENDED};

/// A self-signed certificate and its pin.
struct Cert {
    der: CertificateDer<'static>,
    key: Vec<u8>,
    pin: String,
}

fn cert(name: &str) -> Cert {
    let made = rcgen::generate_simple_self_signed(vec![name.to_owned()]).unwrap();
    let der = made.cert.der().clone();
    let pin = URL_SAFE_NO_PAD.encode(ring::digest::digest(&ring::digest::SHA256, &der));
    Cert {
        der,
        key: made.signing_key.serialize_der(),
        pin,
    }
}

/// A TLS listener on 127.0.0.1 showing `cert`. Each stream that finishes
/// the handshake goes to the test.
async fn listen(cert: &Cert) -> (u16, mpsc::UnboundedReceiver<TlsStream<TcpStream>>) {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(cert.key.clone()));
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert.der.clone()], key)
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (streams_tx, streams) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            let streams = streams_tx.clone();
            tokio::spawn(async move {
                if let Ok(tls) = acceptor.accept(tcp).await {
                    let _ = streams.send(tls);
                }
            });
        }
    });
    (port, streams)
}

/// A port that takes connections and never answers.
async fn silent() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((tcp, _)) = listener.accept().await {
            held.push(tcp);
        }
    });
    port
}

fn route(host: &str, port: u16, pin: &str) -> WsRoute {
    WsRoute {
        host: host.into(),
        port,
        pin: pin.into(),
    }
}

/// What the headers callback was asked for.
type Asked = Arc<Mutex<Vec<(String, String)>>>;

/// Headers with a token and a proof naming the URL, as the phone's are.
fn signing() -> (Headers, Asked) {
    let asked = Asked::default();
    let seen = Arc::clone(&asked);
    let headers: Headers = Arc::new(move |method, url| {
        seen.lock().unwrap().push((method.to_owned(), url.clone()));
        Box::pin(async move {
            Ok(vec![
                ("Authorization".to_owned(), "DPoP token-1".to_owned()),
                ("DPoP".to_owned(), format!("proof for {url}")),
            ])
        })
    });
    (headers, asked)
}

/// How many times a device ended the phone's sign-in, and the hook that
/// counts them.
fn ending() -> (Ended, Arc<Mutex<usize>>) {
    let count = Arc::new(Mutex::new(0));
    let seen = Arc::clone(&count);
    (Arc::new(move || *seen.lock().unwrap() += 1), count)
}

/// The far side's next stream.
async fn accepted<T>(streams: &mut mpsc::UnboundedReceiver<T>) -> T {
    tokio::time::timeout(PATIENCE, streams.recv())
        .await
        .expect("a stream in time")
        .expect("the listener is up")
}

/// The error of a dial that should have failed.
fn failure<T>(dialed: Result<T, DialError>) -> DialError {
    match dialed {
        Ok(_) => panic!("expected the dial to fail"),
        Err(err) => err,
    }
}

#[tokio::test]
async fn only_the_pinned_certificate_connects() {
    let right = cert("devbox.lan");
    let (port, mut streams) = listen(&right).await;
    let ws = PhoneWs::new();
    ws.tls(&route("127.0.0.1", port, &right.pin)).await.unwrap();
    accepted(&mut streams).await;

    let other = cert("devbox.lan");
    let (port, _streams) = listen(&other).await;
    let err = failure(ws.tls(&route("127.0.0.1", port, &right.pin)).await);
    assert_eq!(err.reason, "the device showed a different certificate");
    assert!(!err.host_down);
}

#[tokio::test]
async fn a_pin_that_doesnt_read_is_refused() {
    let right = cert("devbox.lan");
    let (port, _streams) = listen(&right).await;
    let ws = PhoneWs::new();
    let short = URL_SAFE_NO_PAD.encode([7u8; 16]);
    let padded = format!("{}=", right.pin);
    for pin in [short.as_str(), "not base64!", padded.as_str(), ""] {
        let err = failure(ws.tls(&route("127.0.0.1", port, pin)).await);
        assert_eq!(
            err.reason, "the device's certificate pin doesn't read",
            "{pin}"
        );
        assert!(!err.host_down, "{pin}");
    }
}

#[tokio::test]
async fn a_name_goes_as_sni_and_an_address_doesnt() {
    let cert = cert("devbox.lan");
    let (port, mut streams) = listen(&cert).await;
    let ws = PhoneWs::new();
    ws.tls(&route("localhost", port, &cert.pin)).await.unwrap();
    let named = accepted(&mut streams).await;
    assert_eq!(named.get_ref().1.server_name(), Some("localhost"));
    ws.tls(&route("127.0.0.1", port, &cert.pin)).await.unwrap();
    let bare = accepted(&mut streams).await;
    assert_eq!(bare.get_ref().1.server_name(), None);
}

/// What the far side of an upgrade saw.
struct Upgrade {
    path: String,
    authorization: Option<String>,
    proof: Option<String>,
}

/// Opens a WebSocket on a fresh server and returns both ends, with what
/// the server saw of the upgrade and what the callback was asked.
// tungstenite fixes the upgrade callback's error type.
#[allow(clippy::result_large_err)]
async fn socket() -> (
    Conn,
    tokio_tungstenite::WebSocketStream<TlsStream<TcpStream>>,
    Upgrade,
    Asked,
    u16,
) {
    let cert = cert("devbox.lan");
    let (port, mut streams) = listen(&cert).await;
    let (headers, asked) = signing();
    let ws = PhoneWs::new();
    let routes = ws.trust(
        &[route("127.0.0.1", port, &cert.pin)],
        &headers,
        &ending().0,
    );
    let (conn, server) = tokio::join!(ws.dial(&routes[0], None), async {
        let tls = accepted(&mut streams).await;
        let mut upgrade = None;
        let server = tokio_tungstenite::accept_hdr_async(tls, |request: &Request, response| {
            let header = |name: &str| {
                request
                    .headers()
                    .get(name)
                    .map(|value| value.to_str().unwrap().to_owned())
            };
            upgrade = Some(Upgrade {
                path: request.uri().path().to_owned(),
                authorization: header("authorization"),
                proof: header("dpop"),
            });
            Ok::<Response, _>(response)
        })
        .await
        .unwrap();
        (server, upgrade.unwrap())
    });
    let (server, upgrade) = server;
    (conn.unwrap(), server, upgrade, asked, port)
}

#[tokio::test]
async fn the_link_rides_in_binary_messages_with_the_signed_headers() {
    let (mut conn, mut server, upgrade, asked, port) = socket().await;
    let url = format!("wss://127.0.0.1:{port}/link");
    assert_eq!(
        *asked.lock().unwrap(),
        vec![("GET".to_owned(), url.clone())]
    );
    assert_eq!(upgrade.path, "/link");
    assert_eq!(upgrade.authorization.as_deref(), Some("DPoP token-1"));
    assert_eq!(upgrade.proof, Some(format!("proof for {url}")));

    conn.writer.write_all(b"hello").await.unwrap();
    conn.writer.flush().await.unwrap();
    assert_eq!(
        server.next().await.unwrap().unwrap(),
        Message::binary(&b"hello"[..])
    );

    server.send(Message::binary(&b"ab"[..])).await.unwrap();
    server.send(Message::binary(&b"cd"[..])).await.unwrap();
    let mut joined = [0; 4];
    conn.reader.read_exact(&mut joined).await.unwrap();
    assert_eq!(&joined, b"abcd");
}

#[tokio::test]
async fn a_large_write_goes_as_several_messages() {
    let (mut conn, mut server, ..) = socket().await;
    let big: Vec<u8> = (0..200 * 1024).map(|n| (n % 251) as u8).collect();
    let sending = async {
        conn.writer.write_all(&big).await.unwrap();
        conn.writer.flush().await.unwrap();
    };
    let receiving = async {
        let mut got = Vec::new();
        let mut messages = 0;
        while got.len() < big.len() {
            let Message::Binary(data) = server.next().await.unwrap().unwrap() else {
                panic!("expected binary");
            };
            assert!(data.len() <= 64 * 1024);
            got.extend_from_slice(&data);
            messages += 1;
        }
        (got, messages)
    };
    let ((), (got, messages)) = tokio::join!(sending, receiving);
    assert!(messages > 1);
    assert_eq!(got, big);
}

#[tokio::test]
async fn pings_get_a_pong_and_a_close_ends_the_read() {
    let (mut conn, mut server, ..) = socket().await;
    server
        .send(Message::Ping(b"are you there".to_vec().into()))
        .await
        .unwrap();
    let mut buf = [0; 16];
    let (read, ()) = tokio::join!(conn.reader.read(&mut buf), async {
        assert_eq!(
            server.next().await.unwrap().unwrap(),
            Message::Pong(b"are you there".to_vec().into())
        );
        server.close(None).await.unwrap();
    });
    assert_eq!(read.unwrap(), 0);
}

#[tokio::test]
async fn a_text_message_ends_the_read() {
    let (mut conn, mut server, ..) = socket().await;
    server.send(Message::text("hello")).await.unwrap();
    let mut buf = [0; 16];
    assert_eq!(conn.reader.read(&mut buf).await.unwrap(), 0);
    assert_eq!(conn.reader.read(&mut buf).await.unwrap(), 0);
}

#[tokio::test]
async fn a_message_past_the_limit_ends_the_read() {
    let (mut conn, mut server, ..) = socket().await;
    let usual = vec![1; 64 * 1024];
    server.send(Message::binary(usual.clone())).await.unwrap();
    let mut got = vec![0; usual.len()];
    conn.reader.read_exact(&mut got).await.unwrap();
    assert_eq!(got, usual);

    let too_long = vec![2; MAX_LINK_MESSAGE + 1];
    tokio::spawn(async move {
        let _ = server.send(Message::binary(too_long)).await;
    });
    let mut buf = vec![0; 16];
    let read = tokio::time::timeout(Duration::from_secs(10), conn.reader.read(&mut buf)).await;
    assert!(read.unwrap().is_err(), "the read ends with an error");
}

/// A server that answers one post with `status` and `body`, and hands
/// back the request it got.
async fn answering(status: u16, body: Vec<u8>) -> (WsRoute, tokio::task::JoinHandle<String>) {
    let cert = cert("devbox.lan");
    let (port, mut streams) = listen(&cert).await;
    let serving = tokio::spawn(async move {
        let mut tls = accepted(&mut streams).await;
        let mut request = Vec::new();
        let mut buf = [0; 1024];
        let head = loop {
            let n = tls.read(&mut buf).await.unwrap();
            request.extend_from_slice(&buf[..n]);
            if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                break at + 4;
            }
        };
        let text = String::from_utf8(request.clone())
            .unwrap()
            .to_ascii_lowercase();
        let length: usize = text
            .lines()
            .find_map(|line| line.strip_prefix("content-length: "))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        while request.len() < head + length {
            let n = tls.read(&mut buf).await.unwrap();
            request.extend_from_slice(&buf[..n]);
        }
        let mut answer = format!(
            "HTTP/1.1 {status} OK\r\ncontent-length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        answer.extend_from_slice(&body);
        tls.write_all(&answer).await.unwrap();
        tls.flush().await.unwrap();
        String::from_utf8(request).unwrap()
    });
    (route("127.0.0.1", port, &cert.pin), serving)
}

#[tokio::test]
async fn a_post_carries_the_signed_headers_and_brings_back_the_answer() {
    let (route, serving) = answering(201, br#"{"phone":"p1"}"#.to_vec()).await;
    let (headers, asked) = signing();
    let posted = PhoneWs::new()
        .post(&route, "/pair", &headers, br#"{"pair":"x"}"#.to_vec())
        .await
        .unwrap();
    assert_eq!(posted.status, 201);
    assert_eq!(posted.body, br#"{"phone":"p1"}"#);
    let url = format!("https://127.0.0.1:{}/pair", route.port);
    assert_eq!(
        *asked.lock().unwrap(),
        vec![("POST".to_owned(), url.clone())]
    );
    let request = serving.await.unwrap();
    let lower = request.to_ascii_lowercase();
    assert!(request.starts_with("POST /pair HTTP/1.1\r\n"), "{request}");
    assert!(lower.contains("content-type: application/json\r\n"));
    assert!(lower.contains(
        "authorization: DPoP token-1\r\n"
            .to_ascii_lowercase()
            .as_str()
    ));
    assert!(request.contains(&format!("proof for {url}\r\n")));
    assert!(request.ends_with(r#"{"pair":"x"}"#));
}

#[tokio::test]
async fn an_answer_past_64_kib_is_refused() {
    let (headers, _) = signing();
    let (route, _serving) = answering(200, vec![b'x'; 64 * 1024]).await;
    let posted = PhoneWs::new()
        .post(&route, "/token", &headers, b"{}".to_vec())
        .await
        .unwrap();
    assert_eq!(posted.body.len(), 64 * 1024);
    let (route, _serving) = answering(200, vec![b'x'; 64 * 1024 + 1]).await;
    let err = PhoneWs::new()
        .post(&route, "/token", &headers, b"{}".to_vec())
        .await
        .unwrap_err();
    assert_eq!(err, "the device's answer was too long");
}

/// A device on 127.0.0.1 whose WebSockets reach a fake `maxd` each.
async fn device_server(cert: &Cert) -> (u16, mpsc::UnboundedReceiver<UnixStream>) {
    let (port, mut streams) = listen(cert).await;
    let (maxds_tx, maxds) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(tls) = streams.recv().await {
            let Ok(socket) = tokio_tungstenite::accept_async(tls).await else {
                continue;
            };
            let (mut ours, theirs) = UnixStream::pair().unwrap();
            let _ = maxds_tx.send(theirs);
            tokio::spawn(async move {
                let mut conn = conn(socket);
                let (mut read, mut write) = ours.split();
                let _ = tokio::join!(
                    tokio::io::copy(&mut conn.reader, &mut write),
                    tokio::io::copy(&mut read, &mut conn.writer),
                );
            });
        }
    });
    (port, maxds)
}

#[tokio::test]
async fn a_dead_route_gives_way_to_the_next_and_the_live_one_is_remembered() {
    let cert = cert("devbox.lan");
    let (port, mut maxds) = device_server(&cert).await;
    let dead = silent().await;
    let ws = PhoneWs::new();
    let (headers, _) = signing();
    let client = Client::app(crate::Transport::Ws(ws.clone())).timed(FAST);
    client.set_network(Some("wifi".into()));
    let mut routes = ws
        .trust(
            &[
                route("127.0.0.1", dead, &cert.pin),
                route("127.0.0.1", port, &cert.pin),
            ],
            &headers,
            &ending().0,
        )
        .into_iter();
    let first = routes.next().unwrap();
    let (_handle, mut events) = client.link(LinkSpec {
        fallbacks: routes.collect(),
        ..spec(first)
    });
    // The dead route takes far longer than this to time out.
    let mut maxd = FakeMaxd::new(accepted(&mut maxds).await);
    maxd.accept(InstanceId::generate(), "boot-1").await;
    loop {
        if let LinkEvent::RouteWorked { net, route } = next(&mut events).await {
            assert_eq!(net, "wifi");
            assert_eq!(route, ssh(&format!("wss://127.0.0.1:{port}")));
            break;
        }
    }
}

#[tokio::test]
async fn a_route_showing_another_certificate_is_refused() {
    let (port, mut maxds) = device_server(&cert("devbox.lan")).await;
    let ws = PhoneWs::new();
    let (headers, asked) = signing();
    let client = Client::app(crate::Transport::Ws(ws.clone())).timed(FAST);
    let routes = ws.trust(
        &[route("127.0.0.1", port, &cert("devbox.lan").pin)],
        &headers,
        &ending().0,
    );
    let (_handle, mut events) = client.link(spec(routes[0].clone()));
    assert_eq!(
        refused_with(&mut events).await,
        "the device showed a different certificate"
    );
    assert!(maxds.try_recv().is_err(), "nothing opened");
    assert!(asked.lock().unwrap().is_empty(), "nothing signed for it");
}

#[tokio::test]
async fn the_debug_view_shows_routes_and_never_headers() {
    let ws = PhoneWs::new();
    let (headers, _) = signing();
    ws.trust(&[route("192.168.1.20", 7466, "pin")], &headers, &ending().0);
    let shown = format!("{ws:?}");
    assert_eq!(shown, r#"PhoneWs { routes: ["wss://192.168.1.20:7466"] }"#);
}

#[cfg(feature = "russh")]
#[tokio::test]
async fn both_send_each_route_to_the_transport_that_knows_it() {
    use maxhq_protocol::phones::{PhoneBookEntry, PhoneRoute};

    use crate::transport::{phone_route_host, ws_route_host};

    let cert = cert("devbox.lan");
    let (port, mut maxds) = device_server(&cert).await;
    let ws = PhoneWs::new();
    let ssh = crate::PhoneSsh::new(&[1; 32]);
    let (headers, _) = signing();
    let network = ws.trust(
        &[route("127.0.0.1", port, &cert.pin)],
        &headers,
        &ending().0,
    );
    let over_ssh = ssh.trust(&PhoneBookEntry {
        device: device(),
        name: "Test box".into(),
        routes: vec![PhoneRoute {
            user: "dev".into(),
            host: "127.0.0.1".into(),
            port: 22,
        }],
        // A pin PhoneSsh can't read, so the refusal says which one dialed.
        host_key: String::new(),
        network: None,
    });
    let both = crate::transport::Both { ssh, ws };
    both.dial(&network[0], None).await.unwrap();
    accepted(&mut maxds).await;
    let err = failure(both.dial(&over_ssh[0], None).await);
    assert_eq!(err.reason, "the device's host key pin doesn't read");
    // The network access knows its own routes, whatever a name looks like.
    assert!(both.ws.knows(&ws_route_host("127.0.0.1", port)));
    assert!(!both.ws.knows("wss://192.168.1.20:7466"));
    assert!(!both.ws.knows(&phone_route_host(&PhoneRoute {
        user: "dev".into(),
        host: "127.0.0.1".into(),
        port: 22,
    })));
}

/// A device whose upgrades answer `status` with `body`.
// tungstenite fixes the upgrade callback's error type.
#[allow(clippy::result_large_err)]
async fn refusing(status: u16, body: &'static str) -> WsRoute {
    let cert = cert("devbox.lan");
    let (port, mut streams) = listen(&cert).await;
    tokio::spawn(async move {
        while let Some(tls) = streams.recv().await {
            let _ = tokio_tungstenite::accept_hdr_async(tls, |_: &Request, _| {
                let mut answer =
                    tokio_tungstenite::tungstenite::http::Response::new(Some(body.to_owned()));
                *answer.status_mut() = status.try_into().unwrap();
                Err(answer)
            })
            .await;
        }
    });
    route("127.0.0.1", port, &cert.pin)
}

#[tokio::test]
async fn an_ended_sign_in_says_pair_again_and_tells_the_caller() {
    for (status, body, reason, ends) in [
        (401, r#"{"error":"revoked"}"#, SIGN_IN_ENDED, 1),
        (401, r#"{"error":"expired"}"#, SIGN_IN_ENDED, 1),
        (
            401,
            r#"{"error":"bad_proof"}"#,
            "the device didn't take this phone's sign-in",
            0,
        ),
        (
            403,
            r#"{"error":"not_paired"}"#,
            "the device didn't take this phone's sign-in",
            0,
        ),
        (404, "", "that device has no network access", 0),
    ] {
        let route = refusing(status, body).await;
        let ws = PhoneWs::new();
        let (headers, _) = signing();
        let (ended, count) = ending();
        let routes = ws.trust(&[route], &headers, &ended);
        let err = failure(ws.dial(&routes[0], None).await);
        assert_eq!(err.reason, reason, "{body}");
        assert_eq!(*count.lock().unwrap(), ends, "{body}");
    }
}

/// A route for `PhoneSsh` to 127.0.0.1 at `port`, pinned to a key no
/// server there shows.
#[cfg(feature = "russh")]
fn ssh_entry(port: u16) -> maxhq_protocol::phones::PhoneBookEntry {
    maxhq_protocol::phones::PhoneBookEntry {
        device: device(),
        name: "Test box".into(),
        routes: vec![maxhq_protocol::phones::PhoneRoute {
            user: "dev".into(),
            host: "127.0.0.1".into(),
            port,
        }],
        host_key:
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g"
                .into(),
        network: None,
    }
}

/// A port that takes connections, says nothing, and tells the test when
/// each came.
#[cfg(feature = "russh")]
async fn watched() -> (u16, mpsc::UnboundedReceiver<std::time::Instant>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (came_tx, came) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((tcp, _)) = listener.accept().await {
            let _ = came_tx.send(std::time::Instant::now());
            held.push(tcp);
        }
    });
    (port, came)
}

#[cfg(feature = "russh")]
#[tokio::test]
async fn a_link_tries_network_routes_first_and_then_ssh_after_the_stagger() {
    let (network_port, mut network) = watched().await;
    let (ssh_port, mut over_ssh) = watched().await;
    let ws = PhoneWs::new();
    let ssh = crate::PhoneSsh::new(&[1; 32]);
    let (headers, _) = signing();
    let pin = URL_SAFE_NO_PAD.encode([7; 32]);
    let mut routes = ws.trust(
        &[route("127.0.0.1", network_port, &pin)],
        &headers,
        &ending().0,
    );
    routes.extend(ssh.trust(&ssh_entry(ssh_port)));
    let client = Client::app(crate::Transport::Phone { ssh, ws }).timed(FAST);
    let mut routes = routes.into_iter();
    let first = routes.next().unwrap();
    let (_handle, _events) = client.link(LinkSpec {
        fallbacks: routes.collect(),
        ..spec(first)
    });
    let first = accepted(&mut network).await;
    let then = accepted(&mut over_ssh).await;
    // The stagger is 300 ms; the network route never answers, so SSH
    // waits all of it.
    let waited = then.duration_since(first);
    assert!(
        waited >= Duration::from_millis(250),
        "SSH waited {waited:?}"
    );
}

#[cfg(feature = "russh")]
#[tokio::test]
async fn an_ended_sign_in_outranks_a_route_that_doesnt_answer() {
    let ws = PhoneWs::new();
    let ssh = crate::PhoneSsh::new(&[1; 32]);
    let (headers, _) = signing();
    let (ended, count) = ending();
    let closed = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut routes = ws.trust(
        &[refusing(401, r#"{"error":"revoked"}"#).await],
        &headers,
        &ended,
    );
    routes.extend(ssh.trust(&ssh_entry(closed)));
    let client = Client::app(crate::Transport::Phone { ssh, ws }).timed(FAST);
    let mut routes = routes.into_iter();
    let first = routes.next().unwrap();
    let (_handle, mut events) = client.link(LinkSpec {
        fallbacks: routes.collect(),
        ..spec(first)
    });
    assert_eq!(refused_with(&mut events).await, SIGN_IN_ENDED);
    assert!(*count.lock().unwrap() >= 1);
}
