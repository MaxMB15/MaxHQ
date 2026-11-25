// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! `maxd`'s guest links as `main.rs` wires them, between a client on the
//! server's socket and a fake guest on its proxy socket.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use maxhq_protocol::agents::ClientRole;
use maxhq_protocol::frame::{self, Frame, FrameDecoder};
use maxhq_protocol::guest::{
    GuestPort, GuestToHost, HostToGuest, Pace, TokenReply, TokenWant, TunnelTarget, ATTACHMENTS_DIR,
};
use maxhq_protocol::ids::{
    AccountId, BootId, ClientId, DeviceId, ForwardId, IdempotencyKey, PortId, SessionId, StreamId,
    TransferId, WorkspaceId,
};
use maxhq_protocol::message::{
    ActionOutcome, ClientHello, ClientInfo, TunnelEnd, UploadSource, UploadTarget,
};
use maxhq_protocol::model::{BindAddress, Device, Vendor, Workspace, WorkspaceLocation};
use maxhq_protocol::state::{Change, Entity};
use maxhq_protocol::{ClientMessage, ProtocolRange, ServerMessage};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UnixStream};

use super::link::VERSION;
use super::tests::{
    eventually, instance, token, within, Brokers, FakeGuest, Peer, Upgrades, QUIET,
};
use super::wire::{Abort, Read};
use super::{socket_path, GUESTS_DIR};
use crate::db::Db;
use crate::hub::Hub;
use crate::session::fake::FakeHolders;
use crate::session::holders::SessionOwner;
use crate::testing::TempDir;
use crate::workspace::GuestLinks;

/// The container's instance.
const NAME: &str = "mhq-wired";

/// `maxd` on a device with a container workspace and a folder one, linked
/// as `serve` links them, its server on a socket in a temp dir.
struct Rig {
    temp: TempDir,
    hub: Hub,
    owners: crate::session::Owners,
    brokers: Arc<Brokers>,
    linked: crate::Linked,
    container: WorkspaceId,
    folder: WorkspaceId,
    socket: PathBuf,
    _tasks: Vec<Abort<()>>,
}

impl Rig {
    fn new() -> Self {
        let temp = TempDir::new();
        std::fs::create_dir(temp.path().join(GUESTS_DIR)).unwrap();
        let device = DeviceId::generate();
        let hub = Hub::new(
            Device {
                id: device,
                name: "devbox".into(),
                os: "linux".into(),
                boot_id: BootId("boot-1".into()),
                maxd_version: "0.1.0".into(),
                winding_down_until: None,
                workspace_host: None,
                build: None,
                target: None,
                wsl: None,
            },
            HashMap::new(),
        );
        let container = WorkspaceId::generate();
        let folder = WorkspaceId::generate();
        let workspace = |id, location| Change::Upsert {
            value: Entity::Workspace(Workspace {
                id,
                project: None,
                device,
                location,
                managed: None,
                clone: None,
            }),
        };
        hub.apply([
            workspace(
                container,
                WorkspaceLocation::Container {
                    instance: NAME.into(),
                    path: "/work".into(),
                },
            ),
            workspace(
                folder,
                WorkspaceLocation::Folder {
                    path: temp.path().display().to_string(),
                },
            ),
        ]);

        let owners = crate::session::Owners::default();
        let brokers = Arc::new(Brokers::default());
        let linked = crate::link_guests(
            &hub,
            Arc::new(FakeHolders::new()),
            &owners,
            temp.path().join("attachments"),
            crate::GuestSetup {
                runtime: temp.path().to_owned(),
                codex: brokers.clone(),
                github: brokers.clone(),
                upgrader: Arc::new(Upgrades::default()),
                guest_binary: Some(PathBuf::from("/opt/maxd-guest")),
                keepalive: QUIET,
                skills: None,
            },
        );

        let socket = temp.path().join("maxd.sock");
        let listener = crate::server::bind(&socket).unwrap();
        let db = Db::open_in_memory().unwrap();
        let sessions =
            crate::session::for_tests(hub.clone(), db.clone(), temp.path().join("files"));
        let keys = crate::app_keys::AppKeys::open(temp.path().join("phones"), device);
        crate::app_keys::testing::trust(&keys);
        let serve = tokio::spawn({
            let hub = hub.clone();
            let transfers = linked.transfers.clone();
            let tunnels = linked.tunnels.clone();
            async move {
                listener
                    .serve(hub, db, sessions, transfers, tunnels, keys)
                    .await
            }
        });
        let links = tokio::spawn(linked.links.clone().run());
        Self {
            temp,
            hub,
            owners,
            brokers,
            linked,
            container,
            folder,
            socket,
            _tasks: vec![Abort(serve), Abort(links)],
        }
    }

