// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! File transfers between clients and this device or its containers, per
//! report 13 and A-14. Uploads stage on the device or relay to a guest
//! with end-to-end credit, and fetches send a manifest, then chunks.
//!
//! Each transfer is a task with an inbox in [`registry`]. The connection
//! that reads a client's frames hands it events and never waits on a disk
//! or a guest. A task answers on its client's [`Route`], and a client that
//! sends the same begin again after a dropped link takes the transfer
//! over, picking up from what's on disk for good.
//!
//! An upload into this device lands in `attachments/incoming/<transfer>`
//! and moves to `attachments/staged/<id>` once every file's hash checks
//! out, as M2's staging does. One into a container goes to its guest,
//! which does the same in the container. [`Transfers`] is also the
//! session manager's [`AgentFiles`](crate::session::holders::AgentFiles),
//! which moves staged attachments into a container before a prompt goes
//! out.

pub(crate) mod disk;
pub(crate) mod registry;

mod copy;
pub(crate) mod deliver;
mod fetch;
pub(crate) mod look;
mod pull;
mod push;
mod upload;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use maxhq_core::ignored_files::IGNORED_FILES_LIMIT;
use maxhq_core::skills::store::TreeStore;
use maxhq_protocol::frame;
use maxhq_protocol::guest::{GuestToHost, HostToGuest, ATTACHMENTS_DIR};
use maxhq_protocol::ids::{AgentId, AttachmentId, IdempotencyKey, TransferId, WorkspaceId};
use maxhq_protocol::message::SHOW_ANYWAY_SINCE;
use maxhq_protocol::message::{
    ActionOutcome, ErrorCode, ManifestEntry, UploadSource, UploadTarget,
};
use maxhq_protocol::model::WorkspaceLocation;
use maxhq_protocol::skills::TreeHash;
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::timeline::{AttachmentKind, AttachmentRef};
use maxhq_protocol::ServerMessage;
use maxhq_transfer::manifest::{self, Kind};
use maxhq_transfer::{Failure, Room, DEVICE_QUOTA};
use maxhq_wire::{Closed, Queue, Sender};
use tokio::time::Interval;
use tracing::{debug, warn};

use crate::doctext;
use crate::drive::{Place, PlaceHost};
use crate::guestlink::wire::SendError;
use crate::guestlink::{self, GuestFiles, GuestSender};
use crate::hub::Hub;
use crate::session::attach::{Attachments, STALE_AFTER};
use crate::session::clock::BoxFuture;
use crate::thumbnail;
use registry::{Chunk, Inbox, Peer, Registry};

/// How long a transfer waits without a word from its client or guest
/// before its task ends. What landed stays, so a resend picks it up.
pub const IDLE: Duration = Duration::from_secs(10 * 60);

/// How long a push into a container waits for the guest to answer.
pub const STALL: Duration = Duration::from_secs(60);

/// The most chunk bytes a task writes in one go.
const WRITE_BATCH: usize = 4 * 1024 * 1024;

/// The longest path a guest may say an upload landed at.
const MAX_GUEST_PATH: usize = 4096;

/// What transfers need from the rest of `maxd`.
pub struct Config {
    /// The attachments folder, `<data>/attachments`.
    pub attachments: PathBuf,
    /// How much room uploads leave on this disk.
    pub room: Room,
    /// The most the attachments folder may hold once an upload lands.
    pub quota: u64,
    /// Where agents and workspaces run.
    pub places: Arc<dyn Places>,
    /// The links to guests.
    pub guests: Arc<dyn Guests>,
    /// The skill store uploads land in, none while this device keeps none.
    pub skills: Option<Arc<dyn TreeStore>>,
}

impl Config {
    /// Transfers with places from the hub, the default limits and no
    /// guest links yet.
    pub fn local(hub: Hub, attachments: PathBuf) -> Self {
        Self {
            places: Arc::new(HubPlaces {
                hub,
                attachments: attachments.clone(),
            }),
            attachments,
            room: Room::DEFAULT,
            quota: DEVICE_QUOTA,
            guests: Arc::new(NoGuests),
            skills: None,
        }
    }
}

/// Where agents and workspaces run, for picking where a transfer goes.
pub trait Places: Send + Sync + 'static {
    /// Where an agent runs, or `None` for one this device doesn't have.
    fn agent(&self, agent: AgentId) -> Option<Place>;
    /// Whose holders run a workspace's agents, and the workspace's folder
    /// there, or `None` for a workspace this device doesn't have.
    fn workspace(&self, workspace: WorkspaceId) -> Option<(PlaceHost, PathBuf)>;
}

/// Places read from the hub. An agent's folder is its workspace's, and
/// its attachments folder is named for it, on the device or in the
/// container.
pub struct HubPlaces {
    hub: Hub,
    attachments: PathBuf,
}

