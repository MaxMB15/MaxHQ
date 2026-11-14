// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Port forwards over a link, against the fake `maxd`.

use maxhq_protocol::ids::{ForwardId, IdempotencyKey, PortId, StreamId, WorkspaceId};
use maxhq_protocol::message::{ActionOutcome, TunnelEnd};
use maxhq_protocol::model::{BindAddress, ListeningPort};
use tokio::net::TcpStream;

use super::*;
use crate::forward::ForwardError;

/// A frame from the client.
#[derive(Debug)]
enum Got {
    Message(ClientMessage),
    Data { stream: StreamId, data: Vec<u8> },
}

/// The fake `maxd` of a device, with tunnel frames.
struct Device {
    maxd: FakeMaxd,
}

impl Device {
    /// Answers the next dial as a device that does `capabilities`. The
    /// client's sync comes first.
    async fn dialed(rig: &mut Rig, capabilities: Capabilities) -> Self {
        let mut maxd = rig.next_maxd().await;
        maxd.accept_with(InstanceId::generate(), "boot-1", capabilities)
            .await;
        Self { maxd }
    }

    async fn got(&mut self) -> Got {
        let mut buf = vec![0; 64 * 1024];
        loop {
            match self.maxd.decoder.next_frame().unwrap() {
                Some(Frame::Message(json)) => {
                    return Got::Message(serde_json::from_slice(json).unwrap());
                }
                Some(Frame::TunnelData { stream, data }) => {
                    return Got::Data {
                        stream,
                        data: data.to_vec(),
                    };
                }
                Some(other) => panic!("expected a message or tunnel data, got {other:?}"),
                None => {}
            }
            let read = tokio::time::timeout(PATIENCE, self.maxd.stream.read(&mut buf))
                .await
                .expect("a frame in time")
                .unwrap();
            assert_ne!(read, 0, "the client closed the connection");
            self.maxd.decoder.push(&buf[..read]);
        }
    }

    async fn message(&mut self) -> ClientMessage {
        match self.got().await {
            Got::Message(message) => message,
            Got::Data { stream, .. } => panic!("expected a message, got bytes on {stream}"),
        }
    }

    /// `len` bytes the client sent on `stream`.
    async fn bytes(&mut self, stream: StreamId, len: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        while bytes.len() < len {
            match self.got().await {
                Got::Data { stream: on, data } => {
                    assert_eq!(on, stream);
                    bytes.extend(data);
                }
                Got::Message(message) => panic!("expected bytes, got {message:?}"),
            }
        }
        bytes
    }

    async fn send(&mut self, message: &ServerMessage) {
        self.maxd.send(message).await;
    }

    async fn send_bytes(&mut self, stream: StreamId, data: &[u8]) {
        let mut out = Vec::new();
        frame::encode_tunnel_data(&mut out, stream, data);
        self.maxd.stream.write_all(&out).await.unwrap();
    }

    /// Takes the client's next `OpenForward`, which must be for `port`,
    /// and opens a forward for it. Gives the forward, the local port the
    /// client asked with, and the key.
    async fn opens(&mut self, port: PortId) -> (ForwardId, u16, IdempotencyKey) {
        let (key, local_port) = match self.message().await {
            ClientMessage::OpenForward {
                key,
                port: asked,
                local_port,
            } => {
                assert_eq!(asked, port);
                (key, local_port)
            }
            other => panic!("expected an open, got {other:?}"),
        };
        let forward = ForwardId::generate();
        self.send(&ServerMessage::ActionResult {
            key,
            outcome: ActionOutcome::ForwardOpened { forward },
        })
        .await;
        (forward, local_port, key)
    }

    /// The client sends nothing for a while.
    async fn quiet(&mut self) {
        assert!(self.maxd.decoder.next_frame().unwrap().is_none());
        let mut buf = [0; 1024];
        let read =
            tokio::time::timeout(Duration::from_millis(300), self.maxd.stream.read(&mut buf)).await;
        assert!(read.is_err(), "the client sent {read:?}");
    }
}