    /// Links to the container's fake guest, and takes the guest's first
    /// watch.
    async fn link(&self) -> Peer {
        let name = instance(NAME);
        let guest = FakeGuest::bind(&socket_path(self.temp.path(), &name));
        let repo = "https://github.com/example-user/maxhq.git";
        self.linked.guests.open(self.container, &name, Some(repo));
        let wait = self
            .linked
            .guests
            .wait_up(self.container, Duration::from_secs(10));
        let (up, (mut peer, _hello)) =
            within(async { tokio::join!(wait, guest.accept(VERSION, Vec::new())) }).await;
        assert_eq!(up, Ok(Some(VERSION.to_owned())));
        // Nothing is held, so the guest watches nothing, slowly.
        let watch: HostToGuest = peer.recv().await;
        assert_eq!(
            watch,
            HostToGuest::WatchPorts {
                pace: Pace::Slow,
                holders: Vec::new(),
            }
        );
        peer
    }

    /// The port listening on `port` in `workspace`, once it shows.
    async fn port(&self, workspace: WorkspaceId, port: u16) -> PortId {
        let find = || {
            self.hub
                .snapshot()
                .entities
                .into_iter()
                .find_map(|entity| match entity {
                    Entity::ListeningPort(found)
                        if found.workspace == workspace
                            && found.port == port
                            && found.gone_since.is_none() =>
                    {
                        Some(found.id)
                    }
                    _ => None,
                })
        };
        eventually(|| find().is_some()).await;
        find().unwrap()
    }

    /// `session` runs `vendor` in `workspace`, as the session manager says.
    fn own(&self, sessions: &[(SessionId, WorkspaceId, Vendor)]) -> Vec<AccountId> {
        let mut accounts = Vec::new();
        let mut owners = HashMap::new();
        for (session, workspace, vendor) in sessions {
            let account = AccountId::generate();
            accounts.push(account);
            owners.insert(
                *session,
                SessionOwner {
                    workspace: *workspace,
                    account,
                    vendor: *vendor,
                },
            );
        }
        self.owners.set(owners);
        accounts
    }
}

/// What a client heard.
#[derive(Debug, PartialEq)]
enum Heard {
    Message(ServerMessage),
    Tunnel { stream: StreamId, data: Vec<u8> },
}

/// A client on the server's socket.
struct Client {
    stream: UnixStream,
    decoder: FrameDecoder,
}

impl Client {
    /// Connects, says hello and proves it's the app.
    async fn greeted(socket: &Path) -> Self {
        let stream = within(UnixStream::connect(socket)).await.unwrap();
        let mut client = Self {
            stream,
            decoder: FrameDecoder::new(),
        };
        let client_id = ClientId::generate();
        client
            .send(&ClientMessage::Hello(ClientHello {
                protocol: ProtocolRange::OURS,
                client: ClientInfo {
                    name: "maxd-wired-test".into(),
                    version: "0".into(),
                },
                client_id,
                role: ClientRole::App,
            }))
            .await;
        let ServerMessage::Hello(hello) = client.message().await else {
            panic!("expected a hello");
        };
        assert!(hello.capabilities.transfers);
        assert!(hello.capabilities.tunnels);
        let prove = crate::app_keys::testing::prove(&hello, client_id);
        let key = prove.action_key().expect("a proof is an action");
        assert_eq!(client.act(key, prove).await, ActionOutcome::Done);
        client
    }

    fn frame(message: &ClientMessage) -> Vec<u8> {
        let mut out = Vec::new();
        frame::encode_message(&mut out, message).unwrap();
        out
    }

