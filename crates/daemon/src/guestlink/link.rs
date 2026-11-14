// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! One workspace's link: the connect loop with its backoff, the handshake,
//! and the reader that routes what the guest sends. Any breach of the
//! guest link's limits ends the link, and the loop connects again.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant, SystemTime};

use maxhq_protocol::frame::EncodeError;
use maxhq_protocol::guest::{
    GuestToHost, HolderEvent, HostToGuest, TokenReply, TokenWant, GUEST_PROTOCOL_VERSION,
    MAX_HISTORIES, MAX_HOLDER_CONNECTIONS, MAX_PORTS, MAX_TOKEN_REQUESTS_PER_MINUTE, MAX_TUNNELS,
    RECONNECT_MAX, RECONNECT_MIN,
};
use maxhq_protocol::ids::{AgentId, SessionId, StreamId, TransferId, WorkspaceId};
use maxhq_protocol::message::TunnelEnd;
use maxhq_protocol::model::Vendor;
use maxhq_protocol::timeline::TimelineItem;
use thiserror::Error;
use tokio::net::unix::OwnedReadHalf;
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot, watch, OwnedSemaphorePermit, Semaphore};

use super::wire::{self, Abort, Outbox, Read, ReadError, Reader, SendError, TokenRate};
use super::{socket_path, Shared};
use crate::incus::InstanceName;
use crate::secrets::same_repo;
use crate::session::clock::BoxFuture;
use crate::session::holders::PlaceChange;

/// What a call says while its workspace's link is down.
pub(crate) const DOWN: &str = "the guest link is down";

/// How long `git worktree add` may take, which includes checking out the
/// tree.
pub(crate) const WORKTREE_WAIT: Duration = Duration::from_secs(600);

/// How long the guest has to answer anything else.
pub(crate) const REQUEST_WAIT: Duration = Duration::from_secs(120);

/// How long the host waits for a guest's path search, which stops its own
/// walk well before this.
pub(crate) const FIND_WAIT: Duration = Duration::from_secs(10);

/// How long the guest has to list or signal an agent's processes.
pub(crate) const PROCESS_WAIT: Duration = Duration::from_secs(10);

/// How much of the guest's messages one link may hold unread, in KiB, across
/// its holder connections and histories. Each message counts for at least 1
/// KiB, so this also limits how many wait. A guest past this loses the link,
/// and readers pick up where they were once it's back. It's far above what a
/// busy guest sends, and tests use less so they can reach it.
pub(crate) const UNREAD_KIB: usize = if cfg!(test) { 32 * 1024 } else { 1024 * 1024 };

/// Counts a message from the guest as unread until it drops.
pub(crate) type Unread = OwnedSemaphorePermit;

/// A batch of history items from the guest and what it counts as unread.
pub(crate) type HistoryBatch = (Vec<TimelineItem>, Unread);

/// How long the guest has to say hello once connected.
const HELLO_WAIT: Duration = Duration::from_secs(10);

/// How long a connect to the proxy socket may take.
const CONNECT_WAIT: Duration = Duration::from_secs(5);

/// The least time between two upgrades of the same guest, so a guest that
/// keeps coming back with the wrong version isn't upgraded in a loop.
const UPGRADE_EVERY: Duration = Duration::from_secs(60);

/// The most that time grows to while a guest keeps coming back wrong.
const UPGRADE_MOST: Duration = Duration::from_secs(15 * 60);

/// How long a broker has to find a token.
const TOKEN_WAIT: Duration = Duration::from_secs(50);

/// This build's version, which the guest must match.
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Why a request to the guest got no answer.
#[derive(Debug, Error)]
pub(crate) enum Failed {
    /// The link is down, or went down while waiting.
    #[error("the guest link is down")]
    Down,
    /// The request didn't encode.
    #[error(transparent)]
    Encode(EncodeError),
    /// The guest took too long.
    #[error("the guest didn't answer in time")]
    Timeout,
    /// The guest's protocol is older than the request, so it wasn't sent.
    #[error("the guest's maxd is too old for that")]
    TooOld,
}

