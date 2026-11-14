// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! One end of a stream: a TCP connection this process makes to a
//! listening port, pumped to and from a link with credit each way. Host
//! `maxd` runs one for a host folder agent's port, and the guest for a
//! port in its container.
//!
//! Each side may send [`STREAM_CREDIT`] bytes once the stream opens, and
//! more as the other side grants it. Bytes past what this end granted
//! reset the stream. A peer's [`TunnelEnd::Write`] shuts the socket's
//! write side once its bytes are written, and the socket's end of file
//! sends one back. A reset either way aborts the socket.
//!
//! [`open`] pumps any socket the same way, such as the guest's end of an
//! in-memory pipe to its SSH server.

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use maxhq_protocol::frame::MAX_TUNNEL_CHUNK;
use maxhq_protocol::ids::StreamId;
use maxhq_protocol::message::{ErrorCode, ServerMessage, TunnelEnd};
use maxhq_protocol::model::BindAddress;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Notify};

use super::{Out, Sink, MOST_CREDIT, STREAM_CREDIT};

/// How long a connect may take before the stream is refused.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How many bytes written to the socket earn the peer more credit at once.
const GRANT_STEP: u64 = MAX_TUNNEL_CHUNK as u64;

/// The messages one side of a link uses for its streams.
pub(crate) trait Words: Send + Sync + 'static {
    /// The message type the link carries.
    type Message: Send + 'static;
    /// The stream is connected.
    fn opened(stream: StreamId) -> Self::Message;
    /// The stream couldn't connect.
    fn refused(stream: StreamId, reason: String) -> Self::Message;
    /// The peer may send more.
    fn credit(stream: StreamId, granted: u64) -> Self::Message;
    /// The stream, or this end's half of it, is over.
    fn close(stream: StreamId, how: TunnelEnd) -> Self::Message;
}

/// The client link's words. A refusal is [`ErrorCode::Unreachable`].
pub(crate) struct ToClient;

impl Words for ToClient {
    type Message = ServerMessage;
    fn opened(stream: StreamId) -> ServerMessage {
        ServerMessage::TunnelOpened { stream }
    }
    fn refused(stream: StreamId, message: String) -> ServerMessage {
        ServerMessage::TunnelRefused {
            stream,
            code: ErrorCode::Unreachable,
            message,
        }
    }
    fn credit(stream: StreamId, granted: u64) -> ServerMessage {
        ServerMessage::TunnelCredit { stream, granted }
    }
    fn close(stream: StreamId, how: TunnelEnd) -> ServerMessage {
        ServerMessage::TunnelClose { stream, how }
    }
}

/// What a peer sent that the socket's writer acts on, in order.
enum Event {
    Data(Vec<u8>),
    Shut,
}

/// Bytes this end may still send, and a wake for when it may send more.
struct Credit {
    left: AtomicU64,
    more: Notify,
}

/// Sets `value` to what `change` makes of it. False, and nothing changes,
/// when `change` gives none.
fn update(value: &AtomicU64, change: impl Fn(u64) -> Option<u64>) -> bool {
    let mut now = value.load(Ordering::Acquire);
    loop {
        let Some(next) = change(now) else {
            return false;
        };
        match value.compare_exchange_weak(now, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(actual) => now = actual,
        }
    }
}

impl Credit {
    fn grant(&self, granted: u64) {
        update(&self.left, |left| {
            Some(left.saturating_add(granted).min(MOST_CREDIT))
        });
        self.more.notify_one();
    }

    /// Waits until there's credit, and gives how much.
    async fn wait(&self) -> u64 {
        loop {
            let left = self.left.load(Ordering::Acquire);
            if left > 0 {
                return left;
            }
            self.more.notified().await;
        }
    }

    fn spend(&self, n: u64) {
        self.left.fetch_sub(n, Ordering::AcqRel);
    }
}

/// The handle a link's dispatch holds for one stream. Dropping it aborts
/// the stream.
pub(crate) struct End {
    events: mpsc::UnboundedSender<Event>,
    /// Bytes the peer may still send this end.
    window: Arc<AtomicU64>,
    credit: Arc<Credit>,
    abort: Arc<Notify>,
}

impl End {
    /// Takes bytes from the peer. False when they're over the credit this
    /// end granted, and the stream must be reset.
    pub(crate) fn data(&self, bytes: &[u8]) -> bool {
        let len = bytes.len() as u64;
        if !update(&self.window, |left| left.checked_sub(len)) {
            return false;
        }
        let _ = self.events.send(Event::Data(bytes.to_vec()));
        true
    }

    /// The peer may take more.
    pub(crate) fn credit(&self, granted: u64) {
        self.credit.grant(granted);
    }