    async fn send(&mut self, message: &ClientMessage) {
        self.write(&Self::frame(message)).await;
    }

    async fn write(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).await.unwrap();
    }

    async fn tunnel(&mut self, stream: StreamId, data: &[u8]) {
        let mut out = Vec::new();
        frame::encode_tunnel_data(&mut out, stream, data);
        self.write(&out).await;
    }

    /// The next message or stream bytes, past stream credit.
    async fn next(&mut self) -> Heard {
        within(async {
            let mut buf = vec![0; 64 * 1024];
            loop {
                match self.decoder.next_frame().unwrap() {
                    Some(Frame::Message(json)) => match serde_json::from_slice(json).unwrap() {
                        ServerMessage::TunnelCredit { .. } => continue,
                        message => return Heard::Message(message),
                    },
                    Some(Frame::TunnelData { stream, data }) => {
                        return Heard::Tunnel {
                            stream,
                            data: data.to_vec(),
                        }
                    }
                    Some(other) => panic!("expected a message or stream bytes, got {other:?}"),
                    None => {}
                }
                let read = self.stream.read(&mut buf).await.unwrap();
                assert!(read > 0, "the server closed the connection");
                self.decoder.push(&buf[..read]);
            }
        })
        .await
    }

    /// The next message.
    async fn message(&mut self) -> ServerMessage {
        match self.next().await {
            Heard::Message(message) => message,
            other => panic!("expected a message, got {other:?}"),
        }
    }

    /// Asks for an action, and gives its outcome.
    async fn act(&mut self, key: IdempotencyKey, message: ClientMessage) -> ActionOutcome {
        self.send(&message).await;
        loop {
            if let ServerMessage::ActionResult {
                key: answered,
                outcome,
            } = self.message().await
                && answered == key
            {
                return outcome;
            }
        }
    }

    /// Opens a forward to `port`.
    async fn forward(&mut self, port: PortId) -> ForwardId {
        let key = IdempotencyKey::generate();
        let open = ClientMessage::OpenForward {
            key,
            port,
            local_port: 40_000,
        };
        match self.act(key, open).await {
            ActionOutcome::ForwardOpened { forward } => forward,
            other => panic!("expected a forward, got {other:?}"),
        }
    }
}

/// The guest's next message or stream bytes, past stream credit.
async fn heard(peer: &mut Peer) -> Read {
    loop {
        match peer.next().await.unwrap() {
            Read::Message(body)
                if matches!(
                    serde_json::from_slice(&body),
                    Ok(HostToGuest::TunnelCredit { .. })
                ) => {}
            other => return other,
        }
    }
}

/// The guest's next message, past stream credit.
async fn told(peer: &mut Peer) -> HostToGuest {
    match heard(peer).await {
        Read::Message(body) => serde_json::from_slice(&body).unwrap(),
        other => panic!("expected a message, got {other:?}"),
    }
}

/// Sends stream bytes from the guest.
async fn guest_tunnel(peer: &mut Peer, stream: StreamId, data: &[u8]) {
    let mut out = Vec::new();
    frame::encode_tunnel_data(&mut out, stream, data);
    peer.bytes(&out).await;
}

/// Opens a stream through `forward` to the guest's `port`, and connects it.
async fn connect(client: &mut Client, peer: &mut Peer, forward: ForwardId, port: u16) -> StreamId {
    let stream = StreamId::generate();
    client
        .send(&ClientMessage::TunnelOpen { stream, forward })
        .await;
    assert_eq!(
        told(peer).await,
        HostToGuest::TunnelOpen {
            stream,
            target: TunnelTarget::Port(port),
        }
    );
    peer.send(&GuestToHost::TunnelOpened { stream }).await;
    assert_eq!(
        client.message().await,
        ServerMessage::TunnelOpened { stream }
    );
    stream
}