impl From<SendError> for Failed {
    fn from(err: SendError) -> Self {
        match err {
            SendError::Encode(err) => Self::Encode(err),
            SendError::Down | SendError::TooManyTunnels => Self::Down,
        }
    }
}

/// What to say when the guest answers a request with the wrong message.
pub(crate) fn unexpected(_message: &GuestToHost) -> String {
    "the guest sent an answer of the wrong kind".to_owned()
}

/// One live link to a workspace's guest.
pub(crate) struct Link {
    workspace: WorkspaceId,
    repo: Option<String>,
    /// The guest protocol the guest's hello gave.
    protocol: u32,
    outbox: Outbox,
    next: AtomicU64,
    state: Mutex<LinkState>,
    /// How many forgets the guest hasn't answered yet.
    forgets: watch::Sender<usize>,
    /// The room left for messages the host hasn't read, in KiB.
    unread: Arc<Semaphore>,
}

/// Counts a forget as unanswered until it drops.
pub(crate) struct Forgetting {
    link: Arc<Link>,
}

impl Drop for Forgetting {
    fn drop(&mut self) {
        self.link.forgets.send_modify(|count| *count -= 1);
    }
}

#[derive(Default)]
struct LinkState {
    down: bool,
    replies: HashMap<u64, oneshot::Sender<GuestToHost>>,
    conns: HashMap<u64, mpsc::UnboundedSender<(HolderEvent, Unread)>>,
    histories: HashMap<u64, std::sync::mpsc::Sender<HistoryBatch>>,
    streams: Streams,
    rate: TokenRate,
}

/// The tunnel streams open on the link, for [`MAX_TUNNELS`].
#[derive(Debug, Default)]
struct Streams {
    open: HashMap<StreamId, Halves>,
}

/// Which sides of a stream have finished writing.
#[derive(Clone, Copy, Debug, Default)]
struct Halves {
    host: bool,
    guest: bool,
}

impl Streams {
    fn open(&mut self, stream: StreamId) -> bool {
        if self.open.contains_key(&stream) {
            return true;
        }
        if self.open.len() >= MAX_TUNNELS {
            return false;
        }
        self.open.insert(stream, Halves::default());
        true
    }

    fn end(&mut self, stream: StreamId, how: TunnelEnd, from_host: bool) {
        match how {
            TunnelEnd::Reset => {
                self.open.remove(&stream);
            }
            TunnelEnd::Write => {
                let Some(halves) = self.open.get_mut(&stream) else {
                    return;
                };
                if from_host {
                    halves.host = true;
                } else {
                    halves.guest = true;
                }
                if halves.host && halves.guest {
                    self.open.remove(&stream);
                }
            }
        }
    }
}

/// Removes a request's reply slot however the wait ends.
struct Pending<'a> {
    link: &'a Link,
    req: u64,
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        self.link.lock().replies.remove(&self.req);
    }
}

impl Link {
    fn new(workspace: WorkspaceId, repo: Option<String>, protocol: u32, outbox: Outbox) -> Self {
        Self {
            workspace,
            repo,
            protocol,
            outbox,
            next: AtomicU64::new(1),
            state: Mutex::new(LinkState::default()),
            forgets: watch::Sender::new(0),
            unread: Arc::new(Semaphore::new(UNREAD_KIB)),
        }
    }

    fn lock(&self) -> MutexGuard<'_, LinkState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The workspace the link is for.
    pub(crate) fn workspace(&self) -> WorkspaceId {
        self.workspace
    }

    /// Whether the link has ended.
    pub(crate) fn is_down(&self) -> bool {
        self.lock().down
    }

    /// A number no other request, connection or history on this link has.
    fn id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    /// Ends the link for everyone waiting on it. Connections and histories
    /// see their channels close.
    pub(crate) fn close(&self) {
        let mut state = self.lock();
        state.down = true;
        state.replies.clear();
        state.conns.clear();
        state.histories.clear();
        state.streams = Streams::default();
    }