impl HubPlaces {
    fn location(&self, workspace: WorkspaceId) -> Option<WorkspaceLocation> {
        match self.hub.get(&EntityKey::Workspace(workspace)) {
            Some(Entity::Workspace(found)) if found.device == self.hub.device().id => {
                Some(found.location)
            }
            _ => None,
        }
    }
}

impl Places for HubPlaces {
    fn agent(&self, agent: AgentId) -> Option<Place> {
        let Some(Entity::Agent(found)) = self.hub.get(&EntityKey::Agent(agent)) else {
            return None;
        };
        let device = self.hub.device().id;
        let workspace = found.workspace;
        Some(match self.location(workspace)? {
            WorkspaceLocation::Folder { path } => Place {
                device,
                workspace,
                cwd: PathBuf::from(path),
                attachments: self.attachments.join(agent.to_string()),
                host: PlaceHost::Local,
                dispatcher: false,
                trust: None,
                claude_artifacts: true,
            },
            WorkspaceLocation::Container { path, .. } => Place {
                device,
                workspace,
                cwd: PathBuf::from(path),
                attachments: Path::new(ATTACHMENTS_DIR).join(agent.to_string()),
                host: PlaceHost::Guest { workspace },
                dispatcher: false,
                trust: None,
                claude_artifacts: true,
            },
        })
    }

    fn workspace(&self, workspace: WorkspaceId) -> Option<(PlaceHost, PathBuf)> {
        Some(match self.location(workspace)? {
            WorkspaceLocation::Folder { path } => (PlaceHost::Local, PathBuf::from(path)),
            WorkspaceLocation::Container { path, .. } => {
                (PlaceHost::Guest { workspace }, PathBuf::from(path))
            }
        })
    }
}

/// Sends to one workspace's guest, for the transfers through it. The
/// guest link implements it.
pub trait GuestOut: Send + Sync + 'static {
    /// Sends one message.
    fn message(&self, message: HostToGuest) -> BoxFuture<Result<(), Closed>>;
    /// Sends upload bytes, split into chunk frames.
    fn chunk(
        &self,
        transfer: TransferId,
        offset: u64,
        data: Vec<u8>,
    ) -> BoxFuture<Result<(), Closed>>;
    /// Sends the request `make` builds, then waits up to `wait` for the
    /// guest's reply. An error says why there's none.
    fn request(&self, wait: Duration, make: MakeRequest) -> BoxFuture<Result<GuestToHost, String>>;
}

/// Builds a request to a guest from the number its reply comes back with.
pub type MakeRequest = Box<dyn FnOnce(u64) -> HostToGuest + Send>;

/// The links to guests, for transfers into and out of containers.
pub trait Guests: Send + Sync + 'static {
    /// The writer for a workspace's guest link, while one is up.
    fn link(&self, workspace: WorkspaceId) -> Option<Arc<dyn GuestOut>>;
}

/// No guest links, which every device has until the guest link is wired
/// in.
pub struct NoGuests;

impl Guests for NoGuests {
    fn link(&self, _workspace: WorkspaceId) -> Option<Arc<dyn GuestOut>> {
        None
    }
}

/// One client connection's way back, which its transfers answer on.
#[derive(Clone)]
pub struct Route {
    wire: Sender,
    /// The protocol version the client agreed, once it said hello.
    protocol: Option<u32>,
}

impl Route {
    /// Answers on the connection `wire` writes to.
    pub fn new(wire: Sender) -> Self {
        Self {
            wire,
            protocol: None,
        }
    }

    /// Answers a client that agreed `protocol`, in words it reads.
    #[must_use]
    pub fn speaking(mut self, protocol: u32) -> Self {
        self.protocol = Some(protocol);
        self
    }

    /// The protocol version the client agreed, once it said hello.
    pub fn protocol(&self) -> Option<u32> {
        self.protocol
    }

    /// `code` as this client reads it. One from before Show anyway gets
    /// an outside path as a bad one, as it always did.
    fn code(&self, code: ErrorCode) -> ErrorCode {
        let before = self
            .protocol
            .is_none_or(|protocol| protocol < SHOW_ANYWAY_SINCE);
        match code {
            ErrorCode::Outside if before => ErrorCode::BadPath,
            code => code,
        }
    }

    /// Sends a message on the control queue. Nothing is sent once the
    /// client has gone, since a resend picks the transfer up again.
    async fn message(&self, message: &ServerMessage) {
        let mut out = Vec::new();
        if let Err(err) = frame::encode_message(&mut out, message) {
            warn!(%err, "couldn't encode a transfer message");
            return;
        }
        if self.wire.send(Queue::Control, out).await.is_err() {
            debug!("a transfer's client went away");
        }
    }

    /// Sends a transfer's bytes on the bulk queue.
    async fn chunk(&self, transfer: TransferId, offset: u64, data: &[u8]) -> Result<(), Closed> {
        let mut out = Vec::with_capacity(data.len() + 32);
        frame::encode_transfer_chunks(&mut out, transfer, offset, data);
        self.wire.send(Queue::Bulk, out).await
    }
}

