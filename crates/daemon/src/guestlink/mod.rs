// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The host's side of each guest link. It connects to a container's
//! `maxhq-guest` proxy socket, handshakes with
//! [`GUEST_PROTOCOL_VERSION`](maxhq_protocol::guest::GUEST_PROTOCOL_VERSION),
//! and gives the session manager each guest's holders through
//! [`HolderPlaces`].
//!
//! Everything from a guest is untrusted. Any breach of the limits in
//! [`maxhq_protocol::guest`] drops that link, and the host reconnects with
//! a backoff. A guest whose version differs from this build gets this
//! build's guest binary through [`Upgrader`], then a reconnect.
//!
//! Token requests go to [`CodexTokens`] and [`GithubTokens`] once
//! [`SessionOwners`] confirms the session runs in the guest's own
//! workspace, and for GitHub once the repo is the workspace's own. File
//! and tunnel messages go to [`GuestFiles`] and [`GuestTunnels`], which
//! send back through [`Guests::sender`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures::stream::{BoxStream, StreamExt};
use maxhq_protocol::find::PathMatch;
use maxhq_protocol::guest::{
    FolderScan, GuestPort, GuestToHost, HostToGuest, TokenReply, WatchedHolder, Worktree,
};
use maxhq_protocol::ids::{AccountId, AgentId, DeviceId, StreamId, TransferId, WorkspaceId};
use maxhq_protocol::model::Vendor;
use maxhq_protocol::processes::{ProcessList, ProcessSignal};
use maxhq_protocol::skills::PackState;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::drive::pictures::ForGuest;
use crate::incus::InstanceName;
use crate::session::clock::BoxFuture;
use crate::session::holders::{HolderPlaces, Holders, PlaceChange, SessionOwners};
use crate::session::vendors::History;
use crate::workspace::GuestLinks;

pub(crate) use self::link::unexpected;
pub use self::link::GuestSender;

mod holders;
mod link;
pub(crate) mod wire;

// The tests reach the guest over Unix sockets.
#[cfg(all(test, unix))]
pub(crate) mod tests;
#[cfg(all(test, unix))]
mod wired;

/// The folder under `maxd`'s runtime folder that holds each proxy socket.
pub const GUESTS_DIR: &str = "guests";

/// The variable that points the host at a guest binary to install.
pub const GUEST_BINARY_ENV: &str = "MAXHQ_GUEST_BINARY";

/// Where an instance's proxy device listens on the host:
/// `<runtime>/guests/<instance>.sock`, owned by maxd's user, mode 0600.
pub fn socket_path(runtime: &Path, instance: &InstanceName) -> PathBuf {
    runtime.join(GUESTS_DIR).join(format!("{instance}.sock"))
}

/// The static guest binary this host installs in its containers:
/// [`GUEST_BINARY_ENV`] when it's set, or `~/.local/lib/maxhq/maxd-guest`.
pub fn guest_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(GUEST_BINARY_ENV).filter(|path| !path.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty())?;
    Some(Path::new(&home).join(".local/lib/maxhq/maxd-guest"))
}

/// A repository URL or path as GitHub's "owner/name", in lower case since
/// GitHub ignores case there. It takes `https://github.com/o/n.git`,
/// `git@github.com:o/n.git`, `ssh://git@github.com/o/n` and a bare `o/n`.
/// None for anything else.
pub fn github_repo(url: &str) -> Option<String> {
    let url = url.trim();
    let path = if let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("ssh://"))
    {
        let rest = rest.strip_prefix("git@").unwrap_or(rest);
        let (host, path) = rest.split_once('/')?;
        if !host.eq_ignore_ascii_case("github.com") {
            return None;
        }
        path
    } else if let Some(path) = url.strip_prefix("git@github.com:") {
        path
    } else if url.contains(':') {
        return None;
    } else {
        url
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    let fine = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    (fine(owner) && fine(name)).then(|| format!("{owner}/{name}").to_ascii_lowercase())
}