    /// Sends to the guest for transfers and tunnels.
    pub(crate) fn sender(self: &Arc<Self>) -> GuestSender {
        GuestSender { link: self.clone() }
    }

    /// Sends a message without waiting, unless the link is down.
    pub(crate) fn post(&self, message: &HostToGuest) {
        if !self.is_down() {
            self.outbox.post(message);
        }
    }

    /// Counts a forget from now until the guard drops. A forget removes a
    /// session's attachments, so a spawn waits for the guest to answer
    /// each forget asked for before it.
    pub(crate) fn forgetting(self: &Arc<Self>) -> Forgetting {
        self.forgets.send_modify(|count| *count += 1);
        Forgetting { link: self.clone() }
    }

    /// Waits until the guest has answered every forget counted so far.
    /// Each one ends within its request's wait.
    pub(crate) async fn forgotten(&self) {
        let mut forgets = self.forgets.subscribe();
        let _ = forgets.wait_for(|count| *count == 0).await;
    }

    /// Sends the request `make` builds with a fresh `req`, then waits up to
    /// `wait` for the guest's message with the same `req`. A request newer
    /// than the guest's protocol isn't sent, since the guest drops the
    /// link at a message it can't read.
    pub(crate) async fn request(
        &self,
        wait: Duration,
        make: impl FnOnce(u64) -> HostToGuest,
    ) -> Result<GuestToHost, Failed> {
        let req = self.id();
        let message = make(req);
        if message.since() > self.protocol {
            return Err(Failed::TooOld);
        }
        let (tx, rx) = oneshot::channel();
        {
            let mut state = self.lock();
            if state.down {
                return Err(Failed::Down);
            }
            state.replies.insert(req, tx);
        }
        let _pending = Pending { link: self, req };
        self.outbox.send(&message).await?;
        match tokio::time::timeout(wait, rx).await {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(_)) => Err(Failed::Down),
            Err(_) => Err(Failed::Timeout),
        }
    }

    /// Takes room for a message of `len` bytes until the host has read it. An
    /// error is why the link must drop.
    fn unread(&self, len: usize) -> Result<Unread, String> {
        let kib = u32::try_from(len / 1024 + 1).unwrap_or(u32::MAX);
        self.unread
            .clone()
            .try_acquire_many_owned(kib)
            .map_err(|_| format!("the guest sent over {UNREAD_KIB} KiB the host hasn't read"))
    }

    /// Takes a connection number and the channel its holder events come
    /// on. The caller sends the `Connect`.
    pub(crate) fn open_conn(
        &self,
    ) -> Result<(u64, mpsc::UnboundedReceiver<(HolderEvent, Unread)>), String> {
        let conn = self.id();
        let mut state = self.lock();
        if state.down {
            return Err(DOWN.to_owned());
        }
        if state.conns.len() >= MAX_HOLDER_CONNECTIONS {
            return Err(format!(
                "the guest link already has {MAX_HOLDER_CONNECTIONS} holder connections"
            ));
        }
        let (tx, rx) = mpsc::unbounded_channel();
        state.conns.insert(conn, tx);
        Ok((conn, rx))
    }

    /// Lets a connection go, and tells the guest so it frees the slot.
    pub(crate) fn close_conn(&self, conn: u64) {
        let mut state = self.lock();
        state.conns.remove(&conn);
        let down = state.down;
        drop(state);
        if !down {
            self.outbox.post(&HostToGuest::Disconnect { conn });
        }
    }

    /// Opens a history reader in the guest, whose first read takes at most
    /// `bytes` of the newest history, or all of it with no limit. None
    /// while the link is down or already has [`MAX_HISTORIES`] open.
    pub(crate) fn open_history(
        &self,
        vendor: Vendor,
        config_dir: String,
        session: String,
        agent: AgentId,
        bytes: Option<u64>,
    ) -> Option<(u64, std::sync::mpsc::Receiver<HistoryBatch>)> {
        let req = self.id();
        let mut state = self.lock();
        if state.down || state.histories.len() >= MAX_HISTORIES {
            return None;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        state.histories.insert(req, tx);
        drop(state);
        self.outbox.post(&HostToGuest::OpenHistory {
            req,
            vendor,
            config_dir,
            session,
            agent,
            max_bytes: Some(bytes.unwrap_or(u64::MAX)),
        });
        Some((req, rx))
    }

    /// Closes a history reader in the guest.
    pub(crate) fn close_history(&self, req: u64) {
        let mut state = self.lock();
        let open = state.histories.remove(&req).is_some();
        let down = state.down;
        drop(state);
        if open && !down {
            self.outbox.post(&HostToGuest::CloseHistory { req });
        }
    }

    /// Routes one frame from the guest. An error is why the link must drop.
    fn dispatch(&self, shared: &Shared, read: Read, ping: &mut Option<u64>) -> Result<(), String> {
        let config = &shared.config;
        match read {
            Read::Chunk {
                transfer,
                offset,
                data,
            } => {
                config.files.chunk(self.workspace, transfer, offset, data);
                Ok(())
            }
            Read::Tunnel { stream, data } => {
                config.tunnels.data(self.workspace, stream, data);
                Ok(())
            }
            Read::Message(body) => {
                let message = GuestToHost::parse(&body).map_err(|err| err.to_string())?;
                self.message(shared, message, body.len(), ping)
            }
        }
    }

    fn message(
        &self,
        shared: &Shared,
        message: GuestToHost,
        len: usize,
        ping: &mut Option<u64>,
    ) -> Result<(), String> {
        let config = &shared.config;
        match message {
            GuestToHost::Hello { .. } => return Err("the guest said hello twice".to_owned()),
            GuestToHost::SpawnResult { req, .. }
            | GuestToHost::WriteResult { req, .. }
            | GuestToHost::Done { req, .. }
            | GuestToHost::EndResult { req, .. }
            | GuestToHost::Holders { req, .. }
            | GuestToHost::WorktreeReady { req, .. }
            | GuestToHost::FoundPaths { req, .. }
            | GuestToHost::Ignored { req, .. }
            | GuestToHost::Placed { req, .. }
            | GuestToHost::LeaseRan { req, .. }
            | GuestToHost::LeaseScreen { req, .. }
            | GuestToHost::LeaseDone { req, .. }
            | GuestToHost::SkillLinksDone { req, .. }
            | GuestToHost::SkillProposalsRead { req, .. }
            | GuestToHost::FolderTrustRead { req, .. }
            | GuestToHost::WorkflowDetail { req, .. }
            | GuestToHost::Processes { req, .. }
            | GuestToHost::Signalled { req, .. } => {
                // A reply nobody waits for any more is dropped.
                if let Some(tx) = self.lock().replies.remove(&req) {
                    let _ = tx.send(message);
                }
            }
            GuestToHost::Holder { conn, event } => {
                // The connection stays counted until the host lets it go,
                // since the guest keeps its slot until the `Disconnect`.
                if let Some(tx) = self.lock().conns.get(&conn) {
                    let _ = tx.send((event, self.unread(len)?));
                }
            }
            GuestToHost::HistoryItems { req, items } => {
                let mut state = self.lock();
                if let Some(tx) = state.histories.get(&req)
                    && tx.send((items, self.unread(len)?)).is_err()
                {
                    state.histories.remove(&req);
                }
            }
            GuestToHost::TokenRequest { req, session, want } => {
                self.token(shared, req, session, want)?;
            }
            GuestToHost::Ports { listening } => {
                if listening.len() > MAX_PORTS {
                    return Err(format!(
                        "the guest listed {} ports, more than {MAX_PORTS}",
                        listening.len()
                    ));
                }
                config.tunnels.ports(self.workspace, listening);
            }
            GuestToHost::UploadStored { .. }
            | GuestToHost::TransferCredit { .. }
            | GuestToHost::FetchManifest { .. }
            | GuestToHost::TransferFailed { .. } => config.files.message(self.workspace, message),
            GuestToHost::TunnelOpened { .. } | GuestToHost::TunnelCredit { .. } => {
                config.tunnels.message(self.workspace, message);
            }
            GuestToHost::TunnelRefused { stream, .. } => {
                self.lock().streams.open.remove(&stream);
                config.tunnels.message(self.workspace, message);
            }
            GuestToHost::TunnelClose { stream, how } => {
                self.lock().streams.end(stream, how, false);
                config.tunnels.message(self.workspace, message);
            }
            GuestToHost::SshHostKey { public } => match crate::sshkey::ed25519_line(&public) {
                Some(key) => crate::workspace::keep_ssh_host_key(self.workspace, key),
                None => {
                    tracing::warn!(workspace = %self.workspace, "the guest's SSH host key didn't read")
                }
            },
            GuestToHost::UnsavedReport { worktrees } => {
                crate::workspace::keep_unsaved(self.workspace, &worktrees);
            }
            GuestToHost::Pong { nonce } => {
                if *ping == Some(nonce) {
                    *ping = None;
                }
            }
            // The follower checks it before taking any of it.
            GuestToHost::WorkflowSnapshot { snapshot, leads } => {
                crate::workflow::from_guest(self.workspace, snapshot, leads);
            }
        }
        Ok(())
    }

    /// Answers a helper's token request, once the session checks out. A
    /// request for a session in another workspace drops the link, and one
    /// over the rate limit does too.
    fn token(
        &self,
        shared: &Shared,
        req: u64,
        session: SessionId,
        want: TokenWant,
    ) -> Result<(), String> {
        if !self.lock().rate.allow(session, Instant::now()) {
            return Err(format!(
                "session {session} asked for more than {MAX_TOKEN_REQUESTS_PER_MINUTE} tokens in a minute"
            ));
        }
        let config = &shared.config;
        let refuse = |reason: &str| -> BoxFuture<TokenReply> {
            let reason = reason.to_owned();
            Box::pin(async move { TokenReply::Refused { reason } })
        };
        let reply = match config.owners.owner(session) {
            None => refuse("that session isn't running"),
            Some(owner) if owner.workspace != self.workspace => {
                return Err(format!(
                    "the guest asked for a token for session {session}, which runs in another workspace"
                ));
            }
            Some(owner) => match want {
                TokenWant::Codex if owner.vendor != Vendor::Codex => {
                    refuse("that session doesn't run Codex")
                }
                TokenWant::Codex => config.codex.token(owner.account),
                TokenWant::Github { repo } => match self.repo.as_deref() {
                    None => refuse("this workspace has no GitHub repository"),
                    Some(own) if same_repo(&repo, own) => {
                        config.github.token(self.workspace, own.to_owned())
                    }
                    Some(_) => refuse("only the workspace's own repository gets a token"),
                },
            },
        };
        let outbox = self.outbox.clone();
        tokio::spawn(async move {
            let reply = tokio::time::timeout(TOKEN_WAIT, reply)
                .await
                .unwrap_or_else(|_| TokenReply::Refused {
                    reason: "the token broker didn't answer in time".to_owned(),
                });
            let _ = outbox.send(&HostToGuest::TokenReply { req, reply }).await;
        });
        Ok(())
    }
}

