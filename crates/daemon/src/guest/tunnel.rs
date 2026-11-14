// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The guest's end of the host's tunnels. The host opens a stream to a
//! port in the container with `TunnelOpen`, and the guest connects to it
//! on loopback, or on the one address it's bound to. It connects only to
//! ports its port watch found, and to at most [`MAX_TUNNELS`] at once.
//!
//! The host opens an SSH session the same way, and the guest's own SSH
//! server takes it on an in-memory pipe.
//!
//! Each stream pumps through [`connect`] or [`open`], with the same credit
//! as a host folder agent's. Bytes past the credit the guest granted reset
//! the stream.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use maxhq_protocol::guest::{GuestPort, GuestToHost, HostToGuest, TunnelTarget, MAX_TUNNELS};
use maxhq_protocol::ids::StreamId;
use maxhq_protocol::message::TunnelEnd;
use tokio::sync::watch;
use tracing::debug;

use super::ssh::{self, Ssh};
use crate::tunnel::{connect, open, targets, End, Out, Sink, Words};

/// One host link's streams. Dropping it aborts them all.
pub struct GuestTunnels {
    sink: Arc<dyn Sink<GuestToHost>>,
    detected: watch::Receiver<Vec<GuestPort>>,
    ssh: Option<Arc<Ssh>>,
    streams: Arc<Mutex<Streams>>,
}

#[derive(Default)]
struct Streams {
    open: HashMap<StreamId, (u64, End)>,
    /// Numbers each stream, so a pump that ends late leaves a new stream
    /// with the same ID alone.
    next: u64,
}

/// The guest link's words. A refusal carries its reason as text.
struct ToHost;

impl Words for ToHost {
    type Message = GuestToHost;
    fn opened(stream: StreamId) -> GuestToHost {
        GuestToHost::TunnelOpened { stream }
    }
    fn refused(stream: StreamId, reason: String) -> GuestToHost {
        GuestToHost::TunnelRefused { stream, reason }
    }
    fn credit(stream: StreamId, granted: u64) -> GuestToHost {
        GuestToHost::TunnelCredit { stream, granted }
    }
    fn close(stream: StreamId, how: TunnelEnd) -> GuestToHost {
        GuestToHost::TunnelClose { stream, how }
    }
}

impl GuestTunnels {
    /// Streams that answer on `sink` and connect only to the ports in
    /// `detected`, which the port watch keeps. SSH sessions go to `ssh`,
    /// and are refused without one.
    pub fn new(
        sink: Arc<dyn Sink<GuestToHost>>,
        detected: watch::Receiver<Vec<GuestPort>>,
        ssh: Option<Arc<Ssh>>,
    ) -> Self {
        Self {
            sink,
            detected,
            ssh,
            streams: Arc::default(),
        }
    }

    /// Takes a tunnel message from the host. False for any other message.
    pub fn handle(&self, message: &HostToGuest) -> bool {
        match message {
            HostToGuest::TunnelOpen {
                stream,
                target: TunnelTarget::Port(port),
            } => self.open(*stream, *port),
            HostToGuest::TunnelOpen {
                stream,
                target: TunnelTarget::Ssh,
            } => self.open_ssh(*stream),
            HostToGuest::TunnelCredit { stream, granted } => {
                if let Some((_, end)) = self.lock().open.get(stream) {
                    end.credit(*granted);
                }
            }
            HostToGuest::TunnelClose { stream, how } => {
                let mut streams = self.lock();
                match how {
                    TunnelEnd::Reset => drop(streams.open.remove(stream)),
                    TunnelEnd::Write => {
                        if let Some((_, end)) = streams.open.get(stream) {
                            end.close(TunnelEnd::Write);
                        }
                    }
                }
            }
            _ => return false,
        }
        true
    }