/// XC's install of a new guest binary. It pushes the binary into the
/// instance as `/usr/local/bin/maxd` and restarts `maxhq-guest` through
/// exec.
pub trait Upgrader: Send + Sync + 'static {
    /// Installs `binary` in `instance` and restarts its guest service.
    fn upgrade(&self, instance: InstanceName, binary: PathBuf) -> BoxFuture<Result<(), String>>;
}

/// XI's broker for Codex access tokens.
pub trait CodexTokens: Send + Sync + 'static {
    /// A ChatGPT access token for the Codex account's sign-in.
    fn token(&self, account: AccountId) -> BoxFuture<TokenReply>;
}

/// The broker for GitHub installation tokens.
pub trait GithubTokens: Send + Sync + 'static {
    /// An installation token for `repo`, the workspace's own repository as
    /// its URL or "owner/name". The link has already checked the guest
    /// asked for that one.
    fn token(&self, workspace: WorkspaceId, repo: String) -> BoxFuture<TokenReply>;
}

/// Tokens from this device's GitHub App, through [`crate::secrets::github`].
pub struct GithubApp;

impl GithubTokens for GithubApp {
    fn token(&self, _workspace: WorkspaceId, repo: String) -> BoxFuture<TokenReply> {
        Box::pin(async move {
            crate::secrets::github(&repo)
                .await
                .map_or_else(TokenReply::from, TokenReply::from)
        })
    }
}

/// Tokens from this device's ChatGPT sign-in, through
/// [`crate::secrets::codex_token`].
pub struct ChatGptSignIn;

impl CodexTokens for ChatGptSignIn {
    fn token(&self, account: AccountId) -> BoxFuture<TokenReply> {
        Box::pin(crate::secrets::codex_token(account))
    }
}

/// XF's side of transfers into and out of a guest. Each call comes from the
/// link's reader, so it must not block.
pub trait GuestFiles: Send + Sync + 'static {
    /// `UploadStored`, `TransferCredit`, `FetchManifest` or
    /// `TransferFailed` from a workspace's guest.
    fn message(&self, workspace: WorkspaceId, message: GuestToHost);
    /// A download's bytes from a workspace's guest.
    fn chunk(&self, workspace: WorkspaceId, transfer: TransferId, offset: u64, data: Vec<u8>);
    /// The workspace's link dropped, so its transfers have stopped.
    fn link_down(&self, workspace: WorkspaceId);
}

/// XG's side of ports and tunnels in a guest. Each call comes from the
/// link's reader, so it must not block.
pub trait GuestTunnels: Send + Sync + 'static {
    /// A workspace's link came up, and sends through `sender`. It comes
    /// before anything the link reads.
    fn link_up(&self, workspace: WorkspaceId, sender: GuestSender);
    /// A workspace's listening ports, at most
    /// [`MAX_PORTS`](maxhq_protocol::guest::MAX_PORTS) of them.
    fn ports(&self, workspace: WorkspaceId, listening: Vec<GuestPort>);
    /// `TunnelOpened`, `TunnelRefused`, `TunnelCredit` or `TunnelClose`
    /// from a workspace's guest.
    fn message(&self, workspace: WorkspaceId, message: GuestToHost);
    /// A stream's bytes from a workspace's guest.
    fn data(&self, workspace: WorkspaceId, stream: StreamId, data: Vec<u8>);
    /// The workspace's link dropped, so its streams have ended.
    fn link_down(&self, workspace: WorkspaceId);
    /// The workspace was deleted, so whatever is kept about it goes.
    fn forget(&self, _workspace: WorkspaceId) {}
}