/// Sends transfer and tunnel traffic to one workspace's guest, while the
/// link it came from is up. After a reconnect, get a new one from
/// [`Guests::sender`](super::Guests::sender).
#[derive(Clone)]
pub struct GuestSender {
    link: Arc<Link>,
}

impl GuestSender {
    /// Whether the link it came from is still up. Once false, it stays so.
    pub fn is_up(&self) -> bool {
        !self.link.is_down()
    }

    /// Sends one message. A `TunnelOpen` past
    /// [`MAX_TUNNELS`] open streams is refused.
    pub async fn send(&self, message: &HostToGuest) -> Result<(), SendError> {
        {
            let mut state = self.link.lock();
            if state.down {
                return Err(SendError::Down);
            }
            match message {
                HostToGuest::TunnelOpen { stream, .. } if !state.streams.open(*stream) => {
                    return Err(SendError::TooManyTunnels);
                }
                HostToGuest::TunnelClose { stream, how } => state.streams.end(*stream, *how, true),
                _ => {}
            }
        }
        self.link.outbox.send(message).await
    }

    /// Sends the request `make` builds, then waits up to `wait` for the
    /// guest's reply.
    pub async fn request(
        &self,
        wait: Duration,
        make: impl FnOnce(u64) -> HostToGuest,
    ) -> Result<GuestToHost, Failed> {
        self.link.request(wait, make).await
    }

