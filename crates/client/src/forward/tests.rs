// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};

use maxhq_protocol::frame::MAX_TUNNEL_CHUNK;
use maxhq_protocol::ids::WorkspaceId;
use maxhq_protocol::message::ErrorCode;
use maxhq_protocol::model::BindAddress;
use maxhq_protocol::time::Timestamp;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpSocket;
use tokio::sync::mpsc::{self, UnboundedReceiver};

use super::*;

/// A link that's connected until told otherwise, and opens every forward
/// it's asked for unless told to refuse.
struct Fake {
    out: mpsc::UnboundedSender<Out>,
    down: AtomicBool,
    refuse: AtomicBool,
    asks: Mutex<Vec<(PortId, u16)>>,
    opened: Mutex<Vec<(PortId, ForwardId)>>,
}

impl Fake {
    fn set_down(&self, down: bool) {
        self.down.store(down, Ordering::SeqCst);
    }

    fn asks(&self) -> Vec<(PortId, u16)> {
        self.asks.lock().unwrap().clone()
    }

    /// The forward it last opened for `port`.
    fn forward(&self, port: PortId) -> ForwardId {
        let opened = self.opened.lock().unwrap();
        let found = opened.iter().rev().find(|(of, _)| *of == port);
        found.map(|(_, forward)| *forward).expect("never opened")
    }
}

impl ForwardLink for Fake {
    fn send(&self, out: Out) -> Result<(), Gone> {
        if self.down.load(Ordering::SeqCst) {
            return Err(Gone);
        }
        self.out.send(out).map_err(|_| Gone)
    }

    fn open(
        &self,
        port: PortId,
        local_port: u16,
    ) -> BoxFuture<'static, Result<ForwardId, ForwardError>> {
        self.asks.lock().unwrap().push((port, local_port));
        let result = if self.down.load(Ordering::SeqCst) {
            Err(ForwardError::NotConnected)
        } else if self.refuse.load(Ordering::SeqCst) {
            Err(ForwardError::Refused("the port went".into()))
        } else {
            let forward = ForwardId::generate();
            self.opened.lock().unwrap().push((port, forward));
            Ok(forward)
        };
        Box::pin(async move { result })
    }
}

struct World {
    fake: Arc<Fake>,
    rx: UnboundedReceiver<Out>,
    forwards: Forwards,
}

fn world() -> World {
    world_with(MAX_STREAMS_PER_FORWARD, MAX_STREAMS_PER_CLIENT)
}

fn world_with(per_forward: usize, per_client: usize) -> World {
    let (out, rx) = mpsc::unbounded_channel();
    let fake = Arc::new(Fake {
        out,
        down: AtomicBool::new(false),
        refuse: AtomicBool::new(false),
        asks: Mutex::new(Vec::new()),
        opened: Mutex::new(Vec::new()),
    });
    let forwards = Forwards::with_limits(fake.clone(), per_forward, per_client);
    World { fake, rx, forwards }
}

/// A port nothing listened on a moment ago.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn listening(agent: Option<AgentId>) -> ListeningPort {
    ListeningPort {
        id: PortId::generate(),
        workspace: WorkspaceId::generate(),
        agent,
        port: free_port(),
        bind: BindAddress::Loopback,
        process: None,
        url_hint: None,
        first_seen: Timestamp(1),
        gone_since: None,
    }
}

async fn next(rx: &mut UnboundedReceiver<Out>) -> Out {
    let next = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
    next.expect("nothing sent").expect("the link closed")
}

/// The next message, past any credit a stream's writer grants.
async fn next_message(rx: &mut UnboundedReceiver<Out>) -> ClientMessage {
    loop {
        match next(rx).await {
            Out::Message(ClientMessage::TunnelCredit { .. }) => {}
            Out::Message(message) => return message,
            Out::Data { stream, .. } => panic!("bytes on {stream}"),
        }
    }
}

async fn quiet(rx: &mut UnboundedReceiver<Out>) {
    let more = tokio::time::timeout(Duration::from_millis(150), rx.recv()).await;
    assert!(more.is_err(), "sent {more:?}");
}

fn close_forward(forward: ForwardId) -> Out {
    Out::Message(ClientMessage::CloseForward { forward })
}