    /// Takes a tunnel frame's bytes from the host.
    pub fn data(&self, stream: StreamId, data: &[u8]) {
        let mut streams = self.lock();
        let Some((_, end)) = streams.open.get(&stream) else {
            debug!(%stream, "bytes for a stream that isn't open are dropped");
            return;
        };
        if !end.data(data) {
            debug!(%stream, "the host sent past its credit, so the stream resets");
            streams.open.remove(&stream);
            self.send(GuestToHost::TunnelClose {
                stream,
                how: TunnelEnd::Reset,
            });
        }
    }

    fn open(&self, stream: StreamId, port: u16) {
        let mut streams = self.lock();
        if !self.room(&streams, stream) {
            return;
        }
        let bind = self
            .detected
            .borrow()
            .iter()
            .find(|found| found.port == port)
            .map(|found| found.bind.clone());
        let Some(bind) = bind else {
            self.refuse(stream, format!("nothing listens on port {port}"));
            return;
        };
        let sink = self.sink.clone();
        self.insert(&mut streams, stream, |ended| {
            connect::<ToHost>(stream, targets(&bind, port), sink, ended)
        });
    }

    /// Opens an SSH session into the container on `stream`.
    fn open_ssh(&self, stream: StreamId) {
        let Some(ssh) = &self.ssh else {
            self.refuse(stream, "this guest doesn't take SSH".into());
            return;
        };
        let mut streams = self.lock();
        if !self.room(&streams, stream) {
            return;
        }
        let (ours, theirs) = tokio::io::duplex(ssh::PIPE);
        ssh.session(theirs);
        let sink = self.sink.clone();
        self.insert(&mut streams, stream, |ended| {
            open::<ToHost, _>(stream, async move { Ok(ours) }, sink, ended)
        });
    }

    /// Whether a new stream `stream` may open. A stream ID already open is
    /// ignored, and one past the limit is refused.
    fn room(&self, streams: &Streams, stream: StreamId) -> bool {
        if streams.open.contains_key(&stream) {
            debug!(%stream, "a stream ID already open is ignored");
            return false;
        }
        if streams.open.len() >= MAX_TUNNELS {
            self.refuse(stream, "too many tunnels are open".into());
            return false;
        }
        true
    }

    /// Keeps the stream `start` opens, which forgets it when it ends.
    fn insert(
        &self,
        streams: &mut Streams,
        stream: StreamId,
        start: impl FnOnce(Box<dyn FnOnce() + Send>) -> End,
    ) {
        let serial = streams.next;
        streams.next += 1;
        let weak: Weak<Mutex<Streams>> = Arc::downgrade(&self.streams);
        let ended = Box::new(move || {
            let Some(streams) = weak.upgrade() else {
                return;
            };
            let mut streams = streams.lock().unwrap_or_else(PoisonError::into_inner);
            if streams
                .open
                .get(&stream)
                .is_some_and(|(at, _)| *at == serial)
            {
                streams.open.remove(&stream);
            }
        });
        let end = start(ended);
        streams.open.insert(stream, (serial, end));
    }

    fn refuse(&self, stream: StreamId, reason: String) {
        self.send(GuestToHost::TunnelRefused { stream, reason });
    }

    fn send(&self, message: GuestToHost) {
        if self.sink.send(Out::Message(message)).is_err() {
            debug!("the host link is gone");
        }
    }