    /// Sends upload bytes, split into chunk frames.
    pub async fn transfer_chunks(
        &self,
        transfer: TransferId,
        offset: u64,
        bytes: &[u8],
    ) -> Result<(), SendError> {
        if self.link.is_down() {
            return Err(SendError::Down);
        }
        self.link
            .outbox
            .transfer_chunks(transfer, offset, bytes)
            .await
    }

    /// Sends bytes on a tunnel stream, split into data frames.
    pub async fn tunnel_data(&self, stream: StreamId, bytes: &[u8]) -> Result<(), SendError> {
        if self.link.is_down() {
            return Err(SendError::Down);
        }
        self.link.outbox.tunnel_data(stream, bytes).await
    }
}

/// How one connect went.
enum Attempt {
    /// The link came up and served until it dropped, for this reason.
    Served(String),
    /// The guest had another version, and got this build's.
    Upgraded,
}

/// When a guest last got this build, and how long it waits for the next.
pub(super) struct LastUpgrade {
    at: Instant,
    wait: Duration,
}

impl LastUpgrade {
    pub(super) fn new(at: Instant, wait: Duration) -> Self {
        Self { at, wait }
    }
}

/// How long a guest upgraded `now` waits for its next upgrade. Each upgrade
/// soon after the last one doubles the wait, whatever the guest says it's
/// running, and a quiet spell starts it over.
pub(super) fn upgrade_wait(last: Option<&LastUpgrade>, now: Instant) -> Duration {
    match last {
        Some(last) if now.duration_since(last.at) < last.wait * 2 => {
            (last.wait * 2).min(UPGRADE_MOST)
        }
        _ => UPGRADE_EVERY,
    }
}