/// What the guest links need from the rest of `maxd`.
pub struct Config {
    /// `maxd`'s runtime folder, which holds [`GUESTS_DIR`].
    pub runtime: PathBuf,
    /// This device.
    pub device: DeviceId,
    /// The session manager, which says whose each session is.
    pub owners: Arc<dyn SessionOwners>,
    /// XI's Codex broker.
    pub codex: Arc<dyn CodexTokens>,
    /// The GitHub broker, [`GithubApp`] in `maxd`.
    pub github: Arc<dyn GithubTokens>,
    /// XC's guest install.
    pub upgrader: Arc<dyn Upgrader>,
    /// The binary [`Upgrader`] installs, from [`guest_binary`].
    pub guest_binary: Option<PathBuf>,
    /// XF's transfers.
    pub files: Arc<dyn GuestFiles>,
    /// XG's tunnels.
    pub tunnels: Arc<dyn GuestTunnels>,
    /// How often the host pings each guest. A ping unanswered by the next
    /// one drops the link.
    pub keepalive: Duration,
}

impl Config {
    /// The usual keepalive.
    pub const KEEPALIVE: Duration = Duration::from_secs(15);
}

/// The holders on this device and in every container workspace's guest.
pub struct Guests {
    shared: Arc<Shared>,
}

/// What the link tasks share with [`Guests`].
pub(crate) struct Shared {
    local: Arc<dyn Holders>,
    pub(crate) config: Config,
    state: Mutex<State>,
    /// Bumped when the guest binary changes, so each link drops and its
    /// guest is compared with the new binary when it connects again.
    pub(crate) refresh: tokio::sync::watch::Sender<u64>,
}

#[derive(Default)]
struct State {
    workspaces: HashMap<WorkspaceId, Entry>,
    watchers: Vec<mpsc::UnboundedSender<PlaceChange>>,
    next_task: u64,
}

/// One container workspace.
struct Entry {
    /// Names the task, so a replaced one can't touch its successor's entry.
    task_id: u64,
    task: JoinHandle<()>,
    /// The link while it's up.
    link: Option<Arc<link::Link>>,
    /// What the last hello said, while the link is up.
    up: Option<PlaceChange>,
    /// Why the last try to link failed, until one links.
    failure: Option<String>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The workspace's link while it's up.
    pub(crate) fn link(&self, workspace: WorkspaceId) -> Option<Arc<link::Link>> {
        self.lock()
            .workspaces
            .get(&workspace)
            .and_then(|entry| entry.link.clone())
    }

    /// Records a link coming up, unless the task has been replaced.
    pub(crate) fn up(&self, task_id: u64, link: Arc<link::Link>, change: PlaceChange) -> bool {
        let mut state = self.lock();
        let workspace = link.workspace();
        match state.workspaces.get_mut(&workspace) {
            Some(entry) if entry.task_id == task_id => {
                entry.link = Some(link);
                entry.up = Some(change.clone());
                entry.failure = None;
            }
            _ => return false,
        }
        state.watchers.retain(|tx| tx.send(change.clone()).is_ok());
        true
    }

    /// Records why a try to link failed, unless the task has been replaced.
    pub(crate) fn failed(&self, task_id: u64, workspace: WorkspaceId, reason: &str) {
        if let Some(entry) = self.lock().workspaces.get_mut(&workspace)
            && entry.task_id == task_id
        {
            entry.failure = Some(reason.to_owned());
        }
    }

    /// Records a link going down, unless the task has been replaced.
    pub(crate) fn down(&self, task_id: u64, workspace: WorkspaceId, reason: String) {
        let mut state = self.lock();
        match state.workspaces.get_mut(&workspace) {
            Some(entry) if entry.task_id == task_id && entry.up.is_some() => {
                entry.link = None;
                entry.up = None;
            }
            _ => return,
        }
        let change = PlaceChange::Down { workspace, reason };
        state.watchers.retain(|tx| tx.send(change.clone()).is_ok());
    }
}

impl Guests {
    /// No container workspaces yet. `local` is this device's holders.
    pub fn new(local: Arc<dyn Holders>, config: Config) -> Self {
        Self {
            shared: Arc::new(Shared {
                local,
                config,
                state: Mutex::new(State::default()),
                refresh: tokio::sync::watch::Sender::new(0),
            }),
        }
    }

