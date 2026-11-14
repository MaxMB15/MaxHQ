// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The guest's end of the host link. One host link is served at a time,
//! and a new one replaces the old. Holder connections, history readers and
//! open requests belong to the link they came on and end with it. The
//! holders themselves carry on.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures::StreamExt;
use maxhq_hold::RunAs;
use maxhq_protocol::frame::EncodeError;
use maxhq_protocol::guest::{
    GuestLaunch, GuestToHost, HolderEvent, HostToGuest, TokenReply, TokenWant, WatchedHolder,
    GUEST_PROTOCOL_VERSION, MAX_HISTORIES, MAX_HOLDER_CONNECTIONS,
};
use maxhq_protocol::ids::{AgentId, BootId, SessionId, TransferId};
use maxhq_protocol::model::Vendor;
use maxhq_protocol::processes::ProcessSignal;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot};
use tokio::task::{AbortHandle, JoinSet};

use super::accounts::Folders;
use super::{lease, login_env, worktree, Files, Tunnels};
use crate::drive::claude::own_mcp;
use crate::folder_trust;
use crate::guestlink::wire::{self, Abort, Outbox, Read, ReadError, Reader, SendError, TokenRate};
use crate::ports::{processes, Procfs, Scanner};
use crate::session::clock::BoxFuture;
use crate::session::holders::{Event, Holders, Launch, SpawnError, WriteError};
use crate::session::vendors::History;

/// This build's version. The host installs its own build when they differ.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How long the host has to say hello once connected.
const HELLO_WAIT: Duration = Duration::from_secs(10);

/// How long the host has to answer a token request.
const TOKEN_WAIT: Duration = Duration::from_secs(60);

/// The only user a CLI runs as.
pub(crate) const AGENT: &str = "agent";

/// Variables the guest service has that a CLI must not inherit, beside
/// every `XDG_*` one. Root's runtime folders and systemd's hand-offs mean
/// nothing to user `agent`.
const ROOT_ONLY: &[&str] = &[
    maxhq_core::paths::ROLE_ENV,
    maxhq_core::paths::SOCKET_ENV,
    "MAXHQ_HOLD_DETACH",
    "INVOCATION_ID",
    "JOURNAL_STREAM",
    "NOTIFY_SOCKET",
    "SYSTEMD_EXEC_PID",
    "LISTEN_FDS",
    "LISTEN_PID",
    "LISTEN_FDNAMES",
    "MANAGERPID",
    "CREDENTIALS_DIRECTORY",
    "RUNTIME_DIRECTORY",
    "STATE_DIRECTORY",
    "RUST_LOG",
];

/// Opens a vendor's history reader on the container's files, as
/// [`Vendors::history`](crate::session::vendors::Vendors::history) does.
pub(crate) type Opener =
    Arc<dyn Fn(Vendor, &Path, &str, AgentId) -> Option<Box<dyn History>> + Send + Sync>;

/// Finds a container user. None runs as the guest's own user, for tests.
pub(crate) type Users = fn(&str) -> io::Result<Option<RunAs>>;

/// Which processes are holders the guest started, by process ID.
pub(crate) trait HolderPids: Send + Sync + 'static {
    /// Each live holder's process ID and session.
    fn holder_pids(&self) -> BoxFuture<HashMap<u32, SessionId>>;
}

/// Everything the guest serves with.
pub(crate) struct Guest {
    /// The holders in this container.
    pub(crate) holders: Arc<dyn Holders>,
    /// Their process IDs, for the agent socket.
    pub(crate) pids: Arc<dyn HolderPids>,
    /// The history readers.
    pub(crate) histories: Opener,
    /// The transfers.
    pub(crate) files: Arc<dyn Files>,
    /// The ports and tunnels.
    pub(crate) tunnels: Arc<dyn Tunnels>,
    /// The container's name.
    pub(crate) instance: String,
    /// The container's boot.
    pub(crate) boot_id: BootId,
    /// The folder with the repo and its worktrees.
    pub(crate) work: PathBuf,
    /// Finds the user a CLI runs as.
    pub(crate) users: Users,
    /// The account folders a launch names.
    pub(crate) folders: Folders,
    /// How often a history reader looks for more.
    pub(crate) history_poll: Duration,
    /// How often the checkouts' unsaved work is read. None reads it
    /// never, for tests that hear each frame the guest sends.
    pub(crate) unsaved_every: Option<Duration>,
    /// What leases drive, when this guest is a test host.
    pub(crate) test_host: Option<Arc<dyn lease::TestHost>>,
    /// The build the hello names, which the host upgrades a container
    /// guest to match. A macOS VM's guest names none, since it updates
    /// itself from its shared folder.
    pub(crate) build: Option<String>,
    /// Where the host's skills mount is.
    pub(crate) mount: PathBuf,
    /// The guest's data folder, which keeps the pack installers' logs.
    pub(crate) data: PathBuf,
    current: Mutex<Option<Arc<Current>>>,
    rate: Mutex<TokenRate>,
    /// Numbers each host link, so an old link's end leaves a new one be.
    links: AtomicU64,
}