/// Keeps a workspace's link up until the task is aborted or [`Guests`]
/// goes.
///
/// [`Guests`]: super::Guests
pub(crate) async fn keep(
    shared: Weak<Shared>,
    task_id: u64,
    workspace: WorkspaceId,
    instance: InstanceName,
    repo: Option<String>,
) {
    let mut backoff = RECONNECT_MIN;
    let mut upgraded: Option<LastUpgrade> = None;
    loop {
        let Some(strong) = shared.upgrade() else {
            return;
        };
        let attempt = connect(&strong, task_id, workspace, &instance, &repo, &mut upgraded).await;
        if let Err(reason) = &attempt {
            strong.failed(task_id, workspace, reason);
        }
        drop(strong);
        match attempt {
            Ok(Attempt::Served(reason)) => {
                tracing::info!(%workspace, %instance, "the guest link dropped: {reason}");
                backoff = RECONNECT_MIN;
            }
            Ok(Attempt::Upgraded) => {
                tracing::info!(%workspace, %instance, "upgraded the guest");
                backoff = RECONNECT_MIN;
            }
            Err(reason) => {
                tracing::debug!(%workspace, %instance, "couldn't link to the guest: {reason}");
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(RECONNECT_MAX);
    }
}

/// Connects once, handshakes, and serves until the link drops.
async fn connect(
    shared: &Arc<Shared>,
    task_id: u64,
    workspace: WorkspaceId,
    instance: &InstanceName,
    repo: &Option<String>,
    upgraded: &mut Option<LastUpgrade>,
) -> Result<Attempt, String> {
    let config = &shared.config;
    let path = socket_path(&config.runtime, instance);
    let stream = tokio::time::timeout(CONNECT_WAIT, UnixStream::connect(&path))
        .await
        .map_err(|_| "the proxy socket didn't answer".to_owned())?
        .map_err(|err| format!("couldn't connect to {}: {err}", path.display()))?;
    let (read, write) = stream.into_split();
    let (outbox, queue) = Outbox::new();
    let mut writer = Abort(tokio::spawn(wire::write_frames(write, queue)));
    let mut reader = Reader::new(read);

    outbox
        .send(&HostToGuest::Hello {
            protocol: GUEST_PROTOCOL_VERSION,
            maxd_version: VERSION.to_owned(),
            device: config.device,
            workspace,
            repo: repo.clone(),
        })
        .await
        .map_err(|err| err.to_string())?;
    let first = tokio::time::timeout(HELLO_WAIT, reader.next())
        .await
        .map_err(|_| "the guest didn't say hello".to_owned())?
        .map_err(|err| err.to_string())?;
    let Read::Message(body) = first else {
        return Err("the guest's first frame wasn't a message".to_owned());
    };
    let GuestToHost::Hello {
        protocol,
        maxd_version,
        instance: theirs,
        boot_id,
        holders,
        build,
    } = GuestToHost::parse(&body).map_err(|err| err.to_string())?
    else {
        return Err("the guest didn't start with a hello".to_owned());
    };
    // Taken before the build compare, so a new binary from here on drops
    // the link.
    let refresh = shared.refresh.subscribe();
    let wanted = config.guest_binary.as_deref().and_then(binary_build);
    if let Some(differs) = differs(protocol, &maxd_version, build.as_deref(), wanted.as_deref()) {
        drop(writer);
        return upgrade(shared, instance, differs, upgraded).await;
    }
    if theirs != instance.as_str() {
        tracing::warn!(%workspace, %instance, "the guest calls itself {theirs:?}");
    }

    let link = Arc::new(Link::new(workspace, repo.clone(), protocol, outbox));
    let change = PlaceChange::Up {
        workspace,
        boot_id,
        holders,
    };
    if !shared.up(task_id, link.clone(), change) {
        link.close();
        return Err("the workspace's link was replaced".to_owned());
    }
    config.tunnels.link_up(workspace, link.sender());
    crate::workflow::guest_up(workspace, link.sender());
    let reason = serve(shared, &link, &mut reader, &mut writer, refresh).await;
    link.close();
    drop(writer);
    shared.down(task_id, workspace, reason.clone());
    config.files.link_down(workspace);
    config.tunnels.link_down(workspace);
    Ok(Attempt::Served(reason))
}

/// How a guest's hello differs from what this host would install, or none
/// when it matches. `build` is the guest's own and `wanted` the guest
/// binary's on this host. A guest from before builds sends none, and only
/// its version counts.
pub(crate) fn differs(
    protocol: u32,
    version: &str,
    build: Option<&str>,
    wanted: Option<&str>,
) -> Option<String> {
    if protocol != GUEST_PROTOCOL_VERSION || version != VERSION {
        return Some(format!(
            "the guest runs maxd {version} with protocol {protocol}, and this is {VERSION} with {GUEST_PROTOCOL_VERSION}"
        ));
    }
    match (build, wanted) {
        (Some(build), Some(wanted)) if build != wanted => Some(format!(
            "the guest runs build {build}, and the guest binary here is build {wanted}"
        )),
        _ => None,
    }
}

/// The build of the guest binary at `path`, read again only when the file
/// changes. None when it can't be read.
fn binary_build(path: &Path) -> Option<String> {
    type Seen = Option<(PathBuf, SystemTime, u64, String)>;
    static SEEN: Mutex<Seen> = Mutex::new(None);
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let mut seen = SEEN.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((at, when, len, build)) = seen.as_ref()
        && at == path
        && *when == modified
        && *len == meta.len()
    {
        return Some(build.clone());
    }
    let build = crate::identity::build_of(path).ok()?;
    *seen = Some((path.to_owned(), modified, meta.len(), build.clone()));
    Some(build)
}

/// Installs this host's guest binary in a guest that differs, at most once
/// a [`UPGRADE_EVERY`] and less often if it keeps differing.
async fn upgrade(
    shared: &Shared,
    instance: &InstanceName,
    differs: String,
    upgraded: &mut Option<LastUpgrade>,
) -> Result<Attempt, String> {
    let Some(binary) = shared.config.guest_binary.clone() else {
        return Err(format!("{differs}, and there's no guest binary to install"));
    };
    let now = Instant::now();
    if upgraded
        .as_ref()
        .is_some_and(|last| now.duration_since(last.at) < last.wait)
    {
        return Err(format!("{differs}, and it was upgraded a moment ago"));
    }
    *upgraded = Some(LastUpgrade::new(now, upgrade_wait(upgraded.as_ref(), now)));
    tracing::info!(%instance, "{differs}, so it gets this build");
    shared
        .config
        .upgrader
        .upgrade(instance.clone(), binary)
        .await
        .map(|()| Attempt::Upgraded)
        .map_err(|err| format!("{differs}, and the upgrade failed: {err}"))
}

/// Reads and routes until the link drops, and pings on the way. Returns
/// why it dropped.
async fn serve(
    shared: &Shared,
    link: &Link,
    reader: &mut Reader<OwnedReadHalf>,
    writer: &mut Abort<io::Result<()>>,
    mut refresh: tokio::sync::watch::Receiver<u64>,
) -> String {
    let mut ticks = tokio::time::interval(shared.config.keepalive);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticks.tick().await;
    let mut ping: Option<u64> = None;
    let mut nonce = 0;
    loop {
        tokio::select! {
            Ok(()) = refresh.changed() => {
                return "the guest binary here changed".to_owned();
            }
            read = reader.next() => match read {
                Ok(read) => {
                    if let Err(reason) = link.dispatch(shared, read, &mut ping) {
                        tracing::warn!(workspace = %link.workspace, "dropping the guest link: {reason}");
                        return reason;
                    }
                }
                Err(ReadError::Closed) => return "the guest closed the link".to_owned(),
                Err(err) => {
                    tracing::warn!(workspace = %link.workspace, "dropping the guest link: {err}");
                    return err.to_string();
                }
            },
            _ = ticks.tick() => {
                if ping.is_some() {
                    return "the guest stopped answering pings".to_owned();
                }
                nonce += 1;
                ping = Some(nonce);
                link.outbox.post(&HostToGuest::Ping { nonce });
            }
            done = &mut writer.0 => {
                return match done {
                    Ok(Ok(())) => "the link's writer stopped".to_owned(),
                    Ok(Err(err)) => format!("couldn't write to the guest: {err}"),
                    Err(err) => format!("the link's writer failed: {err}"),
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::guest::WatchedHolder;

    use super::*;

    fn list(req: u64) -> HostToGuest {
        HostToGuest::ListProcesses {
            req,
            holder: WatchedHolder {
                session: SessionId::derive("s"),
                agent: None,
                pid: 50,
            },
        }
    }

    #[tokio::test]
    async fn a_request_newer_than_the_guests_protocol_is_never_sent() {
        let since = list(0).since();
        let (outbox, mut queue) = Outbox::new();
        let old = Link::new(WorkspaceId::derive("w"), None, since - 1, outbox);
        let refused = old.request(PROCESS_WAIT, list).await;
        assert!(matches!(refused, Err(Failed::TooOld)), "{refused:?}");
        assert!(queue.try_recv().is_err(), "the request went out");
        assert!(old.lock().replies.is_empty());

        let (outbox, mut queue) = Outbox::new();
        let current = Link::new(WorkspaceId::derive("w"), None, since, outbox);
        let waited = current.request(Duration::from_millis(20), list).await;
        assert!(matches!(waited, Err(Failed::Timeout)), "{waited:?}");
        assert!(queue.try_recv().is_ok(), "the request never went out");
    }
}