    fn lock(&self) -> MutexGuard<'_, Streams> {
        self.streams.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How many streams are open, for tests.
    #[cfg(test)]
    fn open_streams(&self) -> usize {
        self.lock().open.len()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use maxhq_protocol::ids::{ClientId, StreamId};
    use maxhq_protocol::message::{ErrorCode, ServerMessage};
    use maxhq_protocol::model::BindAddress;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    use super::*;
    use crate::tunnel::registry_tests::{forward, listen, next, sink, world, Rx, World};
    use crate::tunnel::{ClientTunnels, GuestRelay};

    /// A client, host `maxd` and a guest, joined by fake links that pass
    /// each side's queue to the other.
    struct Linked {
        world: World,
        client: ClientTunnels,
        to_client: Rx<ServerMessage>,
        guest: Arc<GuestTunnels>,
        detected: watch::Sender<Vec<GuestPort>>,
        links: Vec<JoinHandle<()>>,
    }

    impl Drop for Linked {
        fn drop(&mut self) {
            for link in &self.links {
                link.abort();
            }
        }
    }

    fn linked() -> Linked {
        let world = world();
        let (client_sink, to_client) = sink();
        let client = world.tunnels.attach(ClientId::derive("mac"), client_sink);
        let (guest_sink, mut to_guest) = sink::<HostToGuest>();
        let relay: Arc<GuestRelay> = Arc::new(world.tunnels.guest(world.container, guest_sink));
        let (host_sink, mut to_host) = sink::<GuestToHost>();
        let (detected, watched) = watch::channel(Vec::new());
        let guest = Arc::new(GuestTunnels::new(host_sink, watched, None));
        let down = {
            let guest = guest.clone();
            tokio::spawn(async move {
                while let Some(out) = to_guest.recv().await {
                    match out {
                        Out::Message(message) => assert!(guest.handle(&message)),
                        Out::Data { stream, data } => guest.data(stream, &data),
                    }
                }
            })
        };
        let up = tokio::spawn(async move {
            while let Some(out) = to_host.recv().await {
                match out {
                    Out::Message(message) => assert!(relay.handle(&message)),
                    Out::Data { stream, data } => relay.data(stream, &data),
                }
            }
        });
        Linked {
            world,
            client,
            to_client,
            guest,
            detected,
            links: vec![down, up],
        }
    }

    impl Linked {
        /// A server in the container the guest has found, and the client's
        /// forward to it.
        async fn serve(&self) -> (TcpListener, maxhq_protocol::ids::ForwardId) {
            let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let number = server.local_addr().unwrap().port();
            self.detected.send_replace(vec![GuestPort {
                port: number,
                bind: BindAddress::Loopback,
                session: None,
                process: None,
            }]);
            let port = listen(&self.world.hub, self.world.container, number);
            (server, forward(&self.client, port))
        }

        async fn open(&mut self, forward: maxhq_protocol::ids::ForwardId) -> StreamId {
            let stream = StreamId::generate();
            self.client.open(stream, forward);
            assert_eq!(
                next(&mut self.to_client).await,
                Out::Message(ServerMessage::TunnelOpened { stream })
            );
            stream
        }

        /// Reads what the client hears on `stream` until it closes.
        async fn read_to_close(&mut self, stream: StreamId) -> (Vec<u8>, TunnelEnd) {
            let mut bytes = Vec::new();
            loop {
                match next(&mut self.to_client).await {
                    Out::Data { stream: at, data } if at == stream => bytes.extend(data),
                    Out::Message(ServerMessage::TunnelClose { stream: at, how })
                        if at == stream =>
                    {
                        return (bytes, how);
                    }
                    Out::Message(ServerMessage::TunnelCredit { .. }) => {}
                    other => panic!("unexpected: {other:?}"),
                }
            }
        }
    }

    async fn settle(check: impl Fn() -> bool) {
        for _ in 0..200 {
            if check() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("never settled");
    }

    #[tokio::test]
    async fn a_stream_relays_to_a_container_port_and_half_closes() {
        let mut l = linked();
        let (server, forward) = l.serve().await;
        let stream = l.open(forward).await;
        let (mut socket, _) = server.accept().await.unwrap();

        l.client.data(stream, b"hello");
        l.client.close(stream, TunnelEnd::Write);
        let mut heard = Vec::new();
        socket.read_to_end(&mut heard).await.unwrap();
        assert_eq!(heard, b"hello");

        socket.write_all(b"world").await.unwrap();
        socket.shutdown().await.unwrap();
        assert_eq!(
            l.read_to_close(stream).await,
            (b"world".to_vec(), TunnelEnd::Write)
        );
        let guest = l.guest.clone();
        settle(|| guest.open_streams() == 0).await;
    }

    #[tokio::test]
    async fn a_reset_from_the_client_aborts_the_socket() {
        let mut l = linked();
        let (server, forward) = l.serve().await;
        let stream = l.open(forward).await;
        let (mut socket, _) = server.accept().await.unwrap();
        l.client.close(stream, TunnelEnd::Reset);
        let mut buf = [0; 8];
        // An abort reads as end of file or as a reset connection.
        let read = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut buf))
            .await
            .unwrap();
        assert!(matches!(read, Ok(0) | Err(_)));
        let guest = l.guest.clone();
        settle(|| guest.open_streams() == 0).await;
    }