    /// Starts keeping a link to a container workspace's guest, through
    /// the proxy socket for `instance`. `repo` is the URL the workspace
    /// checks out, the only repository its helpers get tokens for. A
    /// workspace added again starts over with the new details.
    pub fn add(&self, workspace: WorkspaceId, instance: InstanceName, repo: Option<String>) {
        self.remove(workspace);
        let mut state = self.shared.lock();
        state.next_task += 1;
        let task_id = state.next_task;
        let task = tokio::spawn(link::keep(
            Arc::downgrade(&self.shared),
            task_id,
            workspace,
            instance,
            repo,
        ));
        state.workspaces.insert(
            workspace,
            Entry {
                task_id,
                task,
                link: None,
                up: None,
                failure: None,
            },
        );
    }

    /// Stops keeping a workspace's link, and drops it if it's up.
    pub fn remove(&self, workspace: WorkspaceId) {
        let entry = self.shared.lock().workspaces.remove(&workspace);
        let Some(entry) = entry else {
            return;
        };
        entry.task.abort();
        if let Some(link) = entry.link {
            link.close();
            self.shared.config.files.link_down(workspace);
            self.shared.config.tunnels.link_down(workspace);
        }
        if entry.up.is_some() {
            let change = PlaceChange::Down {
                workspace,
                reason: "the workspace was removed".to_owned(),
            };
            self.shared
                .lock()
                .watchers
                .retain(|tx| tx.send(change.clone()).is_ok());
        }
    }

    /// Drops every link to a guest, after the guest binary here changed.
    /// Each connects again and gets the new binary if its build differs.
    /// Only the workspaces added here, the ones MaxHQ made, are touched.
    pub fn refresh(&self) {
        self.shared
            .refresh
            .send_modify(|generation| *generation += 1);
    }

    /// Sends to a workspace's guest, for transfers and tunnels. None while
    /// its link is down.
    pub fn sender(&self, workspace: WorkspaceId) -> Option<GuestSender> {
        self.shared.link(workspace).map(|link| link.sender())
    }

    /// Why the last try to link a workspace's guest failed, while it
    /// hasn't linked since.
    pub fn failure(&self, workspace: WorkspaceId) -> Option<String> {
        self.shared
            .lock()
            .workspaces
            .get(&workspace)
            .and_then(|entry| entry.failure.clone())
    }
}

impl Drop for Guests {
    fn drop(&mut self) {
        let mut state = self.shared.lock();
        for entry in state.workspaces.values() {
            entry.task.abort();
            if let Some(link) = &entry.link {
                link.close();
            }
        }
        state.workspaces.clear();
    }
}

/// How XC's workspace manager opens and closes the links.
impl GuestLinks for Guests {
    fn open(&self, workspace: WorkspaceId, instance: &InstanceName, repo: Option<&str>) {
        self.add(workspace, instance.clone(), repo.map(str::to_owned));
    }

    fn close(&self, workspace: WorkspaceId) {
        self.remove(workspace);
    }

    fn wait_up(
        &self,
        workspace: WorkspaceId,
        wait: Duration,
    ) -> futures::future::BoxFuture<'_, Result<Option<String>, String>> {
        // Watching before the wait starts means a hello that comes first
        // still counts, since a link already up is replayed.
        let mut changes = self.changes();
        Box::pin(async move {
            let up = async {
                while let Some(change) = changes.next().await {
                    if matches!(change, PlaceChange::Up { workspace: up, .. } if up == workspace) {
                        return true;
                    }
                }
                false
            };
            match tokio::time::timeout(wait, up).await {
                Ok(true) => Ok(Some(link::VERSION.to_owned())),
                Ok(false) => Err("the guest links stopped".to_owned()),
                Err(_) => {
                    let waited = format!("the guest didn't link within {} s", wait.as_secs());
                    Err(match self.failure(workspace) {
                        Some(reason) => format!("{waited}: {reason}"),
                        None => waited,
                    })
                }
            }
        })
    }

    fn forget(&self, workspace: WorkspaceId) {
        self.remove(workspace);
        self.shared.config.tunnels.forget(workspace);
        // The session manager forgets the agents that ran there.
        let change = PlaceChange::Gone { workspace };
        self.shared
            .lock()
            .watchers
            .retain(|tx| tx.send(change.clone()).is_ok());
    }
}