#[tokio::test]
async fn a_forward_reaches_the_guests_port_and_carries_a_stream() {
    let rig = Rig::new();
    let mut peer = rig.link().await;
    peer.send(&GuestToHost::Ports {
        listening: vec![GuestPort {
            port: 5173,
            bind: BindAddress::Loopback,
            session: None,
            process: None,
        }],
    })
    .await;
    let port = rig.port(rig.container, 5173).await;
    let mut client = Client::greeted(&rig.socket).await;
    let forward = client.forward(port).await;

    // Bytes both ways, then each half closes on its own.
    let stream = connect(&mut client, &mut peer, forward, 5173).await;
    client.tunnel(stream, b"GET / HTTP/1.1\r\n\r\n").await;
    let Read::Tunnel { stream: to, data } = heard(&mut peer).await else {
        panic!("expected the client's bytes");
    };
    assert_eq!(
        (to, data.as_slice()),
        (stream, &b"GET / HTTP/1.1\r\n\r\n"[..])
    );
    guest_tunnel(&mut peer, stream, b"HTTP/1.1 200 OK\r\n").await;
    assert_eq!(
        client.next().await,
        Heard::Tunnel {
            stream,
            data: b"HTTP/1.1 200 OK\r\n".to_vec(),
        }
    );
    client
        .send(&ClientMessage::TunnelClose {
            stream,
            how: TunnelEnd::Write,
        })
        .await;
    assert_eq!(
        told(&mut peer).await,
        HostToGuest::TunnelClose {
            stream,
            how: TunnelEnd::Write,
        }
    );
    // The guest still writes after the client's half closes.
    guest_tunnel(&mut peer, stream, b"hello").await;
    assert_eq!(
        client.next().await,
        Heard::Tunnel {
            stream,
            data: b"hello".to_vec(),
        }
    );
    peer.send(&GuestToHost::TunnelClose {
        stream,
        how: TunnelEnd::Write,
    })
    .await;
    assert_eq!(
        client.message().await,
        ServerMessage::TunnelClose {
            stream,
            how: TunnelEnd::Write,
        }
    );

    // A reset passes from the client to the guest.
    let stream = connect(&mut client, &mut peer, forward, 5173).await;
    client
        .send(&ClientMessage::TunnelClose {
            stream,
            how: TunnelEnd::Reset,
        })
        .await;
    assert_eq!(
        told(&mut peer).await,
        HostToGuest::TunnelClose {
            stream,
            how: TunnelEnd::Reset,
        }
    );

    // And from the guest to the client.
    let stream = connect(&mut client, &mut peer, forward, 5173).await;
    peer.send(&GuestToHost::TunnelClose {
        stream,
        how: TunnelEnd::Reset,
    })
    .await;
    assert_eq!(
        client.message().await,
        ServerMessage::TunnelClose {
            stream,
            how: TunnelEnd::Reset,
        }
    );
}

#[tokio::test]
async fn stream_bytes_in_the_same_read_as_their_open_come_after_it() {
    let rig = Rig::new();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = listener.local_addr().unwrap().port();
    let port = crate::tunnel::registry_tests::listen(&rig.hub, rig.folder, local);
    let mut client = Client::greeted(&rig.socket).await;
    let forward = client.forward(port).await;

    let stream = StreamId::generate();
    let mut out = Client::frame(&ClientMessage::TunnelOpen { stream, forward });
    frame::encode_tunnel_data(&mut out, stream, b"GET /");
    out.extend(Client::frame(&ClientMessage::TunnelClose {
        stream,
        how: TunnelEnd::Write,
    }));
    client.write(&out).await;

    let (mut accepted, _) = within(listener.accept()).await.unwrap();
    let mut got = Vec::new();
    within(accepted.read_to_end(&mut got)).await.unwrap();
    assert_eq!(got, b"GET /");
    assert_eq!(
        client.message().await,
        ServerMessage::TunnelOpened { stream }
    );
    accepted.write_all(b"ok").await.unwrap();
    accepted.shutdown().await.unwrap();
    assert_eq!(
        client.next().await,
        Heard::Tunnel {
            stream,
            data: b"ok".to_vec(),
        }
    );
    assert_eq!(
        client.message().await,
        ServerMessage::TunnelClose {
            stream,
            how: TunnelEnd::Write,
        }
    );
}