/// A guest link, as one transfer uses it.
#[derive(Clone)]
struct GuestLink {
    workspace: WorkspaceId,
    out: Arc<dyn GuestOut>,
}

impl GuestLink {
    async fn message(&self, message: &HostToGuest) -> Result<(), Closed> {
        self.out.message(message.clone()).await
    }

    async fn chunk(&self, transfer: TransferId, offset: u64, data: Vec<u8>) -> Result<(), Closed> {
        self.out.chunk(transfer, offset, data).await
    }

    async fn chunks(
        &self,
        transfer: TransferId,
        chunks: Vec<(u64, Vec<u8>)>,
    ) -> Result<(), Closed> {
        for (offset, data) in chunks {
            self.chunk(transfer, offset, data).await?;
        }
        Ok(())
    }

    /// Tells the guest to give a transfer up, if the link is still there.
    async fn cancel(&self, transfer: TransferId) {
        let _ = self.message(&HostToGuest::UploadCancel { transfer }).await;
    }
}

/// What a transfer's task hears.
enum Event {
    /// The client sent the begin again, after a dropped link or a hash
    /// mismatch. Answers go to `route` from now on.
    Again {
        /// The connection it came on.
        route: Route,
        /// The key it carried.
        key: IdempotencyKey,
    },
    /// The client gave up.
    Cancel,
    /// Credit from the client, for a download.
    ClientCredit {
        /// Bytes the client has for good.
        committed: u64,
        /// How far the client lets this device go.
        window_end: u64,
    },
    /// Bytes from the client, for an upload.
    ClientChunk(Chunk),
    /// A message from the guest the transfer went to.
    Guest(FromGuest),
    /// Bytes from the guest, for a download.
    GuestChunk(Chunk),
    /// The guest's link dropped.
    GuestDown,
}

/// A guest's transfer messages.
enum FromGuest {
    /// An upload landed.
    Stored(GuestStored),
    /// Credit for an upload.
    Credit {
        /// Bytes on the guest's disk for good.
        committed: u64,
        /// How far the guest lets this device go.
        window_end: u64,
    },
    /// What a download holds.
    Manifest(Vec<ManifestEntry>),
    /// The transfer stopped, or a file's hash didn't match.
    Failed(Failure),
}

/// Where a guest says an upload landed, and what it made beside it. It's
/// the guest's word, so [`guest_attachment`] checks it.
#[derive(Clone, Debug, Default)]
struct GuestStored {
    /// The paths it landed at.
    paths: Vec<String>,
    /// A picture's preview.
    thumbnail: Option<String>,
    /// Where a document's text was saved.
    text: Option<String>,
}

/// How a transfer ended, which a resend is answered with again.
enum End {
    /// An upload landed.
    Stored(AttachmentRef),
    /// A download reached the client.
    Fetched,
    /// An upload landed in the skill store.
    Landed,
    /// It stopped.
    Failed {
        /// Why.
        failure: Failure,
        /// The request's result.
        outcome: ActionOutcome,
    },
    /// The client gave up.
    Cancelled,
}

impl End {
    /// A transfer that stopped, refused with the failure's message.
    fn refused(failure: Failure) -> Self {
        let reason = failure.message.clone();
        Self::Failed {
            failure,
            outcome: ActionOutcome::Refused { reason },
        }
    }
}

/// What a begin turned into, once the target was looked up.
enum Plan {
    /// An upload into this device's staging folder.
    Stage(UploadSource),
    /// An upload into this device's skill store.
    Tree {
        /// The tree it must hash to.
        tree: TreeHash,
        /// The store.
        store: Arc<dyn TreeStore>,
        /// Where the files come from.
        source: UploadSource,
    },
    /// An upload into a container.
    Relay {
        /// The container's guest.
        link: GuestLink,
        /// The agent whose folder it lands in, or none for the staging
        /// folder.
        agent: Option<AgentId>,
        /// Where the files come from.
        source: UploadSource,
    },
    /// A download from this device.
    Fetch {
        /// Where the agent runs.
        place: Place,
        /// The path the client asked for.
        path: String,
        /// Whether it's one file you asked for with Show anyway.
        anyway: bool,
    },
    /// Chosen files from a checkout on this device.
    FetchFiles {
        /// The checkout.
        root: PathBuf,
        /// The files, relative to it.
        paths: Vec<String>,
    },
    /// A tree in this device's skill store, at its folder there.
    FetchTree(PathBuf),
    /// A download from a container.
    FetchRelay {
        /// The container's guest.
        link: GuestLink,
        /// The begin that asks the guest for it.
        begin: HostToGuest,
    },
}

/// An upload that landed in a container, which only its guest holds.
#[derive(Clone, Debug)]
struct Remote {
    workspace: WorkspaceId,
    agent: Option<AgentId>,
    transfer: TransferId,
    entries: Vec<ManifestEntry>,
    attachment: AttachmentRef,
    /// Where the guest saved a document's text.
    text: Option<String>,
    at: Instant,
}