    /// The peer closed its half, or reset the stream.
    pub(crate) fn close(&self, how: TunnelEnd) {
        match how {
            TunnelEnd::Write => {
                let _ = self.events.send(Event::Shut);
            }
            TunnelEnd::Reset => self.abort.notify_one(),
        }
    }
}

impl Drop for End {
    fn drop(&mut self) {
        self.abort.notify_one();
    }
}

/// The addresses to try for a port bound to `bind`. A socket on loopback
/// or on every address answers on 127.0.0.1 or ::1.
pub(crate) fn targets(bind: &BindAddress, port: u16) -> Vec<SocketAddr> {
    match bind {
        BindAddress::Loopback | BindAddress::Any => vec![
            SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port),
            SocketAddr::new(Ipv6Addr::LOCALHOST.into(), port),
        ],
        BindAddress::Address { ip } => ip
            .parse::<IpAddr>()
            .map(|ip| vec![SocketAddr::new(ip, port)])
            .unwrap_or_default(),
    }
}

/// Connects a stream to the first of `addrs` that answers, in a task of
/// its own. It says [`Words::opened`] or [`Words::refused`] on `sink`, then
/// pumps until the stream ends. `ended` runs once it's over, however it
/// ends.
pub(crate) fn connect<W: Words>(
    stream: StreamId,
    addrs: Vec<SocketAddr>,
    sink: Arc<dyn Sink<W::Message>>,
    ended: Box<dyn FnOnce() + Send>,
) -> End {
    open::<W, _>(stream, async move { dial(&addrs).await }, sink, ended)
}

/// What a stream can pump to: a TCP connection, or one end of an
/// in-memory pipe.
pub(crate) trait Socket: AsyncRead + AsyncWrite + Send + Unpin + 'static {
    /// Makes the coming close a reset, where the socket has one.
    fn abort(&self) {}
}

impl Socket for TcpStream {
    fn abort(&self) {
        // A reset, not a quiet close, so the other end sees the abort.
        let _ = self.set_zero_linger();
    }
}

impl Socket for DuplexStream {}

/// Like [`connect`], for the socket `socket` gives, or the reason it
/// couldn't.
pub(crate) fn open<W: Words, S: Socket>(
    stream: StreamId,
    socket: impl Future<Output = Result<S, String>> + Send + 'static,
    sink: Arc<dyn Sink<W::Message>>,
    ended: Box<dyn FnOnce() + Send>,
) -> End {
    let (events, rx) = mpsc::unbounded_channel();
    let end = End {
        events,
        window: Arc::new(AtomicU64::new(STREAM_CREDIT)),
        credit: Arc::new(Credit {
            left: AtomicU64::new(STREAM_CREDIT),
            more: Notify::new(),
        }),
        abort: Arc::new(Notify::new()),
    };
    let task = Task::<W> {
        stream,
        sink,
        window: end.window.clone(),
        credit: end.credit.clone(),
        marker: std::marker::PhantomData,
    };
    let abort = end.abort.clone();
    tokio::spawn(async move {
        task.run(socket, rx, &abort).await;
        ended();
    });
    end
}

/// Why a pump stopped before both halves closed.
enum Abort {
    /// The socket failed here, so the peer must hear of it.
    Local,
    /// The peer reset the stream, or the link is gone.
    Peer,
}

struct Task<W: Words> {
    stream: StreamId,
    sink: Arc<dyn Sink<W::Message>>,
    window: Arc<AtomicU64>,
    credit: Arc<Credit>,
    marker: std::marker::PhantomData<W>,
}

impl<W: Words> Task<W> {
    async fn run<S: Socket>(
        &self,
        socket: impl Future<Output = Result<S, String>>,
        events: mpsc::UnboundedReceiver<Event>,
        abort: &Notify,
    ) {
        let dialed = tokio::select! {
            biased;
            () = abort.notified() => return,
            dialed = socket => dialed,
        };
        let mut socket = match dialed {
            Ok(socket) => socket,
            Err(reason) => {
                let _ = self.say(W::refused(self.stream, reason));
                return;
            }
        };
        if self.say(W::opened(self.stream)).is_err() {
            return;
        }
        let ended = tokio::select! {
            biased;
            () = abort.notified() => Err(Abort::Peer),
            ended = self.pump(&mut socket, events) => ended,
        };
        if let Err(abort) = ended {
            if matches!(abort, Abort::Local) {
                let _ = self.say(W::close(self.stream, TunnelEnd::Reset));
            }
            socket.abort();
        }
    }

    fn say(&self, message: W::Message) -> Result<(), super::Gone> {
        self.sink.send(Out::Message(message))
    }

    async fn pump<S: Socket>(
        &self,
        socket: &mut S,
        events: mpsc::UnboundedReceiver<Event>,
    ) -> Result<(), Abort> {
        let (reader, writer) = tokio::io::split(socket);
        tokio::try_join!(self.reading(reader), self.writing(writer, events)).map(|_| ())
    }