fn tunnels() -> Capabilities {
    Capabilities {
        tunnels: true,
        ..Capabilities::default()
    }
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn listening() -> ListeningPort {
    ListeningPort {
        id: PortId::generate(),
        workspace: WorkspaceId::generate(),
        agent: None,
        port: free_port(),
        bind: BindAddress::Loopback,
        process: None,
        url_hint: None,
        first_seen: Timestamp(1),
        gone_since: None,
    }
}

async fn until_connected(events: &mut mpsc::Receiver<LinkEvent>) {
    while !is_connected(&next_state(events).await) {}
}

/// Waits for the link's one forward to be `forward`.
async fn until_through(handle: &LinkHandle, forward: ForwardId) {
    let through = async {
        while handle.forwards().first().and_then(|local| local.forward) != Some(forward) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::time::timeout(PATIENCE, through)
        .await
        .expect("the forward in time");
}

/// The stream a connection on the local port opened through `forward`.
async fn opened_through(device: &mut Device, forward: ForwardId) -> StreamId {
    match device.message().await {
        ClientMessage::TunnelOpen {
            stream,
            forward: through,
        } => {
            assert_eq!(through, forward);
            stream
        }
        other => panic!("expected a stream, got {other:?}"),
    }
}

#[tokio::test]
async fn a_forward_opens_carries_bytes_and_closes() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    let (handle, mut events) = rig.client.link(spec(Route::Local));
    let mut device = Device::dialed(&mut rig, tunnels()).await;
    until_connected(&mut events).await;

    let port = listening();
    let (opened, (forward, local_port, _)) =
        tokio::join!(handle.open_forward(&port), device.opens(port.id));
    assert_eq!(opened.unwrap(), local_port);
    assert_eq!(
        handle.forwards(),
        vec![LocalForward {
            port: port.id,
            local_port,
            forward: Some(forward),
        }]
    );

    let mut socket = TcpStream::connect(("127.0.0.1", local_port)).await.unwrap();
    let stream = opened_through(&mut device, forward).await;
    device.send(&ServerMessage::TunnelOpened { stream }).await;
    let request = b"GET / HTTP/1.1\r\n\r\n";
    socket.write_all(request).await.unwrap();
    assert_eq!(device.bytes(stream, request.len()).await, request);

    let reply = b"HTTP/1.1 200 OK\r\n\r\nhello";
    device.send_bytes(stream, reply).await;
    let mut got = vec![0; reply.len()];
    socket.read_exact(&mut got).await.unwrap();
    assert_eq!(got, reply);

    device
        .send(&ServerMessage::TunnelClose {
            stream,
            how: TunnelEnd::Write,
        })
        .await;
    let mut rest = Vec::new();
    socket.read_to_end(&mut rest).await.unwrap();
    assert!(rest.is_empty());
    socket.shutdown().await.unwrap();
    assert_eq!(
        device.message().await,
        ClientMessage::TunnelClose {
            stream,
            how: TunnelEnd::Write,
        }
    );

    handle.close_forward(port.id);
    assert_eq!(
        device.message().await,
        ClientMessage::CloseForward { forward }
    );
    assert!(handle.forwards().is_empty());
}

#[tokio::test]
async fn a_forward_asks_again_on_the_same_port_after_a_reconnect() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    let (handle, mut events) = rig.client.link(spec(Route::Local));
    let mut device = Device::dialed(&mut rig, tunnels()).await;
    until_connected(&mut events).await;
    let port = listening();
    let (opened, (_, local_port, first_key)) =
        tokio::join!(handle.open_forward(&port), device.opens(port.id));
    opened.unwrap();

    drop(device);
    let mut device = Device::dialed(&mut rig, tunnels()).await;
    let (forward, asked_on, key) = device.opens(port.id).await;
    assert_eq!(asked_on, local_port);
    assert_ne!(key, first_key);
    until_through(&handle, forward).await;

    let _socket = TcpStream::connect(("127.0.0.1", local_port)).await.unwrap();
    opened_through(&mut device, forward).await;
}

#[tokio::test]
async fn an_open_the_connection_drops_under_fails_and_isnt_sent_again() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    let (handle, mut events) = rig.client.link(spec(Route::Local));
    let mut device = Device::dialed(&mut rig, tunnels()).await;
    until_connected(&mut events).await;

    let port = listening();
    let drops = async move {
        assert!(matches!(
            device.message().await,
            ClientMessage::OpenForward { .. }
        ));
        drop(device);
    };
    let (opened, ()) = tokio::join!(handle.open_forward(&port), drops);
    assert!(matches!(opened, Err(ForwardError::NotConnected)));
    assert!(handle.forwards().is_empty());

    let mut device = Device::dialed(&mut rig, tunnels()).await;
    device.quiet().await;
}

#[tokio::test]
async fn a_forward_waits_for_a_connection_that_tunnels() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    let (handle, mut events) = rig.client.link(spec(Route::Local));
    let port = listening();
    assert!(matches!(
        handle.open_forward(&port).await,
        Err(ForwardError::NotConnected)
    ));

    let mut device = Device::dialed(&mut rig, Capabilities::default()).await;
    until_connected(&mut events).await;
    assert!(matches!(
        handle.open_forward(&port).await,
        Err(ForwardError::Refused(_))
    ));
    assert!(handle.forwards().is_empty());
    device.quiet().await;
}

#[tokio::test]
async fn a_forward_closes_when_its_port_goes() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    let (handle, mut events) = rig.client.link(spec(Route::Local));
    let mut device = Device::dialed(&mut rig, tunnels()).await;
    until_connected(&mut events).await;
    let port = listening();
    let (opened, (forward, _, _)) = tokio::join!(handle.open_forward(&port), device.opens(port.id));
    opened.unwrap();

    device
        .send(&ServerMessage::Snapshot(Snapshot {
            seq: Seq(1),
            entities: vec![Entity::ListeningPort(port.clone())],
        }))
        .await;
    assert!(matches!(next(&mut events).await, LinkEvent::Synced(_)));
    assert_eq!(handle.forwards().len(), 1);

    let gone = Change::Remove {
        key: EntityKey::ListeningPort(port.id),
    };
    device.send(&delta(2, vec![gone])).await;
    assert!(matches!(next(&mut events).await, LinkEvent::Changed(_)));
    assert_eq!(
        device.message().await,
        ClientMessage::CloseForward { forward }
    );
    assert!(handle.forwards().is_empty());
}

#[tokio::test]
async fn a_forward_that_opens_too_late_closes_again() {
    let timing = Timing {
        reply: Duration::from_millis(200),
        ..FAST
    };
    let mut rig = Rig::new(timing, Tailnet::default(), &[]);
    let (handle, mut events) = rig.client.link(spec(Route::Local));
    let mut device = Device::dialed(&mut rig, tunnels()).await;
    until_connected(&mut events).await;

    let port = listening();
    let asked = async {
        match device.message().await {
            ClientMessage::OpenForward { key, .. } => key,
            other => panic!("expected an open, got {other:?}"),
        }
    };
    let (opened, key) = tokio::join!(handle.open_forward(&port), asked);
    assert!(matches!(opened, Err(ForwardError::Refused(_))));
    assert!(handle.forwards().is_empty());

    let forward = ForwardId::generate();
    device
        .send(&ServerMessage::ActionResult {
            key,
            outcome: ActionOutcome::ForwardOpened { forward },
        })
        .await;
    assert_eq!(
        device.message().await,
        ClientMessage::CloseForward { forward }
    );
}