struct Shared {
    root: PathBuf,
    store: Attachments,
    room: Room,
    quota: u64,
    places: Arc<dyn Places>,
    guests: Arc<dyn Guests>,
    skills: Option<Arc<dyn TreeStore>>,
    open: Registry<Event>,
    /// Uploads that landed in containers. They live in memory only, so
    /// after a restart the guest's staging folder is out of reach.
    remote: Mutex<HashMap<AttachmentId, Remote>>,
}

impl Shared {
    fn remote(&self) -> MutexGuard<'_, HashMap<AttachmentId, Remote>> {
        self.remote
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Notes an upload that landed in a container, and forgets ones older
    /// than a day.
    fn note_remote(&self, remote: Remote) {
        let mut all = self.remote();
        all.retain(|_, old| old.at.elapsed() < STALE_AFTER);
        all.insert(remote.attachment.id, remote);
    }

    fn link(&self, workspace: WorkspaceId) -> Option<GuestLink> {
        self.guests
            .link(workspace)
            .map(|out| GuestLink { workspace, out })
    }

    /// The folder an upload to this device lands in.
    fn incoming(&self, transfer: TransferId) -> PathBuf {
        self.root.join("incoming").join(transfer.to_string())
    }
}

/// Every transfer on this device. Clones share them.
#[derive(Clone)]
pub struct Transfers {
    shared: Arc<Shared>,
}