    /// Socket to link, within the credit the peer granted.
    async fn reading(&self, mut reader: impl AsyncRead + Unpin) -> Result<(), Abort> {
        let mut buf = vec![0; MAX_TUNNEL_CHUNK];
        loop {
            let left = self.credit.wait().await;
            let room = usize::try_from(left).unwrap_or(usize::MAX).min(buf.len());
            let n = reader
                .read(&mut buf[..room])
                .await
                .map_err(|_| Abort::Local)?;
            if n == 0 {
                self.say(W::close(self.stream, TunnelEnd::Write))
                    .map_err(|_| Abort::Peer)?;
                return Ok(());
            }
            self.credit.spend(n as u64);
            self.sink
                .send(Out::Data {
                    stream: self.stream,
                    data: buf[..n].to_vec(),
                })
                .map_err(|_| Abort::Peer)?;
        }
    }

    /// Link to socket, granting the peer more as its bytes are written.
    async fn writing(
        &self,
        mut writer: impl AsyncWrite + Unpin,
        mut events: mpsc::UnboundedReceiver<Event>,
    ) -> Result<(), Abort> {
        let mut unacked = 0;
        loop {
            match events.recv().await {
                None => return Err(Abort::Peer),
                Some(Event::Data(bytes)) => {
                    writer.write_all(&bytes).await.map_err(|_| Abort::Local)?;
                    unacked += bytes.len() as u64;
                    if unacked >= GRANT_STEP {
                        // The window grows before the peer hears, so its
                        // next bytes always fit.
                        self.window.fetch_add(unacked, Ordering::AcqRel);
                        self.say(W::credit(self.stream, unacked))
                            .map_err(|_| Abort::Peer)?;
                        unacked = 0;
                    }
                }
                Some(Event::Shut) => {
                    let _ = writer.shutdown().await;
                    return Ok(());
                }
            }
        }
    }
}

/// The first of `addrs` that takes a connection.
async fn dial(addrs: &[SocketAddr]) -> Result<TcpStream, String> {
    let mut why = "no address to connect to".to_owned();
    for addr in addrs {
        match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
            Ok(Ok(socket)) => {
                let _ = socket.set_nodelay(true);
                return Ok(socket);
            }
            Ok(Err(err)) => why = format!("couldn't connect to {addr}: {err}"),
            Err(_) => why = format!("{addr} didn't answer in time"),
        }
    }
    Err(why)
}

#[cfg(test)]
mod tests {
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    use super::*;
    use crate::tunnel::Queue;

    type Rx = mpsc::UnboundedReceiver<Out<ServerMessage>>;

    struct Started {
        stream: StreamId,
        end: End,
        rx: Rx,
        ended: oneshot::Receiver<()>,
    }

    fn start(addr: SocketAddr) -> Started {
        let (queue, rx) = Queue::new();
        let (done, ended) = oneshot::channel();
        let stream = StreamId::generate();
        let end = connect::<ToClient>(
            stream,
            vec![addr],
            Arc::new(queue),
            Box::new(move || {
                let _ = done.send(());
            }),
        );
        Started {
            stream,
            end,
            rx,
            ended,
        }
    }

    async fn next(rx: &mut Rx) -> Out<ServerMessage> {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out")
            .expect("queue closed")
    }

    async fn over(ended: oneshot::Receiver<()>) {
        tokio::time::timeout(Duration::from_secs(5), ended)
            .await
            .expect("timed out")
            .expect("ended without saying");
    }

    #[tokio::test]
    async fn a_half_close_goes_both_ways() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut started = start(listener.local_addr().unwrap());
        let (mut server, _) = listener.accept().await.unwrap();
        assert_eq!(
            next(&mut started.rx).await,
            Out::Message(ServerMessage::TunnelOpened {
                stream: started.stream
            })
        );