impl Guest {
    /// A guest with the usual folders and timings.
    pub(crate) fn new(
        holders: Arc<dyn Holders>,
        pids: Arc<dyn HolderPids>,
        histories: Opener,
        files: Arc<dyn Files>,
        tunnels: Arc<dyn Tunnels>,
        instance: String,
        boot_id: BootId,
    ) -> Self {
        Self {
            holders,
            pids,
            histories,
            files,
            tunnels,
            instance,
            boot_id,
            work: PathBuf::from(crate::recipe::WORK),
            users: |user| RunAs::lookup(user).map(Some),
            folders: Folders::container(),
            history_poll: Duration::from_secs(1),
            unsaved_every: Some(super::unsaved::EVERY),
            test_host: None,
            build: crate::identity::build().map(str::to_owned),
            mount: PathBuf::from(maxhq_core::skills::MOUNT),
            data: PathBuf::from(maxhq_core::paths::GUEST_DATA_DIR),
            current: Mutex::new(None),
            rate: Mutex::new(TokenRate::default()),
            links: AtomicU64::new(1),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Option<Arc<Current>>> {
        self.current.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The host link being served, if any.
    pub(crate) fn current(&self) -> Option<Arc<Current>> {
        self.lock().clone()
    }

    /// Counts a token request for `session`, and says whether it's within
    /// the link's limit. The guest keeps to the limit itself, since the
    /// host drops a link that breaks it.
    pub(crate) fn allow_token(&self, session: SessionId) -> bool {
        self.rate
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .allow(session, std::time::Instant::now())
    }
}

/// The host link being served, as the agent socket sees it.
pub(crate) struct Current {
    outbox: Outbox,
    /// The repository the host says the workspace checks out.
    pub(crate) repo: Option<String>,
    next: AtomicU64,
    tokens: Mutex<HashMap<u64, oneshot::Sender<TokenReply>>>,
}

impl Current {
    fn tokens(&self) -> MutexGuard<'_, HashMap<u64, oneshot::Sender<TokenReply>>> {
        self.tokens.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Asks the host for a token for a helper in `session`.
    pub(crate) async fn token(&self, session: SessionId, want: TokenWant) -> TokenReply {
        let req = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.tokens().insert(req, tx);
        let sent = self
            .outbox
            .send(&GuestToHost::TokenRequest { req, session, want })
            .await;
        let reply = match sent {
            Ok(()) => match tokio::time::timeout(TOKEN_WAIT, rx).await {
                Ok(Ok(reply)) => reply,
                Ok(Err(_)) => refused("the host link dropped"),
                Err(_) => refused("the host didn't answer in time"),
            },
            Err(_) => refused("the host link dropped"),
        };
        self.tokens().remove(&req);
        reply
    }
}

/// A refusal with `reason`.
pub(crate) fn refused(reason: &str) -> TokenReply {
    TokenReply::Refused {
        reason: reason.to_owned(),
    }
}

/// Where host links come from: the guest's Unix socket in a container,
/// and vsock in a macOS VM.
pub(crate) trait Incoming: Send + 'static {
    /// The next host link.
    fn next(&mut self) -> futures::future::BoxFuture<'_, io::Result<UnixStream>>;
}

impl Incoming for UnixListener {
    fn next(&mut self) -> futures::future::BoxFuture<'_, io::Result<UnixStream>> {
        Box::pin(async move { self.accept().await.map(|(stream, _)| stream) })
    }
}

/// Accepts host links for good. Each new one replaces the one before.
pub(crate) async fn accept(guest: Arc<Guest>, mut listener: impl Incoming) {
    let mut serving: Option<tokio::task::JoinHandle<()>> = None;
    loop {
        let stream = match listener.next().await {
            Ok(stream) => stream,
            Err(err) => {
                tracing::warn!("couldn't accept a host link: {err}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        if let Some(old) = serving.take() {
            tracing::info!("a new host link replaces the old one");
            old.abort();
            let _ = old.await;
        }
        serving = Some(tokio::spawn(serve(guest.clone(), stream)));
    }
}

/// Clears what a link left behind, however its task ends.
struct Cleanup {
    guest: Arc<Guest>,
    current: Arc<Current>,
    link: u64,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let mut current = self.guest.lock();
        if current
            .as_ref()
            .is_some_and(|now| Arc::ptr_eq(now, &self.current))
        {
            *current = None;
        }
        drop(current);
        self.current.tokens().clear();
        self.guest.files.link_down();
        self.guest.tunnels.link_down(self.link);
    }
}

/// Serves one host link until it drops.
pub(crate) async fn serve(guest: Arc<Guest>, stream: UnixStream) {
    let (read, write) = stream.into_split();
    let (outbox, queue) = Outbox::new();
    // The writer goes with the link however this task ends, a new link's
    // abort among them, so the host sees the socket close.
    let mut writer = Abort(tokio::spawn(wire::write_frames(write, queue)));
    let mut reader = Reader::new(read);
    let reason = handshake(guest, &outbox, &mut reader, &mut writer.0).await;
    tracing::info!("the host link dropped: {reason}");
}

async fn handshake(
    guest: Arc<Guest>,
    outbox: &Outbox,
    reader: &mut Reader<tokio::net::unix::OwnedReadHalf>,
    writer: &mut tokio::task::JoinHandle<io::Result<()>>,
) -> String {
    let first = match tokio::time::timeout(HELLO_WAIT, reader.next()).await {
        Ok(Ok(Read::Message(body))) => body,
        Ok(Ok(_)) => return "the host's first frame wasn't a message".to_owned(),
        Ok(Err(err)) => return err.to_string(),
        Err(_) => return "the host didn't say hello".to_owned(),
    };
    let (protocol, maxd_version, workspace, repo) = match serde_json::from_slice(&first) {
        Ok(HostToGuest::Hello {
            protocol,
            maxd_version,
            workspace,
            repo,
            ..
        }) => (protocol, maxd_version, workspace, repo),
        Ok(_) => return "the host didn't start with a hello".to_owned(),
        Err(err) => return format!("the host's hello didn't parse: {err}"),
    };
    let holders = guest.holders.list().await.unwrap_or_else(|err| {
        tracing::warn!("couldn't list the holders: {err}");
        Vec::new()
    });
    let hello = GuestToHost::Hello {
        protocol: GUEST_PROTOCOL_VERSION,
        maxd_version: VERSION.to_owned(),
        instance: guest.instance.clone(),
        boot_id: guest.boot_id.clone(),
        holders,
        build: guest.build.clone(),
    };
    if let Err(err) = outbox.send(&hello).await {
        return err.to_string();
    }
    if protocol != GUEST_PROTOCOL_VERSION || maxd_version != VERSION {
        // The host installs its own build and restarts this service, so
        // nothing it sends meanwhile is acted on.
        tracing::info!(
            "the host runs maxd {maxd_version} with protocol {protocol}, so it upgrades this one"
        );
        loop {
            if let Err(err) = reader.next().await {
                return err.to_string();
            }
        }
    }
    tracing::info!(%workspace, "the host linked");

    let current = Arc::new(Current {
        outbox: outbox.clone(),
        repo,
        next: AtomicU64::new(1),
        tokens: Mutex::new(HashMap::new()),
    });
    *guest.lock() = Some(current.clone());
    let link = guest.links.fetch_add(1, Ordering::Relaxed);
    let _cleanup = Cleanup {
        guest: guest.clone(),
        current: current.clone(),
        link,
    };
    guest.tunnels.link_up(link, outbox);
    // The link hears every run in the worktrees, then each change.
    let workflows = crate::workflow::guest::follow(&guest.work, outbox.clone());
    let unsaved = guest
        .unsaved_every
        .map(|every| super::unsaved::Watch::start(&guest, every, outbox.clone()));
    let mut relay = Relay {
        guest,
        workflows,
        unsaved,
        current,
        outbox: outbox.clone(),
        tasks: JoinSet::new(),
        conns: HashMap::new(),
        histories: HashMap::new(),
    };
    loop {
        tokio::select! {
            read = reader.next() => match read {
                Ok(read) => {
                    if let Err(reason) = relay.handle(read) {
                        return reason;
                    }
                }
                Err(ReadError::Closed) => return "the host closed the link".to_owned(),
                Err(err) => return err.to_string(),
            },
            Some(_) = relay.tasks.join_next(), if !relay.tasks.is_empty() => {}
            done = &mut *writer => {
                return match done {
                    Ok(Ok(())) => "the link's writer stopped".to_owned(),
                    Ok(Err(err)) => format!("couldn't write to the host: {err}"),
                    Err(err) => format!("the link's writer failed: {err}"),
                };
            }
        }
    }
}

/// A holder connection the host opened.
struct Slot {
    jobs: mpsc::UnboundedSender<Job>,
    task: AbortHandle,
}

/// Something to do with a connection's writer, in the order it came.
enum Job {
    Write {
        req: u64,
        line: String,
        answers: Option<String>,
    },
    Stop {
        req: u64,
        grace: Duration,
    },
}

/// One link's connections, readers and requests.
struct Relay {
    guest: Arc<Guest>,
    /// Follows the workflow runs in the worktrees.
    workflows: crate::workflow::guest::Following,
    /// Reports the checkouts' unsaved work.
    unsaved: Option<super::unsaved::Watch>,
    current: Arc<Current>,
    outbox: Outbox,
    tasks: JoinSet<()>,
    conns: HashMap<u64, Slot>,
    histories: HashMap<u64, AbortHandle>,
}

impl Relay {
    /// Handles one frame from the host. An error is why the link drops.
    fn handle(&mut self, read: Read) -> Result<(), String> {
        let body = match read {
            Read::Chunk {
                transfer,
                offset,
                data,
            } => {
                self.guest.files.chunk(transfer, offset, data, &self.outbox);
                return Ok(());
            }
            Read::Tunnel { stream, data } => {
                self.guest.tunnels.data(stream, data, &self.outbox);
                return Ok(());
            }
            Read::Message(body) => body,
        };
        let message: HostToGuest = serde_json::from_slice(&body)
            .map_err(|err| format!("a message from the host didn't parse: {err}"))?;
        match message {
            HostToGuest::Hello { .. } => return Err("the host said hello twice".to_owned()),
            HostToGuest::Spawn { req, launch } => self.spawn(req, launch),
            HostToGuest::Connect {
                conn,
                session,
                after,
            } => self.connect(conn, session, after),
            HostToGuest::Write {
                req,
                conn,
                line,
                answers,
            } => self.job(conn, Job::Write { req, line, answers }),
            HostToGuest::Stop {
                req,
                conn,
                grace_ms,
            } => self.job(
                conn,
                Job::Stop {
                    req,
                    grace: Duration::from_millis(grace_ms),
                },
            ),
            HostToGuest::Forget { req, session } => self.forget(req, session),
            HostToGuest::End {
                req,
                session,
                holder_pid,
            } => self.end(req, session, holder_pid),
            HostToGuest::Disconnect { conn } => {
                if let Some(slot) = self.conns.remove(&conn) {
                    slot.task.abort();
                }
            }
            HostToGuest::ListHolders { req } => self.list(req),
            HostToGuest::PrepareWorktree {
                req,
                agent: _,
                branch,
                base,
            } => self.worktree(req, branch, base),
            HostToGuest::FindPaths { req, cwd, query } => self.find_paths(req, cwd, query),
            HostToGuest::ListProcesses { req, holder } => self.processes(req, holder),
            HostToGuest::SignalProcess {
                req,
                holder,
                pid,
                start,
                signal,
            } => self.signal(req, holder, pid, start, signal),
            HostToGuest::OpenHistory {
                req,
                vendor,
                config_dir,
                session,
                agent,
                max_bytes,
            } => {
                let reading = Reading {
                    vendor,
                    config_dir: PathBuf::from(config_dir),
                    session,
                    agent,
                    limit: max_bytes.map(|max| (max != u64::MAX).then_some(max)),
                };
                self.history(req, reading);
            }
            HostToGuest::CloseHistory { req } => {
                if let Some(task) = self.histories.remove(&req) {
                    task.abort();
                }
            }
            HostToGuest::TokenReply { req, reply } => {
                if let Some(tx) = self.current.tokens().remove(&req) {
                    let _ = tx.send(reply);
                }
            }
            HostToGuest::ListIgnored { req, cwd } => self.ignored(req, cwd),
            HostToGuest::Place {
                req,
                staged,
                cwd,
                overwrite,
            } => self.place(req, staged, cwd, overwrite),
            message @ (HostToGuest::UploadBegin { .. }
            | HostToGuest::UploadCancel { .. }
            | HostToGuest::FetchBegin { .. }
            | HostToGuest::FetchFiles { .. }
            | HostToGuest::TransferCredit { .. }) => {
                self.guest.files.message(message, &self.outbox);
            }
            HostToGuest::KeepPicture {
                agent,
                media_type,
                data,
            } => self.guest.files.keep_picture(agent, media_type, data),
            message @ (HostToGuest::WatchPorts { .. }
            | HostToGuest::TunnelOpen { .. }
            | HostToGuest::TunnelCredit { .. }
            | HostToGuest::TunnelClose { .. }
            | HostToGuest::SshAuthorized { .. }) => {
                self.guest.tunnels.message(message, &self.outbox);
            }
            HostToGuest::Ping { nonce } => self.outbox.post(&GuestToHost::Pong { nonce }),
            message @ (HostToGuest::LeaseRun { .. }
            | HostToGuest::LeaseScreenshot { .. }
            | HostToGuest::LeaseInstall { .. }
            | HostToGuest::LeaseClose { .. }) => self.lease(message),
            message @ HostToGuest::SkillLinks { .. } => self.skill_links(message),
            HostToGuest::SkillProposals {
                req,
                checkout,
                remove,
            } => self.skill_proposals(req, checkout, remove),
            HostToGuest::FolderTrustScan { req, cwd, locked } => {
                self.folder_trust_scan(req, cwd, locked);
            }
            message @ (HostToGuest::WorkflowAgents { .. } | HostToGuest::WorkflowDetail { .. }) => {
                if let (HostToGuest::WorkflowAgents { agents }, Some(unsaved)) =
                    (&message, &mut self.unsaved)
                {
                    unsaved.agents(agents);
                }
                self.workflows.take(message);
            }
        }
        Ok(())
    }

    fn lease(&mut self, message: HostToGuest) {
        let host = self.guest.test_host.clone();
        let outbox = self.outbox.clone();
        self.tasks.spawn(async move {
            if let Some(answer) = lease::answer(host.as_ref(), message).await {
                let _ = outbox.send(&answer).await;
            }
        });
    }

    fn spawn(&mut self, req: u64, launch: GuestLaunch) {
        let guest = self.guest.clone();
        let outbox = self.outbox.clone();
        self.tasks.spawn(async move {
            let (session, args) = (launch.session, launch.args.clone());
            let result = async {
                let mut launch = holder_launch(&guest, launch)?;
                guest.folders.ready(&launch).await?;
                login_env::add(&mut launch).await;
                own_mcp::add(&mut launch).await;
                guest.holders.spawn(launch).await
            }
            .await;
            if result.is_ok() {
                guest.files.held(session, args).await;
            }
            let result = result.map_err(|err| err.to_string());
            let _ = outbox.send(&GuestToHost::SpawnResult { req, result }).await;
        });
    }

    fn connect(&mut self, conn: u64, session: SessionId, after: Option<u64>) {
        if self.conns.len() >= MAX_HOLDER_CONNECTIONS || self.conns.contains_key(&conn) {
            let error = if self.conns.contains_key(&conn) {
                format!("connection {conn} is already open")
            } else {
                format!("the guest already has {MAX_HOLDER_CONNECTIONS} holder connections")
            };
            self.outbox.post(&GuestToHost::Holder {
                conn,
                event: HolderEvent::Closed { error: Some(error) },
            });
            return;
        }
        let (jobs, queue) = mpsc::unbounded_channel();
        let task = self.tasks.spawn(relay(
            self.guest.holders.clone(),
            self.outbox.clone(),
            conn,
            session,
            after,
            queue,
        ));
        self.conns.insert(conn, Slot { jobs, task });
    }

    fn job(&mut self, conn: u64, job: Job) {
        let job = match self.conns.get(&conn) {
            Some(slot) => match slot.jobs.send(job) {
                Ok(()) => return,
                Err(mpsc::error::SendError(job)) => job,
            },
            None => job,
        };
        let error = WriteError::Disconnected.to_string();
        match job {
            Job::Write { req, .. } => self.outbox.post(&GuestToHost::WriteResult {
                req,
                outcome: Err(error),
            }),
            Job::Stop { req, .. } => self.outbox.post(&GuestToHost::Done {
                req,
                result: Err(error),
            }),
        }
    }

    fn forget(&mut self, req: u64, session: SessionId) {
        // Forgetting goes through a connection of its own, so the host's
        // connections to the session are left alone.
        let guest = self.guest.clone();
        let outbox = self.outbox.clone();
        self.tasks.spawn(async move {
            let result = match guest.holders.connect(session, None).await {
                Ok(connection) => connection
                    .writer
                    .forget()
                    .await
                    .map_err(|err| err.to_string()),
                Err(err) => Err(err.to_string()),
            };
            // The session's attachments go with it, unless another held
            // session names the same agent.
            if result.is_ok() {
                own_mcp::forget(session);
                guest.files.forgotten(session).await;
            }
            let _ = outbox.send(&GuestToHost::Done { req, result }).await;
        });
    }

    fn end(&mut self, req: u64, session: SessionId, holder_pid: Option<u32>) {
        let holders = self.guest.holders.clone();
        let outbox = self.outbox.clone();
        self.tasks.spawn(async move {
            let ended = wire::ended_out(holders.end(session, holder_pid).await);
            let _ = outbox.send(&GuestToHost::EndResult { req, ended }).await;
        });
    }

    fn list(&mut self, req: u64) {
        let holders = self.guest.holders.clone();
        let outbox = self.outbox.clone();
        self.tasks.spawn(async move {
            match holders.list().await {
                Ok(sessions) => {
                    let _ = outbox.send(&GuestToHost::Holders { req, sessions }).await;
                }
                // No answer beats an empty list, which would read as every
                // holder gone. The host's request times out.
                Err(err) => tracing::warn!("couldn't list the holders: {err}"),
            }
        });
    }

    fn worktree(&mut self, req: u64, branch: String, base: Option<String>) {
        let guest = self.guest.clone();
        let repo = self.current.repo.clone();
        let outbox = self.outbox.clone();
        self.tasks.spawn(async move {
            let result = match (guest.users)(AGENT) {
                Ok(user) => {
                    worktree::prepare(&guest.work, repo.as_deref(), &branch, base.as_deref(), user)
                        .await
                }
                Err(err) => Err(format!("couldn't find user {AGENT}: {err}")),
            };
            let _ = outbox
                .send(&GuestToHost::WorktreeReady { req, result })
                .await;
        });
    }

    /// Links a checkout's skills to the workspace's skills mount, the
    /// always set into each of its homes, and lands its pack, for `agent`.
    fn skill_links(&mut self, message: HostToGuest) {
        let HostToGuest::SkillLinks {
            req,
            checkout,
            names,
            always,
            homes,
            pack,
            retry,
        } = message
        else {
            return;
        };
        let guest = self.guest.clone();
        let outbox = self.outbox.clone();
        self.tasks.spawn(async move {
            let linked = tokio::task::spawn_blocking(move || {
                let user = (guest.users)(AGENT)
                    .map_err(|err| format!("couldn't find user {AGENT}: {err}"))?;
                let links = worktree::SkillLinks {
                    checkout: &checkout,
                    names: &names,
                    always: &always,
                    homes: &homes,
                    pack: pack.as_ref(),
                    retry,
                    mount: &guest.mount,
                    logs: &guest.data.join(crate::skills::pack::LOGS),
                };
                worktree::link_skills(&guest.work, &guest.folders.home, &links, user.as_ref())
            })
            .await;
            let (result, pack) =
                match linked.unwrap_or_else(|err| Err(format!("the skill links failed: {err}"))) {
                    Ok(pack) => (Ok(()), pack),
                    Err(err) => (Err(err), None),
                };
            let _ = outbox
                .send(&GuestToHost::SkillLinksDone { req, result, pack })
                .await;
        });
    }

    /// Reads a checkout's skill proposals, after removing one when asked.
    fn skill_proposals(&mut self, req: u64, checkout: String, remove: Option<String>) {
        let work = self.guest.work.clone();
        let outbox = self.outbox.clone();
        self.tasks.spawn(async move {
            let read = tokio::task::spawn_blocking(move || {
                worktree::read_proposals(&work, &checkout, remove.as_deref())
            })
            .await;
            let result =
                read.unwrap_or_else(|err| Err(format!("reading the proposals failed: {err}")));
            let _ = outbox
                .send(&GuestToHost::SkillProposalsRead { req, result })
                .await;
        });
    }

    /// Reads what in a folder can run when a Claude agent starts there.
    fn folder_trust_scan(&mut self, req: u64, cwd: String, locked: Vec<String>) {
        let work = self.guest.work.clone();
        let home = self.guest.folders.home.clone();
        let outbox = self.outbox.clone();
        self.tasks.spawn(async move {
            let read = tokio::task::spawn_blocking(move || {
                folder_trust::scan_in_guest(Path::new(&cwd), &home, &locked, &work)
            })
            .await;
            let result = read.map_err(|err| format!("reading the folder failed: {err}"));
            let _ = outbox
                .send(&GuestToHost::FolderTrustRead { req, result })
                .await;
        });
    }

    /// Lists the files git ignores in the checkout at `cwd`, as `agent`.
    fn ignored(&mut self, req: u64, cwd: String) {
        let guest = self.guest.clone();
        let outbox = self.outbox.clone();
        self.tasks.spawn(async move {
            let listed = match (guest.users)(AGENT) {
                Ok(user) => worktree::ignored(&guest.work, &cwd, user).await,
                Err(err) => {
                    tracing::warn!("couldn't find user {AGENT}: {err}");
                    Default::default()
                }
            };
            let reply = GuestToHost::Ignored {
                req,
                files: listed.files,
                truncated: listed.truncated,
            };
            let _ = outbox.send(&reply).await;
        });
    }

    /// Moves a staged upload into the checkout at `cwd`.
    fn place(&mut self, req: u64, staged: TransferId, cwd: String, overwrite: bool) {
        let place = self.guest.files.place(staged, cwd, overwrite);
        let outbox = self.outbox.clone();
        self.tasks.spawn(async move {
            let result = place.await;
            let _ = outbox.send(&GuestToHost::Placed { req, result }).await;
        });
    }

    /// Searches an agent's folder for the `@` menu, off the async threads.
    fn find_paths(&mut self, req: u64, cwd: String, query: String) {
        let outbox = self.outbox.clone();
        self.tasks.spawn(async move {
            let walk = move || crate::find::find_paths(Path::new(&cwd), &query);
            let result = tokio::task::spawn_blocking(walk)
                .await
                .unwrap_or_else(|err| Err(format!("the search stopped: {err}")));
            let _ = outbox.send(&GuestToHost::FoundPaths { req, result }).await;
        });
    }

    /// Lists the processes in `holder`'s tree for the Running list, off
    /// the async threads.
    fn processes(&mut self, req: u64, holder: WatchedHolder) {
        let outbox = self.outbox.clone();
        self.tasks.spawn(async move {
            let list = move || Procfs::new("/proc", false).processes(&holder);
            let result = tokio::task::spawn_blocking(list)
                .await
                .map_err(|err| format!("the list stopped: {err}"));
            let _ = outbox.send(&GuestToHost::Processes { req, result }).await;
        });
    }

    /// Signals one process in `holder`'s tree, once it's checked.
    fn signal(
        &mut self,
        req: u64,
        holder: WatchedHolder,
        pid: u32,
        start: u64,
        signal: ProcessSignal,
    ) {
        let outbox = self.outbox.clone();
        self.tasks.spawn(async move {
            let send = move || {
                let mut scanner = Procfs::new("/proc", false);
                processes::signal(&mut scanner, &holder, pid, start, signal)
            };
            let result = tokio::task::spawn_blocking(send)
                .await
                .unwrap_or_else(|err| Err(format!("the signal stopped: {err}")));
            let _ = outbox.send(&GuestToHost::Signalled { req, result }).await;
        });
    }

    fn history(&mut self, req: u64, reading: Reading) {
        if self.histories.len() >= MAX_HISTORIES || self.histories.contains_key(&req) {
            tracing::warn!("the host opened more than {MAX_HISTORIES} histories");
            return;
        }
        let task = self.tasks.spawn(history(
            self.guest.clone(),
            self.outbox.clone(),
            req,
            reading,
        ));
        self.histories.insert(req, task);
    }
}

/// The holder launch for a guest launch. CLIs run only as `agent`, and
/// lose the guest service's root-only variables.
fn holder_launch(guest: &Guest, launch: GuestLaunch) -> Result<Launch, SpawnError> {
    let refuse = |message: String| SpawnError::Holder { message };
    if launch.user != AGENT {
        return Err(refuse(format!(
            "CLIs run as {AGENT} in a workspace, not {}",
            launch.user
        )));
    }
    let run_as = (guest.users)(&launch.user)
        .map_err(|err| refuse(format!("couldn't find user {}: {err}", launch.user)))?;
    if run_as.as_ref().is_some_and(|user| user.uid == 0) {
        return Err(refuse(format!("user {} is root", launch.user)));
    }
    let mut env_remove: Vec<OsString> = std::env::vars_os()
        .map(|(key, _)| key)
        .filter(|key| key.to_str().is_some_and(|key| key.starts_with("XDG_")))
        .collect();
    env_remove.extend(ROOT_ONLY.iter().map(OsString::from));
    env_remove.extend(launch.env_remove.into_iter().map(OsString::from));
    Ok(Launch {
        session: launch.session,
        program: PathBuf::from(launch.program),
        args: launch.args.into_iter().map(OsString::from).collect(),
        env: launch
            .env
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect(),
        env_remove,
        secret_env: launch
            .secret_env
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect(),
        run_as,
        cwd: PathBuf::from(launch.cwd),
        framing: wire::framing_in(launch.framing),
    })
}

/// Relays one holder connection: the hello in parts, then each event,
/// then `Closed`. Writes and stops run in the order they came.
async fn relay(
    holders: Arc<dyn Holders>,
    outbox: Outbox,
    conn: u64,
    session: SessionId,
    after: Option<u64>,
    mut jobs: mpsc::UnboundedReceiver<Job>,
) {
    let connection = match holders.connect(session, after).await {
        Ok(connection) => connection,
        Err(err) => {
            let event = HolderEvent::Closed {
                error: Some(err.to_string()),
            };
            let _ = outbox.send(&GuestToHost::Holder { conn, event }).await;
            refuse_jobs(&outbox, &mut jobs).await;
            return;
        }
    };
    if send_hello(&outbox, conn, &connection.hello).await.is_err() {
        return;
    }
    let writer = connection.writer;
    let mut events = connection.events;
    let pump = async {
        let mut exited = false;
        while let Some(event) = events.next().await {
            let event = match event {
                Event::Line(line) => HolderEvent::Line(wire::line_out(line)),
                Event::Exited(exit) => {
                    exited = true;
                    HolderEvent::Exited(wire::exit_out(exit))
                }
            };
            match outbox.send(&GuestToHost::Holder { conn, event }).await {
                Ok(()) => {}
                Err(SendError::Encode(EncodeError::TooLong(len))) => {
                    tracing::warn!(%session, len, "left out a holder line too big for the link");
                }
                Err(_) => return,
            }
        }
        let error = (!exited).then(|| WriteError::Disconnected.to_string());
        let event = HolderEvent::Closed { error };
        let _ = outbox.send(&GuestToHost::Holder { conn, event }).await;
    };
    let work = async {
        while let Some(job) = jobs.recv().await {
            match job {
                Job::Write { req, line, answers } => {
                    let outcome = writer
                        .write(line, answers)
                        .await
                        .map(wire::outcome_out)
                        .map_err(|err| err.to_string());
                    let _ = outbox
                        .send(&GuestToHost::WriteResult { req, outcome })
                        .await;
                }
                Job::Stop { req, grace } => {
                    let result = writer.stop(grace).await.map_err(|err| err.to_string());
                    let _ = outbox.send(&GuestToHost::Done { req, result }).await;
                }
            }
        }
    };
    tokio::join!(pump, work);
}

/// Answers each job on a connection that never opened.
async fn refuse_jobs(outbox: &Outbox, jobs: &mut mpsc::UnboundedReceiver<Job>) {
    let error = WriteError::Disconnected.to_string();
    while let Some(job) = jobs.recv().await {
        let message = match job {
            Job::Write { req, .. } => GuestToHost::WriteResult {
                req,
                outcome: Err(error.clone()),
            },
            Job::Stop { req, .. } => GuestToHost::Done {
                req,
                result: Err(error.clone()),
            },
        };
        let _ = outbox.send(&message).await;
    }
}

/// Sends a holder's hello in parts that each fit a message. With pending
/// lines, the parts carry them in order and a part with none ends the
/// hello, so the host knows when it has them all.
async fn send_hello(
    outbox: &Outbox,
    conn: u64,
    hello: &maxhq_hold::Hello,
) -> Result<(), SendError> {
    let bare = wire::hello_out(hello);
    let mut dropped = 0;
    let runs = wire::chunks(
        hello.pending.iter().cloned().map(wire::line_out).collect(),
        wire::LINE_BUDGET,
        &mut dropped,
    );
    if dropped > 0 {
        tracing::warn!(dropped, "left out pending lines too big for a hello");
    }
    for run in runs {
        let mut part = bare.clone();
        part.pending = run;
        let event = HolderEvent::Hello(part);
        outbox.send(&GuestToHost::Holder { conn, event }).await?;
    }
    let event = HolderEvent::Hello(bare);
    outbox.send(&GuestToHost::Holder { conn, event }).await
}

/// The history a reader in the guest reads.
struct Reading {
    vendor: Vendor,
    /// The account's config folder.
    config_dir: PathBuf,
    /// The Claude session UUID or the Codex thread ID.
    session: String,
    /// The agent the items belong to.
    agent: AgentId,
    /// The most bytes of the newest history the first read takes, or none
    /// for all of it, when the host named it. A host from before it names
    /// none, and the reader keeps its default.
    limit: Option<Option<u64>>,
}

/// Runs a vendor's history reader on the container's files, and sends
/// what it finds. A reader that can't open yet, since the CLI hasn't
/// written its files, is tried again on each poll. The first read ends
/// with an empty batch, even when it found nothing.
async fn history(guest: Arc<Guest>, outbox: Outbox, req: u64, reading: Reading) {
    let Reading {
        vendor,
        config_dir,
        session,
        agent,
        limit,
    } = reading;
    let config_dir = Arc::new(config_dir);
    let session = Arc::new(session);
    let mut reader: Option<Box<dyn History>> = None;
    let mut first = true;
    let mut ticks = tokio::time::interval(guest.history_poll);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticks.tick().await;
        let opener = guest.histories.clone();
        let (dir, name) = (config_dir.clone(), session.clone());
        let read = tokio::task::spawn_blocking(move || {
            let mut reader = reader.or_else(|| {
                let mut opened = opener(vendor, &dir, &name, agent)?;
                if let Some(bytes) = limit {
                    opened.limit(bytes);
                }
                Some(opened)
            });
            let items = reader.as_mut().map(|reader| reader.read());
            (reader, items)
        })
        .await;
        let items = match read {
            Ok((back, items)) => {
                reader = back;
                items
            }
            Err(err) => {
                tracing::warn!("a history reader failed: {err}");
                return;
            }
        };
        let items = match items {
            None => Vec::new(),
            Some(Ok(items)) => items,
            Some(Err(err)) => {
                tracing::debug!(%session, "couldn't read a history: {err}");
                Vec::new()
            }
        };
        let mut dropped = 0;
        for run in wire::chunks(items, wire::CHUNK_BUDGET, &mut dropped) {
            let message = GuestToHost::HistoryItems { req, items: run };
            if outbox.send(&message).await.is_err() {
                return;
            }
        }
        if dropped > 0 {
            tracing::warn!(%session, dropped, "left out history items too big for the link");
        }
        // An empty batch ends the first read, so the host knows it has the
        // whole history so far.
        if first {
            first = false;
            let message = GuestToHost::HistoryItems {
                req,
                items: Vec::new(),
            };
            if outbox.send(&message).await.is_err() {
                return;
            }
        }
    }
}
