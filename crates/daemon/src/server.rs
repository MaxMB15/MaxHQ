// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Serves the protocol on the Unix socket.
//!
//! A client says hello, asks for the state with a sync and then gets every
//! change as it happens. One `maxd` holds the socket at a time: it takes a
//! lock file next to the socket first, so a second one exits with a clear
//! message, and a socket file left by a crash is removed.

use std::fs::{self, DirBuilder, File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use maxhq_core::prove::Challenge;
use maxhq_protocol::agents::ClientRole;
use maxhq_protocol::cursor_rules::CursorRuleCall;
use maxhq_protocol::frame::{self, DecodeError, EncodeError, Frame, FrameDecoder};
use maxhq_protocol::ids::{
    ClientId, DeviceId, IdempotencyKey, PairId, PhoneId, StreamId, TerminalId, TransferId,
};
use maxhq_protocol::local_models::ModelAction;
use maxhq_protocol::message::{
    ActionOutcome, AttachmentSource, Capabilities, ClientHello, ErrorCode, ServerHello,
    UploadSource,
};
use maxhq_protocol::model::PermissionMode;
use maxhq_protocol::state::{Delta, Seq};
use maxhq_protocol::terminal::TerminalKind;
use maxhq_protocol::{ClientMessage, ProtocolRange, ServerMessage};
use maxhq_wire::{Queue, Sender};
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::net::unix::OwnedReadHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::app_keys::AppKeys;
use crate::db::Db;
use crate::hub::{Command, Hub, Start, Subscription};
use crate::phones::PHONE_CANT;
use crate::session::{Client, Sessions};
use crate::terminals::access::Caller;
use crate::transfer::{Route, Transfers};
use crate::tunnel::{self, ClientTunnels, Gone, Out, Tunnels, Writer};
use crate::{acp_agents, codesign, secrets, workspace};

mod admission;
mod proof;
mod terminals;

/// How much a connection reads from its socket at once.
const READ_CHUNK: usize = 64 * 1024;

/// The refusal for an update no release signed, from a phone.
const PHONE_UNSIGNED: &str = "A phone can't send a maxd no release signed. Send it from the \
                              Mac's Update everything.";

/// The refusal for a folder request from a connection that isn't the app
/// or a proven phone.
const NOT_FOR_FOLDERS: &str = "Only the MaxHQ app and phones that proved their keys may list \
                               or make folders on this device.";

/// The refusal for a notification log request from a connection that
/// isn't the app or a proven phone.
const NOT_FOR_NOTICES: &str = "Only the MaxHQ app and phones that proved their keys may read \
                               this device's notifications.";

/// The refusal for a Cursor rules request from a connection that isn't
/// the app or a proven phone.
const NOT_FOR_CURSOR_RULES: &str = "Only the MaxHQ app and phones that proved their keys may \
                                    read or change Cursor's rules on this device.";

/// The refusal for spending a banked reset from anything but the app.
const NOT_FOR_RESETS: &str = "Only the MaxHQ desktop app may use a Codex reset, after you \
                              confirm it there.";

/// The answer to a request whose part of `maxd` isn't built yet.
pub fn not_yet() -> ActionOutcome {
    ActionOutcome::Refused {
        reason: "not yet".to_owned(),
    }
}

/// Why `maxd` couldn't take the socket.
#[derive(Debug, Error)]
pub enum BindError {
    /// Another `maxd` holds it.
    #[error("{}", already_running(path, *pid))]
    AlreadyRunning {
        /// The socket.
        path: PathBuf,
        /// The other `maxd`'s process ID, when its lock file names it.
        pid: Option<u32>,
    },
    /// Something other than a socket is in the way.
    #[error("{} exists and isn't a socket, so maxd won't replace it", path.display())]
    NotASocket {
        /// The socket path.
        path: PathBuf,
    },
    /// The socket, its folder or its lock file couldn't be made.
    #[error("couldn't set up {}: {source}", path.display())]
    Io {
        /// The file that failed.
        path: PathBuf,
        /// What went wrong.
        source: io::Error,
    },
}

/// The message for a socket another `maxd` holds.
fn already_running(path: &Path, pid: Option<u32>) -> String {
    let pid = pid.map_or(String::new(), |pid| format!(" as process {pid}"));
    format!(
        "another maxd is already running{pid} and holds {}. Stop it first, or set MAXD_SOCKET to run a second one somewhere else",
        path.display()
    )
}

/// The socket `maxd` serves on. Dropping it removes the socket file.
pub struct Listener {
    listener: UnixListener,
    path: PathBuf,
    /// Held for the life of the listener. The operating system releases it
    /// when the process ends, however it ends.
    _lock: File,
}

/// Takes the socket at `path`, creating its folder for this user alone.
/// Must run inside a Tokio runtime.
pub fn bind(path: &Path) -> Result<Listener, BindError> {
    let io_at = |path: &Path| {
        let path = path.to_path_buf();
        move |source| BindError::Io { path, source }
    };
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(io_at(dir))?;
    }

    let lock_path = lock_path(path);
    let mut lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&lock_path)
        .map_err(io_at(&lock_path))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            return Err(BindError::AlreadyRunning {
                path: path.into(),
                pid: read_pid(&mut lock),
            });
        }
        Err(TryLockError::Error(source)) => {
            return Err(BindError::Io {
                path: lock_path,
                source,
            });
        }
    }
    write_pid(&mut lock).map_err(io_at(&lock_path))?;

    // Holding the lock means no other maxd started since, so a socket file
    // here was left by one that crashed. One still answering on it must be
    // a maxd from before the lock file, and it keeps the socket.
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.file_type().is_socket() => {
            return Err(BindError::NotASocket { path: path.into() });
        }
        Ok(_) => {
            if std::os::unix::net::UnixStream::connect(path).is_ok() {
                return Err(BindError::AlreadyRunning {
                    path: path.into(),
                    pid: None,
                });
            }
            fs::remove_file(path).map_err(io_at(path))?;
            info!(socket = %path.display(), "removed a stale socket left by an earlier maxd");
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(BindError::Io {
                path: path.into(),
                source,
            });
        }
    }

    let std = std::os::unix::net::UnixListener::bind(path).map_err(io_at(path))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(io_at(path))?;
    std.set_nonblocking(true).map_err(io_at(path))?;
    let listener = UnixListener::from_std(std).map_err(io_at(path))?;
    Ok(Listener {
        listener,
        path: path.into(),
        _lock: lock,
    })
}

/// The lock file for the socket at `path`: the same name with `.lock` on
/// the end.
pub fn lock_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".lock");
    name.into()
}

/// The process ID the holder of the lock wrote into it.
fn read_pid(lock: &mut File) -> Option<u32> {
    let mut text = String::new();
    lock.read_to_string(&mut text).ok()?;
    text.trim().parse().ok()
}

/// Writes this process's ID into the lock file, for the next `maxd` to
/// name.
fn write_pid(lock: &mut File) -> io::Result<()> {
    lock.set_len(0)?;
    write!(lock, "{}", std::process::id())?;
    lock.flush()
}