/// Connects to a forward's local port and gives the stream it opened.
async fn connect(
    world: &mut World,
    port: &ListeningPort,
    local_port: u16,
) -> (TcpStream, StreamId) {
    let socket = TcpStream::connect(("127.0.0.1", local_port)).await.unwrap();
    let Out::Message(ClientMessage::TunnelOpen { stream, forward }) = next(&mut world.rx).await
    else {
        panic!("no stream opened");
    };
    assert_eq!(forward, world.fake.forward(port.id));
    (socket, stream)
}

/// Connects to a forward's local port and checks the connection is
/// dropped with no stream opened.
async fn dropped(world: &mut World, local_port: u16) {
    let mut socket = TcpStream::connect(("127.0.0.1", local_port)).await.unwrap();
    let mut buf = [0; 8];
    let read = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut buf)).await;
    assert!(matches!(read, Ok(Ok(0) | Err(_))), "{read:?}");
    quiet(&mut world.rx).await;
}

/// Checks the peer aborted `socket`.
async fn aborted(socket: &mut TcpStream) {
    let mut buf = [0; 8];
    let read = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut buf)).await;
    assert!(matches!(read, Ok(Err(_))), "{read:?}");
}

async fn open_streams(forwards: &Forwards, n: usize) {
    for _ in 0..500 {
        if forwards.open_streams() == n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{} streams open, not {n}", forwards.open_streams());
}

/// Waits until nothing listens on `port` on either loopback.
///
/// It binds the port to find out rather than connecting to it. Once the
/// forward lets go, a forward in another test, or in another test run,
/// may take the port, and a connection would open a stream on that one.
async fn closed(port: u16) {
    for _ in 0..500 {
        if !held(port) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("port {port} still listens");
}

/// Whether a socket holds `port` on 127.0.0.1 or ::1. The check sets
/// `SO_REUSEADDR`, so a closed connection still waiting out its time on
/// the port, or a program that listens on every address, doesn't count.
fn held(port: u16) -> bool {
    let loopbacks: [IpAddr; 2] = [Ipv4Addr::LOCALHOST.into(), Ipv6Addr::LOCALHOST.into()];
    loopbacks.into_iter().any(|ip| {
        let socket = match ip {
            IpAddr::V4(_) => TcpSocket::new_v4(),
            IpAddr::V6(_) => TcpSocket::new_v6(),
        };
        let bound = socket.and_then(|socket| {
            socket.set_reuseaddr(true)?;
            socket.bind(SocketAddr::new(ip, port))
        });
        matches!(bound, Err(err) if err.kind() == io::ErrorKind::AddrInUse)
    })
}

#[tokio::test]
async fn a_half_close_goes_both_ways_once_maxd_opens() {
    let mut world = world();
    let port = listening(None);
    let local = world.forwards.open(&port).await.unwrap();
    assert_eq!(world.fake.asks(), [(port.id, local)]);
    let (mut socket, stream) = connect(&mut world, &port, local).await;

    // Nothing goes before maxd says the stream opened.
    socket.write_all(b"hello").await.unwrap();
    quiet(&mut world.rx).await;
    assert!(world
        .forwards
        .handle(&ServerMessage::TunnelOpened { stream }));
    assert_eq!(
        next(&mut world.rx).await,
        Out::Data {
            stream,
            data: b"hello".to_vec(),
        }
    );

    world.forwards.data(stream, b"world");
    let mut buf = [0; 5];
    socket.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"world");

    socket.shutdown().await.unwrap();
    assert_eq!(
        next(&mut world.rx).await,
        Out::Message(ClientMessage::TunnelClose {
            stream,
            how: TunnelEnd::Write,
        })
    );
    assert!(world.forwards.handle(&ServerMessage::TunnelClose {
        stream,
        how: TunnelEnd::Write,
    }));
    assert_eq!(socket.read(&mut buf).await.unwrap(), 0);
    open_streams(&world.forwards, 0).await;
    assert!(!world.forwards.handle(&ServerMessage::Pong { nonce: 1 }));
}

#[tokio::test]
async fn a_reset_from_maxd_aborts_the_connection() {
    let mut world = world();
    let port = listening(None);
    let local = world.forwards.open(&port).await.unwrap();
    let (mut socket, stream) = connect(&mut world, &port, local).await;
    world
        .forwards
        .handle(&ServerMessage::TunnelOpened { stream });
    world.forwards.handle(&ServerMessage::TunnelClose {
        stream,
        how: TunnelEnd::Reset,
    });
    aborted(&mut socket).await;
    open_streams(&world.forwards, 0).await;
    quiet(&mut world.rx).await;
}

#[tokio::test]
async fn a_reset_here_reaches_maxd() {
    let mut world = world();
    let port = listening(None);
    let local = world.forwards.open(&port).await.unwrap();
    let (socket, stream) = connect(&mut world, &port, local).await;
    world
        .forwards
        .handle(&ServerMessage::TunnelOpened { stream });
    socket.set_zero_linger().unwrap();
    drop(socket);
    assert_eq!(
        next(&mut world.rx).await,
        Out::Message(ClientMessage::TunnelClose {
            stream,
            how: TunnelEnd::Reset,
        })
    );
    open_streams(&world.forwards, 0).await;
}

#[tokio::test]
async fn a_stream_maxd_refuses_drops_the_connection() {
    let mut world = world();
    let port = listening(None);
    let local = world.forwards.open(&port).await.unwrap();
    let (mut socket, stream) = connect(&mut world, &port, local).await;
    world.forwards.handle(&ServerMessage::TunnelRefused {
        stream,
        code: ErrorCode::PortGone,
        message: "port 5173 closed".into(),
    });
    aborted(&mut socket).await;
    assert_eq!(world.forwards.open_streams(), 0);
    quiet(&mut world.rx).await;
}

#[tokio::test]
async fn bytes_past_the_credit_reset_the_stream() {
    let mut world = world();
    let port = listening(None);
    let local = world.forwards.open(&port).await.unwrap();
    let (mut socket, stream) = connect(&mut world, &port, local).await;
    world
        .forwards
        .handle(&ServerMessage::TunnelOpened { stream });
    world
        .forwards
        .data(stream, &vec![0; STREAM_CREDIT as usize]);
    world.forwards.data(stream, b"one more");
    assert_eq!(
        next_message(&mut world.rx).await,
        ClientMessage::TunnelClose {
            stream,
            how: TunnelEnd::Reset,
        }
    );
    assert_eq!(world.forwards.open_streams(), 0);
    let mut buf = vec![0; STREAM_CREDIT as usize + 8];
    let mut read = Ok(1);
    while matches!(read, Ok(n) if n > 0) {
        read = socket.read(&mut buf).await;
    }
    assert!(read.is_err(), "{read:?}");
}

#[tokio::test]
async fn reading_stops_at_the_credit_and_goes_on_when_maxd_grants() {
    let mut world = world();
    let port = listening(None);
    let local = world.forwards.open(&port).await.unwrap();
    let (mut socket, stream) = connect(&mut world, &port, local).await;
    world
        .forwards
        .handle(&ServerMessage::TunnelOpened { stream });
    let credit = STREAM_CREDIT as usize;
    socket.write_all(&vec![1; credit + 1000]).await.unwrap();
    let mut got = 0;
    while got < credit {
        let Out::Data { data, .. } = next(&mut world.rx).await else {
            panic!("expected bytes");
        };
        got += data.len();
    }
    assert_eq!(got, credit);
    quiet(&mut world.rx).await;
    world.forwards.handle(&ServerMessage::TunnelCredit {
        stream,
        granted: 1000,
    });
    let Out::Data { data, .. } = next(&mut world.rx).await else {
        panic!("expected bytes");
    };
    assert_eq!(data.len(), 1000);
}

#[tokio::test]
async fn a_huge_grant_only_goes_so_far() {
    let mut world = world();
    let port = listening(None);
    let local = world.forwards.open(&port).await.unwrap();
    let (mut socket, stream) = connect(&mut world, &port, local).await;
    world
        .forwards
        .handle(&ServerMessage::TunnelOpened { stream });
    world.forwards.handle(&ServerMessage::TunnelCredit {
        stream,
        granted: u64::MAX,
    });
    let most = 4 * STREAM_CREDIT as usize;
    socket.write_all(&vec![1; most + 1000]).await.unwrap();
    let mut got = 0;
    while got < most {
        let Out::Data { data, .. } = next(&mut world.rx).await else {
            panic!("expected bytes");
        };
        got += data.len();
    }
    assert_eq!(got, most);
    quiet(&mut world.rx).await;
}

#[tokio::test]
async fn writing_grants_maxd_more() {
    let mut world = world();
    let port = listening(None);
    let local = world.forwards.open(&port).await.unwrap();
    let (mut socket, stream) = connect(&mut world, &port, local).await;
    world
        .forwards
        .handle(&ServerMessage::TunnelOpened { stream });
    world.forwards.data(stream, &vec![3; MAX_TUNNEL_CHUNK]);
    let mut buf = vec![0; MAX_TUNNEL_CHUNK];
    socket.read_exact(&mut buf).await.unwrap();
    assert_eq!(
        next(&mut world.rx).await,
        Out::Message(ClientMessage::TunnelCredit {
            stream,
            granted: MAX_TUNNEL_CHUNK as u64,
        })
    );
}

#[tokio::test]
async fn a_reconnect_keeps_the_local_port_and_asks_again() {
    let mut world = world();
    let port = listening(None);
    let local = world.forwards.open(&port).await.unwrap();
    let first = world.fake.forward(port.id);
    let (mut socket, stream) = connect(&mut world, &port, local).await;
    world
        .forwards
        .handle(&ServerMessage::TunnelOpened { stream });

    world.fake.set_down(true);
    world.forwards.link_down();
    aborted(&mut socket).await;
    assert_eq!(world.forwards.open_streams(), 0);
    assert_eq!(
        world.forwards.list(),
        [LocalForward {
            port: port.id,
            local_port: local,
            forward: None,
        }]
    );
    // While it's down, connections are dropped.
    dropped(&mut world, local).await;

    world.fake.set_down(false);
    world.forwards.link_up();
    for _ in 0..500 {
        if world.forwards.list()[0].forward.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let again = world.fake.forward(port.id);
    assert_ne!(again, first);
    assert_eq!(world.forwards.list()[0].forward, Some(again));
    assert_eq!(world.fake.asks(), [(port.id, local), (port.id, local)]);
    assert_eq!(world.forwards.local_port(port.id), Some(local));
    connect(&mut world, &port, local).await;
}

#[tokio::test]
async fn a_forward_maxd_refuses_after_a_reconnect_closes() {
    let mut world = world();
    let port = listening(None);
    let local = world.forwards.open(&port).await.unwrap();
    world.forwards.link_down();
    world.fake.refuse.store(true, Ordering::SeqCst);
    world.forwards.link_up();
    for _ in 0..500 {
        if world.forwards.list().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(world.forwards.list().is_empty());
    closed(local).await;
    // It had no forward on the new connection to close.
    quiet(&mut world.rx).await;
}

#[tokio::test]
async fn a_forward_closes_when_its_port_or_agent_goes() {
    let mut world = world();
    let (x, y) = (AgentId::generate(), AgentId::generate());
    let ports = [
        listening(None),
        listening(Some(x)),
        listening(None),
        listening(Some(y)),
        listening(None),
    ];
    let mut local = Vec::new();
    for port in &ports {
        local.push(world.forwards.open(port).await.unwrap());
    }
    let forward = |i: usize| world.fake.forward(ports[i].id);
    let forwards: Vec<ForwardId> = (0..ports.len()).map(forward).collect();

    world
        .forwards
        .apply(&LinkEvent::Changed(vec![Change::Remove {
            key: EntityKey::ListeningPort(ports[0].id),
        }]));
    assert_eq!(next(&mut world.rx).await, close_forward(forwards[0]));
    closed(local[0]).await;

    world
        .forwards
        .apply(&LinkEvent::Changed(vec![Change::Remove {
            key: EntityKey::Agent(x),
        }]));
    assert_eq!(next(&mut world.rx).await, close_forward(forwards[1]));
    closed(local[1]).await;

    // The last port is gone from the sync, and so is the agent of the one
    // before.
    world.forwards.apply(&LinkEvent::Synced(vec![
        Entity::ListeningPort(ports[2].clone()),
        Entity::ListeningPort(ports[3].clone()),
    ]));
    let mut closes = vec![next(&mut world.rx).await, next(&mut world.rx).await];
    let mut expected = vec![close_forward(forwards[3]), close_forward(forwards[4])];
    let key = |out: &Out| format!("{out:?}");
    closes.sort_by_key(key);
    expected.sort_by_key(key);
    assert_eq!(closes, expected);
    quiet(&mut world.rx).await;
    assert_eq!(world.forwards.list().len(), 1);
    assert_eq!(world.forwards.list()[0].port, ports[2].id);
}

#[tokio::test]
async fn closing_or_dropping_stops_listening() {
    let mut world = world();
    let (a, b) = (listening(None), listening(None));
    let local_a = world.forwards.open(&a).await.unwrap();
    let local_b = world.forwards.open(&b).await.unwrap();
    let (mut socket, stream) = connect(&mut world, &a, local_a).await;
    world
        .forwards
        .handle(&ServerMessage::TunnelOpened { stream });

    world.forwards.close(a.id);
    assert_eq!(
        next(&mut world.rx).await,
        close_forward(world.fake.forward(a.id))
    );
    aborted(&mut socket).await;
    closed(local_a).await;
    world.forwards.close(a.id);
    quiet(&mut world.rx).await;
    assert_eq!(world.forwards.local_port(a.id), None);

    let World {
        fake,
        mut rx,
        forwards,
    } = world;
    drop(forwards);
    assert_eq!(next(&mut rx).await, close_forward(fake.forward(b.id)));
    closed(local_b).await;
}

#[tokio::test]
async fn streams_stop_at_the_limits() {
    let mut world = world_with(2, 3);
    let (a, b) = (listening(None), listening(None));
    let local_a = world.forwards.open(&a).await.unwrap();
    let local_b = world.forwards.open(&b).await.unwrap();
    let (_first, first) = connect(&mut world, &a, local_a).await;
    let _second = connect(&mut world, &a, local_a).await;
    // Past the forward's limit.
    dropped(&mut world, local_a).await;
    let _third = connect(&mut world, &b, local_b).await;
    // Past the link's limit.
    dropped(&mut world, local_b).await;
    assert_eq!(world.forwards.open_streams(), 3);

    world.forwards.handle(&ServerMessage::TunnelClose {
        stream: first,
        how: TunnelEnd::Reset,
    });
    let _fourth = connect(&mut world, &b, local_b).await;
    assert_eq!(world.forwards.open_streams(), 3);
}

#[tokio::test]
async fn opening_twice_gives_the_same_port() {
    let world = world();
    let port = listening(None);
    let first = world.forwards.open(&port).await.unwrap();
    let second = world.forwards.open(&port).await.unwrap();
    assert_eq!(first, second);
    assert_eq!(world.fake.asks().len(), 1);
}

#[tokio::test]
async fn opening_while_disconnected_fails_and_frees_the_port() {
    let world = world();
    world.fake.set_down(true);
    let port = listening(None);
    let err = world.forwards.open(&port).await.unwrap_err();
    assert!(matches!(err, ForwardError::NotConnected), "{err:?}");
    assert!(world.forwards.list().is_empty());
    let [(_, local)] = world.fake.asks()[..] else {
        panic!("asked {:?}", world.fake.asks());
    };
    closed(local).await;
}

#[tokio::test]
async fn an_attached_pipe_waits_for_maxd_and_ends_with_the_link() {
    let mut world = world_with(MAX_STREAMS_PER_FORWARD, 2);
    let (mut near, far) = tokio::io::duplex(1 << 16);
    let stream = StreamId::generate();
    world.forwards.attach(stream, far).unwrap();
    let (_other, again) = tokio::io::duplex(1 << 16);
    assert_eq!(
        world.forwards.attach(stream, again),
        Err("That stream is already open.".to_owned())
    );

    // Nothing goes before maxd says the stream opened.
    near.write_all(b"hello").await.unwrap();
    quiet(&mut world.rx).await;
    assert!(world
        .forwards
        .handle(&ServerMessage::TunnelOpened { stream }));
    assert_eq!(
        next(&mut world.rx).await,
        Out::Data {
            stream,
            data: b"hello".to_vec(),
        }
    );
    world.forwards.data(stream, b"world");
    let mut buf = [0; 5];
    near.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"world");

    // It counts against the link's streams, and one forgotten frees its
    // place without a word to maxd.
    let (_kept, second) = tokio::io::duplex(1 << 16);
    let refused = StreamId::generate();
    world.forwards.attach(refused, second).unwrap();
    let (_past, third) = tokio::io::duplex(1 << 16);
    assert_eq!(
        world.forwards.attach(StreamId::generate(), third),
        Err("Too many streams are open on this link.".to_owned())
    );
    world.forwards.forget(refused);
    assert_eq!(world.forwards.open_streams(), 1);
    quiet(&mut world.rx).await;

    world.forwards.link_down();
    let ended = tokio::time::timeout(Duration::from_secs(5), near.read(&mut buf)).await;
    assert!(
        matches!(ended, Ok(Ok(0) | Err(_))),
        "the pipe ends: {ended:?}"
    );
    assert_eq!(world.forwards.open_streams(), 0);
}

#[test]
fn forwards_cross_threads() {
    fn shared<T: Send + Sync>() {}
    shared::<Forwards>();
}