    #[tokio::test]
    async fn a_socket_that_fails_resets_the_client() {
        let mut l = linked();
        let (server, forward) = l.serve().await;
        let stream = l.open(forward).await;
        let (socket, _) = server.accept().await.unwrap();
        // Closing with unread bytes and no linger sends a reset.
        socket.set_zero_linger().unwrap();
        l.client.data(stream, b"unread");
        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(socket);
        let (_, how) = l.read_to_close(stream).await;
        assert!(matches!(how, TunnelEnd::Reset | TunnelEnd::Write));
    }

    #[tokio::test]
    async fn the_guest_connects_only_to_ports_it_found() {
        let mut l = linked();
        // A server the hub lists but the guest never found.
        let hidden = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let number = hidden.local_addr().unwrap().port();
        let port = listen(&l.world.hub, l.world.container, number);
        let forward = forward(&l.client, port);
        let stream = StreamId::generate();
        l.client.open(stream, forward);
        match next(&mut l.to_client).await {
            Out::Message(ServerMessage::TunnelRefused {
                stream: at,
                code,
                message,
            }) => {
                assert_eq!((at, code), (stream, ErrorCode::Unreachable));
                assert!(message.contains("nothing listens"), "{message}");
            }
            other => panic!("not refused: {other:?}"),
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(100), hidden.accept())
                .await
                .is_err(),
            "the guest connected anyway"
        );
    }

    #[tokio::test]
    async fn bytes_past_the_credit_reset_the_stream() {
        let (host_sink, mut to_host) = sink::<GuestToHost>();
        let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let number = server.local_addr().unwrap().port();
        let (_detected, watched) = watch::channel(vec![GuestPort {
            port: number,
            bind: BindAddress::Any,
            session: None,
            process: None,
        }]);
        let guest = GuestTunnels::new(host_sink, watched, None);
        let stream = StreamId::generate();
        assert!(guest.handle(&HostToGuest::TunnelOpen {
            stream,
            target: TunnelTarget::Port(number),
        }));
        assert_eq!(
            next(&mut to_host).await,
            Out::Message(GuestToHost::TunnelOpened { stream })
        );
        let _socket = server.accept().await.unwrap();
        let chunk = vec![0; 64 * 1024];
        for _ in 0..4 {
            guest.data(stream, &chunk);
        }
        assert_eq!(guest.open_streams(), 1);
        guest.data(stream, b"x");
        assert_eq!(guest.open_streams(), 0);
        let mut heard = Vec::new();
        while let Ok(out) = to_host.try_recv() {
            heard.push(out);
        }
        assert!(heard.contains(&Out::Message(GuestToHost::TunnelClose {
            stream,
            how: TunnelEnd::Reset,
        })));
    }

    #[tokio::test]
    async fn a_dropped_link_aborts_every_stream() {
        let (host_sink, mut to_host) = sink::<GuestToHost>();
        let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let number = server.local_addr().unwrap().port();
        let (_detected, watched) = watch::channel(vec![GuestPort {
            port: number,
            bind: BindAddress::Loopback,
            session: None,
            process: None,
        }]);
        let guest = GuestTunnels::new(host_sink, watched, None);
        let stream = StreamId::generate();
        guest.handle(&HostToGuest::TunnelOpen {
            stream,
            target: TunnelTarget::Port(number),
        });
        next(&mut to_host).await;
        let (mut socket, _) = server.accept().await.unwrap();
        drop(guest);
        let mut buf = [0; 8];
        let read = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut buf))
            .await
            .unwrap();
        assert!(matches!(read, Ok(0) | Err(_)));
    }
}