impl Guests {
    /// Has a workspace's guest read what in a folder can run.
    fn read_folder(
        &self,
        workspace: WorkspaceId,
        cwd: String,
        locked: Vec<String>,
    ) -> BoxFuture<Result<FolderScan, String>> {
        let link = self.shared.link(workspace);
        Box::pin(async move {
            let link = link.ok_or_else(|| link::DOWN.to_owned())?;
            let reply = link
                .request(link::REQUEST_WAIT, |req| HostToGuest::FolderTrustScan {
                    req,
                    cwd,
                    locked,
                })
                .await
                .map_err(|err| err.to_string())?;
            match reply {
                GuestToHost::FolderTrustRead { result, .. } => result,
                other => Err(link::unexpected(&other)),
            }
        })
    }
}

impl HolderPlaces for Guests {
    fn folder_scan(
        &self,
        workspace: WorkspaceId,
        cwd: String,
        locked: Vec<String>,
    ) -> BoxFuture<Result<FolderScan, String>> {
        self.read_folder(workspace, cwd, locked)
    }

    fn local(&self) -> Arc<dyn Holders> {
        self.shared.local.clone()
    }

    fn guest(&self, workspace: WorkspaceId) -> Option<Arc<dyn Holders>> {
        self.shared.link(workspace)?;
        Some(Arc::new(holders::GuestHolders::new(
            Arc::downgrade(&self.shared),
            workspace,
        )))
    }

    fn changes(&self) -> BoxStream<'static, PlaceChange> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut state = self.shared.lock();
        for entry in state.workspaces.values() {
            if let Some(up) = &entry.up {
                let _ = tx.send(up.clone());
            }
        }
        state.watchers.push(tx);
        futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|change| (change, rx))
        })
        .boxed()
    }

    fn worktree(
        &self,
        workspace: WorkspaceId,
        agent: AgentId,
        branch: String,
        base: Option<String>,
    ) -> BoxFuture<Result<Worktree, String>> {
        let link = self.shared.link(workspace);
        Box::pin(async move {
            let link = link.ok_or_else(|| link::DOWN.to_owned())?;
            let reply = link
                .request(link::WORKTREE_WAIT, |req| HostToGuest::PrepareWorktree {
                    req,
                    agent,
                    branch,
                    base,
                })
                .await
                .map_err(|err| err.to_string())?;
            match reply {
                GuestToHost::WorktreeReady { result, .. } => result,
                other => Err(link::unexpected(&other)),
            }
        })
    }

    fn find_paths(
        &self,
        workspace: WorkspaceId,
        cwd: String,
        query: String,
    ) -> BoxFuture<Result<Vec<PathMatch>, String>> {
        let link = self.shared.link(workspace);
        Box::pin(async move {
            let link = link.ok_or_else(|| link::DOWN.to_owned())?;
            let reply = link
                .request(link::FIND_WAIT, |req| HostToGuest::FindPaths {
                    req,
                    cwd,
                    query,
                })
                .await
                .map_err(|err| err.to_string())?;
            match reply {
                GuestToHost::FoundPaths { result, .. } => result.and_then(crate::find::checked),
                other => Err(link::unexpected(&other)),
            }
        })
    }

    fn list_processes(
        &self,
        workspace: WorkspaceId,
        holder: WatchedHolder,
    ) -> BoxFuture<Result<ProcessList, String>> {
        let link = self.shared.link(workspace);
        Box::pin(async move {
            let link = link.ok_or_else(|| link::DOWN.to_owned())?;
            let reply = link
                .request(link::PROCESS_WAIT, |req| HostToGuest::ListProcesses {
                    req,
                    holder,
                })
                .await;
            match reply {
                Err(link::Failed::TooOld) => Ok(ProcessList::TooOld),
                Err(err) => Err(err.to_string()),
                Ok(GuestToHost::Processes { result, .. }) => {
                    let processes = result.and_then(crate::ports::processes::checked)?;
                    Ok(ProcessList::Listed { processes })
                }
                Ok(other) => Err(link::unexpected(&other)),
            }
        })
    }

    fn signal_process(
        &self,
        workspace: WorkspaceId,
        holder: WatchedHolder,
        pid: u32,
        start: u64,
        signal: ProcessSignal,
    ) -> BoxFuture<Result<(), String>> {
        let link = self.shared.link(workspace);
        Box::pin(async move {
            let link = link.ok_or_else(|| link::DOWN.to_owned())?;
            let reply = link
                .request(link::PROCESS_WAIT, |req| HostToGuest::SignalProcess {
                    req,
                    holder,
                    pid,
                    start,
                    signal,
                })
                .await
                .map_err(|err| err.to_string())?;
            match reply {
                GuestToHost::Signalled { result, .. } => result,
                other => Err(link::unexpected(&other)),
            }
        })
    }

    fn history(
        &self,
        workspace: WorkspaceId,
        vendor: Vendor,
        config_dir: String,
        session: String,
        agent: AgentId,
        bytes: Option<u64>,
    ) -> Option<Box<dyn History>> {
        let link = self.shared.link(workspace)?;
        holders::GuestHistory::open(link, vendor, config_dir, session, agent, bytes)
            .map(|history| Box::new(history) as Box<dyn History>)
    }

    fn keep_picture(&self, picture: ForGuest) {
        let Some(sender) = self.sender(picture.workspace) else {
            return;
        };
        let message = HostToGuest::KeepPicture {
            agent: picture.agent,
            media_type: picture.media_type,
            data: picture.data,
        };
        tokio::spawn(async move {
            if let Err(err) = sender.send(&message).await {
                tracing::debug!(%err, "couldn't hand a picture to a guest");
            }
        });
    }
}