impl Transfers {
    /// Transfers with this config.
    pub fn new(config: Config) -> Self {
        Self {
            shared: Arc::new(Shared {
                store: Attachments::new(config.attachments.clone()),
                root: config.attachments,
                room: config.room,
                quota: config.quota,
                places: config.places,
                guests: config.guests,
                skills: config.skills,
                open: Registry::default(),
                remote: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Answers `UploadBegin`. The result comes later on `route`, once the
    /// upload lands or stops.
    pub fn upload_begin(
        &self,
        route: &Route,
        key: IdempotencyKey,
        transfer: TransferId,
        target: UploadTarget,
        source: UploadSource,
    ) {
        self.begin(route, key, transfer, || self.plan_upload(target, source));
    }

    /// Answers `FetchBegin`. The result comes later on `route`, once the
    /// client has every byte or the download stops. With `anyway` it reads
    /// one file outside the agent's folders, so the connection must have
    /// checked that you sent it.
    pub fn fetch_begin(
        &self,
        route: &Route,
        key: IdempotencyKey,
        transfer: TransferId,
        agent: AgentId,
        path: String,
        anyway: bool,
    ) {
        self.begin(route, key, transfer, || {
            self.plan_fetch(transfer, agent, path, anyway)
        });
    }

    /// Answers `FetchFiles`, a download of chosen files from a workspace's
    /// checkout. The result comes later on `route`, as for `FetchBegin`.
    pub fn fetch_files(
        &self,
        route: &Route,
        key: IdempotencyKey,
        transfer: TransferId,
        workspace: WorkspaceId,
        paths: Vec<String>,
    ) {
        self.begin(route, key, transfer, || {
            self.plan_files(transfer, workspace, paths)
        });
    }

    /// Answers `FetchSkillTree`, a download of a tree in this device's
    /// skill store. The result comes later on `route`, as for
    /// `FetchBegin`.
    pub fn fetch_tree(
        &self,
        route: &Route,
        key: IdempotencyKey,
        transfer: TransferId,
        tree: TreeHash,
    ) {
        self.begin(route, key, transfer, || self.plan_tree(tree));
    }

    /// Hands a begin to the open transfer, or opens one.
    fn begin(
        &self,
        route: &Route,
        key: IdempotencyKey,
        transfer: TransferId,
        plan: impl FnOnce() -> Result<(Plan, Vec<Peer>), Box<End>>,
    ) {
        let again = || Event::Again {
            route: route.clone(),
            key,
        };
        if self
            .shared
            .open
            .send(transfer, Peer::Client, again())
            .is_ok()
        {
            return;
        }
        let (plan, peers) = match plan() {
            Ok((plan, peers)) => (Ok(plan), peers),
            Err(end) => (Err(*end), vec![Peer::Client]),
        };
        // A task that just ended frees its slot under the same lock a send
        // takes, so one of these goes through.
        for _ in 0..3 {
            if let Some(inbox) = self.shared.open.open(transfer, peers.clone()) {
                let task = Task {
                    shared: self.shared.clone(),
                    inbox,
                    route: Some(route.clone()),
                    key: Some(key),
                    transfer,
                    stash: None,
                };
                tokio::spawn(task.run(plan));
                return;
            }
            if self
                .shared
                .open
                .send(transfer, Peer::Client, again())
                .is_ok()
            {
                return;
            }
        }
        warn!(%transfer, "couldn't open a transfer");
    }

    fn plan_upload(
        &self,
        target: UploadTarget,
        source: UploadSource,
    ) -> Result<(Plan, Vec<Peer>), Box<End>> {
        let place = match target {
            UploadTarget::Device => PlaceHost::Local,
            UploadTarget::Workspace { workspace } => self.checkout(workspace)?.0,
            UploadTarget::Agent { agent } => match self.shared.places.agent(agent) {
                Some(place) => match place.host {
                    PlaceHost::Local => PlaceHost::Local,
                    PlaceHost::Guest { workspace } => {
                        let link = self.guest(workspace)?;
                        return Ok((
                            Plan::Relay {
                                link,
                                agent: Some(agent),
                                source,
                            },
                            vec![Peer::Client, Peer::Guest(workspace)],
                        ));
                    }
                },
                None => return Err(Box::new(no_agent())),
            },
            UploadTarget::SkillStore { tree } => {
                let Some(store) = self.shared.skills.clone() else {
                    return Err(Box::new(End::refused(Failure::new(
                        ErrorCode::NotAllowed,
                        None,
                        crate::skills::NOT_YET,
                    ))));
                };
                if store.path(&tree).is_none() {
                    return Err(Box::new(End::refused(Failure::new(
                        ErrorCode::BadPath,
                        None,
                        format!("{} isn't a skill tree hash", tree.0),
                    ))));
                }
                let plan = Plan::Tree {
                    tree,
                    store,
                    source,
                };
                return Ok((plan, vec![Peer::Client]));
            }
        };
        match place {
            PlaceHost::Local => Ok((Plan::Stage(source), vec![Peer::Client])),
            PlaceHost::Guest { workspace } => {
                let link = self.guest(workspace)?;
                Ok((
                    Plan::Relay {
                        link,
                        agent: None,
                        source,
                    },
                    vec![Peer::Client, Peer::Guest(workspace)],
                ))
            }
        }
    }

    fn plan_fetch(
        &self,
        transfer: TransferId,
        agent: AgentId,
        path: String,
        anyway: bool,
    ) -> Result<(Plan, Vec<Peer>), Box<End>> {
        let place = self
            .shared
            .places
            .agent(agent)
            .ok_or_else(|| Box::new(no_agent()))?;
        match place.host {
            PlaceHost::Local => Ok((
                Plan::Fetch {
                    place,
                    path,
                    anyway,
                },
                vec![Peer::Client],
            )),
            // The guest reads only inside the agent's folders.
            PlaceHost::Guest { .. } if anyway => Err(Box::new(End::refused(Failure::new(
                ErrorCode::NotAllowed,
                None,
                "Show anyway doesn't reach agents in containers yet",
            )))),
            PlaceHost::Guest { workspace } => {
                let link = self.guest(workspace)?;
                let path =
                    guest_path(&place, &path).map_err(|failure| Box::new(End::refused(failure)))?;
                Ok((
                    Plan::FetchRelay {
                        link,
                        begin: HostToGuest::FetchBegin { transfer, path },
                    },
                    vec![Peer::Client, Peer::Guest(workspace)],
                ))
            }
        }
    }

    fn plan_files(
        &self,
        transfer: TransferId,
        workspace: WorkspaceId,
        paths: Vec<String>,
    ) -> Result<(Plan, Vec<Peer>), Box<End>> {
        if paths.len() > IGNORED_FILES_LIMIT {
            return Err(Box::new(End::refused(Failure::new(
                ErrorCode::TooLarge,
                None,
                format!("a copy takes at most {IGNORED_FILES_LIMIT} files"),
            ))));
        }
        match self.checkout(workspace)? {
            (PlaceHost::Local, root) => Ok((Plan::FetchFiles { root, paths }, vec![Peer::Client])),
            (PlaceHost::Guest { workspace }, root) => {
                let link = self.guest(workspace)?;
                let cwd = guest_cwd(&root).map_err(|failure| Box::new(End::refused(failure)))?;
                Ok((
                    Plan::FetchRelay {
                        link,
                        begin: HostToGuest::FetchFiles {
                            transfer,
                            cwd,
                            paths,
                        },
                    },
                    vec![Peer::Client, Peer::Guest(workspace)],
                ))
            }
        }
    }

    fn plan_tree(&self, tree: TreeHash) -> Result<(Plan, Vec<Peer>), Box<End>> {
        let refused =
            |code, reason: String| Box::new(End::refused(Failure::new(code, None, reason)));
        let store = self
            .shared
            .skills
            .as_ref()
            .ok_or_else(|| refused(ErrorCode::NotAllowed, crate::skills::NOT_YET.to_owned()))?;
        match store.path(&tree) {
            Some(path) if store.has(&tree) => Ok((Plan::FetchTree(path), vec![Peer::Client])),
            Some(_) => Err(refused(
                ErrorCode::BadPath,
                format!("This device's store doesn't have {}", tree.0),
            )),
            None => Err(refused(
                ErrorCode::BadPath,
                format!("{} isn't a skill tree hash", tree.0),
            )),
        }
    }

    /// Whose holders run a workspace's agents, and its folder there.
    fn checkout(&self, workspace: WorkspaceId) -> Result<(PlaceHost, PathBuf), Box<End>> {
        self.shared
            .places
            .workspace(workspace)
            .ok_or_else(|| Box::new(End::refused(no_workspace())))
    }

    fn guest(&self, workspace: WorkspaceId) -> Result<GuestLink, Box<End>> {
        self.shared.link(workspace).ok_or_else(|| {
            Box::new(End::refused(Failure::new(
                ErrorCode::Unreachable,
                None,
                "the workspace's container isn't connected",
            )))
        })
    }

    /// Handles `UploadCancel`, which stops a download too.
    pub fn cancel(&self, transfer: TransferId) {
        if self
            .shared
            .open
            .send(transfer, Peer::Client, Event::Cancel)
            .is_err()
        {
            debug!(%transfer, "dropped a cancel for a transfer that isn't open");
        }
    }

    /// Handles a client's `TransferCredit`, for a download.
    pub fn credit(&self, transfer: TransferId, committed: u64, window_end: u64) {
        let event = Event::ClientCredit {
            committed,
            window_end,
        };
        if self
            .shared
            .open
            .send(transfer, Peer::Client, event)
            .is_err()
        {
            debug!(%transfer, "dropped credit for a transfer that isn't open");
        }
    }

    /// Handles a transfer frame from a client.
    pub fn chunk(&self, transfer: TransferId, offset: u64, data: Vec<u8>) {
        self.shared
            .open
            .chunk(transfer, Peer::Client, offset, data, Event::ClientChunk);
    }

    /// Handles a transfer message from a workspace's guest. Others are
    /// dropped. This and the next two are the guest link's `GuestFiles`.
    pub fn guest_message(&self, workspace: WorkspaceId, message: GuestToHost) {
        let (transfer, event) = match message {
            GuestToHost::UploadStored {
                transfer,
                paths,
                thumbnail,
                text,
            } => (
                transfer,
                FromGuest::Stored(GuestStored {
                    paths,
                    thumbnail,
                    text,
                }),
            ),
            GuestToHost::TransferCredit {
                transfer,
                committed,
                window_end,
            } => (
                transfer,
                FromGuest::Credit {
                    committed,
                    window_end,
                },
            ),
            GuestToHost::FetchManifest { transfer, entries } => {
                (transfer, FromGuest::Manifest(entries))
            }
            GuestToHost::TransferFailed {
                transfer,
                code,
                entry,
                message,
            } => (
                transfer,
                FromGuest::Failed(Failure::new(code, entry, message)),
            ),
            _ => {
                debug!(%workspace, "dropped a guest message that isn't about a transfer");
                return;
            }
        };
        let from = Peer::Guest(workspace);
        if self
            .shared
            .open
            .send(transfer, from, Event::Guest(event))
            .is_err()
        {
            debug!(%transfer, %workspace, "dropped a guest message for a transfer that isn't open");
        }
    }

    /// Handles a transfer frame from a workspace's guest.
    pub fn guest_chunk(
        &self,
        workspace: WorkspaceId,
        transfer: TransferId,
        offset: u64,
        data: Vec<u8>,
    ) {
        self.shared.open.chunk(
            transfer,
            Peer::Guest(workspace),
            offset,
            data,
            Event::GuestChunk,
        );
    }

    /// Fails every transfer through a workspace's guest, whose link has
    /// dropped.
    pub fn guest_down(&self, workspace: WorkspaceId) {
        self.shared
            .open
            .each(Peer::Guest(workspace), || Event::GuestDown);
    }
}

impl GuestFiles for Transfers {
    fn message(&self, workspace: WorkspaceId, message: GuestToHost) {
        self.guest_message(workspace, message);
    }

    fn chunk(&self, workspace: WorkspaceId, transfer: TransferId, offset: u64, data: Vec<u8>) {
        self.guest_chunk(workspace, transfer, offset, data);
    }

    fn link_down(&self, workspace: WorkspaceId) {
        self.guest_down(workspace);
    }
}

impl GuestOut for GuestSender {
    fn message(&self, message: HostToGuest) -> BoxFuture<Result<(), Closed>> {
        let sender = self.clone();
        Box::pin(async move { sender.send(&message).await.map_err(closed) })
    }

    fn chunk(
        &self,
        transfer: TransferId,
        offset: u64,
        data: Vec<u8>,
    ) -> BoxFuture<Result<(), Closed>> {
        let sender = self.clone();
        Box::pin(async move {
            sender
                .transfer_chunks(transfer, offset, &data)
                .await
                .map_err(closed)
        })
    }

    fn request(&self, wait: Duration, make: MakeRequest) -> BoxFuture<Result<GuestToHost, String>> {
        let sender = self.clone();
        Box::pin(async move {
            sender
                .request(wait, make)
                .await
                .map_err(|err| err.to_string())
        })
    }
}

/// A send to a guest that failed, as the transfer sees it: the link is
/// gone for this transfer either way.
fn closed(err: SendError) -> Closed {
    if let SendError::Encode(err) = err {
        warn!(%err, "a transfer message for the guest didn't encode");
    }
    Closed
}

impl Guests for guestlink::Guests {
    fn link(&self, workspace: WorkspaceId) -> Option<Arc<dyn GuestOut>> {
        self.sender(workspace)
            .map(|sender| Arc::new(sender) as Arc<dyn GuestOut>)
    }
}

fn no_workspace() -> Failure {
    Failure::new(
        ErrorCode::NoSuchWorkspace,
        None,
        "this device has no workspace with that ID",
    )
}

fn no_agent() -> End {
    End::Failed {
        failure: Failure::new(
            ErrorCode::NoSuchAgent,
            None,
            "this device has no agent with that ID",
        ),
        outcome: ActionOutcome::UnknownAgent,
    }
}

/// The absolute container path for a download from an agent in a
/// container. The guest checks it stays inside the agent's folders.
fn guest_path(place: &Place, path: &str) -> Result<String, Failure> {
    let bad = |message: &str| Failure::new(ErrorCode::BadPath, None, message);
    if path.contains('\0') {
        return Err(bad("the path has a zero byte"));
    }
    let joined = place.cwd.join(path);
    if joined
        .components()
        .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(bad("the path has a `..` part"));
    }
    joined
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| bad("the path isn't UTF-8"))
}

/// A container checkout's path, as the guest takes it.
fn guest_cwd(root: &Path) -> Result<String, Failure> {
    root.to_str().map(str::to_owned).ok_or_else(|| {
        Failure::new(
            ErrorCode::BadPath,
            None,
            "the workspace's folder isn't UTF-8",
        )
    })
}

/// What happened while a task waited.
type Next = registry::Next<Event>;

/// One transfer's task.
struct Task {
    shared: Arc<Shared>,
    inbox: Inbox<Event>,
    /// Where answers go, or none for a push nobody asked for over a
    /// connection.
    route: Option<Route>,
    /// The key the latest begin carried.
    key: Option<IdempotencyKey>,
    transfer: TransferId,
    /// An event taken early, handled next.
    stash: Option<Event>,
}

impl Task {
    async fn run(mut self, plan: Result<Plan, End>) {
        let end = match plan {
            Err(end) => Some(end),
            Ok(Plan::Stage(source)) => upload::stage(&mut self, source).await,
            Ok(Plan::Tree {
                tree,
                store,
                source,
            }) => upload::store_tree(&mut self, tree, store, source).await,
            Ok(Plan::Relay {
                link,
                agent,
                source,
            }) => upload::relay(&mut self, link, agent, source).await,
            Ok(Plan::Fetch {
                place,
                path,
                anyway,
            }) => fetch::local(&mut self, place, path, anyway).await,
            Ok(Plan::FetchFiles { root, paths }) => fetch::files(&mut self, root, paths).await,
            Ok(Plan::FetchTree(path)) => fetch::folder(&mut self, path).await,
            Ok(Plan::FetchRelay { link, begin }) => fetch::relay(&mut self, link, begin).await,
        };
        if let Some(end) = end {
            self.end(end).await;
        }
    }

    /// Takes the route and key from a begin sent again.
    fn again(&mut self, route: Route, key: IdempotencyKey) {
        self.route = Some(route);
        self.key = Some(key);
    }

    /// The next event, or a tick, or word that nothing came for
    /// [`IDLE`].
    async fn next(&mut self, tick: Option<&mut Interval>) -> Next {
        match self.stash.take() {
            Some(event) => Next::Event(event),
            None => self.inbox.next(IDLE, tick).await,
        }
    }

    /// The next event, waiting no longer than `wait`.
    async fn next_within(&mut self, wait: Duration) -> Option<Event> {
        if let Some(event) = self.stash.take() {
            return Some(event);
        }
        tokio::time::timeout(wait, self.inbox.recv())
            .await
            .ok()
            .flatten()
    }

    /// Whether an event waits, which is kept for [`Task::next`].
    fn has_event(&mut self) -> bool {
        if self.stash.is_none() {
            self.stash = self.inbox.try_recv();
        }
        self.stash.is_some()
    }

    /// `first` and the client chunks that wait after it, up to
    /// [`WRITE_BATCH`] bytes.
    fn client_chunks(&mut self, first: Chunk) -> Vec<Chunk> {
        let mut len = first.data.len();
        let mut chunks = vec![first];
        while len < WRITE_BATCH && self.stash.is_none() {
            match self.inbox.try_recv() {
                Some(Event::ClientChunk(chunk)) => {
                    len += chunk.data.len();
                    chunks.push(chunk);
                }
                Some(other) => self.stash = Some(other),
                None => break,
            }
        }
        chunks
    }

    async fn send(&self, message: ServerMessage) {
        if let Some(route) = &self.route {
            route.message(&message).await;
        }
    }

    /// Sends a transfer's bytes to the client. Says whether it's still
    /// there.
    async fn chunks(&self, chunks: Vec<(u64, Vec<u8>)>) -> bool {
        let Some(route) = &self.route else {
            return false;
        };
        for (offset, data) in chunks {
            if route.chunk(self.transfer, offset, &data).await.is_err() {
                return false;
            }
        }
        true
    }

    async fn credit(&self, (committed, window_end): (u64, u64)) {
        self.send(ServerMessage::TransferCredit {
            transfer: self.transfer,
            committed,
            window_end,
        })
        .await;
    }

    async fn failed(&self, failure: &Failure) {
        let code = match &self.route {
            Some(route) => route.code(failure.code),
            None => failure.code,
        };
        self.send(ServerMessage::TransferFailed {
            transfer: self.transfer,
            code,
            entry: failure.entry,
            message: failure.message.clone(),
        })
        .await;
    }

    /// Sends how the transfer ended, then answers every resend that came
    /// before the transfer closed the same way.
    async fn end(mut self, end: End) {
        self.send_end(&end).await;
        loop {
            while let Some(event) = self.stash.take().or_else(|| self.inbox.try_recv()) {
                if let Event::Again { route, key } = event {
                    self.again(route, key);
                    self.send_end(&end).await;
                }
            }
            if self.inbox.close_if_empty() {
                return;
            }
        }
    }

    async fn send_end(&self, end: &End) {
        let Some(key) = self.key else {
            return;
        };
        let outcome = match end {
            End::Stored(attachment) => {
                self.send(ServerMessage::UploadDone {
                    transfer: self.transfer,
                    attachment: attachment.clone(),
                })
                .await;
                ActionOutcome::Staged {
                    attachment: attachment.clone(),
                }
            }
            End::Fetched | End::Landed => ActionOutcome::Done,
            End::Failed { failure, outcome } => {
                self.failed(failure).await;
                outcome.clone()
            }
            End::Cancelled => ActionOutcome::Refused {
                reason: "the transfer was cancelled".to_owned(),
            },
        };
        self.send(ServerMessage::ActionResult { key, outcome })
            .await;
    }
}

/// Whether `path` is somewhere a container's guest could have saved an
/// upload. A guest's word on a path, and a message naming one for a
/// container's agent, have to pass this.
pub(crate) fn in_guest_attachments(path: &str) -> bool {
    path.strip_prefix(ATTACHMENTS_DIR)
        .is_some_and(|rest| rest.starts_with('/'))
        && path.len() <= MAX_GUEST_PATH
        && !path.contains('\0')
        && !path.split('/').any(|part| part == "..")
}

/// Describes an upload that landed in a container, from what its guest
/// said, and gives where it saved a document's text. The paths are the
/// guest's word, so they're checked. They name files in the container, so
/// the drivers only name them to its agent and never open them on this
/// device. A preview that isn't one, or text anywhere but beside the
/// document, is left out.
fn guest_attachment(
    transfer: TransferId,
    entries: &[ManifestEntry],
    stored: &GuestStored,
) -> Result<(AttachmentRef, Option<String>), Failure> {
    let bad = |message: &str| Failure::new(ErrorCode::BadMessage, None, message);
    let [path] = stored.paths.as_slice() else {
        return Err(bad(
            "the container said the upload landed in more than one place",
        ));
    };
    if !in_guest_attachments(path) {
        return Err(bad(
            "the container said the upload landed somewhere it can't",
        ));
    }
    let checked = manifest::check(entries)?;
    let top = manifest::tops(&checked)
        .into_iter()
        .next()
        .ok_or_else(|| bad("the upload has nothing in it"))?;
    let name = top.path();
    let total = manifest::Layout::of(entries).total();
    let (kind, mime) = match top.kind {
        Kind::Dir => (AttachmentKind::Folder, None),
        _ => match mime_for(&name) {
            Some(mime) if mime.starts_with("image/") => (AttachmentKind::Image, Some(mime)),
            Some(mime) => (AttachmentKind::Pdf, Some(mime)),
            None => (AttachmentKind::File, None),
        },
    };
    let thumbnail = stored
        .thumbnail
        .clone()
        .filter(|url| kind == AttachmentKind::Image && thumbnail::is_preview(url));
    let text = stored.text.clone().filter(|text| {
        kind == AttachmentKind::File
            && text.strip_suffix(".txt") == Some(path.as_str())
            && doctext::wants(Path::new(path))
    });
    let attachment = AttachmentRef {
        id: AttachmentId(transfer.0),
        name,
        kind,
        bytes: total,
        mime: mime.map(str::to_owned),
        path: path.clone(),
        thumbnail,
    };
    Ok((attachment, text))
}

/// The MIME type staging gives a file with this name, as M2's does.
fn mime_for(name: &str) -> Option<&'static str> {
    let ext = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        _ => return None,
    })
}