impl Listener {
    /// The socket path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Serves every client that connects, each in a task of its own, until
    /// the future is dropped.
    pub async fn serve(
        &self,
        hub: Hub,
        db: Db,
        sessions: Sessions,
        transfers: Transfers,
        tunnels: Tunnels,
        keys: AppKeys,
    ) {
        loop {
            match self.listener.accept().await {
                Ok((stream, _)) => {
                    let (hub, db, sessions) = (hub.clone(), db.clone(), sessions.clone());
                    let (transfers, tunnels) = (transfers.clone(), tunnels.clone());
                    let keys = keys.clone();
                    tokio::spawn(serve_client(
                        stream, hub, db, sessions, transfers, tunnels, keys,
                    ));
                }
                Err(err) => {
                    // Usually out of file descriptors. Waiting a moment
                    // lets connections close before the next try.
                    warn!(%err, "couldn't accept a connection");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        // The lock is still held here, so the file is this maxd's to remove.
        let _ = fs::remove_file(&self.path);
    }
}

/// Serves one client until it disconnects or breaks the protocol.
pub async fn serve_client(
    stream: UnixStream,
    hub: Hub,
    db: Db,
    sessions: Sessions,
    transfers: Transfers,
    tunnels: Tunnels,
    keys: AppKeys,
) {
    match Connection::new(stream, hub, db, sessions, transfers, tunnels, keys)
        .run()
        .await
    {
        Ok(()) => debug!("client disconnected"),
        Err(err) => debug!(%err, "closed a client connection"),
    }
}

/// Why a connection closed early.
#[derive(Debug, Error)]
enum ConnError {
    /// The socket failed.
    #[error("socket error: {0}")]
    Io(#[from] io::Error),
    /// The client sent bytes that aren't frames.
    #[error("bad frame: {0}")]
    Decode(#[from] DecodeError),
    /// A message was too big to send.
    #[error("couldn't send a message: {0}")]
    Encode(#[from] EncodeError),
    /// The client speaks no protocol version this build does.
    #[error("no protocol version in common")]
    Unsupported,
    /// The hub went away, which only happens as `maxd` stops.
    #[error("maxd is stopping")]
    Stopping,
    /// The writer stopped, because the socket failed.
    #[error("{0}")]
    Closed(#[from] maxhq_wire::Closed),
    /// The first frame named a phone or a pairing this device doesn't
    /// have.
    #[error("{0}")]
    TurnedAway(&'static str),
}

/// A frame read from the client, owned so it can be handled while more
/// bytes arrive. Frames are handled in the order they came, so a stream's
/// bytes never pass the message that opened it.
enum Incoming {
    /// A message that parsed.
    Message(Box<ClientMessage>),
    /// A message frame that didn't parse, with the reason.
    Unparsed(String),
    /// Upload bytes.
    Chunk {
        /// The transfer.
        transfer: TransferId,
        /// Where the bytes go in the file.
        offset: u64,
        /// The bytes.
        data: Vec<u8>,
    },
    /// Bytes on a tunnel stream.
    Tunnel {
        /// The stream.
        stream: StreamId,
        /// The bytes.
        data: Vec<u8>,
    },
    /// What the client typed into a terminal.
    TerminalInput {
        /// The terminal.
        terminal: TerminalId,
        /// The bytes, which nothing logs.
        data: Vec<u8>,
    },
}

/// One client's connection.
struct Connection {
    reader: OwnedReadHalf,
    /// The writer's queues. Control messages go ahead of state, and state
    /// ahead of file data.
    wire: Sender,
    /// How this connection's transfers answer.
    route: Route,
    transfers: Transfers,
    decoder: FrameDecoder,
    hub: Hub,
    db: Db,
    sessions: Sessions,
    /// Whether the client has said hello.
    greeted: bool,
    /// Whether the client is `maxd mcp`, which may only sign in and call
    /// the agent tools.
    mcp: bool,
    /// Whether nothing has come yet, so a frame naming a peer may.
    fresh: bool,
    /// The device whose link this is, from `maxd proxy --peer`'s first
    /// frame. The connection then goes to the peer links, which serve it
    /// only once it proves it holds that device's key.
    peer: Option<DeviceId>,
    /// The phone a first frame named, from `maxd proxy --phone` or the
    /// network access listener. Its push requests are for that phone.
    phone: Option<PhoneId>,
    /// Each phone removed, so a phone's connection closes when it goes.
    removed: Option<broadcast::Receiver<PhoneId>>,
    /// Whether a one-time pairing key opened this connection, from `maxd
    /// proxy --pair`'s first frame. It may only pair, once.
    pairs: bool,
    /// The pairing its Pair frame named, and that pairing's one-time
    /// public key, which the connection proves it holds to take it.
    pair: Option<(PairId, [u8; 32])>,
    /// The pairing it took, until it pairs.
    pairing: Option<crate::phones::Pairing>,
    /// Deltas after the last sync, once the client has asked for one.
    deltas: Option<broadcast::Receiver<Delta>>,
    /// The newest version this client has been sent.
    sent: Option<Seq>,
    /// The line to the session manager, once the client has said hello.
    client: Option<Client>,
    tunnels: Tunnels,
    /// This connection's forwards and streams, once the client has said
    /// hello.
    link: Option<ClientTunnels>,
    /// Where requests that run as tasks of their own hand back their
    /// outcomes, and where `exchange` takes them from.
    answered: mpsc::UnboundedSender<(IdempotencyKey, ActionOutcome)>,
    answers: mpsc::UnboundedReceiver<(IdempotencyKey, ActionOutcome)>,
    /// The workflow runs this client has open, once it opens one.
    workflows: Option<crate::workflow::Viewer>,
    /// The app keys this device trusts.
    keys: AppKeys,
    /// What the client signs to prove it holds a key, sent in the hello.
    /// None only when the system gave no random bytes, and then nothing
    /// proves.
    challenge: Option<Challenge>,
    /// The client's ID from its hello, which a proof signs.
    client_id: Option<ClientId>,
    /// The app key this connection proved, once it has. Until then it may
    /// do only what [`proof::open`] lets through.
    app: Option<String>,
    /// Whether the connection proved it holds the key its Phone or Pair
    /// frame named. Until then it may do only what [`proof::open`] lets
    /// through.
    frame_proven: bool,
    /// The last key this connection showed it holds, trusted or not, which
    /// `EnrollAppKey` asks the device to trust.
    held: Option<String>,
    /// The audit token of the program on the other end as it connected,
    /// before the challenge went out.
    caller: Option<codesign::Token>,
    /// Each app key removed, once this connection proved one, so it closes
    /// when its own goes.
    revoked: Option<broadcast::Receiver<String>>,
    /// Its admission card, and whether you let it in.
    seat: crate::admission::Seat,
    /// Whether the program on the other end runs as root, and this `maxd`
    /// doesn't. Root may wind the agents down unproven, as
    /// [`proof::open_to_root`] says.
    root: bool,
}

impl Connection {
    /// A connection that hasn't said hello yet.
    fn new(
        stream: UnixStream,
        hub: Hub,
        db: Db,
        sessions: Sessions,
        transfers: Transfers,
        tunnels: Tunnels,
        keys: AppKeys,
    ) -> Self {
        // Taken before the hello and its challenge go out, so the program
        // that signs is the one that connected.
        let caller = codesign::token(&stream);
        let seat = admission::seat_for(&stream, &keys, caller);
        // A `maxd` that runs as root itself can't tell root from the
        // phones' and guests' connections it bridges, so none count.
        let root = !rustix::process::geteuid().is_root()
            && stream.peer_cred().is_ok_and(|cred| cred.uid() == 0);
        let challenge = Challenge::generate();
        if challenge.is_none() {
            warn!("no random bytes for a challenge, so this connection can't prove a key");
        }
        let (reader, writer) = stream.into_split();
        let (wire, _writer) = maxhq_wire::spawn(writer);
        let (answered, answers) = mpsc::unbounded_channel();
        Self {
            reader,
            route: Route::new(wire.clone()),
            wire,
            transfers,
            decoder: FrameDecoder::new(),
            hub,
            db,
            sessions,
            greeted: false,
            mcp: false,
            fresh: true,
            phone: None,
            removed: None,
            pairs: false,
            pair: None,
            pairing: None,
            peer: None,
            deltas: None,
            sent: None,
            client: None,
            tunnels,
            link: None,
            answered,
            answers,
            workflows: None,
            keys,
            challenge,
            client_id: None,
            app: None,
            frame_proven: false,
            held: None,
            caller,
            revoked: None,
            seat,
            root,
        }
    }

    /// Reads messages and sends deltas until the client goes away, then
    /// sends what's queued and closes the socket.
    async fn run(mut self) -> Result<(), ConnError> {
        let result = self.exchange().await;
        if let (Some(terminals), Some(client)) = (self.sessions.terminals(), &self.client) {
            terminals.gone(client.conn());
        }
        if let (Ok(()), Some(device), Some(peers)) = (&result, self.peer, crate::peers::for_link())
        {
            let hello = self.server_hello().await;
            crate::peers::serve(peers, device, self.reader, self.decoder, self.wire, hello).await;
            return Ok(());
        }
        self.wire.close();
        result
    }

    async fn exchange(&mut self) -> Result<(), ConnError> {
        let mut buf = vec![0; READ_CHUNK];
        loop {
            tokio::select! {
                read = self.reader.read(&mut buf) => {
                    let n = read?;
                    if n == 0 {
                        return Ok(());
                    }
                    self.decoder.push(&buf[..n]);
                    loop {
                        let incoming = self.drain()?;
                        if incoming.is_empty() {
                            break;
                        }
                        for incoming in incoming {
                            self.handle(incoming).await?;
                        }
                        // What follows belongs to the peer links.
                        if self.peer.is_some() {
                            return Ok(());
                        }
                    }
                }
                delta = next_delta(&mut self.deltas) => self.forward(delta).await?,
                message = next_message(&mut self.client) => match message {
                    Some(message) => self.send(&message).await?,
                    None => return Err(ConnError::Stopping),
                },
                Some((key, outcome)) = self.answers.recv() => self.outcome(key, outcome).await?,
                Some(message) = next_workflow(&mut self.workflows) => self.send(&message).await?,
                gone = next_removed(&mut self.removed) => {
                    let phone = self.phone;
                    let here = |phone| crate::phones::installed().is_some_and(|p| p.knows(phone));
                    if phone.is_some_and(|phone| gone.map_or(!here(phone), |gone| gone == phone)) {
                        return Ok(());
                    }
                }
                // Missing some, it can't tell whether its own key went, so
                // it closes and the app proves again.
                gone = next_removed(&mut self.revoked) => {
                    if gone.is_none_or(|gone| self.app.as_ref() == Some(&gone)) {
                        return Ok(());
                    }
                }
                key = self.seat.approved() => self.approved(key),
            }
        }
    }

    /// Takes every whole frame out of the decoder.
    fn drain(&mut self) -> Result<Vec<Incoming>, DecodeError> {
        let mut incoming = Vec::new();
        loop {
            match self.decoder.next_frame() {
                Ok(None) => return Ok(incoming),
                Ok(Some(Frame::Message(json))) => {
                    let message = match serde_json::from_slice(json) {
                        Ok(message) => message,
                        Err(err) => {
                            incoming.push(Incoming::Unparsed(err.to_string()));
                            continue;
                        }
                    };
                    // Frames after one naming a peer may go elsewhere, so
                    // they wait in the decoder.
                    let peer = matches!(message, ClientMessage::Peer { .. });
                    incoming.push(Incoming::Message(Box::new(message)));
                    if peer {
                        return Ok(incoming);
                    }
                }
                Ok(Some(Frame::TerminalInput { terminal, bytes })) => {
                    incoming.push(Incoming::TerminalInput {
                        terminal,
                        data: bytes.to_vec(),
                    });
                }
                Ok(Some(Frame::TerminalOutput { .. })) => {
                    debug!("skipped terminal output a client sent");
                }
                Ok(Some(Frame::TransferChunk {
                    transfer,
                    offset,
                    data,
                })) => incoming.push(Incoming::Chunk {
                    transfer,
                    offset,
                    data: data.to_vec(),
                }),
                Ok(Some(Frame::TunnelData { stream, data })) => incoming.push(Incoming::Tunnel {
                    stream,
                    data: data.to_vec(),
                }),
                // The decoder has consumed these frames, so reading goes on.
                Err(err @ (DecodeError::UnknownKind(_) | DecodeError::ShortHeader(_))) => {
                    debug!(%err, "skipped a frame");
                }
                Err(err) => return Err(err),
            }
        }
    }

    /// Answers one message, or passes on one frame of bytes. Bytes before
    /// hello are dropped.
    async fn handle(&mut self, incoming: Incoming) -> Result<(), ConnError> {
        let message = match incoming {
            Incoming::Message(message) => *message,
            Incoming::Unparsed(reason) => {
                return self.error(ErrorCode::BadMessage, reason).await;
            }
            Incoming::Chunk {
                transfer,
                offset,
                data,
            } => {
                if self.greeted && self.past_the_gate() {
                    self.transfers.chunk(transfer, offset, data);
                }
                return Ok(());
            }
            Incoming::Tunnel { stream, data } => {
                match &self.link {
                    Some(link) => link.data(stream, &data),
                    None => debug!(%stream, "dropped tunnel bytes from before hello"),
                }
                return Ok(());
            }
            Incoming::TerminalInput { terminal, data } => {
                if !self.greeted {
                    debug!(%terminal, "dropped terminal input from before hello");
                    return Ok(());
                }
                return self.terminal_input(terminal, data).await;
            }
        };
        let fresh = std::mem::replace(&mut self.fresh, false);
        if !self.greeted {
            return match message {
                ClientMessage::Peer { device } if fresh => {
                    self.peer = Some(device);
                    Ok(())
                }
                ClientMessage::Hello(hello) if hello.role == ClientRole::Peer => {
                    self.error(
                        ErrorCode::NotAllowed,
                        "only maxd proxy --peer opens a link from another device",
                    )
                    .await
                }
                ClientMessage::Hello(hello) => self.hello(hello).await,
                ClientMessage::Phone { phone } if fresh => {
                    let refusal = match crate::phones::installed() {
                        Some(phones) if phones.knows(phone) => {
                            self.removed = Some(phones.removed());
                            self.phone = Some(phone);
                            return Ok(());
                        }
                        Some(_) => "That phone isn't paired with this device.",
                        None => crate::phones::NOT_YET,
                    };
                    self.error(ErrorCode::NotAllowed, refusal).await?;
                    Err(ConnError::TurnedAway(refusal))
                }
                ClientMessage::Pair { pair } if fresh => {
                    // The pairing waits until the connection proves its
                    // key, so a program that only knows the ID, as maxd's
                    // log shows it, can't use it up.
                    let waiting = match crate::phones::installed() {
                        Some(phones) => phones.waiting(pair),
                        None => Err(crate::phones::NOT_YET),
                    };
                    match waiting {
                        Ok(public) => {
                            self.pairs = true;
                            self.pair = Some((pair, public));
                            Ok(())
                        }
                        Err(refusal) => {
                            self.error(ErrorCode::NotAllowed, refusal).await?;
                            Err(ConnError::TurnedAway(refusal))
                        }
                    }
                }
                _ => self.error(ErrorCode::NoHello, "say hello first").await,
            };
        }
        if self.pairs
            && !matches!(
                message,
                ClientMessage::PairPhone { .. }
                    | ClientMessage::Ping { .. }
                    | ClientMessage::Prove { .. }
            )
        {
            return self
                .error(
                    ErrorCode::NotAllowed,
                    "this connection may only pair a phone",
                )
                .await;
        }
        // A phone has App rights, but a key, a permission, a hook URL or a
        // link to another device it handed out would outlive removing a
        // lost phone, so only the Mac lets keys in, mints them, grants
        // permissions, hands out hook URLs, says which devices this one
        // links to and says what a phone may reach or do. A phone's switch
        // never lifts this.
        if self.phone.is_some()
            && matches!(
                message,
                ClientMessage::BeginPhonePair { .. }
                    | ClientMessage::AddPhone { .. }
                    | ClientMessage::AuthorizePeer { .. }
                    | ClientMessage::SetPeerBook { .. }
                    | ClientMessage::MintKey { .. }
                    | ClientMessage::GrantPermission { .. }
                    | ClientMessage::SetPhoneBook { .. }
                    | ClientMessage::SetPhoneEverything { .. }
                    | ClientMessage::RotateHookToken { .. }
                    | ClientMessage::SetGuestSshKey { .. }
                    | ClientMessage::EnrollAppKey { .. }
                    | ClientMessage::RemoveAppKey { .. }
                    | ClientMessage::AnswerAdmission { .. }
            )
        {
            return self
                .refuse(
                    &message,
                    ErrorCode::NotAllowed,
                    "A phone can't let other keys in. Do that from the Mac.",
                )
                .await;
        }
        // A binary no release signed runs only because you chose it on the
        // Mac, which built it and named the risk first. A phone carries no
        // binaries and can't start Update everything, so one marked
        // unsigned from a phone is someone else's. Its switch never lifts
        // this. Agents' tools and links from other devices can't send an
        // update at all.
        if self.phone.is_some()
            && matches!(message, ClientMessage::InstallUpdate { unsigned: true, .. })
        {
            return self
                .refuse(&message, ErrorCode::NotAllowed, PHONE_UNSIGNED)
                .await;
        }
        // A phone is easier to lose than a Mac, and it never stores or
        // removes a credential, signs in, changes how this device is set
        // up or installs anything. So a stolen phone's key can't either.
        // Nor can it read a file by its path on the device, list or make
        // folders, start or move agents, set one to Auto or Full access,
        // start tasks, make, clone or remove workspaces, place files in
        // them, open a shell, reach a guest's shell or ports, signal a
        // process, copy files out, download models, build or reset a test
        // host, or change hosts, snapshots, moves, skills, the skill
        // library or its sources, or Cursor's saved rules. Nor, as
        // `terminals::access` says, run a `!` command, type into a
        // terminal or watch a shell. The phone asks the dispatcher,
        // answers and steers agents that already run. Unless the Mac
        // turned the phone's switch on, which lets it do all of these.
        if self.phone.is_some()
            && matches!(
                message,
                ClientMessage::StartAgent { .. }
                    | ClientMessage::MoveAgent { .. }
                    | ClientMessage::SetPermission {
                        mode: PermissionMode::Auto | PermissionMode::FullAccess,
                        ..
                    }
                    | ClientMessage::SaveTask { .. }
                    | ClientMessage::SetDeviceSettings { .. }
                    | ClientMessage::UpdateDeviceSettings { .. }
                    | ClientMessage::InstallAcpAgent { .. }
                    | ClientMessage::CreateWorkspace { .. }
                    | ClientMessage::DeleteWorkspace { .. }
                    | ClientMessage::CloneFolder { .. }
                    | ClientMessage::CloneRetry { .. }
                    | ClientMessage::CloneRemove { .. }
                    | ClientMessage::PlaceStaged { .. }
                    | ClientMessage::ListFolder { .. }
                    | ClientMessage::MakeFolder { .. }
                    | ClientMessage::GuestSshOpen { .. }
                    | ClientMessage::TerminalOpen {
                        kind: TerminalKind::Shell,
                        ..
                    }
                    | ClientMessage::OpenForward { .. }
                    | ClientMessage::TunnelOpen { .. }
                    | ClientMessage::SignalProcess { .. }
                    | ClientMessage::FetchFiles { .. }
                    | ClientMessage::ApplyHost { .. }
                    | ClientMessage::RestoreSnapshot { .. }
                    | ClientMessage::RunMoveStep { .. }
                    | ClientMessage::BuildTestHost { .. }
                    | ClientMessage::ResetTestHost { .. }
                    | ClientMessage::SkillsApply { .. }
                    | ClientMessage::SkillsSetProject { .. }
                    | ClientMessage::SkillsSetAlways { .. }
                    | ClientMessage::SkillsGlobalsApply { .. }
                    | ClientMessage::SkillsGlobalsUndo { .. }
                    | ClientMessage::SkillProposalAction { .. }
                    | ClientMessage::SkillSourceAdd { .. }
                    | ClientMessage::SkillSourceFetch { .. }
                    | ClientMessage::SkillSourcePush { .. }
                    | ClientMessage::SkillSourceRemove { .. }
                    | ClientMessage::SkillsTakeLibrary { .. }
                    | ClientMessage::StoreClaudeToken { .. }
                    | ClientMessage::StoreCursorKey { .. }
                    | ClientMessage::SetGithubApp { .. }
                    | ClientMessage::BeginGithubSignIn { .. }
                    | ClientMessage::SetAcpAgent { .. }
                    | ClientMessage::DeleteAcpAgent { .. }
                    | ClientMessage::ForgetCredential { .. }
                    | ClientMessage::BeginChatGptSignIn { .. }
                    | ClientMessage::FinishChatGptSignIn { .. }
                    | ClientMessage::BeginCliLogin { .. }
                    | ClientMessage::FinishCliLogin { .. }
                    | ClientMessage::SetHookSecret { .. }
                    | ClientMessage::SetHookAddress { .. }
                    | ClientMessage::SetApnsKey { .. }
                    | ClientMessage::InstallUpdate { .. }
                    | ClientMessage::InstallTool { .. }
                    | ClientMessage::UpdateTools { .. }
                    | ClientMessage::RemoveTool { .. }
                    | ClientMessage::UploadBegin {
                        source: UploadSource::DevicePath { .. },
                        ..
                    }
                    | ClientMessage::StageAttachment {
                        source: AttachmentSource::Path { .. },
                        ..
                    }
                    | ClientMessage::FetchBegin { anyway: true, .. }
                    | ClientMessage::CursorRules {
                        call: CursorRuleCall::Remove { .. },
                        ..
                    }
                    | ClientMessage::ModelAction {
                        action: ModelAction::AddServer { .. }
                            | ModelAction::ProbeServer { .. }
                            | ModelAction::RemoveServer { .. }
                            | ModelAction::Download { .. }
                            | ModelAction::ResumeDownload { .. }
                            | ModelAction::DeleteModel { .. },
                        ..
                    }
            )
            && !self.may_do_everything()
        {
            return self
                .refuse(&message, ErrorCode::NotAllowed, PHONE_CANT)
                .await;
        }
        if self.mcp
            && !matches!(
                message,
                ClientMessage::AuthKey { .. } | ClientMessage::CallTool { .. }
            )
        {
            return self
                .error(
                    ErrorCode::NotAllowed,
                    "this connection may only call agent tools",
                )
                .await;
        }
        let open = if self.root {
            proof::open_to_root(&message)
        } else {
            proof::open(&message)
        };
        if !self.past_the_gate() && !open {
            let refusal = proof::unproven(&self.hub.device().name);
            self.refuse(&message, ErrorCode::NotAllowed, &refusal)
                .await?;
            self.ask_admission();
            return Ok(());
        }
        match message {
            ClientMessage::Hello(_) => {
                self.error(ErrorCode::BadMessage, "this connection already said hello")
                    .await
            }
            ClientMessage::Peer { .. } => {
                self.error(
                    ErrorCode::BadMessage,
                    "only a connection's first frame names a peer",
                )
                .await
            }
            ClientMessage::PeerRequest { .. } | ClientMessage::PeerAnswer { .. } => {
                self.error(
                    ErrorCode::NotAllowed,
                    "only a link from another device sends that",
                )
                .await
            }
            ClientMessage::PeerKey { key } => {
                self.answer_when(key, tokio::task::spawn_blocking(crate::peers::peer_key));
                Ok(())
            }
            ClientMessage::AuthorizePeer {
                key,
                device,
                public_key,
            } => {
                let authorize = move || crate::peers::authorize(device, &public_key);
                self.answer_when(key, tokio::task::spawn_blocking(authorize));
                Ok(())
            }
            ClientMessage::RevokePeer { key, device } => {
                let revoke = move || crate::peers::revoke(device);
                self.answer_when(key, tokio::task::spawn_blocking(revoke));
                Ok(())
            }
            ClientMessage::SetPeerBook { key, book } => {
                let set = move || crate::peers::set_book(book);
                self.answer_when(key, tokio::task::spawn_blocking(set));
                Ok(())
            }
            ClientMessage::BeginGithubSignIn { key, client_id } => {
                self.answer_when(key, tokio::spawn(secrets::begin_github_sign_in(client_id)));
                Ok(())
            }
            ClientMessage::CancelGithubSignIn { key, sign_in } => {
                self.outcome(key, secrets::cancel_github_sign_in(sign_in))
                    .await
            }
            ClientMessage::CheckGithubSignIn { key, sign_in } => {
                self.outcome(key, secrets::check_github_sign_in(sign_in))
                    .await
            }
            // These read GitHub, or merge and approve as you, so they run
            // as tasks of their own.
            message @ (ClientMessage::ListPulls { key, .. }
            | ClientMessage::ListBranches { key, .. }
            | ClientMessage::GetPull { key, .. }
            | ClientMessage::PullFiles { key, .. }
            | ClientMessage::PullComments { key, .. }
            | ClientMessage::PullChecks { key, .. }
            | ClientMessage::PullAction { key, .. }) => {
                let answer = crate::pulls::requests::handle(message, self.hub.clone());
                self.answer_when(key, tokio::spawn(answer));
                Ok(())
            }
            message @ (ClientMessage::ReadNote { key, .. }
            | ClientMessage::SaveNote { key, .. }
            | ClientMessage::NoteHistory { key, .. }
            | ClientMessage::NoteHashes { key, .. }
            | ClientMessage::NoteVersions { key, .. }
            | ClientMessage::PutNoteVersions { key, .. }) => {
                self.answer_when(key, tokio::spawn(crate::notes::answer(message)));
                Ok(())
            }
            // Detection runs commands, so it answers when it's done.
            ClientMessage::CheckTestHosts { key } => {
                self.answer_when(key, tokio::spawn(crate::leases::check()));
                Ok(())
            }
            ClientMessage::BuildTestHost { key, host } => {
                self.outcome(key, crate::leases::build(host)).await
            }
            ClientMessage::ResetTestHost { key, host } => {
                self.answer_when(key, tokio::spawn(crate::leases::reset(host)));
                Ok(())
            }
            ClientMessage::EndLease { key, lease } => {
                self.answer_when(key, tokio::spawn(crate::leases::end(lease)));
                Ok(())
            }
            ClientMessage::ReadLeaseScreen { key, lease } => {
                self.answer_when(key, tokio::spawn(crate::leases::screen(lease)));
                Ok(())
            }
            ClientMessage::CheckHost { key } => {
                let check = crate::host::check_request(self.hub.clone());
                self.answer_when(key, tokio::spawn(check));
                Ok(())
            }
            ClientMessage::ApplyHost { key, items } => {
                let apply = crate::host::apply_request(self.hub.clone(), items);
                self.answer_when(key, tokio::spawn(apply));
                Ok(())
            }
            ClientMessage::ListSnapshots { key, workspace: id } => {
                self.answer_when(key, tokio::spawn(workspace::list_snapshots(id)));
                Ok(())
            }
            ClientMessage::RestoreSnapshot {
                key,
                workspace: id,
                snapshot,
                as_new,
                discard_unsaved,
            } => {
                let restore = workspace::restore_snapshot(id, snapshot, as_new, discard_unsaved);
                self.answer_when(key, tokio::spawn(restore));
                Ok(())
            }
            ClientMessage::PlanMove { key, instance } => {
                self.answer_when(key, tokio::spawn(crate::moves::plan(instance)));
                Ok(())
            }
            ClientMessage::RunMoveStep { key, moving, step } => {
                self.answer_when(key, tokio::spawn(crate::moves::run_step(moving, step)));
                Ok(())
            }
            ClientMessage::MoveBack { key, moving } => {
                self.answer_when(key, tokio::spawn(crate::moves::move_back(moving)));
                Ok(())
            }
            ClientMessage::RetireMove { key, moving } => {
                self.answer_when(key, tokio::spawn(crate::moves::retire(moving)));
                Ok(())
            }
            ClientMessage::InstallAcpAgent {
                key,
                agent,
                version,
                place,
                confirm_unchecked,
            } => {
                let install =
                    crate::acp_registry::install_request(agent, version, place, confirm_unchecked);
                self.answer_when(key, tokio::spawn(install));
                Ok(())
            }
            // An open sends the whole run first, even for a run already
            // open, so a client that reconnected opens afresh.
            ClientMessage::OpenWorkflow { run } => {
                if self.workflows.is_none() {
                    self.workflows = crate::workflow::Viewer::new();
                }
                match &self.workflows {
                    Some(viewer) => {
                        viewer.open(run);
                        Ok(())
                    }
                    None => {
                        self.error(ErrorCode::NotAllowed, crate::workflow::NOT_YET)
                            .await
                    }
                }
            }
            ClientMessage::CloseWorkflow { run } => {
                if let Some(viewer) = &self.workflows {
                    viewer.close(run);
                }
                Ok(())
            }
            ClientMessage::NodeDetail { key, run, node } => {
                self.answer_when(key, crate::workflow::detail(run, node));
                Ok(())
            }
            // A source's clone and read run git, so they answer when done.
            ClientMessage::SkillSourceAdd {
                key,
                name,
                kind,
                default_ref,
                trust,
            } => {
                let added = crate::skills::add(name, kind, default_ref, trust);
                self.answer_when(key, tokio::spawn(added));
                Ok(())
            }
            ClientMessage::SkillSourceFetch { key, source } => {
                self.answer_when(key, tokio::spawn(crate::skills::fetch(source)));
                Ok(())
            }
            ClientMessage::SkillSourceRemove { key, source } => {
                self.answer_when(key, tokio::spawn(crate::skills::remove(source)));
                Ok(())
            }
            ClientMessage::SkillSourcePush { key, source } => {
                self.answer_when(key, tokio::spawn(crate::skills::push(source)));
                Ok(())
            }
            // Taking the library clones each git source.
            ClientMessage::SkillsTakeLibrary { key, from, sources } => {
                let taken = crate::skills::take(from, sources);
                self.answer_when(key, tokio::spawn(taken));
                Ok(())
            }
            ClientMessage::SkillsSetProject { key, project, lock } => {
                let set = tokio::task::spawn_blocking(move || {
                    crate::skills::deliver::set_project(project, lock)
                });
                self.answer_when(key, set);
                Ok(())
            }
            // An apply copies files and asks guests, so it answers when
            // every checkout has the lock.
            ClientMessage::SkillsApply { key, project } => {
                self.answer_when(key, tokio::spawn(crate::skills::deliver::apply(project)));
                Ok(())
            }
            ClientMessage::SkillsTreesNeeded { key, lock } => {
                self.outcome(key, crate::skills::deliver::trees_needed(&lock))
                    .await
            }
            ClientMessage::SkillsSetAlways { key, skills } => {
                let set = move || crate::skills::set_always(skills);
                self.answer_when(key, tokio::task::spawn_blocking(set));
                Ok(())
            }
            // Approving commits and maybe pushes, so it answers when done.
            ClientMessage::SkillProposalAction {
                key,
                proposal,
                action,
                push,
                fork,
                elsewhere,
                seen,
            } => {
                let answer = crate::skills::deliver::Answer {
                    proposal,
                    action,
                    push,
                    fork,
                    elsewhere,
                    seen,
                };
                let answered = crate::skills::deliver::answer(answer);
                self.answer_when(key, tokio::spawn(answered));
                Ok(())
            }
            ClientMessage::SkillProposalDiff {
                key,
                proposal,
                elsewhere,
            } => {
                let diff = crate::skills::deliver::diff(proposal, elsewhere);
                self.answer_when(key, tokio::spawn(diff));
                Ok(())
            }
            // These read or move skill folders, so they run off the loop.
            // A scan also looks for the project's proposals again.
            ClientMessage::SkillsScan { key, place } => {
                tokio::spawn(crate::skills::deliver::scanned(place.clone()));
                let hub = self.hub.clone();
                let scan = move || crate::skills::scan::request(&hub, &place);
                self.answer_when(key, tokio::task::spawn_blocking(scan));
                Ok(())
            }
            ClientMessage::SkillsGlobalsPlan { key } => {
                let plan = crate::skills::globals::plan_request;
                self.answer_when(key, tokio::task::spawn_blocking(plan));
                Ok(())
            }
            ClientMessage::SkillsGlobalsApply {
                key,
                plan_hash,
                picks,
            } => {
                let apply = move || crate::skills::globals::apply_request(plan_hash, picks);
                self.answer_when(key, tokio::task::spawn_blocking(apply));
                Ok(())
            }
            ClientMessage::SkillsGlobalsUndo { key } => {
                let undo = crate::skills::globals::undo_request;
                self.answer_when(key, tokio::task::spawn_blocking(undo));
                Ok(())
            }
            ClientMessage::BeginPhonePair { key, book, mac } => {
                let begin = move || crate::phones::with(|phones| phones.begin(book, mac));
                self.answer_when(key, tokio::task::spawn_blocking(begin));
                Ok(())
            }
            ClientMessage::PairPhone {
                key,
                name,
                ssh_public,
                push_public,
            } => match self.pairing.take() {
                Some(pairing) => {
                    let pair = move || {
                        crate::phones::with(|phones| {
                            phones.pair(pairing, &name, &ssh_public, &push_public)
                        })
                    };
                    self.answer_when(key, tokio::task::spawn_blocking(pair));
                    Ok(())
                }
                None => {
                    let refused = ActionOutcome::Refused {
                        reason: "Only a phone's one-time key pairs it, once.".to_owned(),
                    };
                    self.outcome(key, refused).await
                }
            },
            ClientMessage::AddPhone {
                key,
                phone,
                name,
                ssh_public,
                push_public,
            } => {
                let add = move || {
                    crate::phones::with(|phones| {
                        phones.add(phone, &name, &ssh_public, &push_public)
                    })
                };
                self.answer_when(key, tokio::task::spawn_blocking(add));
                Ok(())
            }
            ClientMessage::RemovePhone { key, phone } => {
                let remove = move || crate::phones::with(|phones| phones.remove(phone));
                self.answer_when(key, tokio::task::spawn_blocking(remove));
                Ok(())
            }
            ClientMessage::RevokePhoneSession { key, session } => {
                let revoke = move || crate::netaccess::with(|access| access.revoke(session));
                self.answer_when(key, tokio::task::spawn_blocking(revoke));
                Ok(())
            }
            ClientMessage::SetPhoneBook {
                key,
                phone,
                book,
                mac,
            } => {
                let set = move || crate::phones::with(|phones| phones.set_book(phone, book, mac));
                self.answer_when(key, tokio::task::spawn_blocking(set));
                Ok(())
            }
            ClientMessage::SetPhoneEverything {
                key,
                phone,
                everything,
            } => {
                let set =
                    move || crate::phones::with(|phones| phones.set_everything(phone, everything));
                self.answer_when(key, tokio::task::spawn_blocking(set));
                Ok(())
            }
            ClientMessage::Phone { .. } | ClientMessage::Pair { .. } => {
                self.error(
                    ErrorCode::BadMessage,
                    "only a connection's first frame names a phone or a pairing",
                )
                .await
            }
            // Only a phone's own connection sends these.
            ClientMessage::SetPushToken { key, push } => match self.phone {
                Some(phone) => {
                    let set = move || crate::phones::with(|phones| phones.set_token(phone, push));
                    self.answer_when(key, tokio::task::spawn_blocking(set));
                    Ok(())
                }
                None => {
                    let refused = ActionOutcome::Refused {
                        reason: crate::push::ONLY_PHONES.to_owned(),
                    };
                    self.outcome(key, refused).await
                }
            },
            ClientMessage::Presence { agent } => match self.phone {
                Some(phone) => {
                    crate::push::presence(phone, agent);
                    Ok(())
                }
                None => {
                    self.error(ErrorCode::NotAllowed, crate::push::ONLY_PHONES)
                        .await
                }
            },
            ClientMessage::SetApnsKey {
                key,
                key_id,
                team_id,
                private_key,
            } => {
                let set = move || secrets::set_apns_key(&key_id, &team_id, private_key);
                self.answer_when(key, tokio::task::spawn_blocking(set));
                Ok(())
            }
            ClientMessage::Sync { since } => self.sync(since).await,
            ClientMessage::MarkSeen { agent } => {
                let at = self.hub.mark_seen(agent);
                let db = self.db.clone();
                let stored = tokio::task::spawn_blocking(move || db.mark_seen(agent, at)).await;
                match stored {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => warn!(%err, %agent, "couldn't store when the agent was seen"),
                    Err(err) => warn!(%err, %agent, "couldn't store when the agent was seen"),
                }
                Ok(())
            }
            ClientMessage::RefreshUsage { account } => {
                self.hub.send(Command::RefreshUsage(account));
                Ok(())
            }
            ClientMessage::Ping { nonce } => self.send(&ServerMessage::Pong { nonce }).await,
            // Terminals answer only the app and proven phones.
            message @ (ClientMessage::TerminalOpen { .. }
            | ClientMessage::TerminalAttach { .. }
            | ClientMessage::TerminalDetach { .. }
            | ClientMessage::TerminalResize { .. }
            | ClientMessage::TerminalInterrupt { .. }
            | ClientMessage::TerminalClose { .. }
            | ClientMessage::TerminalRename { .. }
            | ClientMessage::TerminalMove { .. }
            | ClientMessage::SessionRun { .. }
            | ClientMessage::SessionShare { .. }
            | ClientMessage::SessionCd { .. }
            | ClientMessage::SessionEnd { .. }) => self.terminal(message).await,
            // Folders answer the app and proven phones, as terminals do.
            ClientMessage::ListFolder {
                key,
                device,
                path,
                files,
                hidden,
                filter,
            } if self.terminal_caller().is_some() => {
                let me = self.hub.device().id;
                let list = crate::folders::list(me, device, path, files, hidden, filter);
                self.answer_when(key, tokio::spawn(list));
                Ok(())
            }
            ClientMessage::MakeFolder { key, device, path } if self.terminal_caller().is_some() => {
                let make = crate::folders::make(self.hub.device().id, device, key, path);
                self.answer_when(key, tokio::spawn(make));
                Ok(())
            }
            message @ (ClientMessage::ListFolder { .. } | ClientMessage::MakeFolder { .. }) => {
                self.refuse(&message, ErrorCode::NotAllowed, NOT_FOR_FOLDERS)
                    .await
            }
            // The notification log answers the app and proven phones too.
            ClientMessage::Notices { key, device, call } if self.terminal_caller().is_some() => {
                let me = self.hub.device().id;
                let call = crate::notices::answer(self.db.clone(), me, device, call);
                self.answer_when(key, tokio::spawn(call));
                Ok(())
            }
            message @ ClientMessage::Notices { .. } => {
                self.refuse(&message, ErrorCode::NotAllowed, NOT_FOR_NOTICES)
                    .await
            }
            // So do the rules Cursor saved.
            ClientMessage::CursorRules { key, device, call }
                if self.terminal_caller().is_some() =>
            {
                let me = self.hub.device().id;
                let call = crate::drive::cursor::rules::answer(me, device, call);
                self.answer_when(key, tokio::spawn(call));
                Ok(())
            }
            message @ ClientMessage::CursorRules { .. } => {
                self.refuse(&message, ErrorCode::NotAllowed, NOT_FOR_CURSOR_RULES)
                    .await
            }
            // A reset can't be undone, so only the app spends one, for a
            // click you confirmed. Not a phone, whatever its switch says,
            // and never agents' tools or a link from another device. A
            // reset that lands lets the agents its limit held carry on.
            ClientMessage::UseResetCredit { key, account }
                if matches!(self.terminal_caller(), Some(Caller::App)) =>
            {
                let sessions = self.sessions.clone();
                let spend = crate::usage::codex::reset::use_credit(
                    self.hub.clone(),
                    key,
                    account,
                    move |account| sessions.reset_used(account),
                );
                self.answer_when(key, tokio::spawn(spend));
                Ok(())
            }
            message @ ClientMessage::UseResetCredit { .. } => {
                self.refuse(&message, ErrorCode::NotAllowed, NOT_FOR_RESETS)
                    .await
            }
            // A phone without its switch on never gets here for the actions
            // only the Mac takes, as the gate above says.
            ClientMessage::ModelAction { key, action } => {
                self.answer_when(key, tokio::spawn(crate::local_models::act(action)));
                Ok(())
            }
            // Updates, tools, load and diagnostics belong to the fleet.
            message @ (ClientMessage::InstallUpdate { key, .. }
            | ClientMessage::UpdateTools { key, .. }
            | ClientMessage::InstallTool { key, .. }
            | ClientMessage::RemoveTool { key, .. }
            | ClientMessage::AcceptLicence { key, .. }
            | ClientMessage::ReadLoad { key }
            | ClientMessage::ReadDiagnostics { key, .. }) => {
                self.answer_when(key, tokio::spawn(crate::fleet::handle(message)));
                Ok(())
            }
            // Answers, actions, timelines and lookups belong to the session
            // manager. Its replies arrive through `run`.
            message @ (ClientMessage::Answer { .. }
            | ClientMessage::StartAgent { .. }
            | ClientMessage::SendInput { .. }
            | ClientMessage::CancelQueued { .. }
            | ClientMessage::Interrupt { .. }
            | ClientMessage::SetPermission { .. }
            | ClientMessage::SetModel { .. }
            | ClientMessage::SetEffort { .. }
            | ClientMessage::StopAgent { .. }
            | ClientMessage::DeleteAgent { .. }
            | ClientMessage::RenameAgent { .. }
            | ClientMessage::SetInbox { .. }
            | ClientMessage::SetProjectHidden { .. }
            | ClientMessage::RemoveCommandRule { .. }
            | ClientMessage::RemoveAllowedEdit { .. }
            | ClientMessage::ContinueHere { .. }
            | ClientMessage::TakeOver { .. }
            | ClientMessage::StageAttachment { .. }
            | ClientMessage::SetDeviceSettings { .. }
            | ClientMessage::UpdateDeviceSettings { .. }
            | ClientMessage::OpenTimeline { .. }
            | ClientMessage::LoadOlder { .. }
            | ClientMessage::CloseTimeline { .. }
            | ClientMessage::FindPaths { .. }
            | ClientMessage::ListProcesses { .. }
            | ClientMessage::SignalProcess { .. }
            | ClientMessage::FindGitItems { .. }
            | ClientMessage::ExcerptAgent { .. }
            | ClientMessage::FindInTimeline { .. }
            | ClientMessage::WindDown { .. }
            | ClientMessage::PullsDue { .. }
            | ClientMessage::LinkPull { .. }
            | ClientMessage::UnlinkPull { .. }
            | ClientMessage::MoveAgent { .. }) => {
                if let Some(client) = &self.client {
                    client.send(message);
                }
                Ok(())
            }
            // Agents that run agents, and the permissions and keys they
            // run under, belong to the session manager too.
            message @ (ClientMessage::AuthKey { .. }
            | ClientMessage::CallTool { .. }
            | ClientMessage::GrantPermission { .. }
            | ClientMessage::RevokePermission { .. }
            | ClientMessage::ListPermissions { .. }
            | ClientMessage::MintKey { .. }
            | ClientMessage::ListKeys { .. }) => {
                if let Some(client) = &self.client {
                    client.send(message);
                }
                Ok(())
            }
            // So do scheduled tasks, which run as callers of the agent
            // tools.
            message @ (ClientMessage::SaveTask { .. }
            | ClientMessage::SetTaskEnabled { .. }
            | ClientMessage::DeleteTask { .. }
            | ClientMessage::RunTaskNow { .. }
            | ClientMessage::ListRuns { .. }) => {
                if let Some(client) = &self.client {
                    client.send(message);
                }
                Ok(())
            }
            // And the dispatcher, whose plans run through its requests.
            message @ (ClientMessage::AskDispatcher { .. }
            | ClientMessage::ConfirmPlan { .. }
            | ClientMessage::DismissPlan { .. }
            | ClientMessage::UndoDispatcherAction { .. }
            | ClientMessage::ActOnChat { .. }) => {
                if let Some(client) = &self.client {
                    client.send(message);
                }
                Ok(())
            }
            ClientMessage::Prove {
                key,
                public_key,
                signature,
            } => {
                let outcome = self.prove(&public_key, &signature).await;
                self.outcome(key, outcome).await
            }
            ClientMessage::EnrollAppKey { key, label } => {
                let outcome = self.enroll(label).await;
                self.outcome(key, outcome).await
            }
            ClientMessage::RemoveAppKey { key, public_key } => {
                let outcome = self.remove_app_key(public_key).await;
                self.outcome(key, outcome).await
            }
            ClientMessage::AnswerAdmission {
                key,
                admission,
                allow,
            } => {
                let outcome = self.answer_admission(admission, allow).await;
                self.outcome(key, outcome).await
            }
            // These wait on Incus, a container or the disk, so they run as
            // tasks of their own and answer through `exchange` when done.
            ClientMessage::CreateWorkspace { key, spec } => {
                self.answer_when(key, tokio::spawn(workspace::create(spec)));
                Ok(())
            }
            ClientMessage::StartWorkspace { key, workspace: id } => {
                self.answer_when(key, tokio::spawn(workspace::start(id)));
                Ok(())
            }
            ClientMessage::StopWorkspace { key, workspace: id } => {
                self.answer_when(key, tokio::spawn(workspace::stop(id)));
                Ok(())
            }
            ClientMessage::DeleteWorkspace {
                key,
                workspace: id,
                discard_unpushed,
            } => {
                let delete = workspace::delete(id, discard_unpushed);
                self.answer_when(key, tokio::spawn(delete));
                Ok(())
            }
            ClientMessage::CloneFolder {
                key,
                project,
                device,
                parent,
                name,
            } => {
                let clone = crate::clone::start(project, device, parent, name);
                self.answer_when(key, tokio::spawn(clone));
                Ok(())
            }
            ClientMessage::CloneCancel { key, workspace: id } => {
                self.answer_when(key, tokio::spawn(crate::clone::cancel(id)));
                Ok(())
            }
            ClientMessage::CloneRetry { key, workspace: id } => {
                self.answer_when(key, tokio::spawn(crate::clone::retry(id)));
                Ok(())
            }
            ClientMessage::CloneRemove { key, workspace: id } => {
                self.answer_when(key, tokio::spawn(crate::clone::remove(id)));
                Ok(())
            }
            ClientMessage::ListIgnoredFiles { key, workspace: id } => {
                let list = self.transfers.list_ignored(id);
                self.answer_when(key, tokio::spawn(list));
                Ok(())
            }
            ClientMessage::PlaceStaged {
                key,
                workspace: id,
                attachment,
                overwrite,
            } => {
                let place = self.transfers.place_staged(id, attachment, overwrite);
                self.answer_when(key, tokio::spawn(place));
                Ok(())
            }
            ClientMessage::SetProjectSettings { key, settings } => {
                let set = workspace::set_project_settings(settings);
                self.answer_when(key, tokio::spawn(set));
                Ok(())
            }
            ClientMessage::StoreClaudeToken {
                key,
                origin,
                label,
                token,
            } => {
                let store = move || secrets::store_claude_token(origin, label, token);
                self.answer_when(key, tokio::task::spawn_blocking(store));
                Ok(())
            }
            ClientMessage::StoreCursorKey { key, label, token } => {
                let store = move || secrets::store_cursor_key(label, token);
                self.answer_when(key, tokio::task::spawn_blocking(store));
                Ok(())
            }
            ClientMessage::SetAcpAgent { key, agent, env } => {
                self.answer_when(key, tokio::spawn(acp_agents::set(agent, env)));
                Ok(())
            }
            ClientMessage::DeleteAcpAgent { key, id } => {
                let delete = move || acp_agents::delete(id);
                self.answer_when(key, tokio::task::spawn_blocking(delete));
                Ok(())
            }
            ClientMessage::BeginChatGptSignIn { key } => {
                self.answer_when(key, tokio::spawn(secrets::begin_chatgpt_sign_in()));
                Ok(())
            }
            ClientMessage::FinishChatGptSignIn {
                key,
                sign_in,
                code,
                state,
                client_id,
            } => {
                let finish = secrets::finish_chatgpt_sign_in(sign_in, code, state, client_id);
                self.answer_when(key, tokio::spawn(finish));
                Ok(())
            }
            ClientMessage::BeginCliLogin {
                key,
                account,
                device_code,
            } => {
                let begin = secrets::begin_cli_login(account, device_code);
                self.answer_when(key, tokio::spawn(begin));
                Ok(())
            }
            ClientMessage::FinishCliLogin { key, sign_in, code } => {
                let finish = secrets::finish_cli_login(sign_in, code);
                self.answer_when(key, tokio::spawn(finish));
                Ok(())
            }
            ClientMessage::CheckCliLogin { key, sign_in } => {
                self.outcome(key, secrets::check_cli_login(sign_in)).await
            }
            ClientMessage::CancelCliLogin { key, sign_in } => {
                self.outcome(key, secrets::cancel_cli_login(sign_in)).await
            }
            ClientMessage::SetGithubApp {
                key,
                app_id,
                private_key,
            } => {
                let set = move || secrets::set_github_app(app_id, private_key);
                self.answer_when(key, tokio::task::spawn_blocking(set));
                Ok(())
            }
            ClientMessage::CheckGithubApp { key } => {
                let check = tokio::task::spawn_blocking(secrets::check_github_app);
                self.answer_when(key, check);
                Ok(())
            }
            ClientMessage::ForgetCredential { key, credential } => {
                self.answer_when(key, tokio::spawn(secrets::forget(credential)));
                Ok(())
            }
            ClientMessage::UploadBegin {
                key,
                transfer: id,
                target,
                source,
            } => {
                self.transfers
                    .upload_begin(&self.route, key, id, target, source);
                Ok(())
            }
            ClientMessage::FetchBegin {
                key,
                transfer: id,
                agent,
                path,
                anyway,
            } => {
                if anyway && !self.yours() {
                    return self
                        .error(
                            ErrorCode::NotAllowed,
                            "only the app can show a file outside the agent's folders",
                        )
                        .await;
                }
                self.transfers
                    .fetch_begin(&self.route, key, id, agent, path, anyway);
                Ok(())
            }
            ClientMessage::FetchFiles {
                key,
                transfer: id,
                workspace,
                paths,
            } => {
                self.transfers
                    .fetch_files(&self.route, key, id, workspace, paths);
                Ok(())
            }
            ClientMessage::FetchSkillTree {
                key,
                transfer: id,
                tree,
            } => {
                self.transfers.fetch_tree(&self.route, key, id, tree);
                Ok(())
            }
            ClientMessage::OpenForward {
                key,
                port,
                local_port,
            } => {
                let outcome = match &self.link {
                    Some(link) => link.open_forward(port, local_port),
                    None => not_yet(),
                };
                self.outcome(key, outcome).await
            }
            // These name a transfer, forward or stream, and answer nothing.
            ClientMessage::UploadCancel { transfer: id } => {
                self.transfers.cancel(id);
                Ok(())
            }
            ClientMessage::TransferCredit {
                transfer: id,
                committed,
                window_end,
            } => {
                self.transfers.credit(id, committed, window_end);
                Ok(())
            }
            ClientMessage::CloseForward { forward } => {
                if let Some(link) = &self.link {
                    link.close_forward(forward);
                }
                Ok(())
            }
            ClientMessage::TunnelOpen { stream, forward } => {
                if let Some(link) = &self.link {
                    link.open(stream, forward);
                }
                Ok(())
            }
            ClientMessage::TunnelCredit { stream, granted } => {
                if let Some(link) = &self.link {
                    link.credit(stream, granted);
                }
                Ok(())
            }
            ClientMessage::TunnelClose { stream, how } => {
                if let Some(link) = &self.link {
                    link.close(stream, how);
                }
                Ok(())
            }
            ClientMessage::GuestSshOpen {
                stream,
                workspace: id,
            } => {
                let Some(link) = &self.link else {
                    return Ok(());
                };
                if let Some(message) = workspace::ssh_blocked(id) {
                    let refused = ServerMessage::TunnelRefused {
                        stream,
                        code: ErrorCode::NoSuchWorkspace,
                        message,
                    };
                    return self.send(&refused).await;
                }
                link.open_ssh(stream, id);
                Ok(())
            }
            ClientMessage::SetGuestSshKey { key, public_key } => {
                let outcome = match (&self.link, crate::sshkey::ed25519_line(&public_key)) {
                    (None, _) => not_yet(),
                    (Some(_), None) => ActionOutcome::Refused {
                        reason: "That isn't an ed25519 public key.".to_owned(),
                    },
                    (Some(link), Some(line)) => link.set_ssh_key(line),
                };
                self.outcome(key, outcome).await
            }
            ClientMessage::SetHookOn { key, task, on } => {
                self.answer_when(key, tokio::spawn(crate::hooks::set_on(task, on)));
                Ok(())
            }
            ClientMessage::RotateHookToken { key, task } => {
                self.answer_when(key, tokio::spawn(crate::hooks::rotate_token(task)));
                Ok(())
            }
            ClientMessage::SetHookSecret { key, task, secret } => {
                self.answer_when(key, tokio::spawn(crate::hooks::set_secret(task, secret)));
                Ok(())
            }
            ClientMessage::SetHookAddress { key, address } => {
                self.answer_when(key, tokio::spawn(crate::hooks::set_address(address)));
                Ok(())
            }
            ClientMessage::ListDeliveries { key, task } => {
                self.outcome(key, crate::hooks::list_deliveries(task)).await
            }
        }
    }

    /// Hands the outcome of `request` to `exchange` once it ends, so deltas
    /// and pongs go on meanwhile. Answers can pass each other, and the
    /// client matches each to its request by `key`.
    fn answer_when(&self, key: IdempotencyKey, request: JoinHandle<ActionOutcome>) {
        let answered = self.answered.clone();
        tokio::spawn(async move {
            let outcome = request.await.unwrap_or_else(|err| {
                warn!(%err, "a request ended without an answer");
                ActionOutcome::Refused {
                    reason: "maxd couldn't finish that. Its log says why.".to_owned(),
                }
            });
            // The client has gone when no one takes it, and the request
            // has still run to its end.
            let _ = answered.send((key, outcome));
        });
    }

    /// Sends the outcome of a request this connection answers itself.
    async fn outcome(
        &mut self,
        key: IdempotencyKey,
        outcome: ActionOutcome,
    ) -> Result<(), ConnError> {
        self.send(&ServerMessage::ActionResult { key, outcome })
            .await
    }

    /// Whether you're on the other end: the app, or a phone you paired,
    /// though the phone's own rules stop it first. Not `maxd mcp`, which
    /// agents' tool calls come through, a link from another device's
    /// `maxd`, which carries its agents' calls, or a one-time pairing key.
    /// A container's guest never connects here.
    fn yours(&self) -> bool {
        self.greeted && !self.mcp && self.peer.is_none() && !self.pairs
    }

    /// Whether this is a phone whose switch the Mac turned on, so it may
    /// do what only the Mac does here, short of letting other keys in.
    fn may_do_everything(&self) -> bool {
        self.phone.is_some_and(crate::phones::lets)
    }

    /// Agrees a protocol version, or turns the client away.
    async fn hello(&mut self, hello: ClientHello) -> Result<(), ConnError> {
        let ours = ProtocolRange::OURS;
        let Some(version) = ours.agree(hello.protocol) else {
            let message = ours.refusal(hello.protocol);
            self.error(ErrorCode::UnsupportedProtocol, message).await?;
            return Err(ConnError::Unsupported);
        };
        info!(
            client = %hello.client.name,
            client_version = %hello.client.version,
            protocol = version,
            "client said hello"
        );
        self.greeted = true;
        self.client_id = Some(hello.client_id);
        self.seat.hello(&hello.client.name, &hello.client.version);
        self.route = self.route.clone().speaking(version);
        self.mcp = hello.role == ClientRole::Mcp;
        self.client = Some(match self.phone {
            _ if self.mcp => self.sessions.tool_client(hello.client_id),
            Some(phone) => self.sessions.phone_client(hello.client_id, phone),
            None => self.sessions.client(hello.client_id),
        });
        let to_client = tunnel::Queue::spawn(ToClient(self.wire.clone()));
        self.link = Some(self.tunnels.attach(hello.client_id, Arc::new(to_client)));
        let reply = ServerMessage::Hello(self.server_hello().await);
        self.send(&reply).await?;
        crate::fleet::served_hello();
        Ok(())
    }

    /// What this `maxd` says hello with.
    async fn server_hello(&self) -> ServerHello {
        // Decided before the hello goes out, since the hello is the only
        // time the client hears it.
        let workspaces = crate::workspace::settled().await;
        let device = self.hub.device();
        ServerHello {
            protocol: ProtocolRange::OURS,
            maxd_version: device.maxd_version.clone(),
            device: device.id,
            boot_id: device.boot_id.clone(),
            instance: self.hub.instance(),
            build: crate::identity::build().map(str::to_owned),
            target: Some(crate::identity::TARGET.to_owned()),
            capabilities: Capabilities {
                transfers: true,
                tunnels: true,
                workspaces,
                github_checks: true,
                inbox: true,
                guest_ssh: workspaces,
                copy: true,
                load: true,
                updates: crate::fleet::updates(),
                tools: true,
                agent_tools: true,
                scheduled_tasks: true,
                peers: crate::peers::for_link().is_some(),
                acp_agents: true,
                github_user: true,
                pulls: true,
                test_hosts: true,
                workflows: crate::workflow::following(),
                host_spec: true,
                snapshots: workspaces,
                // On once moves are built.
                moves: true,
                tool_installs: crate::fleet::installs(),
                skills: true,
                // Takes the APNs key and pushes. Pairing still refuses.
                phones: true,
                library_moves: true,
                proposal_targets: true,
                peer_proofs: true,
                // Holds terminals in PTY holders of its own.
                terminals: self.sessions.terminals().is_some(),
                // Handles every model action once local models run.
                local_models: crate::local_models::running(),
            },
            challenge: self.challenge.map(|challenge| challenge.encode()),
        }
    }

    /// Sends what the client lacks since `since`, and every delta after.
    async fn sync(&mut self, since: Option<Seq>) -> Result<(), ConnError> {
        let Subscription { start, deltas } = self.hub.subscribe(since);
        self.deltas = Some(deltas);
        match start {
            Start::Snapshot(snapshot) => {
                self.sent = Some(snapshot.seq);
                self.send(&ServerMessage::Snapshot(snapshot)).await
            }
            Start::Deltas(missing) => {
                self.sent = since;
                for delta in missing {
                    self.sent = Some(delta.seq);
                    self.send(&ServerMessage::Delta(delta)).await?;
                }
                Ok(())
            }
        }
    }

    /// Sends a delta the client doesn't have yet.
    async fn forward(&mut self, delta: Result<Delta, RecvError>) -> Result<(), ConnError> {
        match delta {
            Ok(delta) => {
                if self.sent.is_some_and(|sent| delta.seq <= sent) {
                    return Ok(());
                }
                self.sent = Some(delta.seq);
                self.send(&ServerMessage::Delta(delta)).await
            }
            Err(RecvError::Lagged(skipped)) => {
                // The client reads slower than the state changes. Catch it
                // up from what it has, as a reconnect would.
                debug!(skipped, "client fell behind, catching it up");
                self.sync(self.sent).await
            }
            Err(RecvError::Closed) => Err(ConnError::Stopping),
        }
    }

    /// Refuses `message`. An action gets the refusal as its outcome, so
    /// the client stops waiting at once and shows the reason where it
    /// shows a failed action. Any other message gets an error.
    async fn refuse(
        &mut self,
        message: &ClientMessage,
        code: ErrorCode,
        reason: &str,
    ) -> Result<(), ConnError> {
        let Some(key) = message.action_key() else {
            return self.error(code, reason).await;
        };
        debug!(?code, %reason, "client action refused");
        let reason = reason.to_owned();
        self.outcome(key, ActionOutcome::Refused { reason }).await
    }

    /// Tells the client its message was refused. The connection stays
    /// open.
    async fn error(
        &mut self,
        code: ErrorCode,
        message: impl Into<String>,
    ) -> Result<(), ConnError> {
        let message = message.into();
        debug!(?code, %message, "client message refused");
        self.send(&ServerMessage::Error { code, message }).await
    }

    /// Queues one message frame. State waits behind control messages, so
    /// a pong or an action's result never waits for a big snapshot.
    async fn send(&mut self, message: &ServerMessage) -> Result<(), ConnError> {
        let mut out = Vec::new();
        frame::encode_message(&mut out, message)?;
        let queue = match message {
            ServerMessage::Snapshot(_)
            | ServerMessage::Delta(_)
            | ServerMessage::Timeline { .. }
            | ServerMessage::Workflow { .. } => Queue::Timeline,
            _ => Queue::Control,
        };
        self.wire.send(queue, out).await?;
        Ok(())
    }
}

/// Writes what the tunnels send a client, on the bulk queue behind the
/// state, so a stream's bytes never hold up a pong or a delta. A stream's
/// messages go the same way as its bytes, so a close never passes them.
pub(crate) struct ToClient(pub(crate) Sender);

impl Writer<ServerMessage> for ToClient {
    async fn write(&mut self, out: Out<ServerMessage>) -> Result<(), Gone> {
        let mut bytes = Vec::new();
        if let Err(err) = out.encode(&mut bytes) {
            warn!(%err, "a tunnel message for the client didn't encode");
            return Ok(());
        }
        self.0.send(Queue::Bulk, bytes).await.map_err(|_| Gone)
    }
}

/// The next delta once the client has synced. Before that it never
/// resolves.
async fn next_delta(deltas: &mut Option<broadcast::Receiver<Delta>>) -> Result<Delta, RecvError> {
    match deltas {
        Some(deltas) => deltas.recv().await,
        None => std::future::pending().await,
    }
}

/// The next phone or app key removed, once the connection is a phone's or
/// has proved a key. None means some were missed.
async fn next_removed<T: Clone>(removed: &mut Option<broadcast::Receiver<T>>) -> Option<T> {
    let Some(removed) = removed else {
        return std::future::pending().await;
    };
    match removed.recv().await {
        Ok(phone) => Some(phone),
        Err(RecvError::Lagged(_)) => None,
        Err(RecvError::Closed) => std::future::pending().await,
    }
}

/// The next message about a run the client has open. Before it opens one
/// it never resolves.
async fn next_workflow(viewer: &mut Option<crate::workflow::Viewer>) -> Option<ServerMessage> {
    match viewer {
        Some(viewer) => viewer.recv().await,
        None => std::future::pending().await,
    }
}

/// The session manager's next message for the client, or none once the
/// manager ended. Before the client says hello it never resolves.
async fn next_message(client: &mut Option<Client>) -> Option<ServerMessage> {
    match client {
        Some(client) => client.recv().await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;

    use maxhq_core::prove::Kind;
    use maxhq_protocol::attention::{AnswerOutcome, AttentionAnswer};
    use maxhq_protocol::ids::{
        AccountId, AgentId, AttentionId, BootId, ClientId, DeviceId, IdempotencyKey, ProjectId,
        TransferId, WorkspaceId,
    };
    use maxhq_protocol::message::{ClientInfo, UploadTarget, SHOW_ANYWAY_SINCE};
    use maxhq_protocol::model::{
        Activity, Agent, AgentLink, Control, Device, Vendor, Workspace, WorkspaceLocation,
        WorkspaceState,
    };
    use maxhq_protocol::state::{Change, Entity, EntityKey, Replica};
    use maxhq_protocol::time::Timestamp;
    use maxhq_protocol::timeline::TimelineUpdate;
    use maxhq_protocol::OLDEST_PROTOCOL_VERSION;
    use tokio::io::AsyncWriteExt;
    use tokio::time::timeout;

    use crate::testing::TempDir;

    /// How long a test waits for a reply before failing.
    pub(super) const WAIT: Duration = Duration::from_secs(5);

    pub(super) fn hub() -> Hub {
        Hub::new(
            Device {
                id: DeviceId::derive("test"),
                name: "mac".into(),
                os: "macos".into(),
                boot_id: BootId("boot-1".into()),
                maxd_version: "0.1.0".into(),
                winding_down_until: None,
                workspace_host: None,
                build: None,
                target: None,
            },
            HashMap::new(),
        )
    }

    fn agent(name: &str, activity: Activity) -> Entity {
        Entity::Agent(Agent {
            id: AgentId::derive(name),
            workspace: WorkspaceId::derive("w"),
            parent: None,
            vendor: Vendor::Claude,
            account: None,
            model: None,
            permission: None,
            session: None,
            vendor_session_id: Some(name.into()),
            control: Control::Observed,
            pid: Some(42),
            link: AgentLink::Live,
            title: name.into(),
            activity,
            context: None,
            started_at: Timestamp(1),
            last_activity_at: Timestamp(1),
            branch: None,
            inbox: Default::default(),
            origin: None,
            pulls: Vec::new(),
            taking_over: false,
            asleep: false,
            effort: None,
            running: None,
            moved: None,
            local: None,
        })
    }

    fn upsert(entity: Entity) -> Change {
        Change::Upsert { value: entity }
    }

    /// A running server on a socket in a temp dir.
    pub(crate) struct Served {
        pub(crate) hub: Hub,
        pub(crate) db: Db,
        pub(crate) socket: PathBuf,
        pub(crate) keys: AppKeys,
        /// The terminals the server's connections reach.
        pub(crate) terminals: crate::terminals::Terminals,
        _task: AbortOnDrop,
        _dir: TempDir,
    }

    struct AbortOnDrop(tokio::task::JoinHandle<()>);

    impl Drop for AbortOnDrop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }

    pub(crate) fn serve() -> Served {
        serve_on(hub())
    }

    /// A running server on `hub`, which trusts the test app key.
    pub(crate) fn serve_on(hub: Hub) -> Served {
        let served = serve_bare(hub);
        crate::app_keys::testing::trust(&served.keys);
        served
    }

    /// A running server on `hub` that trusts no app key yet, as on a
    /// device setup hasn't reached.
    pub(super) fn serve_bare(hub: Hub) -> Served {
        // Cards and enrollments read maxd's own signature, once a run. For
        // a test binary on a Mac that lists the whole build folder, which
        // can take seconds, so it happens here, before any test's wait.
        #[cfg(target_os = "macos")]
        crate::codesign::own(None);
        let dir = TempDir::new();
        let socket = dir.path().join("maxd.sock");
        let listener = bind(&socket).unwrap();
        let db = Db::open_in_memory().unwrap();
        let sessions = crate::session::for_tests(hub.clone(), db.clone(), dir.path().join("files"));
        let terminals = sessions
            .terminals()
            .expect("the test sessions hold terminals")
            .clone();
        let transfers = Transfers::new(crate::transfer::Config {
            // A CI runner's disk can be nearly full, and these tests aren't
            // about room.
            room: maxhq_transfer::Room {
                keep_bytes: 0,
                keep_percent: 0,
                keep_at_most: 0,
            },
            ..crate::transfer::Config::local(hub.clone(), dir.path().join("attachments"))
        });
        let tunnels = Tunnels::new(hub.clone());
        let keys = AppKeys::open(dir.path().join("phones"), hub.device().id);
        keys.publish_to(hub.clone());
        let task = tokio::spawn({
            let (hub, db, keys) = (hub.clone(), db.clone(), keys.clone());
            async move {
                listener
                    .serve(hub, db, sessions, transfers, tunnels, keys)
                    .await
            }
        });
        Served {
            hub,
            db,
            socket,
            keys,
            terminals,
            _task: AbortOnDrop(task),
            _dir: dir,
        }
    }

    /// The client side of the protocol, as plainly as possible.
    pub(super) struct TestClient {
        stream: UnixStream,
        decoder: FrameDecoder,
    }

    impl TestClient {
        pub(super) async fn connect(socket: &Path) -> Self {
            Self {
                stream: UnixStream::connect(socket).await.unwrap(),
                decoder: FrameDecoder::new(),
            }
        }

        /// Connects, says hello as the app and proves it with the test app
        /// key, returning the server's hello.
        pub(super) async fn greeted(socket: &Path) -> (Self, ServerHello) {
            Self::greeted_with(socket, hello(ProtocolRange::OURS)).await
        }

        /// Connects, says `hello` and proves it's the app.
        pub(super) async fn greeted_with(
            socket: &Path,
            hello: ClientMessage,
        ) -> (Self, ServerHello) {
            let (mut client, server) = Self::unproven_with(socket, &hello).await;
            client.prove(&server, &hello).await;
            (client, server)
        }

        /// Connects and says hello as the app, without proving it.
        pub(super) async fn unproven(socket: &Path) -> (Self, ServerHello) {
            Self::unproven_with(socket, &hello(ProtocolRange::OURS)).await
        }

        /// Connects and says `hello`, without proving anything.
        pub(super) async fn unproven_with(
            socket: &Path,
            hello: &ClientMessage,
        ) -> (Self, ServerHello) {
            let mut client = Self::connect(socket).await;
            client.send(hello).await;
            match client.recv().await {
                ServerMessage::Hello(hello) => (client, hello),
                other => panic!("expected a hello, got {other:?}"),
            }
        }

        /// Proves this connection, which said `hello`, is the app.
        pub(super) async fn prove(&mut self, server: &ServerHello, hello: &ClientMessage) {
            let seed = &crate::app_keys::testing::SEED;
            let outcome = self.prove_as(Kind::App, seed, server, hello).await;
            assert_eq!(outcome, ActionOutcome::Done);
        }

        /// Proves this connection, which said `hello`, holds the key whose
        /// seed is `seed`, as a `kind` connection, and gives the answer.
        pub(super) async fn prove_as(
            &mut self,
            kind: Kind,
            seed: &[u8; 32],
            server: &ServerHello,
            hello: &ClientMessage,
        ) -> ActionOutcome {
            let ClientMessage::Hello(hello) = hello else {
                panic!("expected a hello, got {hello:?}");
            };
            let prove = crate::app_keys::testing::prove_as(kind, seed, server, hello.client_id);
            let ClientMessage::Prove { key, .. } = &prove else {
                unreachable!("the helper makes a proof");
            };
            let key = *key;
            self.send(&prove).await;
            self.outcome(key).await
        }

        /// The outcome of the request with `key`, past anything else.
        pub(super) async fn outcome(&mut self, key: IdempotencyKey) -> ActionOutcome {
            loop {
                if let ServerMessage::ActionResult { key: got, outcome } = self.recv().await
                    && got == key
                {
                    return outcome;
                }
            }
        }

        pub(super) async fn send(&mut self, message: &ClientMessage) {
            let mut out = Vec::new();
            frame::encode_message(&mut out, message).unwrap();
            self.stream.write_all(&out).await.unwrap();
        }

        /// Sends `bytes` to a terminal, as typing does.
        pub(super) async fn type_into(&mut self, terminal: TerminalId, bytes: &[u8]) {
            let mut out = Vec::new();
            frame::encode_terminal_input(&mut out, terminal, bytes);
            self.stream.write_all(&out).await.unwrap();
        }

        pub(super) async fn recv(&mut self) -> ServerMessage {
            timeout(WAIT, self.try_recv())
                .await
                .expect("a reply in time")
                .expect("the server keeps the connection open")
        }

        /// The next message, or none once the server closes.
        pub(super) async fn try_recv(&mut self) -> Option<ServerMessage> {
            let mut buf = [0; 4096];
            loop {
                if let Some(Frame::Message(json)) = self.decoder.next_frame().unwrap() {
                    return Some(serde_json::from_slice(json).unwrap());
                }
                let n = self.stream.read(&mut buf).await.unwrap();
                if n == 0 {
                    return None;
                }
                self.decoder.push(&buf[..n]);
            }
        }

        /// The next frame, a message or a transfer chunk's length.
        async fn recv_any(&mut self) -> Result<ServerMessage, usize> {
            let mut buf = [0; 64 * 1024];
            loop {
                match self.decoder.next_frame().unwrap() {
                    Some(Frame::Message(json)) => return Ok(serde_json::from_slice(json).unwrap()),
                    Some(Frame::TransferChunk { data, .. }) => return Err(data.len()),
                    Some(_) | None => {}
                }
                let read = timeout(WAIT, self.stream.read(&mut buf)).await;
                let n = read.expect("a frame in time").unwrap();
                assert!(n > 0, "the server keeps the connection open");
                self.decoder.push(&buf[..n]);
            }
        }

        /// Fails if anything arrives within a short wait.
        pub(super) async fn expect_quiet(&mut self) {
            let next = timeout(Duration::from_millis(200), self.try_recv()).await;
            assert!(next.is_err(), "expected nothing, got {next:?}");
        }
    }

    pub(crate) fn hello(protocol: ProtocolRange) -> ClientMessage {
        ClientMessage::Hello(ClientHello {
            protocol,
            client: ClientInfo {
                name: "maxd-test".into(),
                version: "0".into(),
            },
            client_id: ClientId::derive("test"),
            role: ClientRole::App,
        })
    }

    #[tokio::test]
    async fn hello_names_the_device_and_this_run() {
        let served = serve();
        let (_, hello) = TestClient::greeted(&served.socket).await;
        assert_eq!(hello.protocol, ProtocolRange::OURS);
        assert_eq!(hello.device, DeviceId::derive("test"));
        assert_eq!(hello.boot_id, BootId("boot-1".into()));
        assert_eq!(hello.instance, served.hub.instance());
        assert_eq!(hello.maxd_version, "0.1.0");
    }

    #[tokio::test]
    async fn a_sync_from_nothing_gets_a_snapshot() {
        let served = serve();
        served.hub.apply([upsert(agent("a", Activity::Busy))]);
        let (mut client, _) = TestClient::greeted(&served.socket).await;
        client.send(&ClientMessage::Sync { since: None }).await;
        let ServerMessage::Snapshot(snapshot) = client.recv().await else {
            panic!("expected a snapshot");
        };
        assert_eq!(snapshot.seq, served.hub.seq());
        let mut replica = Replica::default();
        replica.apply_snapshot(snapshot).unwrap();
        assert!(replica
            .get(&EntityKey::Device(DeviceId::derive("test")))
            .is_some());
        assert!(replica
            .get(&EntityKey::Agent(AgentId::derive("a")))
            .is_some());
    }

    #[tokio::test]
    async fn a_sync_from_a_known_seq_gets_only_the_deltas_after_it() {
        let served = serve();
        // The proof dates the app key, which is a delta of its own.
        let (mut client, _) = TestClient::greeted(&served.socket).await;
        let known = served.hub.seq();
        served.hub.apply([upsert(agent("a", Activity::Busy))]);
        served.hub.apply([upsert(agent("b", Activity::Idle))]);

        client
            .send(&ClientMessage::Sync { since: Some(known) })
            .await;
        let mut seqs = Vec::new();
        for _ in 0..2 {
            let ServerMessage::Delta(delta) = client.recv().await else {
                panic!("expected a delta");
            };
            seqs.push(delta.seq);
        }
        assert_eq!(seqs, [known.next(), known.next().next()]);
        client.expect_quiet().await;

        // A client that already has everything gets nothing until a change.
        let (mut current, _) = TestClient::greeted(&served.socket).await;
        let now = served.hub.seq();
        current
            .send(&ClientMessage::Sync { since: Some(now) })
            .await;
        current.expect_quiet().await;
    }

    #[tokio::test]
    async fn deltas_after_the_sync_arrive_in_order() {
        let served = serve();
        let (mut client, _) = TestClient::greeted(&served.socket).await;
        client.send(&ClientMessage::Sync { since: None }).await;
        let ServerMessage::Snapshot(snapshot) = client.recv().await else {
            panic!("expected a snapshot");
        };
        let mut replica = Replica::default();
        replica.apply_snapshot(snapshot).unwrap();

        served.hub.apply([upsert(agent("a", Activity::Busy))]);
        served.hub.apply([upsert(agent(
            "a",
            Activity::NeedsInput {
                attention: AttentionId::derive("q"),
            },
        ))]);
        for _ in 0..2 {
            let ServerMessage::Delta(delta) = client.recv().await else {
                panic!("expected a delta");
            };
            replica.apply_delta(delta).unwrap();
        }
        assert_eq!(replica.seq(), Some(served.hub.seq()));
        let Some(Entity::Agent(agent)) = replica.get(&EntityKey::Agent(AgentId::derive("a")))
        else {
            panic!("the agent arrived");
        };
        assert!(matches!(agent.activity, Activity::NeedsInput { .. }));
    }

    #[tokio::test]
    async fn nothing_but_a_hello_is_accepted_first() {
        let served = serve();
        let mut client = TestClient::connect(&served.socket).await;
        client.send(&ClientMessage::Sync { since: None }).await;
        assert!(matches!(
            client.recv().await,
            ServerMessage::Error {
                code: ErrorCode::NoHello,
                ..
            }
        ));
        client.send(&hello(ProtocolRange::OURS)).await;
        assert!(matches!(client.recv().await, ServerMessage::Hello(_)));
    }

    #[tokio::test]
    async fn a_client_with_no_shared_version_is_turned_away() {
        let served = serve();
        let mut client = TestClient::connect(&served.socket).await;
        let future = ProtocolRange {
            current: ProtocolRange::OURS.current + 2,
            oldest: ProtocolRange::OURS.current + 1,
        };
        client.send(&hello(future)).await;
        assert!(matches!(
            client.recv().await,
            ServerMessage::Error {
                code: ErrorCode::UnsupportedProtocol,
                ..
            }
        ));
        let closed = timeout(WAIT, client.try_recv()).await.unwrap();
        assert_eq!(closed, None, "the server closes the connection");
    }

    fn peer_hello() -> ClientMessage {
        let ClientMessage::Hello(mut hello) = hello(ProtocolRange::OURS) else {
            unreachable!()
        };
        hello.role = ClientRole::Peer;
        ClientMessage::Hello(hello)
    }

    #[tokio::test]
    async fn a_phone_pairs_once_through_its_one_time_key_and_then_has_its_own() {
        use base64::Engine;
        use maxhq_protocol::phones::{PhoneBookEntry, PhoneRoute, PushEnvironment, PushToken};
        use russh::keys::ssh_key::private::Ed25519Keypair;
        use russh::keys::ssh_key::PublicKey;

        let phones = crate::phones::for_tests();
        let served = serve();
        let key = |n: u8| {
            PublicKey::from(Ed25519Keypair::from_seed(&[n; 32]).public)
                .to_openssh()
                .unwrap()
        };
        let book = vec![PhoneBookEntry {
            device: DeviceId::derive("devbox"),
            name: "devbox".into(),
            routes: vec![PhoneRoute {
                user: "dev".into(),
                host: "devbox.local".into(),
                port: 22,
            }],
            host_key: key(200),
            network: None,
        }];
        let (mut app, _) = TestClient::greeted(&served.socket).await;
        let home = maxhq_protocol::phones::PhoneMac {
            device: DeviceId::derive("Home Mac"),
            name: "Home Mac".into(),
        };
        app.send(&ClientMessage::BeginPhonePair {
            key: IdempotencyKey::derive("begin"),
            book: book.clone(),
            mac: Some(home.clone()),
        })
        .await;
        let ServerMessage::ActionResult {
            outcome: ActionOutcome::PhonePair { pair, seed, .. },
            ..
        } = app.recv().await
        else {
            panic!("expected a pairing");
        };
        let seed: [u8; 32] = base64::engine::general_purpose::STANDARD
            .decode(seed.expose())
            .unwrap()
            .try_into()
            .unwrap();

        // The one-time key's connection proves it, and then may only pair.
        let mut pairing = TestClient::connect(&served.socket).await;
        pairing.send(&ClientMessage::Pair { pair }).await;
        pairing.send(&hello(ProtocolRange::OURS)).await;
        let ServerMessage::Hello(server) = pairing.recv().await else {
            panic!("expected a hello");
        };
        let proved = pairing
            .prove_as(Kind::Pair, &seed, &server, &hello(ProtocolRange::OURS))
            .await;
        assert_eq!(proved, ActionOutcome::Done);
        pairing.send(&ClientMessage::Sync { since: None }).await;
        assert!(matches!(
            pairing.recv().await,
            ServerMessage::Error {
                code: ErrorCode::NotAllowed,
                ..
            }
        ));
        let pair_phone = |n: &str| ClientMessage::PairPhone {
            key: IdempotencyKey::derive(n),
            name: "iPhone".into(),
            ssh_public: key(201),
            push_public: base64::engine::general_purpose::STANDARD.encode([7u8; 32]),
        };
        pairing.send(&pair_phone("pair")).await;
        let phone = match pairing.recv().await {
            ServerMessage::ActionResult {
                outcome:
                    ActionOutcome::PhonePaired {
                        phone,
                        book: got,
                        mac,
                    },
                ..
            } => {
                assert_eq!(got, book);
                assert_eq!(mac, Some(home), "the phone hears which Mac paired it");
                phone
            }
            other => panic!("{other:?}"),
        };
        pairing.send(&pair_phone("again")).await;
        assert!(matches!(
            pairing.recv().await,
            ServerMessage::ActionResult {
                outcome: ActionOutcome::Refused { .. },
                ..
            }
        ));
        // The key worked once, so its pairing is gone.
        let mut late = TestClient::connect(&served.socket).await;
        late.send(&ClientMessage::Pair { pair }).await;
        assert!(matches!(
            late.recv().await,
            ServerMessage::Error {
                code: ErrorCode::NotAllowed,
                ..
            }
        ));
        assert!(late.try_recv().await.is_none());

        // Only the phone's own connection sends push requests.
        let token = || PushToken {
            token: maxhq_protocol::Secret::new("00ff".repeat(8)),
            topic: "com.example.maxhq".into(),
            environment: PushEnvironment::Sandbox,
            kinds: Vec::new(),
        };
        app.send(&ClientMessage::SetPushToken {
            key: IdempotencyKey::derive("token-app"),
            push: token(),
        })
        .await;
        assert!(matches!(
            app.recv().await,
            ServerMessage::ActionResult {
                outcome: ActionOutcome::Refused { reason },
                ..
            } if reason == crate::push::ONLY_PHONES
        ));
        let mut own = TestClient::connect(&served.socket).await;
        own.send(&ClientMessage::Phone { phone }).await;
        own.send(&hello(ProtocolRange::OURS)).await;
        let ServerMessage::Hello(server) = own.recv().await else {
            panic!("expected a hello");
        };
        let proved = own
            .prove_as(
                Kind::Phone,
                &[201; 32],
                &server,
                &hello(ProtocolRange::OURS),
            )
            .await;
        assert_eq!(proved, ActionOutcome::Done);
        own.send(&ClientMessage::Presence { agent: None }).await;
        own.send(&ClientMessage::SetPushToken {
            key: IdempotencyKey::derive("token-phone"),
            push: token(),
        })
        .await;
        // No push service runs in this test, so it says so, and not that
        // only a phone sends it.
        let ServerMessage::ActionResult {
            outcome: ActionOutcome::Refused { reason },
            ..
        } = own.recv().await
        else {
            panic!("expected an answer");
        };
        assert_ne!(reason, crate::push::ONLY_PHONES);

        // A phone has App rights, but it can't let another key in.
        let add = |n: &str, at: u8| ClientMessage::AddPhone {
            key: IdempotencyKey::derive(n),
            phone: PhoneId::derive(&key(at)),
            name: "iPad".into(),
            ssh_public: key(at),
            push_public: base64::engine::general_purpose::STANDARD.encode([8u8; 32]),
        };
        for message in [
            ClientMessage::BeginPhonePair {
                key: IdempotencyKey::derive("begin-phone"),
                book: book.clone(),
                mac: None,
            },
            add("add-phone", 202),
            ClientMessage::AuthorizePeer {
                key: IdempotencyKey::derive("peer-phone"),
                device: DeviceId::derive("other"),
                public_key: key(203),
            },
        ] {
            own.send(&message).await;
            assert!(
                matches!(
                    own.recv().await,
                    ServerMessage::ActionResult {
                        outcome: ActionOutcome::Refused { .. },
                        ..
                    }
                ),
                "{message:?}"
            );
        }
        assert!(!phones.knows(PhoneId::derive(&key(202))));
        // The Mac still can.
        app.send(&add("add-app", 202)).await;
        assert!(matches!(
            app.recv().await,
            ServerMessage::ActionResult {
                outcome: ActionOutcome::Done,
                ..
            }
        ));

        // Removing the phone closes its connection.
        app.send(&ClientMessage::RemovePhone {
            key: IdempotencyKey::derive("remove"),
            phone,
        })
        .await;
        assert!(matches!(
            app.recv().await,
            ServerMessage::ActionResult {
                outcome: ActionOutcome::Done,
                ..
            }
        ));
        assert!(timeout(WAIT, own.try_recv()).await.unwrap().is_none());
        assert!(!phones.knows(phone));
        let mut stranger = TestClient::connect(&served.socket).await;
        stranger.send(&ClientMessage::Phone { phone }).await;
        assert!(matches!(
            stranger.recv().await,
            ServerMessage::Error {
                code: ErrorCode::NotAllowed,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn only_the_peer_frame_gives_a_connection_peer_rights() {
        let served = serve();
        // A peer hello with no frame first is refused.
        let mut client = TestClient::connect(&served.socket).await;
        client.send(&peer_hello()).await;
        assert!(matches!(
            client.recv().await,
            ServerMessage::Error {
                code: ErrorCode::NotAllowed,
                ..
            }
        ));

        // The frame counts only first, and an app can't send peer messages.
        let (mut app, _) = TestClient::greeted(&served.socket).await;
        app.send(&ClientMessage::Peer {
            device: DeviceId::derive("other"),
        })
        .await;
        assert!(matches!(
            app.recv().await,
            ServerMessage::Error {
                code: ErrorCode::BadMessage,
                ..
            }
        ));
        app.send(&ClientMessage::PeerAnswer {
            key: IdempotencyKey::generate(),
            outcome: ActionOutcome::Done,
        })
        .await;
        assert!(matches!(
            app.recv().await,
            ServerMessage::Error {
                code: ErrorCode::NotAllowed,
                ..
            }
        ));

        // With no peer links on this device, the frame closes the
        // connection with no hello.
        let mut peer = TestClient::connect(&served.socket).await;
        peer.send(&ClientMessage::Peer {
            device: DeviceId::derive("other"),
        })
        .await;
        peer.send(&peer_hello()).await;
        let closed = timeout(WAIT, peer.try_recv()).await.unwrap();
        assert_eq!(closed, None, "the server closes the connection");
    }

    #[tokio::test]
    async fn show_anyway_comes_only_from_the_app_and_old_apps_hear_a_bad_path() {
        let served = serve();
        let folder = TempDir::new();
        let outside = TempDir::new();
        fs::write(outside.path().join("plan.md"), b"# Plan").unwrap();
        let plan = outside.path().join("plan.md");
        served.hub.apply([
            upsert(Entity::Workspace(Workspace {
                id: WorkspaceId::derive("w"),
                project: None,
                device: DeviceId::derive("test"),
                location: WorkspaceLocation::Folder {
                    path: folder.path().to_str().unwrap().to_owned(),
                },
                managed: None,
                clone: None,
            })),
            upsert(agent("a", Activity::Idle)),
        ]);
        let fetch = |anyway| ClientMessage::FetchBegin {
            key: IdempotencyKey::generate(),
            transfer: TransferId::generate(),
            agent: AgentId::derive("a"),
            path: plan.to_str().unwrap().to_owned(),
            anyway,
        };
        let refused = |message: ServerMessage| match message {
            ServerMessage::TransferFailed { code, .. } => code,
            other => panic!("expected a refusal, got {other:?}"),
        };

        // An agent tools connection can't send it.
        let mut tools = TestClient::connect(&served.socket).await;
        let ClientMessage::Hello(mut greeting) = hello(ProtocolRange::OURS) else {
            unreachable!()
        };
        greeting.role = ClientRole::Mcp;
        tools.send(&ClientMessage::Hello(greeting)).await;
        assert!(matches!(tools.recv().await, ServerMessage::Hello(_)));
        tools.send(&fetch(true)).await;
        assert!(matches!(
            tools.recv().await,
            ServerMessage::Error {
                code: ErrorCode::NotAllowed,
                ..
            }
        ));

        // An app from before Show anyway hears an outside path as a bad one.
        let old_hello = hello(ProtocolRange {
            current: SHOW_ANYWAY_SINCE - 1,
            oldest: OLDEST_PROTOCOL_VERSION,
        });
        let (mut old, _) = TestClient::greeted_with(&served.socket, old_hello).await;
        old.send(&fetch(false)).await;
        assert_eq!(refused(old.recv().await), ErrorCode::BadPath);

        // Today's app hears it's outside, and can show it anyway.
        let (mut app, _) = TestClient::greeted(&served.socket).await;
        app.send(&fetch(false)).await;
        assert_eq!(refused(app.recv().await), ErrorCode::Outside);
        assert!(matches!(
            app.recv().await,
            ServerMessage::ActionResult {
                outcome: ActionOutcome::Refused { .. },
                ..
            }
        ));
        app.send(&fetch(true)).await;
        let ServerMessage::FetchManifest { entries, .. } = app.recv().await else {
            panic!("expected a manifest");
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].size, 6);
    }

    #[tokio::test]
    async fn pings_get_pongs_and_junk_gets_an_error() {
        let served = serve();
        let (mut client, _) = TestClient::greeted(&served.socket).await;
        client.send(&ClientMessage::Ping { nonce: 7 }).await;
        assert_eq!(client.recv().await, ServerMessage::Pong { nonce: 7 });

        let mut out = Vec::new();
        frame::encode_message(&mut out, &serde_json::json!({"type": "dance"})).unwrap();
        client.stream.write_all(&out).await.unwrap();
        assert!(matches!(
            client.recv().await,
            ServerMessage::Error {
                code: ErrorCode::BadMessage,
                ..
            }
        ));
        client.send(&ClientMessage::Ping { nonce: 8 }).await;
        assert_eq!(client.recv().await, ServerMessage::Pong { nonce: 8 });
    }

    /// The managed state a change gives `workspace`, if it's one of its.
    fn workspace_state(change: &Change, workspace: WorkspaceId) -> Option<&WorkspaceState> {
        match change {
            Change::Upsert {
                value: Entity::Workspace(changed),
            } if changed.id == workspace => changed.managed.as_ref().map(|managed| &managed.state),
            _ => None,
        }
    }

    // The manager the workspace requests answer through is global, so this
    // is the one test that installs it. A multi-thread runtime runs the
    // fake Incus while the connection waits.
    #[tokio::test(flavor = "multi_thread")]
    async fn deltas_and_pongs_go_on_while_a_stop_waits() {
        let installed = workspace::install_slow_stop(Duration::from_secs(1)).await;
        let served = serve_on(installed.hub.clone());
        let (mut client, _) = TestClient::greeted(&served.socket).await;
        client.send(&ClientMessage::Sync { since: None }).await;
        assert!(matches!(client.recv().await, ServerMessage::Snapshot(_)));

        let key = IdempotencyKey::derive("stop");
        let workspace = installed.workspace;
        client
            .send(&ClientMessage::StopWorkspace { key, workspace })
            .await;
        client.send(&ClientMessage::Ping { nonce: 9 }).await;

        // Stopping shows and the pong comes back while the answer waits for
        // the container.
        let (mut stopping, mut ponged) = (false, false);
        loop {
            match client.recv().await {
                ServerMessage::Delta(delta) => {
                    stopping |= delta.changes.iter().any(|change| {
                        workspace_state(change, workspace) == Some(&WorkspaceState::Stopping)
                    });
                }
                ServerMessage::Pong { nonce: 9 } => ponged = true,
                ServerMessage::ActionResult {
                    key: answered,
                    outcome,
                } => {
                    assert_eq!(answered, key);
                    assert_eq!(outcome, ActionOutcome::Done);
                    break;
                }
                other => panic!("expected a delta, a pong or the answer, got {other:?}"),
            }
        }
        assert!(stopping, "Stopping showed before the answer");
        assert!(ponged, "the pong came before the answer");

        // The answer came once the container had stopped.
        let Some(Entity::Workspace(stopped)) = installed.hub.get(&EntityKey::Workspace(workspace))
        else {
            panic!("the workspace is there");
        };
        assert_eq!(stopped.managed.unwrap().state, WorkspaceState::Stopped);
    }

    #[tokio::test]
    async fn an_upload_over_the_socket_lands_in_staging() {
        let served = serve();
        let (mut client, _) = TestClient::greeted(&served.socket).await;
        let src = TempDir::new();
        let path = src.path().join("notes.txt");
        let data = b"a few notes".repeat(40_000);
        fs::write(&path, &data).unwrap();
        let walk = maxhq_transfer::walk(&path).unwrap();
        let (key, transfer) = (IdempotencyKey::generate(), TransferId::generate());
        client
            .send(&ClientMessage::UploadBegin {
                key,
                transfer,
                target: UploadTarget::Device,
                source: UploadSource::Stream {
                    name: walk.name.clone(),
                    entries: walk.entries.clone(),
                },
            })
            .await;
        let credit = client.recv().await;
        let ServerMessage::TransferCredit { committed: 0, .. } = credit else {
            panic!("expected credit from the start, got {credit:?}");
        };
        // The first window takes the whole file.
        let mut out = Vec::new();
        frame::encode_transfer_chunks(&mut out, transfer, 0, &data);
        client.stream.write_all(&out).await.unwrap();
        let attachment = loop {
            match client.recv().await {
                ServerMessage::TransferCredit { .. } => {}
                ServerMessage::UploadDone { attachment, .. } => break attachment,
                other => panic!("expected the upload done, got {other:?}"),
            }
        };
        assert_eq!(fs::read(&attachment.path).unwrap(), data);
        assert_eq!(
            client.recv().await,
            ServerMessage::ActionResult {
                key,
                outcome: ActionOutcome::Staged { attachment },
            }
        );
    }

    #[tokio::test]
    async fn a_pong_overtakes_a_download_in_flight() {
        let served = serve();
        let folder = TempDir::new();
        let size = 16 * 1024 * 1024;
        fs::write(folder.path().join("big.bin"), vec![7u8; size]).unwrap();
        served.hub.apply([
            upsert(Entity::Workspace(Workspace {
                id: WorkspaceId::derive("w"),
                project: None,
                device: DeviceId::derive("test"),
                location: WorkspaceLocation::Folder {
                    path: folder.path().to_str().unwrap().to_owned(),
                },
                managed: None,
                clone: None,
            })),
            upsert(agent("a", Activity::Idle)),
        ]);
        let (mut client, _) = TestClient::greeted(&served.socket).await;
        let (key, transfer) = (IdempotencyKey::generate(), TransferId::generate());
        client
            .send(&ClientMessage::FetchBegin {
                key,
                transfer,
                agent: AgentId::derive("a"),
                path: "big.bin".into(),
                anyway: false,
            })
            .await;
        let ServerMessage::FetchManifest { entries, .. } = client.recv().await else {
            panic!("expected a manifest");
        };
        assert_eq!(entries[0].size, size as u64);
        let granted = 8 * 1024 * 1024;
        client
            .send(&ClientMessage::TransferCredit {
                transfer,
                committed: 0,
                window_end: granted,
            })
            .await;
        // Let the download fill what the writer and the socket hold.
        assert!(client.recv_any().await.is_err(), "a chunk comes first");
        tokio::time::sleep(Duration::from_millis(200)).await;

        client.send(&ClientMessage::Ping { nonce: 9 }).await;
        let mut before = 0;
        loop {
            match client.recv_any().await {
                Ok(ServerMessage::Pong { nonce: 9 }) => break,
                Ok(other) => panic!("expected a pong, got {other:?}"),
                Err(len) => before += len as u64,
            }
        }
        assert!(
            before < granted / 2,
            "the pong waited behind {before} bytes of the download"
        );
    }

    #[tokio::test]
    async fn mark_seen_clears_done_unread_and_is_stored() {
        let served = serve();
        let only_agents = |e: &Entity| matches!(e, Entity::Agent(_));
        served
            .hub
            .replace(only_agents, [agent("a", Activity::Busy)]);
        let Entity::Agent(mut done) = agent("a", Activity::Idle) else {
            unreachable!()
        };
        done.last_activity_at = Timestamp::now();
        served.hub.replace(only_agents, [Entity::Agent(done)]);

        let (mut client, _) = TestClient::greeted(&served.socket).await;
        client.send(&ClientMessage::Sync { since: None }).await;
        let ServerMessage::Snapshot(_) = client.recv().await else {
            panic!("expected a snapshot");
        };
        let id = AgentId::derive("a");
        client.send(&ClientMessage::MarkSeen { agent: id }).await;
        let ServerMessage::Delta(delta) = client.recv().await else {
            panic!("expected a delta");
        };
        assert!(matches!(
            &delta.changes[..],
            [Change::Upsert { value: Entity::Agent(a) }] if a.activity == Activity::Idle
        ));
        // The write happens before the next message is read, so a ping
        // round trip means it's done.
        client.send(&ClientMessage::Ping { nonce: 1 }).await;
        client.recv().await;
        assert!(served.db.seen().unwrap().contains_key(&id));
    }

    #[tokio::test]
    async fn refresh_usage_reaches_the_usage_tasks() {
        let served = serve();
        let mut commands = served.hub.commands();
        let (mut client, _) = TestClient::greeted(&served.socket).await;
        let account = AccountId::derive("claude");
        client
            .send(&ClientMessage::RefreshUsage {
                account: Some(account),
            })
            .await;
        let command = timeout(WAIT, commands.recv()).await.unwrap().unwrap();
        assert_eq!(command, Command::RefreshUsage(Some(account)));
    }

    #[tokio::test]
    async fn guest_ssh_takes_only_an_ed25519_key_and_a_running_container() {
        let served = serve();
        let (mut client, _) = TestClient::greeted(&served.socket).await;
        let key = IdempotencyKey::derive("ssh key");
        client
            .send(&ClientMessage::SetGuestSshKey {
                key,
                public_key: "ssh-rsa AAAAB3NzaC1yc2E dev@devbox".to_owned(),
            })
            .await;
        let ServerMessage::ActionResult {
            key: answered,
            outcome,
        } = client.recv().await
        else {
            panic!("expected an answer");
        };
        assert_eq!(answered, key);
        assert!(matches!(outcome, ActionOutcome::Refused { .. }));

        // This device runs no containers, so nothing opens.
        let stream = StreamId::generate();
        client
            .send(&ClientMessage::GuestSshOpen {
                stream,
                workspace: WorkspaceId::derive("w"),
            })
            .await;
        assert!(matches!(
            client.recv().await,
            ServerMessage::TunnelRefused { stream: refused, code: ErrorCode::NoSuchWorkspace, .. }
                if refused == stream
        ));
    }

    #[tokio::test]
    async fn folders_answer_the_app_and_never_agents_tools() {
        let served = serve();
        let me = served.hub.device().id;
        let dir = TempDir::new();
        fs::create_dir(dir.path().join("example-app")).unwrap();
        let list = ClientMessage::ListFolder {
            key: IdempotencyKey::derive("list"),
            device: me,
            path: dir.path().display().to_string(),
            files: false,
            hidden: false,
            filter: None,
        };
        let make = ClientMessage::MakeFolder {
            key: IdempotencyKey::derive("make"),
            device: me,
            path: dir.path().join("new").display().to_string(),
        };

        let mut tools = TestClient::connect(&served.socket).await;
        let ClientMessage::Hello(mut hello) = hello(ProtocolRange::OURS) else {
            unreachable!()
        };
        hello.role = ClientRole::Mcp;
        tools.send(&ClientMessage::Hello(hello)).await;
        assert!(matches!(tools.recv().await, ServerMessage::Hello(_)));
        for message in [&list, &make] {
            tools.send(message).await;
            assert!(matches!(
                tools.recv().await,
                ServerMessage::Error {
                    code: ErrorCode::NotAllowed,
                    ..
                }
            ));
        }
        assert!(!dir.path().join("new").exists());

        let (mut app, _) = TestClient::greeted(&served.socket).await;
        app.send(&list).await;
        match app.recv().await {
            ServerMessage::ActionResult {
                outcome: ActionOutcome::FolderListed { listing },
                ..
            } => {
                assert!(listing.exists);
                assert_eq!(listing.entries.len(), 1);
                assert_eq!(listing.entries[0].name, "example-app");
            }
            other => panic!("{other:?}"),
        }
        app.send(&make).await;
        assert!(matches!(
            app.recv().await,
            ServerMessage::ActionResult {
                outcome: ActionOutcome::Done,
                ..
            }
        ));
        assert!(dir.path().join("new").is_dir());
    }

    // The rules on this device come from the real home folder, so this test
    // asks for another device's, and the command-line tests read this one's.
    #[tokio::test]
    async fn cursor_rules_answer_the_app_and_never_agents_tools() {
        let served = serve();
        let devbox = DeviceId::derive("devbox");
        let list = ClientMessage::CursorRules {
            key: IdempotencyKey::derive("list"),
            device: devbox,
            call: CursorRuleCall::List,
        };
        let remove = ClientMessage::CursorRules {
            key: IdempotencyKey::derive("remove"),
            device: devbox,
            call: CursorRuleCall::Remove {
                rule: "Shell(git status)".into(),
            },
        };

        let mut tools = TestClient::connect(&served.socket).await;
        let ClientMessage::Hello(mut hello) = hello(ProtocolRange::OURS) else {
            unreachable!()
        };
        hello.role = ClientRole::Mcp;
        tools.send(&ClientMessage::Hello(hello)).await;
        assert!(matches!(tools.recv().await, ServerMessage::Hello(_)));
        for message in [&list, &remove] {
            tools.send(message).await;
            assert!(matches!(
                tools.recv().await,
                ServerMessage::Error {
                    code: ErrorCode::NotAllowed,
                    ..
                }
            ));
        }

        let (mut app, _) = TestClient::greeted(&served.socket).await;
        for message in [&list, &remove] {
            app.send(message).await;
            assert!(matches!(
                app.recv().await,
                ServerMessage::ActionResult {
                    outcome: ActionOutcome::Refused { .. },
                    ..
                }
            ));
        }
    }

    #[tokio::test]
    async fn only_the_app_may_use_a_codex_reset() {
        let served = serve();
        let reset = || ClientMessage::UseResetCredit {
            key: IdempotencyKey::generate(),
            account: maxhq_protocol::ids::AccountId::derive("codex"),
        };

        // Agents' tools, the dispatcher's among them, can't.
        let mut tools = TestClient::connect(&served.socket).await;
        let ClientMessage::Hello(mut hello) = hello(ProtocolRange::OURS) else {
            unreachable!()
        };
        hello.role = ClientRole::Mcp;
        tools.send(&ClientMessage::Hello(hello)).await;
        assert!(matches!(tools.recv().await, ServerMessage::Hello(_)));
        tools.send(&reset()).await;
        assert!(matches!(
            tools.recv().await,
            ServerMessage::Error {
                code: ErrorCode::NotAllowed,
                ..
            }
        ));

        // Nor an app that proved no key.
        let (mut unproven, _) = TestClient::unproven(&served.socket).await;
        let message = reset();
        unproven.send(&message).await;
        let key = message.action_key().unwrap();
        let outcome = unproven.outcome(key).await;
        assert!(
            matches!(&outcome, ActionOutcome::Refused { reason } if reason != NOT_FOR_RESETS),
            "{outcome:?}"
        );

        // The app gets through to the spend, which knows no such account
        // here, so nothing starts.
        let (mut app, _) = TestClient::greeted(&served.socket).await;
        let message = reset();
        app.send(&message).await;
        let outcome = app.outcome(message.action_key().unwrap()).await;
        assert_eq!(
            outcome,
            ActionOutcome::Refused {
                reason: "maxd doesn't know that account.".into()
            }
        );
    }

    #[tokio::test]
    async fn only_the_app_may_set_or_delete_acp_agents() {
        let served = serve();
        let id = maxhq_protocol::ids::AcpAgentId::derive("gemini");
        let agent = maxhq_protocol::acp::AcpAgent {
            id,
            device: DeviceId::derive("devbox"),
            name: "Gemini".to_owned(),
            program: "gemini".to_owned(),
            args: Vec::new(),
            env: Vec::new(),
            containers: false,
            problem: None,
            skills: Default::default(),
        };
        let set = ClientMessage::SetAcpAgent {
            key: IdempotencyKey::derive("set"),
            agent,
            env: Vec::new(),
        };
        let delete = ClientMessage::DeleteAcpAgent {
            key: IdempotencyKey::derive("delete"),
            id,
        };

        // An agent tools connection never reaches the store, so an agent
        // can't make maxd run a program it names.
        let mut tools = TestClient::connect(&served.socket).await;
        let ClientMessage::Hello(mut hello) = hello(ProtocolRange::OURS) else {
            unreachable!()
        };
        hello.role = ClientRole::Mcp;
        tools.send(&ClientMessage::Hello(hello)).await;
        assert!(matches!(tools.recv().await, ServerMessage::Hello(_)));
        for message in [&set, &delete] {
            tools.send(message).await;
            assert!(matches!(
                tools.recv().await,
                ServerMessage::Error {
                    code: ErrorCode::NotAllowed,
                    ..
                }
            ));
        }

        // The app's reach it, which isn't open in these tests.
        let (mut app, _) = TestClient::greeted(&served.socket).await;
        for message in [&set, &delete] {
            app.send(message).await;
            let ServerMessage::ActionResult { outcome, .. } = app.recv().await else {
                panic!("expected an answer");
            };
            assert_eq!(outcome, not_yet());
        }
    }

    #[tokio::test]
    async fn actions_reach_the_session_manager() {
        let served = serve();
        let (mut client, _) = TestClient::greeted(&served.socket).await;
        let key = IdempotencyKey::derive("stop");
        client
            .send(&ClientMessage::StopAgent {
                key,
                agent: AgentId::derive("a"),
            })
            .await;
        assert_eq!(
            client.recv().await,
            ServerMessage::ActionResult {
                key,
                outcome: ActionOutcome::UnknownAgent,
            }
        );
    }

    #[tokio::test]
    async fn a_load_read_answers_with_this_devices_load() {
        let served = serve();
        let (mut client, hello) = TestClient::greeted(&served.socket).await;
        assert!(hello.capabilities.load);
        let key = IdempotencyKey::derive("load");
        client.send(&ClientMessage::ReadLoad { key }).await;
        let ServerMessage::ActionResult {
            key: answered,
            outcome: ActionOutcome::Load(load),
        } = client.recv().await
        else {
            panic!("expected the load");
        };
        assert_eq!(answered, key);
        assert!(load.cpu_count > 0);
        assert!(load.mem_total >= load.mem_available);
    }

    #[tokio::test]
    async fn a_phone_cant_do_what_only_the_mac_does_until_its_switch_is_on() {
        use base64::Engine;
        use maxhq_protocol::acp::{AcpAgent, AcpSkills};
        use maxhq_protocol::agents::{Actor, LeaveContainer, Limits, PeerBook, Permission};
        use maxhq_protocol::hosts::MoveStep;
        use maxhq_protocol::ids::{
            AccountId, AcpAgentId, AdmissionId, AttachmentId, CredentialId, ForwardId, KeyId,
            ModelServerId, MoveId, PermissionId, PortId, SignInId, SkillProposalId, SkillSourceId,
            StreamId, TaskId, TestHostId,
        };
        use maxhq_protocol::message::{AgentPlace, AgentSpec, ToolTarget, UpdateKind};
        use maxhq_protocol::model::{
            DeviceSettingChange, PermissionMode, Tool, Vendor, WorkspaceSpec,
        };
        use maxhq_protocol::processes::ProcessSignal;
        use maxhq_protocol::secret::Secret;
        use maxhq_protocol::skills::{ProposalAction, SkillLock, SourceKind, SourceTrust};
        use maxhq_protocol::state::{ToolKind, ToolPlace};
        use maxhq_protocol::tasks::{HookAddress, Maker, Task, TaskTarget, Trigger};
        use russh::keys::ssh_key::private::Ed25519Keypair;
        use russh::keys::ssh_key::PublicKey;

        let phones = crate::phones::for_tests();
        let served = serve();
        let me = served.hub.device().id;
        // Every test shares one phone store, so this phone's key is its own:
        // with another test's key, that test removing its phone would take
        // this one away mid-run.
        const SEED: [u8; 32] = [231; 32];
        let ssh_public = PublicKey::from(Ed25519Keypair::from_seed(&SEED).public)
            .to_openssh()
            .unwrap();
        let phone = PhoneId::derive(&ssh_public);
        let (mut app, _) = TestClient::greeted(&served.socket).await;
        app.send(&ClientMessage::AddPhone {
            key: IdempotencyKey::derive("add"),
            phone,
            name: "iPhone".into(),
            ssh_public: ssh_public.clone(),
            push_public: base64::engine::general_purpose::STANDARD.encode([8u8; 32]),
        })
        .await;
        assert!(matches!(
            app.recv().await,
            ServerMessage::ActionResult {
                outcome: ActionOutcome::Done,
                ..
            }
        ));
        let mut own = TestClient::connect(&served.socket).await;
        own.send(&ClientMessage::Phone { phone }).await;
        own.send(&hello(ProtocolRange::OURS)).await;
        let ServerMessage::Hello(server) = own.recv().await else {
            panic!("expected a hello");
        };
        let proved = own
            .prove_as(Kind::Phone, &SEED, &server, &hello(ProtocolRange::OURS))
            .await;
        assert_eq!(proved, ActionOutcome::Done);

        let dir = TempDir::new();
        let private = dir.path().join("id_ed25519");
        fs::write(&private, "not for a phone").unwrap();
        let path = private.to_str().unwrap().to_owned();
        let key = IdempotencyKey::derive;
        let secret = || Secret::new("not-a-real-secret".into());
        let permission = || {
            Box::new(Permission {
                id: PermissionId::derive("minted"),
                holder: Actor::Key {
                    key: KeyId::derive("minted"),
                },
                home: me,
                devices: vec![me],
                vendors: vec![Vendor::Claude],
                accounts: None,
                places: None,
                depth: 3,
                limits: Limits::default(),
                test_kinds: Vec::new(),
                leave_container: LeaveContainer::Ask,
            })
        };
        let tool = || ToolKind::Agent { tool: Tool::Claude };
        let spec = || AgentSpec {
            effort: None,
            vendor: Vendor::Claude,
            account: AccountId::derive("account"),
            place: AgentPlace::Folder {
                path: dir.path().display().to_string(),
            },
            model: None,
            permission: None,
            title: None,
            first: None,
            branch: None,
            origin: None,
        };
        let workspace = WorkspaceId::derive("workspace");
        let denied = [
            ClientMessage::StartAgent {
                key: key("start"),
                spec: spec(),
            },
            ClientMessage::SaveTask {
                key: key("task"),
                task: Box::new(Task {
                    id: TaskId::derive("task"),
                    name: "stolen".into(),
                    prompt: "run".into(),
                    trigger: Trigger::Every { minutes: 1 },
                    target: TaskTarget::New {
                        device: me,
                        spec: Box::new(spec()),
                    },
                    enabled: true,
                    permission: PermissionId::derive("minted"),
                    made_by: Maker::User,
                    next_run: None,
                    last_run: None,
                    approval_asked: None,
                    by_dispatcher: false,
                }),
            },
            ClientMessage::SetDeviceSettings {
                key: key("settings"),
                settings: crate::hub::default_settings(served.hub.device()),
            },
            ClientMessage::UpdateDeviceSettings {
                key: key("setting"),
                device: me,
                changes: vec![DeviceSettingChange::DefaultPermission {
                    value: PermissionMode::FullAccess,
                }],
            },
            ClientMessage::InstallAcpAgent {
                key: key("acp-install"),
                agent: "stolen".into(),
                version: "1.0.0".into(),
                place: ToolPlace::Host,
                confirm_unchecked: true,
            },
            ClientMessage::CreateWorkspace {
                key: key("create"),
                spec: WorkspaceSpec {
                    project: ProjectId::derive("project"),
                    device: me,
                    name: None,
                    start_at_boot: false,
                },
            },
            ClientMessage::DeleteWorkspace {
                key: key("delete"),
                workspace,
                discard_unpushed: true,
            },
            ClientMessage::CloneFolder {
                key: key("clone"),
                project: ProjectId::derive("project"),
                device: me,
                parent: "/home/dev/src".into(),
                name: "stolen".into(),
            },
            ClientMessage::CloneRetry {
                key: key("clone-retry"),
                workspace,
            },
            ClientMessage::CloneRemove {
                key: key("clone-remove"),
                workspace,
            },
            ClientMessage::PlaceStaged {
                key: key("place"),
                workspace,
                attachment: AttachmentId::derive("place"),
                overwrite: true,
            },
            ClientMessage::ListFolder {
                key: key("list-folder"),
                device: me,
                path: dir.path().display().to_string(),
                files: true,
                hidden: true,
                filter: None,
            },
            ClientMessage::MakeFolder {
                key: key("make-folder"),
                device: me,
                path: dir.path().join("stolen").display().to_string(),
            },
            ClientMessage::GuestSshOpen {
                stream: StreamId::generate(),
                workspace,
            },
            ClientMessage::TerminalOpen {
                key: key("shell"),
                agent: AgentId::derive("a"),
                kind: TerminalKind::Shell,
                cols: 80,
                rows: 24,
            },
            ClientMessage::OpenForward {
                key: key("forward"),
                port: PortId::derive("port"),
                local_port: 8080,
            },
            ClientMessage::TunnelOpen {
                stream: StreamId::generate(),
                forward: ForwardId::derive("forward"),
            },
            ClientMessage::SignalProcess {
                key: key("signal"),
                agent: AgentId::derive("a"),
                pid: 1,
                start: 0,
                signal: ProcessSignal::Kill,
            },
            ClientMessage::FetchFiles {
                key: key("fetch-files"),
                transfer: TransferId::generate(),
                workspace,
                paths: vec!["/".into()],
            },
            ClientMessage::ApplyHost {
                key: key("host"),
                items: vec!["ssh".into()],
            },
            ClientMessage::RestoreSnapshot {
                key: key("restore"),
                workspace,
                snapshot: "snap".into(),
                as_new: false,
                discard_unsaved: true,
            },
            ClientMessage::RunMoveStep {
                key: key("move"),
                moving: MoveId::derive("move"),
                step: MoveStep::Retire,
            },
            ClientMessage::BuildTestHost {
                key: key("test-host-build"),
                host: TestHostId::derive("host"),
            },
            ClientMessage::ResetTestHost {
                key: key("test-host-reset"),
                host: TestHostId::derive("host"),
            },
            ClientMessage::SkillsApply {
                key: key("skills"),
                project: ProjectId::derive("project"),
            },
            ClientMessage::SkillsSetProject {
                key: key("skills-lock"),
                project: ProjectId::derive("project"),
                lock: SkillLock {
                    skills: Vec::new(),
                    committed: false,
                    pack: None,
                },
            },
            ClientMessage::SkillsSetAlways {
                key: key("skills-always"),
                skills: Vec::new(),
            },
            ClientMessage::SkillsGlobalsApply {
                key: key("skills-tidy"),
                plan_hash: "plan".into(),
                picks: Vec::new(),
            },
            ClientMessage::SkillsGlobalsUndo {
                key: key("skills-untidy"),
            },
            ClientMessage::SkillProposalAction {
                key: key("skills-approve"),
                proposal: SkillProposalId::derive("proposal"),
                action: ProposalAction::Approve,
                push: true,
                fork: None,
                elsewhere: None,
                seen: None,
            },
            ClientMessage::SkillSourceAdd {
                key: key("source-add"),
                name: "stolen".into(),
                kind: SourceKind::Git {
                    url: "https://example.com/owner/repo.git".into(),
                },
                default_ref: None,
                trust: SourceTrust::Own,
            },
            ClientMessage::SkillSourceFetch {
                key: key("source-fetch"),
                source: SkillSourceId::derive("source"),
            },
            ClientMessage::SkillSourcePush {
                key: key("source-push"),
                source: SkillSourceId::derive("source"),
            },
            ClientMessage::SkillSourceRemove {
                key: key("source-remove"),
                source: SkillSourceId::derive("source"),
            },
            ClientMessage::SkillsTakeLibrary {
                key: key("skills-take"),
                from: "devbox".into(),
                sources: Vec::new(),
            },
            ClientMessage::StoreClaudeToken {
                key: key("claude"),
                origin: AccountId::derive("account"),
                label: "stolen".into(),
                token: secret(),
            },
            ClientMessage::StoreCursorKey {
                key: key("cursor"),
                label: "stolen".into(),
                token: secret(),
            },
            ClientMessage::SetGithubApp {
                key: key("github"),
                app_id: 1,
                private_key: secret(),
            },
            ClientMessage::BeginGithubSignIn {
                key: key("github-sign-in"),
                client_id: None,
            },
            ClientMessage::SetAcpAgent {
                key: key("acp"),
                agent: AcpAgent {
                    id: AcpAgentId::derive("acp"),
                    device: me,
                    name: "stolen".into(),
                    program: "/bin/sh".into(),
                    args: Vec::new(),
                    env: Vec::new(),
                    containers: false,
                    problem: None,
                    skills: AcpSkills::default(),
                },
                env: Vec::new(),
            },
            ClientMessage::DeleteAcpAgent {
                key: key("acp-delete"),
                id: AcpAgentId::derive("acp"),
            },
            ClientMessage::ForgetCredential {
                key: key("forget"),
                credential: CredentialId::derive("credential"),
            },
            ClientMessage::BeginChatGptSignIn {
                key: key("sign-in"),
            },
            ClientMessage::FinishChatGptSignIn {
                key: key("signed-in"),
                sign_in: SignInId::derive("round"),
                code: secret(),
                state: "state".into(),
                client_id: None,
            },
            ClientMessage::BeginCliLogin {
                key: key("cli-login"),
                account: AccountId::derive("account"),
                device_code: true,
            },
            ClientMessage::FinishCliLogin {
                key: key("cli-code"),
                sign_in: SignInId::derive("round"),
                code: secret(),
            },
            ClientMessage::SetHookSecret {
                key: key("hook-secret"),
                task: TaskId::derive("task"),
                secret: secret(),
            },
            ClientMessage::SetHookAddress {
                key: key("hook-address"),
                address: HookAddress::Funnel,
            },
            ClientMessage::SetApnsKey {
                key: key("apns"),
                key_id: "key".into(),
                team_id: "team".into(),
                private_key: secret(),
            },
            ClientMessage::InstallUpdate {
                key: key("update"),
                staged: AttachmentId::derive("maxd"),
                kind: UpdateKind::Maxd,
                sha256: "0".repeat(64),
                release: None,
                unsigned: false,
            },
            ClientMessage::InstallTool {
                key: key("install"),
                place: ToolPlace::Host,
                tool: tool(),
                version: None,
            },
            ClientMessage::UpdateTools {
                key: key("update-tools"),
                tools: vec![ToolTarget {
                    tool: Tool::Claude,
                    place: ToolPlace::Host,
                }],
            },
            ClientMessage::RemoveTool {
                key: key("remove"),
                place: ToolPlace::Host,
                tool: tool(),
            },
            ClientMessage::UploadBegin {
                key: key("upload"),
                transfer: TransferId::generate(),
                target: UploadTarget::Device,
                source: UploadSource::DevicePath { path: path.clone() },
            },
            ClientMessage::StageAttachment {
                key: key("stage"),
                source: AttachmentSource::Path { path: path.clone() },
            },
            ClientMessage::FetchBegin {
                key: key("anyway"),
                transfer: TransferId::generate(),
                agent: AgentId::derive("a"),
                path: dir.path().join("plan.md").to_str().unwrap().to_owned(),
                anyway: true,
            },
            // Adding, probing and removing a server URL, and downloading
            // and deleting a model, are the Mac's.
            ClientMessage::ModelAction {
                key: key("model-add"),
                action: ModelAction::AddServer {
                    label: "stolen".into(),
                    url: "http://gpu-box.example.com:11434".into(),
                    token: None,
                },
            },
            ClientMessage::ModelAction {
                key: key("model-probe"),
                action: ModelAction::ProbeServer {
                    url: "http://gpu-box.example.com:11434".into(),
                    token: None,
                },
            },
            ClientMessage::ModelAction {
                key: key("model-remove"),
                action: ModelAction::RemoveServer {
                    server: ModelServerId::derive("server"),
                },
            },
            ClientMessage::ModelAction {
                key: key("model-download"),
                action: ModelAction::Download {
                    model: "qwen3.6-35b-a3b".into(),
                    file: "Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf".into(),
                },
            },
            ClientMessage::ModelAction {
                key: key("model-delete"),
                action: ModelAction::DeleteModel {
                    model: "qwen3.6-35b-a3b".into(),
                    file: "Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf".into(),
                },
            },
            // A resume starts a download the Mac never began just as well.
            ClientMessage::ModelAction {
                key: key("model-resume"),
                action: ModelAction::ResumeDownload {
                    model: "qwen3.6-35b-a3b".into(),
                    file: "Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf".into(),
                },
            },
            // Nor does it let an agent act without asking.
            ClientMessage::SetPermission {
                key: key("auto"),
                agent: AgentId::derive("a"),
                mode: PermissionMode::Auto,
            },
            ClientMessage::SetPermission {
                key: key("full-access"),
                agent: AgentId::derive("a"),
                mode: PermissionMode::FullAccess,
            },
            // Removing a rule Cursor saved changes what it runs unasked.
            ClientMessage::CursorRules {
                key: key("cursor-rule-remove"),
                device: DeviceId::derive("devbox"),
                call: CursorRuleCall::Remove {
                    rule: "Shell(git status)".into(),
                },
            },
        ];
        // What hands out access, which a phone's switch never lifts.
        let handed_out = [
            ClientMessage::BeginPhonePair {
                key: key("pair"),
                book: Vec::new(),
                mac: None,
            },
            ClientMessage::AddPhone {
                key: key("add-another"),
                phone: PhoneId::derive("another"),
                name: "stolen".into(),
                ssh_public: ssh_public.clone(),
                push_public: base64::engine::general_purpose::STANDARD.encode([8u8; 32]),
            },
            ClientMessage::AuthorizePeer {
                key: key("peer"),
                device: DeviceId::derive("stolen"),
                public_key: ssh_public.clone(),
            },
            ClientMessage::SetPeerBook {
                key: key("peers"),
                book: PeerBook {
                    ssh: false,
                    peers: Vec::new(),
                },
            },
            ClientMessage::RotateHookToken {
                key: key("rotate"),
                task: TaskId::derive("task"),
            },
            ClientMessage::MintKey {
                key: key("mint"),
                name: "stolen".into(),
                permission: permission(),
            },
            ClientMessage::GrantPermission {
                key: key("grant"),
                permission: permission(),
            },
            ClientMessage::SetGuestSshKey {
                key: key("guest-key"),
                public_key: ssh_public,
            },
            ClientMessage::EnrollAppKey {
                key: key("enroll"),
                label: "stolen".into(),
            },
            ClientMessage::RemoveAppKey {
                key: key("remove-app-key"),
                public_key: crate::app_keys::testing::public_key(),
            },
            ClientMessage::AnswerAdmission {
                key: key("admit"),
                admission: AdmissionId::derive("admission"),
                allow: true,
            },
            ClientMessage::SetPhoneBook {
                key: key("phone-book"),
                phone,
                book: Vec::new(),
                mac: None,
            },
            ClientMessage::SetPhoneEverything {
                key: key("everything"),
                phone,
                everything: true,
            },
        ];
        // An action's refusal is its outcome, under its key, so the phone
        // stops waiting and shows why. Anything else gets an error.
        let refused_with =
            |sent: &ClientMessage, answer: &ServerMessage, refusal: &str| match sent.action_key() {
                Some(sent) => matches!(
                    answer,
                    ServerMessage::ActionResult {
                        key,
                        outcome: ActionOutcome::Refused { reason },
                    } if *key == sent && reason == refusal
                ),
                None => matches!(
                    answer,
                    ServerMessage::Error {
                        code: ErrorCode::NotAllowed,
                        message,
                    } if message == refusal
                ),
            };
        assert!(denied.iter().any(|message| message.action_key().is_none()));
        const NO_KEYS: &str = "A phone can't let other keys in. Do that from the Mac.";
        for message in &denied {
            own.send(message).await;
            let answer = own.recv().await;
            assert!(
                refused_with(message, &answer, PHONE_CANT),
                "{message:?} got {answer:?}"
            );
        }
        for message in &handed_out {
            own.send(message).await;
            let answer = own.recv().await;
            assert!(
                refused_with(message, &answer, NO_KEYS),
                "{message:?} got {answer:?}"
            );
        }
        assert!(!phones.everything(phone), "the phone can't turn it on");
        // Nor use a Codex reset, which only the app does.
        let reset = || ClientMessage::UseResetCredit {
            key: IdempotencyKey::generate(),
            account: AccountId::derive("codex"),
        };
        let message = reset();
        own.send(&message).await;
        let answer = own.recv().await;
        assert!(
            refused_with(&message, &answer, NOT_FOR_RESETS),
            "{answer:?}"
        );
        // A sign-in the Mac began can still be read and stopped from it, an
        // agent can still be set to a mode that asks, and the phone can
        // still have this device look at what it hosts.
        let round = SignInId::derive("round");
        for message in [
            ClientMessage::CheckCliLogin {
                key: key("cli-check"),
                sign_in: round,
            },
            ClientMessage::CancelCliLogin {
                key: key("cli-cancel"),
                sign_in: round,
            },
            ClientMessage::SetPermission {
                key: key("ask"),
                agent: AgentId::derive("a"),
                mode: PermissionMode::Ask,
            },
            ClientMessage::SetPermission {
                key: key("accept-edits"),
                agent: AgentId::derive("a"),
                mode: PermissionMode::AcceptEdits,
            },
            ClientMessage::CheckTestHosts {
                key: key("test-host-check"),
            },
            // It may load, unload and test a model server, and pause a
            // download the Mac started.
            ClientMessage::ModelAction {
                key: key("model-test"),
                action: ModelAction::TestServer {
                    server: ModelServerId::derive("server"),
                },
            },
            ClientMessage::ModelAction {
                key: key("model-load"),
                action: ModelAction::Load {
                    server: ModelServerId::derive("server"),
                    model: "qwen3.6-35b-a3b@ud-q4_k_xl".into(),
                },
            },
            ClientMessage::ModelAction {
                key: key("model-unload"),
                action: ModelAction::Unload {
                    server: ModelServerId::derive("server"),
                    model: "qwen3.6-35b-a3b@ud-q4_k_xl".into(),
                },
            },
            ClientMessage::ModelAction {
                key: key("model-pause"),
                action: ModelAction::PauseDownload {
                    model: "qwen3.6-35b-a3b".into(),
                    file: "Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf".into(),
                },
            },
            // It may read the rules Cursor saved. Another device's, so this
            // home folder stays unread.
            ClientMessage::CursorRules {
                key: key("cursor-rule-list"),
                device: DeviceId::derive("devbox"),
                call: CursorRuleCall::List,
            },
        ] {
            own.send(&message).await;
            let answer = own.recv().await;
            assert!(
                !refused_with(&message, &answer, PHONE_CANT),
                "{message:?} got {answer:?}"
            );
        }

        // Nor can agents' tools.
        let mut tools = TestClient::connect(&served.socket).await;
        let ClientMessage::Hello(mut greeting) = hello(ProtocolRange::OURS) else {
            unreachable!()
        };
        greeting.role = ClientRole::Mcp;
        tools.send(&ClientMessage::Hello(greeting)).await;
        assert!(matches!(tools.recv().await, ServerMessage::Hello(_)));
        tools.send(&handed_out[handed_out.len() - 1]).await;
        assert!(matches!(
            tools.recv().await,
            ServerMessage::Error {
                code: ErrorCode::NotAllowed,
                ..
            }
        ));
        assert!(!phones.everything(phone));

        // The Mac turns it on, and the phone may then do what only the Mac
        // did, but still not hand out access, nor turn its switch off.
        let switch = |everything: bool| ClientMessage::SetPhoneEverything {
            key: IdempotencyKey::generate(),
            phone,
            everything,
        };
        app.send(&switch(true)).await;
        assert!(matches!(
            app.recv().await,
            ServerMessage::ActionResult {
                outcome: ActionOutcome::Done,
                ..
            }
        ));
        assert!(phones.everything(phone));
        for message in handed_out.iter().cloned().chain([switch(false)]) {
            own.send(&message).await;
            let answer = own.recv().await;
            assert!(
                refused_with(&message, &answer, NO_KEYS),
                "{message:?} got {answer:?}"
            );
        }
        assert!(phones.everything(phone));
        // Nor use a Codex reset, whatever its switch says.
        let message = reset();
        own.send(&message).await;
        let answer = own.recv().await;
        assert!(
            refused_with(&message, &answer, NOT_FOR_RESETS),
            "{answer:?}"
        );
        // Nor send a maxd no release signed, which only the Mac builds.
        let unsigned = ClientMessage::InstallUpdate {
            key: key("unsigned"),
            staged: AttachmentId::derive("maxd"),
            kind: UpdateKind::Maxd,
            sha256: "0".repeat(64),
            release: None,
            unsigned: true,
        };
        own.send(&unsigned).await;
        let answer = own.recv().await;
        assert!(
            refused_with(&unsigned, &answer, PHONE_UNSIGNED),
            "{answer:?}"
        );
        let stage = || ClientMessage::StageAttachment {
            key: IdempotencyKey::generate(),
            source: AttachmentSource::Path { path: path.clone() },
        };
        own.send(&stage()).await;
        assert!(matches!(
            own.recv().await,
            ServerMessage::ActionResult {
                outcome: ActionOutcome::Staged { .. },
                ..
            }
        ));
        // And set an agent to Full access.
        let full = ClientMessage::SetPermission {
            key: IdempotencyKey::generate(),
            agent: AgentId::derive("a"),
            mode: PermissionMode::FullAccess,
        };
        own.send(&full).await;
        let answer = own.recv().await;
        assert!(!refused_with(&full, &answer, PHONE_CANT), "{answer:?}");
        // And list and make folders, as it may start an agent in one.
        own.send(&ClientMessage::ListFolder {
            key: IdempotencyKey::generate(),
            device: me,
            path: dir.path().display().to_string(),
            files: true,
            hidden: false,
            filter: None,
        })
        .await;
        assert!(matches!(
            own.recv().await,
            ServerMessage::ActionResult {
                outcome: ActionOutcome::FolderListed { .. },
                ..
            }
        ));
        own.send(&ClientMessage::MakeFolder {
            key: IdempotencyKey::generate(),
            device: me,
            path: dir.path().join("made").display().to_string(),
        })
        .await;
        assert!(matches!(
            own.recv().await,
            ServerMessage::ActionResult {
                outcome: ActionOutcome::Done,
                ..
            }
        ));

        // Off again, the phone is back to what it may do on its own.
        app.send(&switch(false)).await;
        assert!(matches!(
            app.recv().await,
            ServerMessage::ActionResult {
                outcome: ActionOutcome::Done,
                ..
            }
        ));
        let staged = stage();
        own.send(&staged).await;
        assert!(refused_with(&staged, &own.recv().await, PHONE_CANT));

        // Pasted bytes still stage from a phone, and the path still does from
        // the Mac.
        own.send(&ClientMessage::StageAttachment {
            key: key("pasted"),
            source: AttachmentSource::Bytes {
                name: "a.txt".into(),
                mime: "text/plain".into(),
                data: base64::engine::general_purpose::STANDARD.encode("pasted"),
            },
        })
        .await;
        assert!(matches!(
            own.recv().await,
            ServerMessage::ActionResult {
                outcome: ActionOutcome::Staged { .. },
                ..
            }
        ));
        app.send(&stage()).await;
        assert!(matches!(
            app.recv().await,
            ServerMessage::ActionResult {
                outcome: ActionOutcome::Staged { .. },
                ..
            }
        ));
    }

    #[tokio::test]
    async fn an_update_is_refused_where_maxd_cant_install_one() {
        let served = serve();
        let (mut client, hello) = TestClient::greeted(&served.socket).await;
        assert!(!hello.capabilities.updates);
        let key = IdempotencyKey::derive("update");
        client
            .send(&ClientMessage::InstallUpdate {
                key,
                staged: maxhq_protocol::ids::AttachmentId::derive("maxd"),
                kind: maxhq_protocol::message::UpdateKind::Maxd,
                sha256: "0".repeat(64),
                release: None,
                unsigned: false,
            })
            .await;
        let ServerMessage::ActionResult {
            outcome: ActionOutcome::Refused { reason },
            ..
        } = client.recv().await
        else {
            panic!("expected a refusal");
        };
        assert_eq!(reason, "This maxd can't install updates.");
    }

    #[tokio::test]
    async fn pr_requests_are_refused_in_words() {
        let served = serve();
        let (mut client, hello) = TestClient::greeted(&served.socket).await;
        assert!(hello.capabilities.pulls && hello.capabilities.github_user);
        let asks = [
            ClientMessage::BeginGithubSignIn {
                key: IdempotencyKey::derive("sign in"),
                client_id: None,
            },
            ClientMessage::GetPull {
                key: IdempotencyKey::derive("read"),
                repo: "owner/repo".into(),
                number: 12,
            },
        ];
        // The sign-in waits for the secret store, which this test never
        // opens, and without it there's no App to read the PR with.
        let no_app = crate::secrets::GithubError::NoApp.to_string();
        let reasons = ["not yet", no_app.as_str()];
        for (ask, reason) in asks.iter().zip(reasons) {
            client.send(ask).await;
            let ServerMessage::ActionResult {
                outcome: ActionOutcome::Refused { reason: said },
                ..
            } = client.recv().await
            else {
                panic!("expected a refusal");
            };
            assert_eq!(said, reason);
        }
    }

    #[tokio::test]
    async fn test_host_requests_are_refused_in_words() {
        let served = serve();
        let (mut client, hello) = TestClient::greeted(&served.socket).await;
        // The flag is on, and a maxd whose store isn't open says so.
        assert!(hello.capabilities.test_hosts);
        let lease = maxhq_protocol::ids::LeaseId::derive("lease");
        let asks = [
            ClientMessage::CheckTestHosts {
                key: IdempotencyKey::derive("check"),
            },
            ClientMessage::EndLease {
                key: IdempotencyKey::derive("end"),
                lease,
            },
            ClientMessage::ReadLeaseScreen {
                key: IdempotencyKey::derive("screen"),
                lease,
            },
        ];
        for ask in asks {
            client.send(&ask).await;
            let ServerMessage::ActionResult {
                outcome: ActionOutcome::Refused { reason },
                ..
            } = client.recv().await
            else {
                panic!("expected a refusal");
            };
            assert_eq!(reason, crate::leases::NOT_YET);
        }
    }

    #[tokio::test]
    async fn workflow_skills_and_phone_requests_are_refused_in_words() {
        let served = serve();
        let (mut client, hello) = TestClient::greeted(&served.socket).await;
        assert!(!hello.capabilities.workflows && hello.capabilities.skills);
        assert!(hello.capabilities.phones);
        let run = maxhq_protocol::workflow::RunKey {
            device: DeviceId::derive("devbox"),
            project: ProjectId::derive("git:github.com/owner/repo"),
            workflow: "devflow".into(),
            run: "run-1".into(),
        };
        let asks = [
            (
                ClientMessage::NodeDetail {
                    key: IdempotencyKey::derive("detail"),
                    run: run.clone(),
                    node: "plan".into(),
                },
                crate::workflow::NOT_YET,
            ),
            (
                ClientMessage::SkillsApply {
                    key: IdempotencyKey::derive("apply"),
                    project: run.project,
                },
                crate::skills::NOT_YET,
            ),
            (
                ClientMessage::SkillProposalAction {
                    key: IdempotencyKey::derive("proposal"),
                    proposal: maxhq_protocol::ids::SkillProposalId::derive("proposal"),
                    action: maxhq_protocol::skills::ProposalAction::Reject,
                    push: false,
                    fork: None,
                    elsewhere: None,
                    seen: None,
                },
                crate::skills::NOT_YET,
            ),
            (
                ClientMessage::SkillProposalDiff {
                    key: IdempotencyKey::derive("diff"),
                    proposal: maxhq_protocol::ids::SkillProposalId::derive("proposal"),
                    elsewhere: None,
                },
                crate::skills::NOT_YET,
            ),
        ];
        // An agent's tools connection can't answer a proposal.
        let mut tools = TestClient::connect(&served.socket).await;
        let ClientMessage::Hello(mut greeting) = super::tests::hello(ProtocolRange::OURS) else {
            unreachable!()
        };
        greeting.role = ClientRole::Mcp;
        tools.send(&ClientMessage::Hello(greeting)).await;
        assert!(matches!(tools.recv().await, ServerMessage::Hello(_)));
        tools.send(&asks[2].0).await;
        assert!(matches!(
            tools.recv().await,
            ServerMessage::Error {
                code: ErrorCode::NotAllowed,
                ..
            }
        ));
        for (ask, words) in asks {
            client.send(&ask).await;
            let ServerMessage::ActionResult {
                outcome: ActionOutcome::Refused { reason },
                ..
            } = client.recv().await
            else {
                panic!("expected a refusal");
            };
            assert_eq!(reason, words);
        }
        client.send(&ClientMessage::OpenWorkflow { run }).await;
        assert_eq!(
            client.recv().await,
            ServerMessage::Error {
                code: ErrorCode::NotAllowed,
                message: crate::workflow::NOT_YET.into(),
            }
        );
    }

    #[tokio::test]
    async fn move_and_install_requests_are_refused_in_words() {
        let served = serve();
        let (mut client, hello) = TestClient::greeted(&served.socket).await;
        let offered = hello.capabilities;
        assert!(offered.host_spec);
        // Snapshots go with container workspaces, which need Incus.
        assert_eq!(offered.snapshots, offered.workspaces);
        assert!(offered.moves);
        assert!(!offered.tool_installs);
        let workspace = maxhq_protocol::ids::WorkspaceId::derive("ws-alpha");
        client
            .send(&ClientMessage::ListSnapshots {
                key: IdempotencyKey::derive("snapshots"),
                workspace,
            })
            .await;
        assert!(matches!(
            client.recv().await,
            ServerMessage::ActionResult {
                outcome: ActionOutcome::Refused { .. },
                ..
            }
        ));
        // Moves reach the moves module, which a maxd without Incus doesn't
        // run.
        let moving = maxhq_protocol::ids::MoveId::derive("ws-alpha");
        let asks = [
            ClientMessage::PlanMove {
                key: IdempotencyKey::derive("plan"),
                instance: "ws-alpha".into(),
            },
            ClientMessage::RunMoveStep {
                key: IdempotencyKey::derive("step"),
                moving,
                step: maxhq_protocol::hosts::MoveStep::Copy,
            },
            ClientMessage::MoveBack {
                key: IdempotencyKey::derive("back"),
                moving,
            },
            ClientMessage::RetireMove {
                key: IdempotencyKey::derive("retire"),
                moving,
            },
        ];
        for ask in asks {
            client.send(&ask).await;
            let ServerMessage::ActionResult {
                outcome: ActionOutcome::Refused { reason },
                ..
            } = client.recv().await
            else {
                panic!("expected a refusal");
            };
            assert_eq!(reason, crate::moves::NO_MOVES);
        }
        // Installs and licences go to the fleet, and a maxd that doesn't
        // track tools says so.
        let asks = [
            ClientMessage::InstallTool {
                key: IdempotencyKey::derive("install"),
                place: maxhq_protocol::state::ToolPlace::Host,
                tool: maxhq_protocol::state::ToolKind::Helper {
                    tool: maxhq_protocol::state::HelperTool::Gh,
                },
                version: None,
            },
            ClientMessage::AcceptLicence {
                key: IdempotencyKey::derive("licence"),
                licence: "android-sdk-license".into(),
                text_sha256: "0".repeat(64),
            },
        ];
        for ask in asks {
            client.send(&ask).await;
            let ServerMessage::ActionResult {
                outcome: ActionOutcome::Refused { reason },
                ..
            } = client.recv().await
            else {
                panic!("expected a refusal");
            };
            assert_eq!(reason, "This maxd can't install tools.");
        }
    }

    #[tokio::test]
    async fn answers_reach_the_session_manager() {
        let served = serve();
        let (mut client, _) = TestClient::greeted(&served.socket).await;
        let key = IdempotencyKey::derive("answer");
        client
            .send(&ClientMessage::Answer {
                idempotency_key: key,
                event: AttentionId::derive("nothing"),
                answer: AttentionAnswer::Text { text: "yes".into() },
                refs: Vec::new(),
            })
            .await;
        assert_eq!(
            client.recv().await,
            ServerMessage::AnswerResult {
                idempotency_key: key,
                outcome: AnswerOutcome::UnknownEvent,
            }
        );
    }

    #[tokio::test]
    async fn an_opened_timeline_starts_with_a_reset() {
        let served = serve();
        let (mut client, _) = TestClient::greeted(&served.socket).await;
        let agent = AgentId::derive("a");
        client
            .send(&ClientMessage::OpenTimeline {
                agent,
                since: None,
                limit: 50,
                lead: None,
            })
            .await;
        match client.recv().await {
            ServerMessage::Timeline {
                agent: got,
                update: TimelineUpdate::Reset { items, .. },
            } => {
                assert_eq!(got, agent);
                assert!(items.is_empty());
            }
            other => panic!("expected a reset, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_second_maxd_is_refused_while_the_first_holds_the_socket() {
        let served = serve();
        match bind(&served.socket) {
            Err(err @ BindError::AlreadyRunning { pid: Some(pid), .. }) => {
                assert_eq!(pid, std::process::id());
                assert!(err.to_string().contains("already running"));
            }
            Err(other) => panic!("expected AlreadyRunning, got {other}"),
            Ok(_) => panic!("expected AlreadyRunning, got a listener"),
        }
        // The refusal leaves the first one serving.
        TestClient::greeted(&served.socket).await;
    }

    #[tokio::test]
    async fn a_stale_socket_is_replaced_and_removed_on_drop() {
        let dir = TempDir::new();
        let socket = dir.path().join("maxd.sock");
        // A socket nobody listens on, as a crashed maxd leaves it.
        drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
        assert!(socket.exists());

        let listener = bind(&socket).unwrap();
        assert_eq!(
            fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let accept = tokio::spawn(async move {
            let (stream, _) = listener.listener.accept().await.unwrap();
            (listener, stream)
        });
        UnixStream::connect(&socket).await.unwrap();
        let (listener, _) = accept.await.unwrap();
        drop(listener);
        assert!(!socket.exists());
    }

    #[tokio::test]
    async fn a_file_in_the_way_is_left_alone() {
        let dir = TempDir::new();
        let socket = dir.path().join("maxd.sock");
        fs::write(&socket, "keep me").unwrap();
        assert!(matches!(bind(&socket), Err(BindError::NotASocket { .. })));
        assert_eq!(fs::read_to_string(&socket).unwrap(), "keep me");
    }
}