        assert!(started.end.data(b"ping"));
        started.end.close(TunnelEnd::Write);
        let mut got = Vec::new();
        server.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, b"ping");

        server.write_all(b"pong").await.unwrap();
        server.shutdown().await.unwrap();
        let mut back = Vec::new();
        loop {
            match next(&mut started.rx).await {
                Out::Data { data, .. } => back.extend(data),
                Out::Message(ServerMessage::TunnelClose { how, .. }) => {
                    assert_eq!(how, TunnelEnd::Write);
                    break;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(back, b"pong");
        over(started.ended).await;
    }

    #[tokio::test]
    async fn a_reset_from_the_peer_aborts_the_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut started = start(listener.local_addr().unwrap());
        let (mut server, _) = listener.accept().await.unwrap();
        next(&mut started.rx).await;
        started.end.close(TunnelEnd::Reset);
        let mut buf = [0; 8];
        let read = server.read(&mut buf).await;
        assert!(read.is_err(), "{read:?}");
        over(started.ended).await;
    }

    #[tokio::test]
    async fn a_reset_from_the_socket_reaches_the_peer() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut started = start(listener.local_addr().unwrap());
        let (server, _) = listener.accept().await.unwrap();
        next(&mut started.rx).await;
        server.set_zero_linger().unwrap();
        drop(server);
        assert_eq!(
            next(&mut started.rx).await,
            Out::Message(ServerMessage::TunnelClose {
                stream: started.stream,
                how: TunnelEnd::Reset,
            })
        );
        over(started.ended).await;
    }

    #[tokio::test]
    async fn dropping_the_end_aborts_the_stream() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut started = start(listener.local_addr().unwrap());
        let (mut server, _) = listener.accept().await.unwrap();
        next(&mut started.rx).await;
        drop(started.end);
        let mut buf = [0; 8];
        assert!(server.read(&mut buf).await.is_err());
        over(started.ended).await;
    }

    #[tokio::test]
    async fn bytes_over_the_window_are_refused() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let started = start(listener.local_addr().unwrap());
        let window = vec![0; STREAM_CREDIT as usize];
        assert!(started.end.data(&window));
        assert!(!started.end.data(b"one more"));
    }

    #[tokio::test]
    async fn writing_grants_the_peer_more() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut started = start(listener.local_addr().unwrap());
        let (mut server, _) = listener.accept().await.unwrap();
        next(&mut started.rx).await;
        assert!(started.end.data(&vec![3; GRANT_STEP as usize]));
        let mut buf = vec![0; GRANT_STEP as usize];
        server.read_exact(&mut buf).await.unwrap();
        assert_eq!(
            next(&mut started.rx).await,
            Out::Message(ServerMessage::TunnelCredit {
                stream: started.stream,
                granted: GRANT_STEP,
            })
        );
    }

    #[tokio::test]
    async fn reading_stops_at_the_credit_and_goes_on_when_granted() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut started = start(listener.local_addr().unwrap());
        let (mut server, _) = listener.accept().await.unwrap();
        next(&mut started.rx).await;
        let credit = STREAM_CREDIT as usize;
        server.write_all(&vec![1; credit + 1000]).await.unwrap();
        let mut got = 0;
        while got < credit {
            if let Out::Data { data, .. } = next(&mut started.rx).await {
                got += data.len();
            }
        }
        assert_eq!(got, credit);
        let more = tokio::time::timeout(Duration::from_millis(200), started.rx.recv()).await;
        assert!(more.is_err(), "sent past the credit: {more:?}");
        started.end.credit(1000);
        let Out::Data { data, .. } = next(&mut started.rx).await else {
            panic!("expected bytes");
        };
        assert_eq!(data.len(), 1000);
    }

    #[tokio::test]
    async fn a_huge_grant_only_goes_as_far_as_the_most_credit() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut started = start(listener.local_addr().unwrap());
        let (mut server, _) = listener.accept().await.unwrap();
        next(&mut started.rx).await;
        started.end.credit(u64::MAX);
        let most = MOST_CREDIT as usize;
        server.write_all(&vec![1; most + 1000]).await.unwrap();
        let mut got = 0;
        while got < most {
            if let Out::Data { data, .. } = next(&mut started.rx).await {
                got += data.len();
            }
        }
        assert_eq!(got, most);
        let more = tokio::time::timeout(Duration::from_millis(200), started.rx.recv()).await;
        assert!(more.is_err(), "sent past the most credit: {more:?}");
        started.end.credit(1000);
        let Out::Data { data, .. } = next(&mut started.rx).await else {
            panic!("expected bytes");
        };
        assert_eq!(data.len(), 1000);
    }

    #[tokio::test]
    async fn nothing_listening_is_refused() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let mut started = start(addr);
        assert!(matches!(
            next(&mut started.rx).await,
            Out::Message(ServerMessage::TunnelRefused { .. })
        ));
        over(started.ended).await;
    }

    #[test]
    fn targets_follow_the_bind() {
        assert_eq!(
            targets(&BindAddress::Loopback, 5173),
            vec![
                "127.0.0.1:5173".parse().unwrap(),
                "[::1]:5173".parse().unwrap()
            ]
        );
        assert_eq!(targets(&BindAddress::Any, 5173).len(), 2);
        assert_eq!(
            targets(
                &BindAddress::Address {
                    ip: "10.0.3.15".into()
                },
                80
            ),
            vec!["10.0.3.15:80".parse().unwrap()]
        );
        assert!(targets(
            &BindAddress::Address {
                ip: "nonsense".into()
            },
            80
        )
        .is_empty());
    }
}