#[tokio::test]
async fn a_guest_gets_tokens_only_for_sessions_the_manager_places_there() {
    let rig = Rig::new();
    let ours = SessionId::generate();
    let theirs = SessionId::generate();
    let accounts = rig.own(&[
        (ours, rig.container, Vendor::Codex),
        (theirs, rig.folder, Vendor::Codex),
    ]);
    let mut peer = rig.link().await;

    peer.send(&GuestToHost::TokenRequest {
        req: 1,
        session: ours,
        want: TokenWant::Codex,
    })
    .await;
    assert_eq!(
        told(&mut peer).await,
        HostToGuest::TokenReply {
            req: 1,
            reply: token(),
        }
    );
    assert_eq!(rig.brokers.asked(), [format!("codex {}", accounts[0])]);

    peer.send(&GuestToHost::TokenRequest {
        req: 2,
        session: SessionId::generate(),
        want: TokenWant::Codex,
    })
    .await;
    assert_eq!(
        told(&mut peer).await,
        HostToGuest::TokenReply {
            req: 2,
            reply: TokenReply::Refused {
                reason: "that session isn't running".into(),
            },
        }
    );

    // Another workspace's session gets no answer, and the link drops.
    peer.send(&GuestToHost::TokenRequest {
        req: 3,
        session: theirs,
        want: TokenWant::Codex,
    })
    .await;
    peer.closed().await;
    assert_eq!(rig.brokers.asked().len(), 1);
}

#[tokio::test]
async fn an_upload_to_a_workspace_relays_through_its_guest() {
    let rig = Rig::new();
    let mut peer = rig.link().await;
    let source = TempDir::new();
    let path = source.path().join("data.txt");
    std::fs::write(&path, b"hello").unwrap();
    let walk = maxhq_transfer::walk(&path).unwrap();
    let mut client = Client::greeted(&rig.socket).await;

    let key = IdempotencyKey::generate();
    let transfer = TransferId::generate();
    client
        .send(&ClientMessage::UploadBegin {
            key,
            transfer,
            target: UploadTarget::Workspace {
                workspace: rig.container,
            },
            source: UploadSource::Stream {
                name: walk.name.clone(),
                entries: walk.entries.clone(),
            },
        })
        .await;
    assert_eq!(
        told(&mut peer).await,
        HostToGuest::UploadBegin {
            transfer,
            agent: None,
            entries: walk.entries.clone(),
        }
    );
    peer.send(&GuestToHost::TransferCredit {
        transfer,
        committed: 0,
        window_end: 5,
    })
    .await;
    assert_eq!(
        client.message().await,
        ServerMessage::TransferCredit {
            transfer,
            committed: 0,
            window_end: 5,
        }
    );

    let mut chunk = Vec::new();
    frame::encode_transfer_chunks(&mut chunk, transfer, 0, b"hello");
    client.write(&chunk).await;
    let Read::Chunk {
        transfer: to,
        offset,
        data,
    } = heard(&mut peer).await
    else {
        panic!("expected the upload's bytes");
    };
    assert_eq!((to, offset, data.as_slice()), (transfer, 0, &b"hello"[..]));

    peer.send(&GuestToHost::UploadStored {
        transfer,
        paths: vec![format!("{ATTACHMENTS_DIR}/staged/{transfer}/data.txt")],
        thumbnail: None,
        text: None,
    })
    .await;
    let outcome = loop {
        if let ServerMessage::ActionResult {
            key: answered,
            outcome,
        } = client.message().await
            && answered == key
        {
            break outcome;
        }
    };
    let ActionOutcome::Staged { attachment } = outcome else {
        panic!("expected the upload staged, got {outcome:?}");
    };
    assert_eq!(attachment.name, "data.txt");
}

#[tokio::test(flavor = "multi_thread")]
async fn quiesce_from_maxds_own_user_is_refused_on_the_socket() {
    let rig = Rig::new();
    let outcome = within(async {
        let mut stream = maxhq_os::ipc::LocalStream::connect(&rig.socket).await?;
        crate::wind_down(&mut stream, Duration::from_secs(60)).await
    })
    .await
    .unwrap();
    // It proves nothing, so only root, as the shutdown unit runs it, may,
    // and only to a maxd that doesn't run as root. This one runs as the
    // same user.
    assert!(
        matches!(&outcome, ActionOutcome::Refused { reason } if reason.contains("hasn't shown")),
        "{outcome:?}"
    );
}