impl crate::skills::deliver::GuestSkills for Guests {
    fn link_skills(
        &self,
        workspace: WorkspaceId,
        links: crate::skills::deliver::GuestLinks,
    ) -> BoxFuture<Result<Option<PackState>, String>> {
        let link = self.shared.link(workspace);
        Box::pin(async move {
            let link = link.ok_or_else(|| link::DOWN.to_owned())?;
            let reply = link
                .request(link::REQUEST_WAIT, |req| HostToGuest::SkillLinks {
                    req,
                    checkout: links.checkout,
                    names: links.names,
                    always: links.always,
                    homes: links.homes,
                    pack: links.pack,
                    retry: links.retry,
                })
                .await
                .map_err(|err| err.to_string())?;
            match reply {
                GuestToHost::SkillLinksDone { result, pack, .. } => result.map(|()| pack),
                other => Err(link::unexpected(&other)),
            }
        })
    }

    fn read_proposals(
        &self,
        workspace: WorkspaceId,
        checkout: String,
        remove: Option<String>,
    ) -> BoxFuture<Result<Vec<maxhq_protocol::guest::GuestProposal>, String>> {
        let link = self.shared.link(workspace);
        Box::pin(async move {
            let link = link.ok_or_else(|| link::DOWN.to_owned())?;
            let reply = link
                .request(link::REQUEST_WAIT, |req| HostToGuest::SkillProposals {
                    req,
                    checkout,
                    remove,
                })
                .await
                .map_err(|err| err.to_string())?;
            match reply {
                GuestToHost::SkillProposalsRead { result, .. } => result,
                other => Err(link::unexpected(&other)),
            }
        })
    }

    fn folder_scan(
        &self,
        workspace: WorkspaceId,
        cwd: String,
        locked: Vec<String>,
    ) -> BoxFuture<Result<FolderScan, String>> {
        self.read_folder(workspace, cwd, locked)
    }
}
