// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The devices the app connects to. The registry says which ones you
//! added, each one gets a link from the client, and what the links report
//! goes into the view, apart from timelines and workflow runs, which go to
//! the windows that opened them. The commands that act on a device come through here.
//!
//! An action goes to the device it's for. For an agent that's the device of
//! the agent's workspace, as the view shows it. So do uploads, downloads and
//! forwards, which the client carries on the same links.
//!
//! An inbox change for an agent whose device isn't connected waits in the
//! outbox, in the app's database, and shows on the agent at once. Once the
//! device is back the changes go out in the order they were made, each
//! under the key it got when it was made, so one that went out before
//! can't apply twice. A device that stays connected but doesn't answer gets
//! the outbox again after a wait that grows each time.
//!
//! Once the shell says where MaxHQ's SSH files go, each device that takes
//! editor sessions gets the app's SSH key whenever its link comes up, and
//! the files follow the view's running containers.
//!
//! While two or more devices are connected, the app keeps their project
//! notes in step through this machine's `maxd`, in `devices/notes.rs`.
//!
//! The client sits behind [`Connector`] and [`LinkControl`], so tests drive
//! the devices with fake links.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
#[cfg(feature = "desktop")]
use std::io;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
#[cfg(feature = "desktop")]
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::Duration;

use maxhq_client::forward::{ForwardError, LocalForward};
use maxhq_client::media::Media;
use maxhq_client::{
    Client, Copied, LinkError, LinkEvent, LinkHandle, LinkSpec, TerminalSink, TransferError,
    TransferProgress,
};
use maxhq_protocol::attention::{AnswerOutcome, AttentionAnswer};
use maxhq_protocol::context::InlineRef;
use maxhq_protocol::ids::{
    AccountId, AgentId, AttentionId, DeviceId, ForwardId, IdempotencyKey, LinkId, PortId,
    TerminalId, TransferId, WorkspaceId,
};
use maxhq_protocol::link::{DeviceLink, LinkState, Network, Route};
use maxhq_protocol::message::{
    ActionOutcome, AgentPlace, AgentSpec, AttachmentSource, Capabilities, UploadTarget,
    MAX_INLINE_ATTACHMENT,
};
use maxhq_protocol::model::{DeviceSettings, InboxChange, ListeningPort};
use maxhq_protocol::phones::PhoneBook;
use maxhq_protocol::skills::TreeHash;
use maxhq_protocol::state::{Change, Entity, EntityKey, Seq, ToolPlace};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::workflow::RunKey;
use maxhq_protocol::ClientMessage;
use tokio::runtime::Handle;
use tokio::sync::mpsc::Receiver;
use tokio::task::JoinHandle;

use crate::merge::{Merge, Waiting};
use crate::notify;
use crate::registry::{local_link, Queued, Registry, RegistryError};
use crate::request::CommandError;
#[cfg(feature = "desktop")]
use crate::restart::{self, Here, Restarter};
#[cfg(feature = "desktop")]
use crate::ssh_config::{SshFiles, SshIncludeState};
use crate::terminals::Terminals;
use crate::timeline::{Close, Sink, Timelines, PAGE};
use crate::view::View;
use crate::workflow::{self, Workflows};

#[cfg(feature = "desktop")]
mod app_key;
mod claims;
#[cfg(feature = "desktop")]
mod local_maxd;
mod notes;
mod peers;
#[cfg(feature = "desktop")]
mod phones;
mod push;
mod terminals;
mod workflows;

#[cfg(feature = "desktop")]
pub use peers::SshFacts;
pub use peers::{PeerFacts, PeerLink, PeerLinkState};
#[cfg(feature = "desktop")]
pub use phones::{PhoneDevice, PhonePairing};

/// How long the outbox waits after a link comes up before it goes, so the
/// client has the connection ready for requests by then. A session that
/// picks up where it left off may bring no snapshot to go by.
const RESUME_WAIT: Duration = Duration::from_millis(if cfg!(test) { 20 } else { 500 });

/// How long the outbox waits to go again after a device that stayed
/// connected didn't answer. The wait doubles with each miss in a row, up to
/// [`RETRY_MOST`], from the base and cap decision N-4 gives reconnects.
const RETRY_WAIT: Duration = Duration::from_millis(if cfg!(test) { 20 } else { 1000 });

/// The longest the outbox waits to go again.
const RETRY_MOST: Duration = Duration::from_millis(if cfg!(test) { 80 } else { 60_000 });

/// Hears how far an upload or download has got.
pub type OnProgress = Arc<dyn Fn(TransferProgress) + Send + Sync>;

/// Opens links to devices. The client does it for the app.
pub trait Connector: Send + Sync + 'static {
    /// Controls one open link. Dropping it closes the link.
    type Link: LinkControl;

    /// Opens a link to a device. With `device` it reaches that device only,
    /// and a hello from any other is refused. It reports through the
    /// receiver until the link drops.
    fn open(&self, spec: LinkSpec, device: Option<DeviceId>) -> (Self::Link, Receiver<LinkEvent>);

    /// Calls `on_change` with the networks this machine is on, now and each
    /// time they change.
    fn watch_networks(&self, on_change: Box<dyn Fn(Vec<Network>) + Send + Sync>);

    /// The tailnet peers this machine sees, by name, for setup to offer.
    fn tailnet_hosts(&self) -> Vec<String> {
        Vec::new()
    }

    /// Lets links reach the devices in a phone's books, one book per Mac,
    /// each trusting only its pinned host key. It comes before their links
    /// open. Only the phone's connector reads it.
    fn trust_books(&self, _books: &[PhoneBook]) {}

    /// Has each open link that proved itself prove itself again, such as
    /// after the app made a new key.
    fn reprove(&self) {}

    /// Runs one fixed command on an SSH host over the links' SSH, and
    /// waits for it.
    #[cfg(feature = "desktop")]
    fn run_over_ssh(
        &self,
        _host: &str,
        _command: &str,
    ) -> impl Future<Output = io::Result<maxhq_client::setup::Output>> + Send {
        async {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "this connection can't run commands",
            ))
        }
    }
}

/// What the app asks of one open link.
pub trait LinkControl: Send + Sync + 'static {
    /// Sends a request to the device.
    fn send(&self, message: ClientMessage) -> impl Future<Output = Result<(), LinkError>> + Send;

    /// Sends an action and waits for its result.
    fn act(
        &self,
        action: ClientMessage,
    ) -> impl Future<Output = Result<ActionOutcome, LinkError>> + Send;

    /// Answers an attention event, with the pills in what you typed, and
    /// waits for what came of it.
    fn answer(
        &self,
        key: IdempotencyKey,
        event: AttentionId,
        answer: AttentionAnswer,
        refs: Vec<InlineRef>,
    ) -> impl Future<Output = Result<AnswerOutcome, LinkError>> + Send;

    /// Opens an agent's timeline, whose updates then come as
    /// [`LinkEvent::Timeline`]. With `since` it picks up after that
    /// version, and otherwise starts from the newest items. `lead` names
    /// the lead of a sub-agent.
    fn open_timeline(
        &self,
        agent: AgentId,
        since: Option<Seq>,
        limit: u32,
        lead: Option<AgentId>,
    ) -> impl Future<Output = Result<(), LinkError>> + Send;

    /// Asks for up to `limit` items before the ordinal `before`.
    fn load_older(
        &self,
        agent: AgentId,
        before: u64,
        limit: u32,
    ) -> impl Future<Output = Result<(), LinkError>> + Send;

    /// Closes an agent's timeline.
    fn close_timeline(&self, agent: AgentId) -> impl Future<Output = Result<(), LinkError>> + Send;

    /// Opens a workflow run, whose updates then come as
    /// [`LinkEvent::Workflow`], starting with a reset.
    fn open_workflow(&self, run: RunKey) -> impl Future<Output = Result<(), LinkError>> + Send;

    /// Closes one open of a workflow run.
    fn close_workflow(&self, run: RunKey) -> impl Future<Output = Result<(), LinkError>> + Send;

    /// Probes the device now instead of at the next backoff step.
    fn refresh(&self);

    /// Uploads a file or folder on this machine to `target` on the device,
    /// and waits for what the device made of it.
    fn upload(
        &self,
        target: UploadTarget,
        path: PathBuf,
        on_progress: OnProgress,
    ) -> impl Future<Output = Result<ActionOutcome, TransferError>> + Send;

    /// Uploads bytes, such as a paste, as one file called `name`.
    fn upload_bytes(
        &self,
        target: UploadTarget,
        name: String,
        mime: String,
        bytes: Vec<u8>,
        on_progress: OnProgress,
    ) -> impl Future<Output = Result<ActionOutcome, TransferError>> + Send;

    /// Fetches a file or folder an agent can see into the downloads folder,
    /// and gives where it landed.
    fn download(
        &self,
        agent: AgentId,
        path: String,
        on_progress: OnProgress,
    ) -> impl Future<Output = Result<PathBuf, TransferError>> + Send;

    /// Fetches a file or folder an agent can see into a staging folder on
    /// this machine, to hand on to a device.
    fn copy(
        &self,
        agent: AgentId,
        path: String,
        on_progress: OnProgress,
    ) -> impl Future<Output = Result<Copied, TransferError>> + Send;

    /// Fetches some files in a workspace's checkout into a staging folder
    /// on this machine, as one folder that holds them at their paths.
    fn copy_files(
        &self,
        workspace: WorkspaceId,
        paths: Vec<String>,
        on_progress: OnProgress,
    ) -> impl Future<Output = Result<Copied, TransferError>> + Send;

    /// Fetches a tree in the device's skill store into a staging folder on
    /// this machine, to hand on to another device's store.
    fn copy_tree(
        &self,
        tree: TreeHash,
        on_progress: OnProgress,
    ) -> impl Future<Output = Result<Copied, TransferError>> + Send;

    /// Fetches a picture or video an agent can see into the media cache,
    /// unless it's there already or is larger than `limit` bytes. `anyway`
    /// asks for one file outside the agent's folders.
    fn media(
        &self,
        agent: AgentId,
        path: String,
        limit: u64,
        anyway: bool,
        on_progress: OnProgress,
    ) -> impl Future<Output = Result<Media, TransferError>> + Send;

    /// Stops a transfer on this link. One the link doesn't carry is left
    /// alone.
    fn cancel_transfer(&self, transfer: TransferId);

    /// Forwards a port that listens on the device to one here, and gives
    /// the port it listens on here.
    fn open_forward(
        &self,
        port: ListeningPort,
    ) -> impl Future<Output = Result<u16, ForwardError>> + Send;

    /// Closes the forward to a port, if one is open.
    fn close_forward(&self, port: PortId);

    /// The forwards open on this link.
    fn forwards(&self) -> Vec<LocalForward>;

    /// Attaches a terminal from byte `after` of its output on, which then
    /// goes to `sink`. A reconnect attaches it again from where it got to.
    fn attach_terminal(
        &self,
        terminal: TerminalId,
        after: Option<u64>,
        sink: Arc<dyn TerminalSink>,
    ) -> impl Future<Output = Result<(), LinkError>> + Send;

    /// Detaches a terminal, if it's attached.
    fn detach_terminal(
        &self,
        terminal: TerminalId,
    ) -> impl Future<Output = Result<(), LinkError>> + Send;

    /// Sends what you typed into a terminal. Nothing keeps it.
    fn terminal_input(
        &self,
        terminal: TerminalId,
        bytes: Vec<u8>,
    ) -> impl Future<Output = Result<(), LinkError>> + Send;
}

impl Connector for Client {
    type Link = LinkHandle;

    fn open(&self, spec: LinkSpec, device: Option<DeviceId>) -> (LinkHandle, Receiver<LinkEvent>) {
        match device {
            Some(device) => self.link_to(spec, device),
            None => self.link(spec),
        }
    }

    fn watch_networks(&self, on_change: Box<dyn Fn(Vec<Network>) + Send + Sync>) {
        let mut networks = self.networks();
        tokio::spawn(async move {
            loop {
                let now = networks.borrow_and_update().clone();
                on_change(now);
                if networks.changed().await.is_err() {
                    break;
                }
            }
        });
    }

    #[cfg(feature = "desktop")]
    fn tailnet_hosts(&self) -> Vec<String> {
        Client::tailnet_hosts(self)
    }

    fn reprove(&self) {
        Client::reprove(self);
    }

    #[cfg(feature = "desktop")]
    fn run_over_ssh(
        &self,
        host: &str,
        command: &str,
    ) -> impl Future<Output = io::Result<maxhq_client::setup::Output>> + Send {
        Client::run_over_ssh(self, host, command)
    }
}

/// Runs a restart's commands for real: on this machine as a process, and
/// over SSH through the connector.
#[cfg(feature = "desktop")]
struct SystemRestarter<'a, C>(&'a C);

#[cfg(feature = "desktop")]
impl<C: Connector> Restarter for SystemRestarter<'_, C> {
    fn here(
        &self,
        program: &str,
        args: Vec<String>,
    ) -> impl Future<Output = io::Result<maxhq_client::setup::Output>> + Send {
        use maxhq_client::setup::{Run, System};
        async move { System.run(program, args, None).await }
    }

    fn over_ssh(
        &self,
        host: &str,
        command: &str,
    ) -> impl Future<Output = io::Result<maxhq_client::setup::Output>> + Send {
        self.0.run_over_ssh(host, command)
    }
}

impl LinkControl for LinkHandle {
    fn send(&self, message: ClientMessage) -> impl Future<Output = Result<(), LinkError>> + Send {
        LinkHandle::send(self, message)
    }

    fn act(
        &self,
        action: ClientMessage,
    ) -> impl Future<Output = Result<ActionOutcome, LinkError>> + Send {
        LinkHandle::act(self, action)
    }

    fn answer(
        &self,
        key: IdempotencyKey,
        event: AttentionId,
        answer: AttentionAnswer,
        refs: Vec<InlineRef>,
    ) -> impl Future<Output = Result<AnswerOutcome, LinkError>> + Send {
        LinkHandle::answer(self, key, event, answer, refs)
    }

    fn open_timeline(
        &self,
        agent: AgentId,
        since: Option<Seq>,
        limit: u32,
        lead: Option<AgentId>,
    ) -> impl Future<Output = Result<(), LinkError>> + Send {
        LinkHandle::open_timeline(self, agent, since, limit, lead)
    }

    fn load_older(
        &self,
        agent: AgentId,
        before: u64,
        limit: u32,
    ) -> impl Future<Output = Result<(), LinkError>> + Send {
        LinkHandle::load_older(self, agent, before, limit)
    }

    fn close_timeline(&self, agent: AgentId) -> impl Future<Output = Result<(), LinkError>> + Send {
        LinkHandle::close_timeline(self, agent)
    }

    fn open_workflow(&self, run: RunKey) -> impl Future<Output = Result<(), LinkError>> + Send {
        LinkHandle::open_workflow(self, run)
    }

    fn close_workflow(&self, run: RunKey) -> impl Future<Output = Result<(), LinkError>> + Send {
        LinkHandle::close_workflow(self, run)
    }

    fn refresh(&self) {
        LinkHandle::refresh(self);
    }

    async fn upload(
        &self,
        target: UploadTarget,
        path: PathBuf,
        on_progress: OnProgress,
    ) -> Result<ActionOutcome, TransferError> {
        let report = move |progress| on_progress(progress);
        let mut results = LinkHandle::upload(self, target, vec![path], report).await;
        // One path gives one result.
        results.pop().unwrap_or(Err(TransferError::Cancelled))
    }

    async fn upload_bytes(
        &self,
        target: UploadTarget,
        name: String,
        mime: String,
        bytes: Vec<u8>,
        on_progress: OnProgress,
    ) -> Result<ActionOutcome, TransferError> {
        let report = move |progress| on_progress(progress);
        LinkHandle::upload_bytes(self, target, name, &mime, bytes, report).await
    }

    fn download(
        &self,
        agent: AgentId,
        path: String,
        on_progress: OnProgress,
    ) -> impl Future<Output = Result<PathBuf, TransferError>> + Send {
        LinkHandle::download(self, agent, path, move |progress| on_progress(progress))
    }

    fn copy(
        &self,
        agent: AgentId,
        path: String,
        on_progress: OnProgress,
    ) -> impl Future<Output = Result<Copied, TransferError>> + Send {
        LinkHandle::copy(self, agent, path, move |progress| on_progress(progress))
    }

    fn copy_files(
        &self,
        workspace: WorkspaceId,
        paths: Vec<String>,
        on_progress: OnProgress,
    ) -> impl Future<Output = Result<Copied, TransferError>> + Send {
        LinkHandle::copy_files(self, workspace, paths, move |progress| {
            on_progress(progress)
        })
    }

    fn copy_tree(
        &self,
        tree: TreeHash,
        on_progress: OnProgress,
    ) -> impl Future<Output = Result<Copied, TransferError>> + Send {
        LinkHandle::copy_tree(self, tree, move |progress| on_progress(progress))
    }

    fn media(
        &self,
        agent: AgentId,
        path: String,
        limit: u64,
        anyway: bool,
        on_progress: OnProgress,
    ) -> impl Future<Output = Result<Media, TransferError>> + Send {
        LinkHandle::media(self, agent, path, limit, anyway, move |progress| {
            on_progress(progress)
        })
    }

    fn cancel_transfer(&self, transfer: TransferId) {
        LinkHandle::cancel_transfer(self, transfer);
    }

    async fn open_forward(&self, port: ListeningPort) -> Result<u16, ForwardError> {
        LinkHandle::open_forward(self, &port).await
    }

    fn close_forward(&self, port: PortId) {
        LinkHandle::close_forward(self, port);
    }

    fn forwards(&self) -> Vec<LocalForward> {
        LinkHandle::forwards(self)
    }

    fn attach_terminal(
        &self,
        terminal: TerminalId,
        after: Option<u64>,
        sink: Arc<dyn TerminalSink>,
    ) -> impl Future<Output = Result<(), LinkError>> + Send {
        LinkHandle::attach_terminal(self, terminal, after, sink)
    }

    fn detach_terminal(
        &self,
        terminal: TerminalId,
    ) -> impl Future<Output = Result<(), LinkError>> + Send {
        LinkHandle::detach_terminal(self, terminal)
    }

    async fn terminal_input(&self, terminal: TerminalId, bytes: Vec<u8>) -> Result<(), LinkError> {
        LinkHandle::terminal_input(self, terminal, &bytes).await
    }
}

/// Starts the Mac's client on `runtime`, as [`start_client`] does.
#[cfg(feature = "desktop")]
pub fn client(runtime: &Handle, downloads: Option<PathBuf>, media: Option<PathBuf>) -> Client {
    start_client(runtime, Client::new, downloads, media)
}

/// Starts the client `make` gives on `runtime`, since it starts tasks.
/// With `downloads`, files from devices land in that folder, and with
/// `media`, the pictures and videos the timeline shows are kept in that
/// one.
pub fn start_client(
    runtime: &Handle,
    make: impl FnOnce() -> Client,
    downloads: Option<PathBuf>,
    media: Option<PathBuf>,
) -> Client {
    let _entered = runtime.enter();
    let mut client = make();
    if let Some(folder) = downloads {
        client = client.downloading_into(folder);
    }
    if let Some(folder) = media {
        client = client.caching_media_in(folder);
    }
    client
}

/// Where the `maxd` that ships with the app would be: beside the app's own
/// binary, which in the bundle is `Contents/MacOS`. In a dev build that's
/// the target folder, where Cargo puts `maxd` too.
#[cfg(feature = "desktop")]
pub fn bundled_maxd() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.with_file_name(format!("maxd{}", std::env::consts::EXE_SUFFIX)))
}

/// Every device you added, each with its open link.
pub struct Devices<C: Connector> {
    connector: Arc<C>,
    /// What the commands share with the tasks that forward each link's
    /// reports.
    core: Arc<Core<C::Link>>,
    /// The `maxd` that `start_local_maxd` installs, if this build knows
    /// where one would be. The phone has none.
    #[cfg_attr(not(feature = "desktop"), allow(dead_code))]
    maxd: Option<PathBuf>,
}

/// The open links, the view they feed and the timelines and workflow runs
/// they carry.
struct Core<L> {
    links: Mutex<BTreeMap<LinkId, Open<L>>>,
    /// Opens a link, through the connector.
    dial: Dial<L>,
    /// Hands the connector a phone's books, before their links open.
    trust: Trust,
    view: View,
    /// The devices you added, and the outbox.
    registry: Mutex<Registry>,
    /// One turn for each device's outbox, held while its changes go out so
    /// they reach the device in the order they were made.
    turns: Mutex<BTreeMap<DeviceId, Arc<tokio::sync::Mutex<Turn>>>>,
    /// Every window's timeline channel.
    timelines: Timelines,
    /// The lead of each sub-agent whose timeline a window opened, so its
    /// timeline still opens through the lead once it finishes and the view
    /// no longer lists it.
    leads: Mutex<HashMap<AgentId, AgentId>>,
    /// Every window's workflow channel.
    workflows: Workflows,
    /// Every window's terminal channel.
    terminals: Terminals,
    /// Held from changing which timelines and runs a link should have open
    /// until
    /// the link has the change, so the links get the changes in the order
    /// they were made. Without it a close could reach a link before the
    /// open it takes back, which leaves the timeline open there for good.
    order: tokio::sync::Mutex<()>,
    /// Runs the tasks the devices start, and the client's, which start
    /// tasks of their own on whatever runtime they're called from.
    runtime: Handle,
    /// MaxHQ's SSH files, once the shell says where they go.
    #[cfg(feature = "desktop")]
    ssh: OnceLock<SshFiles>,
    /// Whether a rewrite of the SSH files is on its way, so a burst of
    /// reports brings one.
    #[cfg(feature = "desktop")]
    ssh_due: AtomicBool,
    /// What links the devices to each other, once the shell gives it.
    peers: OnceLock<Box<dyn peers::PeerFacts>>,
    /// Whether a pass over the device links is on its way.
    peers_due: AtomicBool,
    /// Held through a pass over the device links or the phones, so one
    /// runs at a time.
    peer_turn: tokio::sync::Mutex<()>,
    /// Whether a pass over the phones is on its way.
    #[cfg(feature = "desktop")]
    phones_due: AtomicBool,
    /// The codes devices gave for phones the Mac let in, until each phone
    /// has its token there. Only in memory, so no code reaches the disk.
    #[cfg(feature = "desktop")]
    phone_codes: Mutex<phones::Codes>,
    /// The links whose snapshot has come in since the app opened, for the
    /// first pass over the phones to know it has seen them all.
    #[cfg(feature = "desktop")]
    phone_synced: Mutex<std::collections::BTreeSet<LinkId>>,
    /// The phones' books this run has sent this Mac's name to, by phone
    /// and the device that keeps each, so a keeper too old to keep the
    /// name isn't sent it again and again.
    #[cfg(feature = "desktop")]
    phone_named: Mutex<std::collections::BTreeSet<(maxhq_protocol::ids::PhoneId, DeviceId)>>,
    /// This Mac's app key, once the shell gives it.
    #[cfg(feature = "desktop")]
    app_key: OnceLock<crate::app_keys::AppKeys>,
    /// The phone's push token and presence.
    push: push::Push,
    /// Keeps the devices' project notes in step.
    notes: notes::Relay,
}

/// How the core opens a link.
type Dial<L> = Box<dyn Fn(LinkSpec, Option<DeviceId>) -> (L, Receiver<LinkEvent>) + Send + Sync>;

/// How the core hands the connector a phone's books.
type Trust = Box<dyn Fn(&[PhoneBook]) + Send + Sync>;

/// One open link.
struct Open<L> {
    link: Arc<L>,
    /// Feeds what the link reports into the view and the timelines.
    forward: JoinHandle<()>,
}

/// A link a request can go through, with the device's name for errors.
type Target = (LinkId, String);

/// What a device's outbox remembers between its turns.
#[derive(Default)]
struct Turn {
    /// How many times in a row the device didn't answer while it stayed
    /// connected.
    misses: u32,
    /// Whether the outbox is waiting to go again.
    retrying: bool,
}

impl<L> Drop for Open<L> {
    fn drop(&mut self) {
        self.forward.abort();
    }
}

impl<C: Connector> Devices<C> {
    /// Opens a link to every device in the registry, this machine first,
    /// and keeps the view's networks current. Every task they start runs
    /// on `runtime`.
    pub fn start(
        runtime: Handle,
        connector: C,
        registry: Registry,
        view: View,
        maxd: Option<PathBuf>,
    ) -> Self {
        // The app's setup starts the devices from outside the runtime.
        let _entered = runtime.enter();
        let specs = registry.list().unwrap_or_else(|error| {
            tracing::error!(%error, "couldn't read the device list");
            Vec::new()
        });
        let networks = view.clone();
        connector.watch_networks(Box::new(move |now| {
            networks.update(|merge| merge.set_networks(now));
        }));
        if let Ok(books) = registry.phone_books() {
            connector.trust_books(&books);
        }
        let connector = Arc::new(connector);
        let dialer = Arc::clone(&connector);
        let truster = Arc::clone(&connector);
        let devices = Self {
            connector,
            core: Arc::new(Core {
                links: Mutex::new(BTreeMap::new()),
                dial: Box::new(move |spec, device| dialer.open(spec, device)),
                trust: Box::new(move |books| truster.trust_books(books)),
                view,
                registry: Mutex::new(registry),
                turns: Mutex::new(BTreeMap::new()),
                timelines: Timelines::start(),
                leads: Mutex::default(),
                workflows: Workflows::default(),
                terminals: Terminals::start(),
                order: tokio::sync::Mutex::new(()),
                runtime: runtime.clone(),
                #[cfg(feature = "desktop")]
                ssh: OnceLock::new(),
                #[cfg(feature = "desktop")]
                ssh_due: AtomicBool::new(false),
                peers: OnceLock::new(),
                peers_due: AtomicBool::new(false),
                peer_turn: tokio::sync::Mutex::new(()),
                #[cfg(feature = "desktop")]
                phones_due: AtomicBool::new(false),
                #[cfg(feature = "desktop")]
                phone_codes: Mutex::default(),
                #[cfg(feature = "desktop")]
                phone_synced: Mutex::default(),
                #[cfg(feature = "desktop")]
                phone_named: Mutex::default(),
                #[cfg(feature = "desktop")]
                app_key: OnceLock::new(),
                push: push::Push::default(),
                notes: notes::Relay::default(),
            }),
            maxd,
        };
        // What waited from the last run shows as each agent comes back.
        devices.core.show_waiting();
        for spec in specs {
            devices.core.open(spec);
        }
        devices
    }

    /// Records that you looked at an agent, through a link that holds it.
    pub async fn mark_seen(&self, agent: AgentId) -> Result<(), CommandError> {
        let message = ClientMessage::MarkSeen { agent };
        self.send_through(EntityKey::Agent(agent), "This agent", message)
            .await
    }

    /// Asks for usage now. One account's request goes through a link that
    /// holds it, and a request for every account goes to every device.
    pub async fn refresh_usage(&self, account: Option<AccountId>) -> Result<(), CommandError> {
        let message = ClientMessage::RefreshUsage { account };
        match account {
            Some(id) => {
                self.send_through(EntityKey::Account(id), "This account", message)
                    .await
            }
            None => self.send_everywhere(message).await,
        }
    }

    /// Probes one device's link now, or every device's.
    pub fn refresh_links(&self, link: Option<LinkId>) -> Result<(), CommandError> {
        let links = lock(&self.core.links);
        match link {
            Some(id) => links
                .get(&id)
                .ok_or_else(|| gone("This device"))?
                .link
                .refresh(),
            None => links.values().for_each(|open| open.link.refresh()),
        }
        Ok(())
    }

    /// Adds an SSH host and starts connecting to it. Adding one you have
    /// returns it.
    pub fn add_device(&self, host: &str) -> Result<LinkId, CommandError> {
        let spec = lock(&self.core.registry).add(host)?;
        let id = spec.id;
        self.core.open(spec);
        Ok(id)
    }

    /// Renames a device.
    pub fn rename_device(&self, link: LinkId, name: &str) -> Result<(), CommandError> {
        let spec = lock(&self.core.registry).rename(link, name)?;
        self.core
            .view
            .update(|merge| merge.rename_link(link, &spec.name));
        self.core.sync_peers();
        Ok(())
    }

    /// Sets the hosts a device is reached at, in the order to try them,
    /// and opens its link again on them.
    pub fn set_routes(&self, link: LinkId, hosts: &[String]) -> Result<(), CommandError> {
        let spec = lock(&self.core.registry).set_routes(link, hosts)?;
        drop(lock(&self.core.links).remove(&link));
        self.core.open(spec);
        self.core.sync_peers();
        Ok(())
    }

    /// The tailnet peers this machine sees, for setup to offer.
    pub fn tailnet_hosts(&self) -> Vec<String> {
        self.connector.tailnet_hosts()
    }

    /// The app's own database, for its settings. Let go of it before
    /// anything waits.
    pub fn registry(&self) -> MutexGuard<'_, Registry> {
        lock(&self.core.registry)
    }

    /// Forgets a device and closes its link, which takes what only it
    /// reported out of the view. This machine stays. Timelines that came
    /// through the link move to another link to the same device, and so do
    /// the changes waiting for it. Without one, they're dropped.
    pub fn remove_device(&self, link: LinkId) -> Result<(), CommandError> {
        let heir = self.core.view.read(|merge| heir(merge, link));
        {
            let registry = lock(&self.core.registry);
            if let Some(heir) = heir.filter(|_| link != local_link()) {
                registry.hand_over(link, heir)?;
            }
            registry.remove(link)?;
        }
        self.core.drop_links(&[link]);
        self.core.sync_peers();
        Ok(())
    }

    /// Puts `book`, the book the phone got when it paired with a Mac, in
    /// place of the one it had from that Mac, or beside the others for a
    /// new Mac. Later books come from the device that keeps each one.
    /// Answers the devices no book lists any more.
    pub fn put_phone_book(&self, book: &PhoneBook) -> Result<Vec<DeviceId>, CommandError> {
        let book = book.clone();
        Ok(self
            .core
            .change_books(|books| crate::phone::put_book(books, book))?)
    }

    /// Forgets the book `keeper` keeps, and with it each device no other
    /// book lists, with what waited for it. The other Macs' books stay.
    /// Answers the devices that went.
    pub fn forget_phone_book(&self, keeper: DeviceId) -> Result<Vec<DeviceId>, CommandError> {
        Ok(self
            .core
            .change_books(|books| books.retain(|book| book.device != keeper))?)
    }

    /// Restarts the `maxd` behind a link through its service manager, not
    /// through `maxd`, so it works on one too old to read the app's
    /// requests. Then probes the link so the app connects again without
    /// waiting for the next retry.
    #[cfg(feature = "desktop")]
    pub async fn restart_maxd(&self, link: LinkId) -> Result<(), CommandError> {
        let runner = SystemRestarter(&*self.connector);
        self.restart_maxd_with(link, Here::now(), &runner).await
    }

    /// [`Self::restart_maxd`], with what runs the commands.
    #[cfg(feature = "desktop")]
    pub(crate) async fn restart_maxd_with(
        &self,
        link: LinkId,
        here: Here,
        runner: &impl Restarter,
    ) -> Result<(), CommandError> {
        let route = self
            .core
            .view
            .read(
                |merge| match merge.store().get(&EntityKey::DeviceLink(link)) {
                    Some(Entity::DeviceLink(found)) => Some(found.route.clone()),
                    _ => None,
                },
            )
            .ok_or_else(|| gone("This device"))?;
        restart::restart(&route, here, runner).await?;
        self.refresh_links(Some(link))
    }

    /// Sends an action to the device it's for, and waits for its result.
    /// One for an agent goes to the device of the agent's workspace, a new
    /// agent to the device of its workspace or account, one for a workspace
    /// to the workspace's device, and settings to their device. Give each
    /// action a fresh key when you create it.
    pub async fn act(&self, action: ClientMessage) -> Result<ActionOutcome, CommandError> {
        let targets = self
            .core
            .view
            .read(|merge| action_targets(merge, &action))?;
        self.act_through(targets, action).await
    }

    /// Like [`Self::act`], but none when the device's `maxd` is too old to
    /// read the action, for a request whose answer says so.
    pub async fn act_unless_old(
        &self,
        action: ClientMessage,
    ) -> Result<Option<ActionOutcome>, CommandError> {
        let targets = self
            .core
            .view
            .read(|merge| action_targets(merge, &action))?;
        self.core.act_unless_old(targets, action).await
    }

    /// Sends an action to a device you name, for an action that doesn't
    /// say which device it's for, and waits for its result.
    pub async fn act_on(
        &self,
        device: DeviceId,
        action: ClientMessage,
    ) -> Result<ActionOutcome, CommandError> {
        let targets = self.core.view.read(|merge| device_targets(merge, device))?;
        self.act_through(targets, action).await
    }

    /// Like [`Self::act_on`], but none when the device's `maxd` is too old
    /// to read the action.
    pub async fn act_on_unless_old(
        &self,
        device: DeviceId,
        action: ClientMessage,
    ) -> Result<Option<ActionOutcome>, CommandError> {
        let targets = self.core.view.read(|merge| device_targets(merge, device))?;
        self.core.act_unless_old(targets, action).await
    }

    /// Saves a device's settings by sending what differs from the row this
    /// app shows for it, so a setting nobody changed stays as the device
    /// keeps it. An older `maxd` reads only a whole save.
    pub async fn save_settings(
        &self,
        settings: DeviceSettings,
    ) -> Result<ActionOutcome, CommandError> {
        let device = settings.device;
        let changes = self.core.view.read(|merge| {
            let shown = match merge.store().get(&EntityKey::DeviceSettings(device)) {
                Some(Entity::DeviceSettings(shown)) => Some(shown),
                _ => None,
            };
            Ok::<_, CommandError>(settings.changes_from(shown))
        })?;
        if changes.is_empty() {
            return Ok(ActionOutcome::Done);
        }
        let update = ClientMessage::UpdateDeviceSettings {
            key: IdempotencyKey::generate(),
            device,
            changes,
        };
        match self.act_on_unless_old(device, update).await? {
            Some(outcome) => Ok(outcome),
            None => {
                let whole = ClientMessage::SetDeviceSettings {
                    key: IdempotencyKey::generate(),
                    settings,
                };
                self.act_on(device, whole).await
            }
        }
    }

    /// Sends an action to each device in turn, under a fresh key for each,
    /// and gives one result for each device in the same order.
    pub async fn act_on_each(
        &self,
        devices: &[DeviceId],
        action: impl Fn(IdempotencyKey) -> ClientMessage,
    ) -> Vec<Result<ActionOutcome, CommandError>> {
        let mut outcomes = Vec::with_capacity(devices.len());
        for &device in devices {
            let action = action(IdempotencyKey::generate());
            outcomes.push(self.act_on(device, action).await);
        }
        outcomes
    }

    /// Changes where each agent sits in the sidebar, one action for each
    /// under a fresh key, and gives one result for each agent in the same
    /// order. A `maxd` from before inboxes is refused here in words, since
    /// it would only say it can't. A change for a device that isn't
    /// connected waits in the outbox and comes back done.
    pub async fn set_inbox(
        &self,
        agents: &[AgentId],
        change: &InboxChange,
    ) -> Vec<Result<ActionOutcome, CommandError>> {
        let mut outcomes = Vec::with_capacity(agents.len());
        for &agent in agents {
            let core = Arc::clone(&self.core);
            outcomes.push(core.set_inbox(agent, change.clone()).await);
        }
        outcomes
    }

    /// Answers an approval or a question on the device of the agent that
    /// asked, with the pills in what you typed. An answer that came too
    /// late comes back refused, with why.
    pub async fn answer(
        &self,
        event: AttentionId,
        answer: AttentionAnswer,
        refs: Vec<InlineRef>,
    ) -> Result<ActionOutcome, CommandError> {
        let targets =
            self.core.view.read(
                |merge| match merge.store().get(&EntityKey::Attention(event)) {
                    Some(Entity::Attention(found)) => agent_targets(merge, found.agent),
                    _ => Err(gone("This request")),
                },
            )?;
        let key = IdempotencyKey::generate();
        let outcome = self
            .core
            .request_through(targets, move |link| {
                let (answer, refs) = (answer.clone(), refs.clone());
                async move { link.answer(key, event, answer, refs).await }
            })
            .await?;
        Ok(answered(outcome))
    }

    /// Readies a file, a folder or pasted bytes on a device, to send to an
    /// agent there.
    pub async fn stage(
        &self,
        device: DeviceId,
        source: AttachmentSource,
    ) -> Result<ActionOutcome, CommandError> {
        if let AttachmentSource::Bytes { data, .. } = &source {
            match decoded_len(data) {
                None => return Err(failed("The pasted data isn't base64".to_owned())),
                Some(len) if len > MAX_INLINE_ATTACHMENT => {
                    return Err(failed(format!(
                        "A paste can be at most {} MiB",
                        MAX_INLINE_ATTACHMENT >> 20
                    )));
                }
                Some(_) => {}
            }
        }
        let action = ClientMessage::StageAttachment {
            key: IdempotencyKey::generate(),
            source,
        };
        self.act_on(device, action).await
    }

    /// Has the device check its GitHub App now. A `maxd` from before the
    /// request is refused here in words, since it wouldn't answer at all.
    pub async fn check_github_app(&self, device: DeviceId) -> Result<ActionOutcome, CommandError> {
        let targets = self.core.view.read(|merge| {
            let targets = device_targets(merge, device)?;
            if capabilities(merge, device).is_some_and(|does| !does.github_checks) {
                let name = targets.first().map_or("The device", |(_, name)| name);
                return Err(failed(format!(
                    "{name}'s maxd is too old to check when asked. Update it, or wait for it to check again by itself"
                )));
            }
            Ok(targets)
        })?;
        let action = ClientMessage::CheckGithubApp {
            key: IdempotencyKey::generate(),
        };
        self.act_through(targets, action).await
    }

    /// Lists the files git ignores in a workspace's checkout, for a copy
    /// of its project to take along. A `maxd` from before copying is
    /// refused here in words, since it wouldn't answer at all.
    pub async fn list_ignored_files(
        &self,
        workspace: WorkspaceId,
    ) -> Result<ActionOutcome, CommandError> {
        let targets = self.core.view.read(|merge| {
            let device = workspace_device(merge, workspace)?;
            copy_targets(merge, device)
        })?;
        let action = ClientMessage::ListIgnoredFiles {
            key: IdempotencyKey::generate(),
            workspace,
        };
        self.act_through(targets, action).await
    }

    /// Refuses a device whose `maxd` is from before copying projects, in
    /// words, since it wouldn't answer at all.
    pub fn check_copies(&self, device: DeviceId) -> Result<(), CommandError> {
        self.core
            .view
            .read(|merge| copy_targets(merge, device).map(drop))
    }

    /// Whether this machine reaches the device without a network, through
    /// a link over [`Route::Local`].
    pub fn is_local(&self, device: DeviceId) -> bool {
        self.core.view.read(|merge| is_local(merge, device))
    }

    /// How this machine reaches the device: [`Route::Local`] when a link
    /// does, else a connected link's route first. None for a device with no
    /// link.
    pub fn route(&self, device: DeviceId) -> Option<Route> {
        self.core
            .view
            .read(|merge| best_link(merge, device).map(|link| link.route.clone()))
    }

    /// What a device does, from the last hello one of its links had, or
    /// None while none has had one.
    pub fn capabilities(&self, device: DeviceId) -> Option<Capabilities> {
        self.core.view.read(|merge| capabilities(merge, device))
    }

    /// Keeps MaxHQ's SSH files in `dir`, with the app's key, made the
    /// first time, and lets `home`'s `~/.ssh/config` include them once you
    /// say so. Every connected device that takes editor sessions gets the
    /// key now, and each one again whenever its link comes up.
    #[cfg(feature = "desktop")]
    pub fn keep_ssh(&self, dir: PathBuf, home: PathBuf) -> io::Result<()> {
        let files = SshFiles::open(dir, home, self.maxd.clone())?;
        if self.core.ssh.set(files).is_err() {
            return Ok(());
        }
        let _entered = self.core.runtime.enter();
        let open: Vec<LinkId> = lock(&self.core.links).keys().copied().collect();
        let up: Vec<LinkId> = self.core.view.read(|merge| {
            open.into_iter()
                .filter(|id| connected(merge, *id) && takes_ssh(merge, *id))
                .collect()
        });
        for id in up {
            self.core.push_ssh_key(id);
        }
        self.core.rewrite_ssh();
        Ok(())
    }

    /// Where an editor stands with a container workspace over SSH. With
    /// `add` it first adds the `Include` line to your `~/.ssh/config`, or
    /// remembers that you said no.
    #[cfg(feature = "desktop")]
    pub async fn ssh_include(
        &self,
        workspace: WorkspaceId,
        add: Option<bool>,
    ) -> Result<SshIncludeState, CommandError> {
        let core = Arc::clone(&self.core);
        self.core
            .runtime
            .spawn_blocking(move || core.ssh_include(workspace, add))
            .await
            .map_err(|error| failed(sentence(error.to_string())))?
    }

    /// Uploads a file or folder on this machine to `target` on a device,
    /// and gives what the device made of it. `on_progress` hears how far
    /// it has got. A device that doesn't take files refuses, with why.
    pub async fn upload(
        &self,
        device: DeviceId,
        target: UploadTarget,
        path: PathBuf,
        on_progress: OnProgress,
    ) -> Result<ActionOutcome, CommandError> {
        let targets = self.core.view.read(|merge| device_targets(merge, device))?;
        self.core
            .request_through(targets, move |link| {
                let (target, path) = (target.clone(), path.clone());
                let on_progress = Arc::clone(&on_progress);
                async move { refused_if_unsupported(link.upload(target, path, on_progress).await) }
            })
            .await
    }

    /// Uploads bytes, such as a paste, to `target` on a device as one file
    /// called `name`, the way [`Devices::upload`] does a path.
    pub async fn upload_bytes(
        &self,
        device: DeviceId,
        target: UploadTarget,
        name: String,
        mime: String,
        bytes: Vec<u8>,
        on_progress: OnProgress,
    ) -> Result<ActionOutcome, CommandError> {
        let targets = self.core.view.read(|merge| device_targets(merge, device))?;
        self.core
            .request_through(targets, move |link| {
                let (target, name, mime) = (target.clone(), name.clone(), mime.clone());
                let (bytes, on_progress) = (bytes.clone(), Arc::clone(&on_progress));
                async move {
                    let uploaded = link.upload_bytes(target, name, mime, bytes, on_progress);
                    refused_if_unsupported(uploaded.await)
                }
            })
            .await
    }

    /// Fetches a file or folder an agent can see into the downloads folder,
    /// through a link to the agent's device, and gives where it landed.
    /// `on_progress` hears how far it has got.
    pub async fn download(
        &self,
        agent: AgentId,
        path: String,
        on_progress: OnProgress,
    ) -> Result<PathBuf, CommandError> {
        let targets = self.core.view.read(|merge| agent_targets(merge, agent))?;
        self.core
            .request_through(targets, move |link| {
                let (path, on_progress) = (path.clone(), Arc::clone(&on_progress));
                async move { link.download(agent, path, on_progress).await }
            })
            .await
    }

    /// Fetches a file or folder an agent can see into a staging folder on
    /// this machine, through a link to the agent's device, to hand on to
    /// another. The device decides what the agent can see, as it does for
    /// a download.
    pub async fn copy(
        &self,
        agent: AgentId,
        path: String,
        on_progress: OnProgress,
    ) -> Result<Copied, CommandError> {
        let targets = self.core.view.read(|merge| agent_targets(merge, agent))?;
        self.core
            .request_through(targets, move |link| {
                let (path, on_progress) = (path.clone(), Arc::clone(&on_progress));
                async move { link.copy(agent, path, on_progress).await }
            })
            .await
    }

    /// Fetches some files in a workspace's checkout into a staging folder
    /// on this machine, through a link to the workspace's device, as one
    /// folder that holds them at their paths. The device decides which
    /// paths it hands out.
    pub async fn copy_files(
        &self,
        workspace: WorkspaceId,
        paths: Vec<String>,
        on_progress: OnProgress,
    ) -> Result<Copied, CommandError> {
        let targets = self.core.view.read(|merge| {
            let device = workspace_device(merge, workspace)?;
            device_targets(merge, device)
        })?;
        self.core
            .request_through(targets, move |link| {
                let (paths, on_progress) = (paths.clone(), Arc::clone(&on_progress));
                async move { link.copy_files(workspace, paths, on_progress).await }
            })
            .await
    }

    /// Fetches a tree in a device's skill store into a staging folder on
    /// this machine, through a link to that device, to hand on to another
    /// device's store.
    pub async fn copy_tree(
        &self,
        device: DeviceId,
        tree: TreeHash,
        on_progress: OnProgress,
    ) -> Result<Copied, CommandError> {
        let targets = self.core.view.read(|merge| device_targets(merge, device))?;
        self.core
            .request_through(targets, move |link| {
                let (tree, on_progress) = (tree.clone(), Arc::clone(&on_progress));
                async move { link.copy_tree(tree, on_progress).await }
            })
            .await
    }

    /// Fetches a picture or video an agent can see into the media cache,
    /// through a link to the agent's device, unless it's there already or
    /// is larger than `limit` bytes. The device decides what the agent can
    /// see, as it does for a download, unless `anyway` asks for one file
    /// outside its folders.
    pub async fn media(
        &self,
        agent: AgentId,
        path: String,
        limit: u64,
        anyway: bool,
        on_progress: OnProgress,
    ) -> Result<Media, CommandError> {
        let targets = self.core.view.read(|merge| agent_targets(merge, agent))?;
        self.core
            .request_through(targets, move |link| {
                let (path, on_progress) = (path.clone(), Arc::clone(&on_progress));
                async move { link.media(agent, path, limit, anyway, on_progress).await }
            })
            .await
    }

    /// Stops an upload or download. Every link hears it and only the one
    /// carrying the transfer acts, so one that ended changes nothing.
    pub fn cancel_transfer(&self, transfer: TransferId) {
        for link in self.core.all_links() {
            link.cancel_transfer(transfer);
        }
    }

    /// Forwards a port that listens in a workspace to a port on this
    /// machine, the same number or the next free one, and gives that port
    /// with the device's forward once it has answered. A device that
    /// doesn't forward ports refuses before anything listens here.
    pub async fn open_forward(&self, port: PortId) -> Result<Forwarded, CommandError> {
        let (listening, targets) = self.core.view.read(|merge| {
            let Some(Entity::ListeningPort(found)) =
                merge.store().get(&EntityKey::ListeningPort(port))
            else {
                return Err(gone("This port"));
            };
            let device = workspace_device(merge, found.workspace)?;
            let targets = device_targets(merge, device)?;
            if capabilities(merge, device).is_some_and(|does| !does.tunnels) {
                let name = targets.first().map_or("The device", |(_, name)| name);
                return Err(failed(format!("{name} doesn't forward ports")));
            }
            Ok((found.clone(), targets))
        })?;
        self.core
            .request_through(targets, move |link| {
                let listening = listening.clone();
                async move {
                    let local_port = link.open_forward(listening.clone()).await?;
                    let forward = link
                        .forwards()
                        .into_iter()
                        .find(|open| open.port == listening.id)
                        .and_then(|open| open.forward);
                    Ok::<_, ForwardError>(Forwarded {
                        local_port,
                        forward,
                    })
                }
            })
            .await
    }

    /// Closes a forward this machine opened. One whose link is
    /// reconnecting has no forward from the device yet, so it's found by
    /// its port. A forward that's gone changes nothing, and one another
    /// computer opened can't be closed from here.
    pub fn close_forward(&self, forward: ForwardId) -> Result<(), CommandError> {
        let port =
            self.core.view.read(
                |merge| match merge.store().get(&EntityKey::Forward(forward)) {
                    Some(Entity::Forward(found)) => Some(found.port),
                    _ => None,
                },
            );
        let mut closed = false;
        for link in self.core.all_links() {
            for local in link.forwards() {
                let ours = match local.forward {
                    Some(id) => id == forward,
                    None => port == Some(local.port),
                };
                if ours {
                    link.close_forward(local.port);
                    closed = true;
                }
            }
        }
        if closed || port.is_none() {
            Ok(())
        } else {
            Err(failed(
                "Another computer opened this forward, so only it can stop it".to_owned(),
            ))
        }
    }

    /// Has a window watch an agent's timeline through its channel. The
    /// first window to watch it opens it on a link to the agent's device,
    /// and a window that joins later gets what the app has at once.
    pub async fn open_timeline(
        &self,
        window: &str,
        sink: Arc<dyn Sink>,
        agent: AgentId,
    ) -> Result<(), CommandError> {
        self.core.open_timeline(window, sink, agent).await
    }

    /// Takes back one of a window's opens of an agent's timeline. The
    /// timeline closes once no window watches it.
    pub async fn close_timeline(&self, window: &str, agent: AgentId) {
        if let Some(close) = self.core.timelines.unsubscribe(window, agent) {
            let _order = self.core.order.lock().await;
            self.core.close_all(vec![close]).await;
        }
    }

    /// Lets go of a window that closed or reloaded, and of every timeline
    /// it watched, every workflow run and every terminal. The window lets
    /// go at once, and the timelines no window watches now close on their
    /// links in the background.
    pub fn release_window(&self, window: &str) -> JoinHandle<()> {
        let closes = self.core.timelines.release(window);
        let runs = self.core.workflows.release(window);
        let terminals = self.core.terminals.release(window);
        let core = Arc::clone(&self.core);
        self.core.runtime.spawn(async move {
            let _order = core.order.lock().await;
            core.close_all(closes).await;
            core.close_runs(runs).await;
            core.detach_terminals(terminals).await;
        })
    }

    /// Asks for the items before ordinal `before` in a timeline the window
    /// watches. They come to that window only.
    pub async fn load_older(
        &self,
        window: &str,
        agent: AgentId,
        before: u64,
    ) -> Result<(), CommandError> {
        self.core.load_older(window, agent, before).await
    }

    /// Sends an action through the first of `targets` that takes it, and
    /// waits for its result. The action keeps its key from one link to the
    /// next, so the device acts on it once however many links carry it.
    async fn act_through(
        &self,
        targets: Vec<Target>,
        action: ClientMessage,
    ) -> Result<ActionOutcome, CommandError> {
        self.core.act_through(targets, action).await
    }

    /// Sends a request through the first link holding `key` that takes it,
    /// connected links first. `what` names the thing for the error when no
    /// link holds it.
    async fn send_through(
        &self,
        key: EntityKey,
        what: &str,
        message: ClientMessage,
    ) -> Result<(), CommandError> {
        let holders: Vec<(LinkId, String)> = self.core.view.read(|merge| {
            merge
                .holders(&key)
                .into_iter()
                .map(|link| (link.id, link.name.clone()))
                .collect()
        });
        let Some((_, first)) = holders.first() else {
            return Err(gone(what));
        };
        let device = first.clone();
        for (id, _) in holders {
            if self.send_to(id, message.clone()).await {
                return Ok(());
            }
        }
        Err(CommandError::NotConnected { device })
    }

    /// Sends a request about a terminal through a link to the device that
    /// holds it, for one that answers on the link rather than with an
    /// outcome, such as attaching to it.
    pub async fn send_terminal(
        &self,
        terminal: TerminalId,
        message: ClientMessage,
    ) -> Result<(), CommandError> {
        self.send_through(EntityKey::Terminal(terminal), "This terminal", message)
            .await
    }

    /// Sends a request to every device. It's enough that one takes it,
    /// since the rest catch up once they connect.
    async fn send_everywhere(&self, message: ClientMessage) -> Result<(), CommandError> {
        let ids: Vec<LinkId> = lock(&self.core.links).keys().copied().collect();
        let mut sent = false;
        for id in ids {
            sent |= self.send_to(id, message.clone()).await;
        }
        if sent {
            Ok(())
        } else {
            Err(failed("No device is connected".to_owned()))
        }
    }

    /// Sends a request on one link, and says whether it went out.
    async fn send_to(&self, id: LinkId, message: ClientMessage) -> bool {
        let Some(link) = self.core.link(id) else {
            return false;
        };
        match link.send(message).await {
            Ok(()) => true,
            Err(error) => {
                tracing::debug!(%id, %error, "a link didn't take a request");
                false
            }
        }
    }
}

impl<L: LinkControl> Core<L> {
    /// Shows a device and opens its link, unless it's open already. A link
    /// that has reached a device reaches only that one from then on.
    fn open(self: &Arc<Self>, spec: LinkSpec) {
        let device = lock(&self.registry)
            .known_device(spec.id)
            .unwrap_or_else(|error| {
                tracing::warn!(%error, link = %spec.id, "couldn't read which device a link reaches");
                None
            });
        let mut links = lock(&self.links);
        if links.contains_key(&spec.id) {
            return;
        }
        self.view.update(|merge| merge.add_link(&spec));
        let id = spec.id;
        let _entered = self.runtime.enter();
        let (link, events) = (self.dial)(spec, device);
        let link = Arc::new(link);
        let forward = tokio::spawn(forward(id, events, Arc::downgrade(self)));
        links.insert(id, Open { link, forward });
    }

    /// Closes the links, which takes what only they reported out of the
    /// view, and lets their timelines and runs move or go.
    fn drop_links(self: &Arc<Self>, gone: &[LinkId]) {
        // Dropping a link closes it and stops it feeding the view.
        let dropped: Vec<_> = {
            let mut links = lock(&self.links);
            gone.iter().filter_map(|id| links.remove(id)).collect()
        };
        drop(dropped);
        for id in gone {
            self.view.update(|merge| merge.remove_link(*id));
        }
        self.show_waiting();
        self.runtime
            .spawn(Arc::clone(self).settle(Vec::new(), Vec::new()));
        // An entry that went through a link goes another way, or goes.
        #[cfg(feature = "desktop")]
        self.rewrite_ssh();
    }

    /// The book in a report, when this is a phone and the book is a newer
    /// one of its own from the device that keeps it. Each Mac's book has
    /// its own keeper, and only that device speaks for it, over its own
    /// link: any other device could point this phone's routes and host
    /// keys somewhere else.
    fn newer_book(&self, id: LinkId, event: &LinkEvent) -> Option<PhoneBook> {
        let found = books(event);
        if found.is_empty() {
            return None;
        }
        let books = lock(&self.registry).phone_books().ok()?;
        let have = books
            .iter()
            .find(|have| id == crate::phone::book_link(have.device))?;
        found
            .into_iter()
            .rev()
            .find(|book| book.phone == have.phone && book.device == have.device)
            .filter(|book| *book != have)
            .cloned()
    }

    /// Takes a newer book from the device that keeps it, as
    /// [`crate::phone::take_newer`] says.
    fn take_book(self: &Arc<Self>, book: PhoneBook) -> Result<Vec<DeviceId>, RegistryError> {
        self.change_books(|books| crate::phone::take_newer(books, book))
    }

    /// Changes the phone's books with `change`, then makes the devices the
    /// ones they list: one that left them is closed, one whose routes
    /// changed opens again on them, and a new one opens. Answers the
    /// devices no book lists any more.
    fn change_books(
        self: &Arc<Self>,
        change: impl FnOnce(&mut Vec<PhoneBook>),
    ) -> Result<Vec<DeviceId>, RegistryError> {
        let (before, after, books, left) = {
            let registry = lock(&self.registry);
            let before = registry.list()?;
            let mut books = registry.phone_books()?;
            let had: Vec<DeviceId> = crate::phone::listed(&books)
                .into_iter()
                .map(|(_, entry)| entry.device)
                .collect();
            change(&mut books);
            registry.set_phone_books(&books)?;
            let listed = crate::phone::listed(&books);
            let left: Vec<DeviceId> = had
                .into_iter()
                .filter(|device| listed.iter().all(|(_, entry)| entry.device != *device))
                .collect();
            (before, registry.list()?, books, left)
        };
        (self.trust)(&books);
        let gone: Vec<LinkId> = before
            .iter()
            .filter(|old| {
                after.iter().all(|new| {
                    new.id != old.id || new.route != old.route || new.fallbacks != old.fallbacks
                })
            })
            .map(|old| old.id)
            .collect();
        self.drop_links(&gone);
        for spec in after {
            match before.iter().find(|old| old.id == spec.id) {
                Some(old) if !gone.contains(&old.id) => {
                    if old.name != spec.name {
                        self.view
                            .update(|merge| merge.rename_link(spec.id, &spec.name));
                    }
                }
                _ => self.open(spec),
            }
        }
        self.sync_peers();
        Ok(left)
    }

    /// Subscribes a window to a timeline and opens it on a link if it's
    /// the first, all in turn with the other changes.
    async fn open_timeline(
        &self,
        window: &str,
        sink: Arc<dyn Sink>,
        agent: AgentId,
    ) -> Result<(), CommandError> {
        let _order = self.order.lock().await;
        let known = lock(&self.leads).get(&agent).copied();
        let (targets, lead) = self
            .view
            .read(|merge| timeline_targets(merge, agent, known))?;
        if let Some(lead) = lead {
            lock(&self.leads).insert(agent, lead);
        }
        let Some((id, link)) = targets
            .iter()
            .find_map(|(id, _)| self.link(*id).map(|link| (*id, link)))
        else {
            return Err(gone("This device"));
        };
        let plan = self.timelines.subscribe(window, sink, agent, id);
        self.close_all(plan.close).await;
        if plan.open.is_some()
            && let Err(error) = link.open_timeline(agent, None, PAGE, lead).await
        {
            // Only a link that closed fails, and nothing opened.
            self.timelines.unsubscribe(window, agent);
            return Err(failed(sentence(error.to_string())));
        }
        Ok(())
    }

    /// Asks the timeline's link for a page of older items for one window.
    async fn load_older(
        &self,
        window: &str,
        agent: AgentId,
        before: u64,
    ) -> Result<(), CommandError> {
        // The open that the page needs reached the link first.
        let _order = self.order.lock().await;
        let Some((id, ask)) = self.timelines.ask_older(window, agent, before) else {
            return Err(gone("This timeline"));
        };
        let device = self.view.read(|merge| link_name(merge, id));
        let asked = self
            .request_through(vec![(id, device)], move |link| async move {
                link.load_older(agent, before, PAGE).await
            })
            .await;
        if asked.is_err() {
            self.timelines.forget_ask(agent, ask);
        }
        asked
    }

    /// Closes `closes` on their links, then moves each timeline whose link
    /// isn't connected to a connected link to the same device. The new
    /// link picks up after the version the app holds, so the windows miss
    /// nothing and get nothing twice.
    async fn settle(self: Arc<Self>, closes: Vec<Close>, runs: Vec<workflow::Close>) {
        let _order = self.order.lock().await;
        self.close_all(closes).await;
        self.close_runs(runs).await;
        self.relink_runs().await;
        for (agent, from) in self.timelines.watched() {
            let known = lock(&self.leads).get(&agent).copied();
            let to = self.view.read(|merge| {
                if connected(merge, from) {
                    return None;
                }
                let (targets, lead) = timeline_targets(merge, agent, known).ok()?;
                let mut ids = targets.into_iter().map(|(id, _)| id);
                ids.find(|id| *id != from && connected(merge, *id))
                    .map(|id| (id, lead))
            });
            let Some((to, lead, link)) =
                to.and_then(|(to, lead)| self.link(to).map(|link| (to, lead, link)))
            else {
                continue;
            };
            let Some(since) = self.timelines.relink(agent, from, to) else {
                continue;
            };
            if let Err(error) = link.open_timeline(agent, since, PAGE, lead).await {
                tracing::debug!(%to, %agent, %error, "a link didn't open a timeline");
                self.timelines.relink(agent, to, from);
                continue;
            }
            self.close_all(vec![(from, agent)]).await;
        }
    }

    /// Sends one inbox change, or keeps it in the outbox when no link to the
    /// agent's device takes it. A change for a device that has changes
    /// waiting goes behind them, so the device gets them in order.
    async fn set_inbox(
        self: Arc<Self>,
        agent: AgentId,
        change: InboxChange,
    ) -> Result<ActionOutcome, CommandError> {
        let key = IdempotencyKey::generate();
        let place = self.view.read(|merge| inbox_place(merge, agent))?;
        let turn = place.device.map(|device| self.turn(device));
        let _turn = match &turn {
            Some(turn) => Some(turn.lock().await),
            None => None,
        };
        let behind = place
            .device
            .is_some_and(|device| self.next_queued(device).is_some());
        if !behind {
            let action = ClientMessage::SetInbox {
                key,
                agent,
                change: change.clone(),
            };
            if let Some(outcome) = self.send_inbox(&place.targets, &action).await? {
                return Ok(outcome);
            }
        }

        let Some(device) = place.device else {
            return Err(CommandError::NotConnected { device: place.name });
        };
        if !place.known {
            return Err(failed(format!(
                "MaxHQ hasn't heard from {}'s maxd yet, so it can't keep the change for later. Try again once it connects",
                place.name
            )));
        }
        let queued = Queued {
            key,
            link: place.link,
            device,
            agent,
            title: place.title,
            change,
            at: Timestamp::now(),
        };
        let kept = lock(&self.registry).queue(&queued);
        if let Err(error) = kept {
            tracing::error!(%error, "couldn't keep an inbox change in the outbox");
            return Err(failed(format!(
                "MaxHQ couldn't keep the change for {}",
                place.name
            )));
        }
        self.show_waiting();
        // A device that shows connected may only have been between steps,
        // or have changes waiting that this one went behind.
        if self.view.read(|merge| device_connected(merge, device)) {
            let core = Arc::clone(&self);
            self.runtime.spawn(core.flush_after(device, RESUME_WAIT));
        }
        Ok(ActionOutcome::Done)
    }

    /// Sends an inbox change through the first of `targets` that takes it,
    /// and gives its result. None means no link carried it, or one did and
    /// dropped before the answer came, so the outbox can keep it: it goes
    /// again under the same key, and the device acts on it once. A device
    /// still connected that didn't answer in time is an error, as with any
    /// action.
    async fn send_inbox(
        &self,
        targets: &[Target],
        action: &ClientMessage,
    ) -> Result<Option<ActionOutcome>, CommandError> {
        for (id, name) in targets {
            let Some(link) = self.link(*id) else {
                continue;
            };
            match link.act(action.clone()).await {
                Ok(outcome) => return Ok(Some(outcome)),
                Err(error) if error.try_next() => {}
                Err(error)
                    if error.timed_out() && !self.view.read(|merge| connected(merge, *id)) => {}
                Err(error) => return Err(request_failed(&error, name)),
            }
        }
        Ok(None)
    }

    /// Sends the changes waiting for `device`, oldest first, each under the
    /// key it got when it was made. A change the device answered leaves the
    /// outbox whatever the answer, and you hear about a refusal. One that
    /// didn't go out stops the rest, which wait for the device to connect
    /// again, or for a retry while it stays connected. The revokes waiting
    /// for the device go first.
    async fn flush(self: Arc<Self>, device: DeviceId) {
        let turn = self.turn(device);
        let mut turn = turn.lock().await;
        self.send_revokes(device).await;
        while let Some(queued) = self.next_queued(device) {
            let Ok(targets) = self.view.read(|merge| device_targets(merge, device)) else {
                return;
            };
            let action = ClientMessage::SetInbox {
                key: queued.key,
                agent: queued.agent,
                change: queued.change.clone(),
            };
            let outcome = match self.send_inbox(&targets, &action).await {
                Ok(Some(outcome)) => outcome,
                Ok(None) => {
                    turn.misses = 0;
                    return;
                }
                Err(error) => {
                    tracing::warn!(%device, %error, "the outbox didn't go out, so it waits");
                    self.retry(device, &mut turn);
                    return;
                }
            };
            turn.misses = 0;
            let name = targets.first().map_or("The device", |(_, name)| name);
            if !self.landed(&queued, name, outcome) {
                return;
            }
        }
    }

    /// Sends each revoke waiting for `device`, under a fresh key, since
    /// revoking a permission it doesn't hold is done too. A revoke stays
    /// until the device answers done.
    async fn send_revokes(&self, device: DeviceId) {
        let pending = lock(&self.registry).pending_revokes();
        let pending = match pending {
            Ok(pending) => pending,
            Err(error) => {
                tracing::error!(%error, "couldn't read the waiting revokes");
                return;
            }
        };
        for (_, permission) in pending.into_iter().filter(|(to, _)| *to == device) {
            let Ok(targets) = self.view.read(|merge| device_targets(merge, device)) else {
                return;
            };
            let action = ClientMessage::RevokePermission {
                key: IdempotencyKey::generate(),
                permission,
            };
            match self.act_through(targets, action).await {
                Ok(ActionOutcome::Done) => {
                    let forgot = lock(&self.registry).forget_revoke(device, permission);
                    if let Err(error) = forgot {
                        tracing::error!(%error, "couldn't let go of a revoke that landed");
                    }
                }
                Ok(outcome) => {
                    tracing::warn!(%device, ?outcome, "a device didn't take a waiting revoke")
                }
                Err(error) => {
                    tracing::warn!(%device, %error, "a waiting revoke didn't go out, so it waits");
                    return;
                }
            }
        }
    }

    /// Flushes `device`'s outbox after `wait`.
    async fn flush_after(self: Arc<Self>, device: DeviceId, wait: Duration) {
        tokio::time::sleep(wait).await;
        self.flush(device).await;
    }

    /// Has `device`'s outbox go again after a miss, unless a retry already
    /// waits. A device that's gone by then leaves it to wait for the link.
    fn retry(self: &Arc<Self>, device: DeviceId, turn: &mut Turn) {
        turn.misses = turn.misses.saturating_add(1);
        if turn.retrying {
            return;
        }
        turn.retrying = true;
        let wait = retry_wait(turn.misses);
        let core = Arc::clone(self);
        self.runtime.spawn(async move {
            tokio::time::sleep(wait).await;
            core.turn(device).lock().await.retrying = false;
            core.flush(device).await;
        });
    }

    /// Flushes the outbox of the device a link reaches after `wait`, if the
    /// link is still connected by then.
    async fn flush_link(self: Arc<Self>, id: LinkId, wait: Duration) {
        tokio::time::sleep(wait).await;
        let device = self.view.read(
            |merge| match merge.store().get(&EntityKey::DeviceLink(id)) {
                Some(Entity::DeviceLink(link))
                    if matches!(link.state, LinkState::Connected { .. }) =>
                {
                    link.device
                }
                _ => None,
            },
        );
        if let Some(device) = device {
            self.flush(device).await;
        }
    }

    /// Takes a change its device answered out of the outbox, and tells you
    /// if the device refused it. Says whether the outbox let it go.
    fn landed(&self, queued: &Queued, device: &str, outcome: ActionOutcome) -> bool {
        let dropped = lock(&self.registry).unqueue(queued.key);
        if let Err(error) = dropped {
            tracing::error!(%error, "couldn't take a change out of the outbox");
            return false;
        }
        self.show_waiting();
        match outcome {
            // An agent the device no longer has has nowhere to sit.
            ActionOutcome::Done | ActionOutcome::Unknown | ActionOutcome::UnknownAgent => {}
            ActionOutcome::Refused { reason } => {
                tracing::info!(agent = %queued.agent, device, reason, "a device refused a change from the outbox");
                self.view.refused(queued, device, &reason);
            }
            outcome => tracing::debug!(?outcome, "a change from the outbox came back oddly"),
        }
        true
    }

    /// The oldest change waiting for `device`.
    fn next_queued(&self, device: DeviceId) -> Option<Queued> {
        let queued = lock(&self.registry).queued();
        match queued {
            Ok(queued) => queued.into_iter().find(|queued| queued.device == device),
            Err(error) => {
                tracing::error!(%error, "couldn't read the outbox");
                None
            }
        }
    }

    /// Shows the outbox on the agents in the view.
    fn show_waiting(&self) {
        let queued = lock(&self.registry).queued();
        let queued = match queued {
            Ok(queued) => queued,
            Err(error) => {
                tracing::error!(%error, "couldn't read the outbox");
                return;
            }
        };
        let mut waiting = Waiting::new();
        for queued in queued {
            waiting
                .entry(queued.agent)
                .or_default()
                .push((queued.link, queued.change, queued.at));
        }
        self.view.update(|merge| merge.set_waiting(waiting));
    }

    /// The turn for `device`'s outbox.
    fn turn(&self, device: DeviceId) -> Arc<tokio::sync::Mutex<Turn>> {
        Arc::clone(lock(&self.turns).entry(device).or_default())
    }

    /// Sends an action through the first of `targets` that takes it, under
    /// the same key on every link, and waits for its result.
    async fn act_through(
        &self,
        targets: Vec<Target>,
        action: ClientMessage,
    ) -> Result<ActionOutcome, CommandError> {
        self.request_through(targets, move |link| {
            let action = action.clone();
            async move { link.act(action).await }
        })
        .await
    }

    /// Like [`Self::act_through`], but none when the device's `maxd` is
    /// too old to read the action.
    async fn act_unless_old(
        &self,
        targets: Vec<Target>,
        action: ClientMessage,
    ) -> Result<Option<ActionOutcome>, CommandError> {
        self.request_through(targets, move |link| {
            let action = action.clone();
            async move {
                match link.act(action).await {
                    Ok(outcome) => Ok(Some(outcome)),
                    Err(LinkError::TooOld) => Ok(None),
                    Err(error) => Err(error),
                }
            }
        })
        .await
    }

    /// Makes a request through the first of `targets` whose link is
    /// connected. A link that isn't connected refuses at once, so the next
    /// one gets a turn.
    async fn request_through<T, E, F, R>(
        &self,
        targets: Vec<Target>,
        request: F,
    ) -> Result<T, CommandError>
    where
        E: RequestError,
        F: Fn(Arc<L>) -> R,
        R: Future<Output = Result<T, E>>,
    {
        let Some((_, first)) = targets.first() else {
            return Err(gone("This device"));
        };
        let device = first.clone();
        for (id, name) in targets {
            let Some(link) = self.link(id) else {
                continue;
            };
            match request(link).await {
                Ok(result) => return Ok(result),
                Err(error) if error.try_next() => {}
                Err(error) => return Err(request_failed(&error, &name)),
            }
        }
        Err(CommandError::NotConnected { device })
    }

    /// Closes timelines no window watches any more. Call it with the order
    /// lock held.
    async fn close_all(&self, closes: Vec<Close>) {
        for (id, agent) in closes {
            let Some(link) = self.link(id) else {
                continue;
            };
            if let Err(error) = link.close_timeline(agent).await {
                tracing::debug!(%id, %agent, %error, "a link didn't close a timeline");
            }
        }
    }

    /// Gives the device on link `id` the app's SSH key, apart, so the
    /// reports keep coming. The device forgets it when the link drops.
    #[cfg(feature = "desktop")]
    fn push_ssh_key(&self, id: LinkId) {
        let (Some(files), Some(link)) = (self.ssh.get(), self.link(id)) else {
            return;
        };
        let action = ClientMessage::SetGuestSshKey {
            key: IdempotencyKey::generate(),
            public_key: files.public_key().to_owned(),
        };
        self.runtime.spawn(async move {
            match link.act(action).await {
                Ok(ActionOutcome::Done) => {}
                Ok(outcome) => tracing::warn!(%id, ?outcome, "a device refused the app's SSH key"),
                Err(error) => tracing::debug!(%id, %error, "a device didn't get the app's SSH key"),
            }
        });
    }

    /// Rewrites the SSH files from the view, apart, unless a rewrite is on
    /// its way already.
    #[cfg(feature = "desktop")]
    fn rewrite_ssh(self: &Arc<Self>) {
        if self.ssh.get().is_none() || self.ssh_due.swap(true, Ordering::AcqRel) {
            return;
        }
        let core = Arc::clone(self);
        self.runtime.spawn_blocking(move || core.write_ssh());
    }

    /// Writes the SSH files with the entries the view gives now. Blocks.
    #[cfg(feature = "desktop")]
    fn write_ssh(&self) {
        let Some(files) = self.ssh.get() else {
            return;
        };
        let written = files.write_with(|| {
            // A change from here on brings another rewrite.
            self.ssh_due.store(false, Ordering::Release);
            self.view.read(|merge| files.entries(merge))
        });
        if let Err(error) = written {
            tracing::warn!(%error, "couldn't write MaxHQ's SSH config");
        }
    }

    /// Where an editor stands with `workspace`, after adding the `Include`
    /// or remembering a no when `add` says to. Blocks.
    #[cfg(feature = "desktop")]
    fn ssh_include(
        &self,
        workspace: WorkspaceId,
        add: Option<bool>,
    ) -> Result<SshIncludeState, CommandError> {
        let Some(files) = self.ssh.get() else {
            return Ok(SshIncludeState::Unavailable {
                reason: "This app doesn't open containers in an editor.".into(),
            });
        };
        let entry = match self.view.read(|merge| files.entry(merge, workspace)) {
            None => return Err(gone("This workspace")),
            Some(Err(reason)) => {
                return Ok(SshIncludeState::Unavailable {
                    reason: reason.into(),
                });
            }
            Some(Ok(entry)) => entry,
        };
        let couldnt =
            |error: io::Error| failed(format!("Couldn't set up SSH for editors: {error}"));
        // The entry is in MaxHQ's config before anything points an editor
        // at it.
        files
            .write_with(|| self.view.read(|merge| files.entries(merge)))
            .map_err(couldnt)?;
        match add {
            None => files.include_state(&entry),
            Some(add) => files.set_include(&entry, add),
        }
        .map_err(couldnt)
    }

    /// The open link with this ID.
    fn link(&self, id: LinkId) -> Option<Arc<L>> {
        lock(&self.links).get(&id).map(|open| open.link.clone())
    }

    /// Every open link, so a request can go to each without holding the
    /// lock.
    fn all_links(&self) -> Vec<Arc<L>> {
        lock(&self.links)
            .values()
            .map(|open| open.link.clone())
            .collect()
    }
}

/// A port forward that opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Forwarded {
    /// The port it listens on here.
    pub local_port: u16,
    /// The device's forward, once it has answered on this connection.
    pub forward: Option<ForwardId>,
}

/// A request's error, as [`Core::request_through`] reads it.
trait RequestError: std::fmt::Display {
    /// Whether the link couldn't carry the request at all, so the next
    /// link can try it.
    fn try_next(&self) -> bool;

    /// Whether the device didn't answer in time.
    fn timed_out(&self) -> bool {
        false
    }

    /// Whether the device's `maxd` is too old to read the request.
    fn too_old(&self) -> bool {
        false
    }
}

/// Why a request failed, naming the device it went to.
fn request_failed(error: &impl RequestError, name: &str) -> CommandError {
    if error.timed_out() {
        failed(format!("{name} didn't answer in time"))
    } else if error.too_old() {
        failed(format!("maxd on {name} is too old for this; update it"))
    } else {
        failed(sentence(error.to_string()))
    }
}

impl RequestError for LinkError {
    fn try_next(&self) -> bool {
        matches!(self, Self::NotConnected | Self::Closed)
    }

    fn timed_out(&self) -> bool {
        matches!(self, Self::TimedOut)
    }

    fn too_old(&self) -> bool {
        matches!(self, Self::TooOld)
    }
}

impl RequestError for TransferError {
    fn try_next(&self) -> bool {
        matches!(self, Self::Link(error) if error.try_next())
    }

    fn timed_out(&self) -> bool {
        matches!(self, Self::Link(error) if error.timed_out())
    }

    fn too_old(&self) -> bool {
        matches!(self, Self::Link(error) if error.too_old())
    }
}

impl RequestError for ForwardError {
    fn try_next(&self) -> bool {
        matches!(self, Self::NotConnected)
    }
}

/// An upload's result, with a device that doesn't take files refusing the
/// way an action does.
fn refused_if_unsupported(
    result: Result<ActionOutcome, TransferError>,
) -> Result<ActionOutcome, TransferError> {
    match result {
        Err(TransferError::Unsupported) => Ok(ActionOutcome::Refused {
            reason: sentence(TransferError::Unsupported.to_string()),
        }),
        result => result,
    }
}

/// Feeds what one link reports into the view, and its timelines to the
/// windows watching them, until the link closes. It holds the devices
/// weakly, since they hold the task.
async fn forward<L: LinkControl>(id: LinkId, mut events: Receiver<LinkEvent>, core: Weak<Core<L>>) {
    while let Some(mut event) = events.recv().await {
        let Some(core) = core.upgrade() else {
            break;
        };
        // A link whose device needs your answer first reaches none, so
        // nothing meant for a device goes through it.
        let held = match &mut event {
            LinkEvent::Link(link) => core.screen(id, link),
            _ => core.view.read(|merge| claims::held(merge, id)),
        };
        let moved = matches!(event, LinkEvent::Link(_));
        let synced = matches!(event, LinkEvent::Synced(_));
        #[cfg(feature = "desktop")]
        let reshaped =
            moved || synced || changes(&event, |key| matches!(key, EntityKey::Workspace(_)));
        let was_up = moved && core.view.read(|merge| connected(merge, id));
        let hosted = (synced || changes(&event, |key| matches!(key, EntityKey::TestHost(_))))
            .then(|| core.view.read(peers::test_kinds));
        let shared = (synced || changes(&event, |key| matches!(key, EntityKey::ModelServer(_))))
            .then(|| core.view.read(peers::shared_models));
        // Agents change often, so only an added or removed one, or a
        // changed account, looks again at whose models they use.
        let using = (synced
            || changes(&event, |key| matches!(key, EntityKey::Account(_)))
            || core.view.read(|merge| reaccounts(merge, &event)))
        .then(|| core.view.read(peers::models_from));
        let book = core.newer_book(id, &event);
        let noted = notes::touched(&event);
        #[cfg(feature = "desktop")]
        let phoned = synced
            || changes(&event, |key| {
                matches!(
                    key,
                    EntityKey::Phone { .. }
                        | EntityKey::PhoneBook(_)
                        | EntityKey::NetworkAccess(_)
                        | EntityKey::PhoneSession(_)
                )
            });
        match event {
            LinkEvent::Timeline {
                agent,
                update,
                live,
            } => core.timelines.deliver(id, agent, update, live),
            LinkEvent::Workflow { run, update } => core.workflows.deliver(id, run, *update),
            LinkEvent::RouteWorked {
                net,
                route: Route::Ssh { host },
            } => {
                if let Err(error) = lock(&core.registry).record_good(id, &net, &host) {
                    tracing::warn!(%error, link = %id, "couldn't keep the route that worked");
                }
            }
            event => core.view.update(|merge| merge.link_event(id, event)),
        }
        // A phone's devices follow its books. They change apart, since this
        // link may be one that closes.
        if let Some(book) = book {
            let taker = Arc::clone(&core);
            core.runtime.spawn(async move {
                if let Err(error) = taker.take_book(book) {
                    tracing::warn!(%error, "couldn't take the phone's new book");
                }
            });
        }
        let came_up = !held && moved && !was_up && core.view.read(|merge| connected(merge, id));
        // The outbox goes once the device's snapshot is in, or soon after
        // the link comes up, for a session that brings no snapshot.
        let flush = if held {
            None
        } else if synced {
            Some(Duration::ZERO)
        } else if came_up {
            Some(RESUME_WAIT)
        } else {
            None
        };
        if let Some(wait) = flush {
            core.runtime.spawn(Arc::clone(&core).flush_link(id, wait));
        }
        #[cfg(feature = "desktop")]
        if came_up && core.view.read(|merge| takes_ssh(merge, id)) {
            core.push_ssh_key(id);
        }
        // A device that takes phones gets the push token and presence.
        if came_up {
            core.phone_came_up(id);
        }
        // A device that came up over SSH gets the host key your own
        // known_hosts has for it, when MaxHQ has none.
        if came_up {
            core.pin_host_key(id);
        }
        // Notes swap through this machine's `maxd` once a link comes up,
        // and whenever a device's notes or workspaces change.
        if !held {
            core.relay_notes(if came_up { notes::Touched::All } else { noted });
        }
        // The other devices' books say which kinds each one hosts, and
        // which managed model server each one has.
        let rehosted = hosted.is_some_and(|before| before != core.view.read(peers::test_kinds))
            || shared.is_some_and(|before| before != core.view.read(peers::shared_models))
            || using.is_some_and(|before| before != core.view.read(peers::models_from));
        if rehosted || came_up && core.view.read(|merge| links_peers(merge, id)) {
            core.sync_peers();
        }
        // A new phone reaches the other devices in its book, and a device
        // that comes up takes the phones waiting for it. A link that moves
        // may also be the last one the first pass over the phones waits for.
        #[cfg(feature = "desktop")]
        {
            if synced {
                lock(&core.phone_synced).insert(id);
            }
            if phoned || moved {
                core.sync_phones();
            }
        }
        // The SSH files follow the workspaces and the routes to them.
        #[cfg(feature = "desktop")]
        if reshaped {
            core.rewrite_ssh();
        }
        // A window whose channel stopped taking batches left timelines no
        // one watches, which this link closes once it reports again. A link
        // that came up or dropped may leave a timeline to move.
        let orphans = core.timelines.orphans(id);
        let runs = core.workflows.orphans(id);
        if moved || !orphans.is_empty() || !runs.is_empty() {
            let closes = orphans.into_iter().map(|agent| (id, agent)).collect();
            let runs = runs.into_iter().map(|run| (id, run)).collect();
            // The settling waits its turn apart, so the reports keep coming.
            let runtime = core.runtime.clone();
            runtime.spawn(core.settle(closes, runs));
        }
    }
}

/// Whether the view shows a link as connected.
fn connected(merge: &Merge, id: LinkId) -> bool {
    matches!(
        merge.store().get(&EntityKey::DeviceLink(id)),
        Some(Entity::DeviceLink(link)) if matches!(link.state, LinkState::Connected { .. })
    )
}

/// Whether the device on link `id` takes editor sessions, from the link's
/// last hello.
#[cfg(feature = "desktop")]
fn takes_ssh(merge: &Merge, id: LinkId) -> bool {
    matches!(
        merge.store().get(&EntityKey::DeviceLink(id)),
        Some(Entity::DeviceLink(link))
            if link.capabilities.is_some_and(|does| does.guest_ssh)
    )
}

/// Whether the device on link `id` links to other devices, from the
/// link's last hello.
fn links_peers(merge: &Merge, id: LinkId) -> bool {
    matches!(
        merge.store().get(&EntityKey::DeviceLink(id)),
        Some(Entity::DeviceLink(link)) if link.capabilities.is_some_and(|does| does.peers)
    )
}

/// The device books a report carries.
fn books(event: &LinkEvent) -> Vec<&PhoneBook> {
    fn book(entity: &Entity) -> Option<&PhoneBook> {
        match entity {
            Entity::PhoneBook(book) => Some(book),
            _ => None,
        }
    }
    match event {
        LinkEvent::Synced(entities) => entities.iter().filter_map(book).collect(),
        LinkEvent::Changed(changes) => changes
            .iter()
            .filter_map(|change| match change {
                Change::Upsert { value } => book(value),
                Change::Remove { .. } => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Whether a report adds, changes or removes an entity whose key `is`.
fn changes(event: &LinkEvent, is: fn(&EntityKey) -> bool) -> bool {
    let LinkEvent::Changed(changes) = event else {
        return false;
    };
    changes.iter().any(|change| match change {
        Change::Upsert { value } => is(&value.key()),
        Change::Remove { key } => is(key),
    })
}

/// Whether `event` adds or removes an agent, or moves one to another
/// account, as the view held before it.
fn reaccounts(merge: &Merge, event: &LinkEvent) -> bool {
    let LinkEvent::Changed(changes) = event else {
        return false;
    };
    changes.iter().any(|change| match change {
        Change::Upsert {
            value: Entity::Agent(agent),
        } => match merge.store().get(&EntityKey::Agent(agent.id)) {
            Some(Entity::Agent(before)) => before.account != agent.account,
            _ => true,
        },
        Change::Remove {
            key: EntityKey::Agent(_),
        } => true,
        _ => false,
    })
}

/// Whether a link to the device runs over [`Route::Local`].
pub(crate) fn is_local(merge: &Merge, device: DeviceId) -> bool {
    device_links(merge, device).any(|link| link.route == Route::Local)
}

/// Whether the view shows a link to the device as connected.
pub(crate) fn device_connected(merge: &Merge, device: DeviceId) -> bool {
    device_links(merge, device).any(|link| matches!(link.state, LinkState::Connected { .. }))
}

/// How long the outbox waits to go again after `misses` in a row.
fn retry_wait(misses: u32) -> Duration {
    let doublings = misses.saturating_sub(1).min(16);
    RETRY_WAIT.saturating_mul(1 << doublings).min(RETRY_MOST)
}

/// Another link the view shows to the device `link` reaches.
fn heir(merge: &Merge, link: LinkId) -> Option<LinkId> {
    let Some(Entity::DeviceLink(found)) = merge.store().get(&EntityKey::DeviceLink(link)) else {
        return None;
    };
    let device = found.device?;
    device_links(merge, device)
        .map(|other| other.id)
        .find(|id| *id != link)
}

/// What a device does, from the last hello one of its links had. A link
/// that's connected now says it best.
pub(crate) fn capabilities(merge: &Merge, device: DeviceId) -> Option<Capabilities> {
    let mut links: Vec<_> = device_links(merge, device)
        .filter(|link| link.capabilities.is_some())
        .collect();
    links.sort_by_key(|link| !matches!(link.state, LinkState::Connected { .. }));
    links.first().and_then(|link| link.capabilities)
}

/// The link this machine reaches a device by: over [`Route::Local`] when one
/// is, else a connected one first.
pub(crate) fn best_link(merge: &Merge, device: DeviceId) -> Option<&DeviceLink> {
    device_links(merge, device).min_by_key(|link| {
        let connected = matches!(link.state, LinkState::Connected { .. });
        (link.route != Route::Local, !connected)
    })
}

/// The links the view shows to a device. A device with a link over
/// [`Route::Local`] has only that one: this machine knows it reaches its own
/// `maxd`, and a remote link can name any device in its hello.
pub(crate) fn device_links(merge: &Merge, device: DeviceId) -> impl Iterator<Item = &DeviceLink> {
    let links = || {
        merge
            .store()
            .entities()
            .filter_map(move |entity| match entity {
                Entity::DeviceLink(link) if link.device == Some(device) => Some(link),
                _ => None,
            })
    };
    let local = links().any(|link| link.route == Route::Local);
    links().filter(move |link| !local || link.route == Route::Local)
}

/// The links an action goes through, those to the device it's for first.
fn action_targets(merge: &Merge, action: &ClientMessage) -> Result<Vec<Target>, CommandError> {
    match action {
        ClientMessage::StartAgent { spec, .. } => place_targets(merge, spec),
        ClientMessage::SendInput { agent, .. }
        | ClientMessage::CancelQueued { agent, .. }
        | ClientMessage::Interrupt { agent, .. }
        | ClientMessage::SetPermission { agent, .. }
        | ClientMessage::SetModel { agent, .. }
        | ClientMessage::SetEffort { agent, .. }
        | ClientMessage::StopAgent { agent, .. }
        | ClientMessage::DeleteAgent { agent, .. }
        | ClientMessage::RenameAgent { agent, .. }
        | ClientMessage::SetInbox { agent, .. }
        | ClientMessage::ContinueHere { agent, .. }
        | ClientMessage::TakeOver { agent, .. }
        | ClientMessage::FindPaths { agent, .. }
        | ClientMessage::ListProcesses { agent, .. }
        | ClientMessage::SignalProcess { agent, .. }
        | ClientMessage::FindGitItems { agent, .. }
        | ClientMessage::ExcerptAgent { agent, .. }
        | ClientMessage::FindInTimeline { agent, .. }
        | ClientMessage::TerminalOpen { agent, .. }
        | ClientMessage::SessionRun { agent, .. }
        | ClientMessage::SessionShare { agent, .. }
        | ClientMessage::SessionCd { agent, .. }
        | ClientMessage::SessionEnd { agent, .. } => agent_targets(merge, *agent),
        // A terminal runs where its agent does.
        ClientMessage::TerminalInterrupt { terminal, .. }
        | ClientMessage::TerminalClose { terminal, .. }
        | ClientMessage::TerminalRename { terminal, .. }
        | ClientMessage::TerminalMove { terminal, .. } => {
            match merge.store().get(&EntityKey::Terminal(*terminal)) {
                Some(Entity::Terminal(found)) => agent_targets(merge, found.agent),
                _ => Err(gone("This terminal")),
            }
        }
        ClientMessage::StageAttachment {
            source: AttachmentSource::Found { agent, .. },
            ..
        } => agent_targets(merge, *agent),
        ClientMessage::CreateWorkspace { spec, .. } => device_targets(merge, spec.device),
        ClientMessage::CloneFolder { device, .. }
        | ClientMessage::ListFolder { device, .. }
        | ClientMessage::MakeFolder { device, .. }
        | ClientMessage::Notices { device, .. }
        | ClientMessage::CursorRules { device, .. } => device_targets(merge, *device),
        ClientMessage::StartWorkspace { workspace, .. }
        | ClientMessage::StopWorkspace { workspace, .. }
        | ClientMessage::DeleteWorkspace { workspace, .. }
        | ClientMessage::ListIgnoredFiles { workspace, .. }
        | ClientMessage::FetchFiles { workspace, .. }
        | ClientMessage::PlaceStaged { workspace, .. }
        | ClientMessage::CloneCancel { workspace, .. }
        | ClientMessage::CloneRetry { workspace, .. }
        | ClientMessage::CloneRemove { workspace, .. } => {
            device_targets(merge, workspace_device(merge, *workspace)?)
        }
        ClientMessage::SetProjectSettings { settings, .. } => {
            device_targets(merge, settings.device)
        }
        ClientMessage::SetProjectHidden { device, .. } => device_targets(merge, *device),
        ClientMessage::RemoveCommandRule { device, .. }
        | ClientMessage::RemoveAllowedEdit { device, .. } => device_targets(merge, *device),
        ClientMessage::SetDeviceSettings { settings, .. } => device_targets(merge, settings.device),
        ClientMessage::UpdateDeviceSettings { device, .. } => device_targets(merge, *device),
        ClientMessage::RevokePhoneSession { session, .. } => {
            match merge.store().get(&EntityKey::PhoneSession(*session)) {
                Some(Entity::PhoneSession(found)) => device_targets(merge, found.device),
                _ => Err(gone("That phone session")),
            }
        }
        ClientMessage::AnswerAdmission { admission, .. } => {
            match merge.store().get(&EntityKey::Admission(*admission)) {
                Some(Entity::Admission(found)) => device_targets(merge, found.device),
                _ => Err(gone("That card")),
            }
        }
        // Only the device whose folder holds the login spends its resets.
        ClientMessage::UseResetCredit { account, .. } => {
            match merge.store().get(&EntityKey::Account(*account)) {
                Some(Entity::Account(found)) => device_targets(merge, found.device),
                _ => Err(gone("This account")),
            }
        }
        // Fleet tools: a workspace's CLIs go to its device. The rest name
        // no device, so they go through `act_on`.
        ClientMessage::UpdateTools { tools, .. } => match tools.first().map(|tool| &tool.place) {
            Some(ToolPlace::Workspace { workspace }) => {
                device_targets(merge, workspace_device(merge, *workspace)?)
            }
            _ => Err(failed("Say which device's agent CLIs to update".to_owned())),
        },
        ClientMessage::ReadLoad { .. }
        | ClientMessage::ReadDiagnostics { .. }
        | ClientMessage::InstallUpdate { .. }
        | ClientMessage::ReadNote { .. }
        | ClientMessage::SaveNote { .. }
        | ClientMessage::NoteHistory { .. }
        | ClientMessage::NoteHashes { .. }
        | ClientMessage::NoteVersions { .. }
        | ClientMessage::PutNoteVersions { .. } => {
            Err(failed("Say which device to send that to".to_owned()))
        }
        _ => Err(failed(
            "That request isn't an action for a device".to_owned(),
        )),
    }
}

/// The device an agent runs on, when the view has its workspace.
pub(crate) fn agent_device(merge: &Merge, agent: AgentId) -> Option<DeviceId> {
    let store = merge.store();
    let Some(Entity::Agent(found)) = store.get(&EntityKey::Agent(agent)) else {
        return None;
    };
    match store.get(&EntityKey::Workspace(found.workspace)) {
        Some(Entity::Workspace(workspace)) => Some(workspace.device),
        _ => None,
    }
}

/// Where an inbox change for an agent goes, and what the outbox needs to
/// keep it.
struct InboxPlace {
    /// The links to the agent's device.
    targets: Vec<Target>,
    /// The first of them, which a change the outbox keeps goes with.
    link: LinkId,
    /// What the view calls that link.
    name: String,
    /// The agent's device, when the view knows it.
    device: Option<DeviceId>,
    /// Whether the device's `maxd` has said what it does.
    known: bool,
    /// The agent's title, for a message about the change later.
    title: String,
}

/// Where an inbox change for `agent` goes. A `maxd` from before inboxes is
/// refused here in words, since it would only say it can't.
fn inbox_place(merge: &Merge, agent: AgentId) -> Result<InboxPlace, CommandError> {
    let targets = agent_targets(merge, agent)?;
    let Some((link, name)) = targets.first().cloned() else {
        return Err(gone("This device"));
    };
    let key = EntityKey::Agent(agent);
    let device = agent_device(merge, agent)
        .or_else(|| merge.holders(&key).into_iter().find_map(|link| link.device));
    let does = device.and_then(|device| capabilities(merge, device));
    if does.is_some_and(|does| !does.inbox) {
        return Err(failed(format!(
            "{name}'s maxd is too old to settle, snooze or pin its agents. Update it first"
        )));
    }
    let Some(Entity::Agent(found)) = merge.store().get(&key) else {
        return Err(gone("This agent"));
    };
    Ok(InboxPlace {
        title: notify::title(found),
        targets,
        link,
        name,
        device,
        known: does.is_some(),
    })
}

/// The links to the device an agent runs on: the device of its workspace.
/// When the view lacks the workspace, the links that reported the agent.
fn agent_targets(merge: &Merge, agent: AgentId) -> Result<Vec<Target>, CommandError> {
    let store = merge.store();
    let Some(Entity::Agent(found)) = store.get(&EntityKey::Agent(agent)) else {
        return Err(gone("This agent"));
    };
    match store.get(&EntityKey::Workspace(found.workspace)) {
        Some(Entity::Workspace(workspace)) => device_targets(merge, workspace.device),
        _ => Ok(merge
            .holders(&EntityKey::Agent(agent))
            .into_iter()
            .map(|link| (link.id, link.name.clone()))
            .collect()),
    }
}

/// How far up a sub-agent's parents [`root_lead`] looks. `maxd` nests
/// sub-agents three deep.
const NESTING: usize = 8;

/// The links that carry an agent's timeline, and the lead to name for a
/// sub-agent, at the top of its tree. A sub-agent the view no longer lists,
/// such as one that finished, goes through its lead: `known`, the one it had
/// when a window last opened it, or the lead of a workflow run it's bound to.
fn timeline_targets(
    merge: &Merge,
    agent: AgentId,
    known: Option<AgentId>,
) -> Result<(Vec<Target>, Option<AgentId>), CommandError> {
    if merge.store().get(&EntityKey::Agent(agent)).is_some() {
        let lead = root_lead(merge, agent).filter(|lead| *lead != agent);
        return Ok((agent_targets(merge, agent)?, lead));
    }
    let lead = known
        .or_else(|| run_lead(merge, agent))
        .and_then(|lead| root_lead(merge, lead))
        .ok_or_else(|| gone("This agent"))?;
    Ok((agent_targets(merge, lead)?, Some(lead)))
}

/// The lead at the top of `agent`'s tree, when the view lists it and every
/// agent on the way.
fn root_lead(merge: &Merge, agent: AgentId) -> Option<AgentId> {
    let store = merge.store();
    let mut at = agent;
    for _ in 0..=NESTING {
        let Some(Entity::Agent(found)) = store.get(&EntityKey::Agent(at)) else {
            return None;
        };
        match found.parent {
            Some(parent) => at = parent,
            None => return Some(at),
        }
    }
    None
}

/// The workflow whose runs list a lead's sub-agents, `maxhq.agents`.
const SUB_AGENTS: &str = "maxhq.agents";

/// The lead of the sub-agents run with `agent` bound to one of its nodes.
/// Other workflows bind agents of their own, which aren't sub-agents.
fn run_lead(merge: &Merge, agent: AgentId) -> Option<AgentId> {
    merge.store().entities().find_map(|entity| match entity {
        Entity::WorkflowRun(run)
            if run.key.workflow == SUB_AGENTS && run.agents.contains(&agent) =>
        {
            run.orchestrators.first().copied()
        }
        _ => None,
    })
}

/// The links to the device a new agent starts on: its workspace's device,
/// or for a folder, the device of the account it runs as.
fn place_targets(merge: &Merge, spec: &AgentSpec) -> Result<Vec<Target>, CommandError> {
    let device = match &spec.place {
        AgentPlace::Workspace { workspace } => workspace_device(merge, *workspace)?,
        AgentPlace::Folder { .. } => match merge.store().get(&EntityKey::Account(spec.account)) {
            Some(Entity::Account(found)) => found.device,
            _ => return Err(gone("This account")),
        },
    };
    device_targets(merge, device)
}

/// The device a workspace is on, as the view shows it.
pub(crate) fn workspace_device(
    merge: &Merge,
    workspace: WorkspaceId,
) -> Result<DeviceId, CommandError> {
    match merge.store().get(&EntityKey::Workspace(workspace)) {
        Some(Entity::Workspace(found)) => Ok(found.device),
        _ => Err(gone("This workspace")),
    }
}

/// The links to a device for a copy, or why its `maxd` can't take part.
fn copy_targets(merge: &Merge, device: DeviceId) -> Result<Vec<Target>, CommandError> {
    let targets = device_targets(merge, device)?;
    if capabilities(merge, device).is_some_and(|does| !does.copy) {
        let name = targets.first().map_or("The device", |(_, name)| name);
        return Err(failed(format!(
            "{name}'s maxd is too old to copy projects. Update it first"
        )));
    }
    Ok(targets)
}

/// The links to a device, connected ones first.
fn device_targets(merge: &Merge, device: DeviceId) -> Result<Vec<Target>, CommandError> {
    let mut links: Vec<_> = device_links(merge, device).collect();
    if links.is_empty() {
        return Err(gone("This device"));
    }
    links.sort_by_key(|link| !matches!(link.state, LinkState::Connected { .. }));
    Ok(links
        .into_iter()
        .map(|link| (link.id, link.name.clone()))
        .collect())
}

/// What the view calls a link.
fn link_name(merge: &Merge, id: LinkId) -> String {
    match merge.store().get(&EntityKey::DeviceLink(id)) {
        Some(Entity::DeviceLink(link)) => link.name.clone(),
        _ => "The device".to_owned(),
    }
}

/// An answer's result as an action's. An answer that lost comes back
/// refused, with why in words a window shows.
fn answered(outcome: AnswerOutcome) -> ActionOutcome {
    let reason = match outcome {
        AnswerOutcome::Accepted => return ActionOutcome::Done,
        AnswerOutcome::Unknown => return ActionOutcome::Unknown,
        AnswerOutcome::AlreadyAnswered { .. } => "Someone else answered first",
        AnswerOutcome::Expired => "This request expired before your answer arrived",
        AnswerOutcome::UnknownEvent => "The device no longer has this request",
    };
    ActionOutcome::Refused {
        reason: reason.to_owned(),
    }
}

/// How many bytes padded base64 text decodes to, from its length alone, or
/// None when no padded base64 has that length. The device checks the rest.
fn decoded_len(data: &str) -> Option<usize> {
    let bytes = data.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let padding = bytes.iter().rev().take_while(|byte| **byte == b'=').count();
    if padding > 2 {
        return None;
    }
    Some(bytes.len() / 4 * 3 - padding)
}

impl From<RegistryError> for CommandError {
    fn from(error: RegistryError) -> Self {
        match error {
            RegistryError::NotFound => gone("This device"),
            RegistryError::Sqlite(_) => {
                tracing::error!(%error, "the device list failed");
                failed(sentence(error.to_string()))
            }
            _ => failed(sentence(error.to_string())),
        }
    }
}

/// The error for something that isn't there.
fn gone(what: &str) -> CommandError {
    CommandError::NotFound {
        what: what.to_owned(),
    }
}

/// The error for anything else.
fn failed(message: String) -> CommandError {
    CommandError::Failed { message }
}

/// Starts `text` with a capital, since a window shows it as it is.
fn sentence(text: String) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => text,
    }
}

/// Locks `mutex`. Each change under these locks is made whole, so a panic
/// elsewhere can't leave one half-written.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::{BTreeSet, VecDeque};
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    use maxhq_client::TransferState;
    use maxhq_protocol::admission::Admission;
    use maxhq_protocol::attention::{AttentionEvent, AttentionKind};
    use maxhq_protocol::find::{AgentExcerpt, GitItemSearch};
    use maxhq_protocol::ids::ProjectId;
    use maxhq_protocol::ids::{AdmissionId, ClientId, NetworkId, WorkspaceId};
    use maxhq_protocol::link::{NetworkKind, NetworkState, Route};
    use maxhq_protocol::model::{
        Account, Activity, DeviceSettings, Inbox, LoginKind, PermissionMode, ProjectSettings,
        Settler, Vendor, WorkspaceSpec,
    };
    use maxhq_protocol::state::{Delta, Seq};
    use maxhq_protocol::time::Timestamp;
    use maxhq_protocol::timeline::{
        ItemBody, ItemId, ItemStatus, TextField, TimelineEvent, TimelineFind, TimelineItem,
        TimelineUpdate,
    };
    use tokio::sync::mpsc::{channel, Sender};

    use crate::merge::tests::{agent, connected, link_state, unreachable, workspace};
    use crate::notify::tests::{FakeSender, Sent};
    use crate::tests::{block_on, runtime};
    use crate::timeline::tests::Recorder;
    use crate::timeline::TimelineMessage;
    use crate::view::tests::recording;

    /// A note version for a fake link to answer with.
    fn fake_note(
        project: ProjectId,
        note: maxhq_protocol::notes::NoteId,
        body: &str,
        deleted: bool,
    ) -> maxhq_protocol::notes::NoteVersion {
        maxhq_protocol::notes::NoteVersion {
            hash: maxhq_protocol::notes::VersionHash("ab".repeat(32)),
            project,
            note,
            parents: Vec::new(),
            title: "Release checklist".to_owned(),
            body: body.to_owned(),
            deleted,
            at: Timestamp(1_700_000_000_000),
            by: None,
        }
    }

    /// What a fake link was asked to do. Actions, answers and pages of
    /// older items show as the request they send.
    #[derive(Clone, Debug, PartialEq)]
    pub(crate) enum Call {
        Send(ClientMessage),
        /// An open, with the version it picks up after and the lead it
        /// names.
        Open(AgentId, Option<Seq>, Option<AgentId>),
        Close(AgentId),
        Refresh,
        Upload(UploadTarget, PathBuf),
        /// Bytes uploaded, by the name they go under.
        UploadBytes(UploadTarget, String),
        Download(AgentId, String),
        /// A file fetched to hand on to a device.
        Copy(AgentId, String),
        /// Files fetched from a workspace's checkout to copy to another.
        CopyFiles(WorkspaceId, Vec<String>),
        CopyTree(TreeHash),
        /// A picture or video fetched for the timeline, with its limit.
        Media(AgentId, String, u64, bool),
        Cancel(TransferId),
        OpenForward(PortId),
        CloseForward(PortId),
        /// A terminal attached from where it says.
        AttachTerminal(TerminalId, Option<u64>),
        DetachTerminal(TerminalId),
        /// Input to a terminal, by its length alone.
        TerminalInput(TerminalId, usize),
    }

    /// Everything the fake connector and its links saw.
    #[derive(Default)]
    pub(crate) struct Record {
        pub(crate) opened: Vec<LinkSpec>,
        /// The device each link last opened for, if it named one.
        pub(crate) opened_to: BTreeMap<LinkId, Option<DeviceId>>,
        pub(crate) events: BTreeMap<LinkId, Sender<LinkEvent>>,
        /// The links whose requests go out. The rest refuse them.
        pub(crate) up: BTreeSet<LinkId>,
        /// Links whose devices never send a result.
        silent: BTreeSet<LinkId>,
        /// Links whose devices' `maxd` is too old for every request.
        pub(crate) old: BTreeSet<LinkId>,
        /// Links whose devices run the oldest `maxd` this app still talks
        /// to, which refuses only the requests newer than that.
        pub(crate) oldest: BTreeSet<LinkId>,
        /// What an answer comes back with, or accepted.
        answer: Option<AnswerOutcome>,
        /// What a change to each agent's inbox comes back with, or done.
        inbox: BTreeMap<AgentId, ActionOutcome>,
        /// Agents whose inbox change goes out and then loses its link
        /// before the answer comes back.
        drops: BTreeSet<AgentId>,
        pub(crate) calls: Vec<(LinkId, Call)>,
        /// The sink each terminal was last attached with.
        pub(crate) terminal_sinks: BTreeMap<TerminalId, Arc<dyn TerminalSink>>,
        pub(crate) closed: Vec<LinkId>,
        networks: Option<Box<dyn Fn(Vec<Network>) + Send + Sync>>,
        /// Holds each open back until it gets a permit, when set.
        gate: Option<Arc<tokio::sync::Semaphore>>,
        /// The opens of each timeline on each link not yet matched by a
        /// close. As in the client, a close with none open does nothing.
        open: BTreeMap<(LinkId, AgentId), usize>,
        /// Holds each upload or download until it's cancelled, when set.
        pub(crate) hold: bool,
        /// The transfers held, each with its link and what wakes it.
        pub(crate) held: BTreeMap<TransferId, (LinkId, Arc<tokio::sync::Notify>)>,
        /// Links whose devices don't do file transfers.
        pub(crate) unsupported: BTreeSet<LinkId>,
        /// The forwards open on each link.
        pub(crate) forwards: BTreeMap<LinkId, Vec<LocalForward>>,
        /// What the next actions without an answer of their own come back
        /// with, in order, before done.
        pub(crate) acts: VecDeque<ActionOutcome>,
        /// What an upload comes back with, or done.
        pub(crate) uploaded: Option<ActionOutcome>,
        /// The notes each link's device holds, for note sync requests.
        pub(crate) notes: BTreeMap<LinkId, notes::tests::FakeNotes>,
        /// Holds each note sync request until it gets a permit, when set.
        pub(crate) notes_gate: Option<Arc<tokio::sync::Semaphore>>,
    }

    impl Record {
        /// Records a request, and says how a link in this state answers.
        fn take(&mut self, id: LinkId, call: Call) -> Result<(), LinkError> {
            self.calls.push((id, call));
            if !self.up.contains(&id) {
                Err(LinkError::NotConnected)
            } else if self.silent.contains(&id) {
                Err(LinkError::TimedOut)
            } else if self.old.contains(&id)
                || (self.oldest.contains(&id)
                    && matches!(
                        self.calls.last(),
                        Some((_, Call::Send(message)))
                            if !message.reaches(Some(maxhq_protocol::OLDEST_PROTOCOL_VERSION))
                    ))
            {
                Err(LinkError::TooOld)
            } else {
                Ok(())
            }
        }

        /// Records a transfer, and says whether it starts.
        fn transfer(&mut self, id: LinkId, call: Call) -> Result<(), TransferError> {
            self.take(id, call)?;
            if self.unsupported.contains(&id) {
                Err(TransferError::Unsupported)
            } else {
                Ok(())
            }
        }
    }

    /// Runs a fake transfer of four bytes on `link`, held until it's
    /// cancelled when the record says so.
    async fn fake_transfer(
        record: &Mutex<Record>,
        link: LinkId,
        file: String,
        on_progress: &OnProgress,
    ) -> Result<(), TransferError> {
        let transfer = TransferId::generate();
        let step = |sent, state| {
            on_progress(TransferProgress {
                transfer,
                sent,
                total: 4,
                file: file.clone(),
                state,
            });
        };
        step(0, TransferState::Uploading);
        let held = {
            let mut record = record.lock().unwrap();
            if record.hold {
                let cancelled = Arc::new(tokio::sync::Notify::new());
                record.held.insert(transfer, (link, cancelled.clone()));
                Some(cancelled)
            } else {
                None
            }
        };
        if let Some(cancelled) = held {
            cancelled.notified().await;
            step(0, TransferState::Failed);
            return Err(TransferError::Cancelled);
        }
        step(4, TransferState::Done);
        Ok(())
    }

    /// Opens fake links and records what they're asked.
    #[derive(Clone, Default)]
    pub(crate) struct Fake(Arc<Mutex<Record>>);

    impl Fake {
        /// Everything seen so far.
        pub(crate) fn record(&self) -> MutexGuard<'_, Record> {
            self.0.lock().unwrap()
        }

        /// What the links were asked since the last call.
        pub(crate) fn calls(&self) -> Vec<(LinkId, Call)> {
            std::mem::take(&mut self.record().calls)
        }
    }

    /// One fake link.
    pub(crate) struct FakeLink {
        id: LinkId,
        record: Arc<Mutex<Record>>,
    }

    impl Connector for Fake {
        type Link = FakeLink;

        fn open(
            &self,
            spec: LinkSpec,
            device: Option<DeviceId>,
        ) -> (FakeLink, Receiver<LinkEvent>) {
            let (events, receiver) = channel(16);
            let mut record = self.record();
            record.events.insert(spec.id, events);
            record.opened_to.insert(spec.id, device);
            let link = FakeLink {
                id: spec.id,
                record: self.0.clone(),
            };
            record.opened.push(spec);
            (link, receiver)
        }

        fn watch_networks(&self, on_change: Box<dyn Fn(Vec<Network>) + Send + Sync>) {
            self.record().networks = Some(on_change);
        }
    }

    impl LinkControl for FakeLink {
        fn send(
            &self,
            message: ClientMessage,
        ) -> impl Future<Output = Result<(), LinkError>> + Send {
            let result = self
                .record
                .lock()
                .unwrap()
                .take(self.id, Call::Send(message));
            async move { result }
        }

        fn act(
            &self,
            action: ClientMessage,
        ) -> impl Future<Output = Result<ActionOutcome, LinkError>> + Send {
            let mut record = self.record.lock().unwrap();
            let (outcome, drops) = match &action {
                ClientMessage::SetInbox { agent, .. } => (
                    record.inbox.get(agent).cloned(),
                    record.drops.contains(agent),
                ),
                // A lookup finds nothing.
                ClientMessage::FindPaths { .. } => (
                    Some(ActionOutcome::Paths {
                        matches: Vec::new(),
                    }),
                    false,
                ),
                ClientMessage::FindGitItems { .. } => (
                    Some(ActionOutcome::GitItems {
                        search: GitItemSearch {
                            repo: None,
                            items: Vec::new(),
                            unavailable: None,
                        },
                    }),
                    false,
                ),
                ClientMessage::ExcerptAgent { .. } => (
                    Some(ActionOutcome::Excerpt {
                        excerpt: AgentExcerpt {
                            name: "agent.md".to_owned(),
                            text: String::new(),
                            messages: 0,
                            omitted: 0,
                        },
                    }),
                    false,
                ),
                ClientMessage::ListIgnoredFiles { .. } => (
                    Some(ActionOutcome::IgnoredFiles {
                        files: Vec::new(),
                        truncated: false,
                    }),
                    false,
                ),
                ClientMessage::FindInTimeline { .. } => (
                    Some(ActionOutcome::TimelineFind {
                        find: TimelineFind {
                            total: 0,
                            active: 0,
                            found: None,
                            near: Vec::new(),
                        },
                    }),
                    false,
                ),
                // A list finds nothing unless a test queued one.
                ClientMessage::ListPermissions { .. } => (
                    record.acts.pop_front().or(Some(ActionOutcome::Permissions {
                        permissions: Vec::new(),
                    })),
                    false,
                ),
                ClientMessage::ListKeys { .. } => (
                    record
                        .acts
                        .pop_front()
                        .or(Some(ActionOutcome::Keys { keys: Vec::new() })),
                    false,
                ),
                ClientMessage::ListRuns { .. } => (
                    record
                        .acts
                        .pop_front()
                        .or(Some(ActionOutcome::Runs { runs: Vec::new() })),
                    false,
                ),
                ClientMessage::Notices {
                    call: maxhq_protocol::notices::NoticeCall::List { .. },
                    ..
                } => (
                    record.acts.pop_front().or(Some(ActionOutcome::Notices {
                        page: Default::default(),
                    })),
                    false,
                ),
                // Each link's device has a key of its own.
                ClientMessage::PeerKey { .. } => (
                    Some(ActionOutcome::PeerKey {
                        public_key: format!("ssh-ed25519 key-of-{}", self.id),
                    }),
                    false,
                ),
                ClientMessage::NodeDetail { .. } => (
                    record.acts.pop_front().or(Some(ActionOutcome::NodeDetail {
                        detail: maxhq_protocol::workflow::NodeDetail { lists: Vec::new() },
                    })),
                    false,
                ),
                ClientMessage::ListDeliveries { .. } => (
                    record.acts.pop_front().or(Some(ActionOutcome::Deliveries {
                        deliveries: Vec::new(),
                    })),
                    false,
                ),
                ClientMessage::ListProcesses { .. } => (
                    record.acts.pop_front().or(Some(ActionOutcome::Processes {
                        list: maxhq_protocol::processes::ProcessList::Listed {
                            processes: Vec::new(),
                        },
                    })),
                    false,
                ),
                ClientMessage::ListFolder { path, .. } => (
                    record
                        .acts
                        .pop_front()
                        .or(Some(ActionOutcome::FolderListed {
                            listing: maxhq_protocol::folders::FolderListing {
                                path: path.clone(),
                                home: Some("/home/dev".to_owned()),
                                exists: false,
                                entries: Vec::new(),
                                truncated: false,
                            },
                        })),
                    false,
                ),
                ClientMessage::CursorRules { .. } => (
                    record.acts.pop_front().or(Some(ActionOutcome::CursorRules {
                        rules: maxhq_protocol::cursor_rules::CursorRules {
                            path: "~/.cursor/cli-config.json".to_owned(),
                            rules: Vec::new(),
                        },
                    })),
                    false,
                ),
                // A note reads back as an empty one, and a save as what
                // was sent.
                ClientMessage::ReadNote { project, note, .. } => (
                    record.acts.pop_front().or(Some(ActionOutcome::NoteRead {
                        version: fake_note(*project, *note, "", false),
                    })),
                    false,
                ),
                ClientMessage::SaveNote {
                    project,
                    note,
                    body,
                    deleted,
                    ..
                } => (
                    record.acts.pop_front().or(Some(ActionOutcome::NoteSaved {
                        version: fake_note(*project, *note, body, *deleted),
                        merged: false,
                    })),
                    false,
                ),
                ClientMessage::NoteHistory { .. } => (
                    record.acts.pop_front().or(Some(ActionOutcome::NoteHistory {
                        versions: Vec::new(),
                    })),
                    false,
                ),
                // Note sync requests go to the link's notes, below.
                ClientMessage::NoteHashes { .. }
                | ClientMessage::NoteVersions { .. }
                | ClientMessage::PutNoteVersions { .. } => (None, false),
                _ => (record.acts.pop_front(), false),
            };
            let synced = notes::tests::syncing(&record, self.id, &action);
            let result = record.take(self.id, Call::Send(action)).and_then(|()| {
                if drops {
                    Err(LinkError::Closed)
                } else {
                    Ok(outcome.unwrap_or(ActionOutcome::Done))
                }
            });
            if drops {
                record.up.remove(&self.id);
            }
            async move {
                let outcome = result?;
                match synced {
                    Some(sync) => Ok(sync.answer().await),
                    None => Ok(outcome),
                }
            }
        }

        fn answer(
            &self,
            key: IdempotencyKey,
            event: AttentionId,
            answer: AttentionAnswer,
            refs: Vec<InlineRef>,
        ) -> impl Future<Output = Result<AnswerOutcome, LinkError>> + Send {
            let mut record = self.record.lock().unwrap();
            let message = ClientMessage::Answer {
                idempotency_key: key,
                event,
                answer,
                refs,
            };
            let outcome = record.answer.clone().unwrap_or(AnswerOutcome::Accepted);
            let result = record.take(self.id, Call::Send(message)).map(|()| outcome);
            async move { result }
        }

        fn open_timeline(
            &self,
            agent: AgentId,
            since: Option<Seq>,
            _limit: u32,
            lead: Option<AgentId>,
        ) -> impl Future<Output = Result<(), LinkError>> + Send {
            // A link opens timelines whether it's connected or not.
            let gate = {
                let mut record = self.record.lock().unwrap();
                let _ = record.take(self.id, Call::Open(agent, since, lead));
                record.gate.clone()
            };
            let (id, record) = (self.id, self.record.clone());
            async move {
                if let Some(gate) = gate {
                    gate.acquire().await.unwrap().forget();
                }
                *record.lock().unwrap().open.entry((id, agent)).or_default() += 1;
                Ok(())
            }
        }

        fn load_older(
            &self,
            agent: AgentId,
            before: u64,
            limit: u32,
        ) -> impl Future<Output = Result<(), LinkError>> + Send {
            let message = ClientMessage::LoadOlder {
                agent,
                before,
                limit,
            };
            let result = self
                .record
                .lock()
                .unwrap()
                .take(self.id, Call::Send(message));
            async move { result }
        }

        fn close_timeline(
            &self,
            agent: AgentId,
        ) -> impl Future<Output = Result<(), LinkError>> + Send {
            let mut record = self.record.lock().unwrap();
            let _ = record.take(self.id, Call::Close(agent));
            if let Some(opens) = record.open.get_mut(&(self.id, agent)) {
                *opens -= 1;
                if *opens == 0 {
                    record.open.remove(&(self.id, agent));
                }
            }
            async { Ok(()) }
        }

        fn open_workflow(&self, run: RunKey) -> impl Future<Output = Result<(), LinkError>> + Send {
            // A link opens runs whether it's connected or not.
            let message = ClientMessage::OpenWorkflow { run };
            let _ = self
                .record
                .lock()
                .unwrap()
                .take(self.id, Call::Send(message));
            async { Ok(()) }
        }

        fn close_workflow(
            &self,
            run: RunKey,
        ) -> impl Future<Output = Result<(), LinkError>> + Send {
            let message = ClientMessage::CloseWorkflow { run };
            let _ = self
                .record
                .lock()
                .unwrap()
                .take(self.id, Call::Send(message));
            async { Ok(()) }
        }

        fn refresh(&self) {
            let mut record = self.record.lock().unwrap();
            record.calls.push((self.id, Call::Refresh));
        }

        fn upload(
            &self,
            target: UploadTarget,
            path: PathBuf,
            on_progress: OnProgress,
        ) -> impl Future<Output = Result<ActionOutcome, TransferError>> + Send {
            let file = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let started = self
                .record
                .lock()
                .unwrap()
                .transfer(self.id, Call::Upload(target, path));
            let (id, record) = (self.id, self.record.clone());
            async move {
                started?;
                fake_transfer(&record, id, file, &on_progress).await?;
                let uploaded = record.lock().unwrap().uploaded.clone();
                Ok(uploaded.unwrap_or(ActionOutcome::Done))
            }
        }

        fn upload_bytes(
            &self,
            target: UploadTarget,
            name: String,
            _mime: String,
            _bytes: Vec<u8>,
            on_progress: OnProgress,
        ) -> impl Future<Output = Result<ActionOutcome, TransferError>> + Send {
            let started = self
                .record
                .lock()
                .unwrap()
                .transfer(self.id, Call::UploadBytes(target, name.clone()));
            let (id, record) = (self.id, self.record.clone());
            async move {
                started?;
                fake_transfer(&record, id, name, &on_progress).await?;
                Ok(ActionOutcome::Done)
            }
        }

        fn download(
            &self,
            agent: AgentId,
            path: String,
            on_progress: OnProgress,
        ) -> impl Future<Output = Result<PathBuf, TransferError>> + Send {
            let started = self
                .record
                .lock()
                .unwrap()
                .transfer(self.id, Call::Download(agent, path.clone()));
            let (id, record) = (self.id, self.record.clone());
            async move {
                started?;
                fake_transfer(&record, id, path.clone(), &on_progress).await?;
                Ok(PathBuf::from("/downloads").join(path))
            }
        }

        /// Every copy lands in a folder of its own under `/copies`.
        fn copy(
            &self,
            agent: AgentId,
            path: String,
            on_progress: OnProgress,
        ) -> impl Future<Output = Result<Copied, TransferError>> + Send {
            let started = self
                .record
                .lock()
                .unwrap()
                .transfer(self.id, Call::Copy(agent, path.clone()));
            let (id, record) = (self.id, self.record.clone());
            async move {
                started?;
                fake_transfer(&record, id, path.clone(), &on_progress).await?;
                let folder = PathBuf::from("/copies").join(TransferId::generate().to_string());
                let name = path.rsplit('/').next().unwrap_or_default();
                Ok(Copied::new(folder.clone(), folder.join(name)))
            }
        }

        /// The files land in a real folder `files` in a staging folder of
        /// their own under the temp folder, so a test can see it go.
        fn copy_files(
            &self,
            workspace: WorkspaceId,
            paths: Vec<String>,
            on_progress: OnProgress,
        ) -> impl Future<Output = Result<Copied, TransferError>> + Send {
            let started = self
                .record
                .lock()
                .unwrap()
                .transfer(self.id, Call::CopyFiles(workspace, paths));
            let (id, record) = (self.id, self.record.clone());
            async move {
                started?;
                fake_transfer(&record, id, "files".to_owned(), &on_progress).await?;
                let folder =
                    std::env::temp_dir().join(format!("maxhq-app-copy-{}", TransferId::generate()));
                std::fs::create_dir_all(folder.join("files")).unwrap();
                std::fs::write(folder.join("files/.env"), "TOKEN=1").unwrap();
                Ok(Copied::new(folder.clone(), folder.join("files")))
            }
        }

        /// Every tree lands in a folder of its own under `/copies`, named
        /// for its hash.
        fn copy_tree(
            &self,
            tree: TreeHash,
            on_progress: OnProgress,
        ) -> impl Future<Output = Result<Copied, TransferError>> + Send {
            let started = self
                .record
                .lock()
                .unwrap()
                .transfer(self.id, Call::CopyTree(tree.clone()));
            let (id, record) = (self.id, self.record.clone());
            async move {
                started?;
                fake_transfer(&record, id, tree.0.clone(), &on_progress).await?;
                let folder = PathBuf::from("/copies").join(TransferId::generate().to_string());
                Ok(Copied::new(folder.clone(), folder.join(&tree.0)))
            }
        }

        /// Every file is 4 bytes, named for its path.
        fn media(
            &self,
            agent: AgentId,
            path: String,
            limit: u64,
            anyway: bool,
            on_progress: OnProgress,
        ) -> impl Future<Output = Result<Media, TransferError>> + Send {
            let started = self
                .record
                .lock()
                .unwrap()
                .transfer(self.id, Call::Media(agent, path.clone(), limit, anyway));
            let (id, record) = (self.id, self.record.clone());
            async move {
                started?;
                if limit < 4 {
                    return Ok(Media::TooLarge { size: 4 });
                }
                fake_transfer(&record, id, path.clone(), &on_progress).await?;
                let name = path.rsplit('/').next().unwrap_or_default().to_owned();
                Ok(Media::Cached { name, size: 4 })
            }
        }

        fn cancel_transfer(&self, transfer: TransferId) {
            let mut record = self.record.lock().unwrap();
            record.calls.push((self.id, Call::Cancel(transfer)));
            if let Some((link, cancelled)) = record.held.get(&transfer)
                && *link == self.id
            {
                cancelled.notify_one();
            }
        }

        fn open_forward(
            &self,
            port: ListeningPort,
        ) -> impl Future<Output = Result<u16, ForwardError>> + Send {
            let mut record = self.record.lock().unwrap();
            let result = match record.take(self.id, Call::OpenForward(port.id)) {
                Ok(()) => {
                    record
                        .forwards
                        .entry(self.id)
                        .or_default()
                        .push(LocalForward {
                            port: port.id,
                            local_port: port.port,
                            forward: Some(ForwardId::generate()),
                        });
                    Ok(port.port)
                }
                Err(_) => Err(ForwardError::NotConnected),
            };
            async move { result }
        }

        fn close_forward(&self, port: PortId) {
            let mut record = self.record.lock().unwrap();
            record.calls.push((self.id, Call::CloseForward(port)));
            if let Some(open) = record.forwards.get_mut(&self.id) {
                open.retain(|forward| forward.port != port);
            }
        }

        fn forwards(&self) -> Vec<LocalForward> {
            let record = self.record.lock().unwrap();
            record.forwards.get(&self.id).cloned().unwrap_or_default()
        }

        fn attach_terminal(
            &self,
            terminal: TerminalId,
            after: Option<u64>,
            sink: Arc<dyn TerminalSink>,
        ) -> impl Future<Output = Result<(), LinkError>> + Send {
            let mut record = self.record.lock().unwrap();
            let result = record.take(self.id, Call::AttachTerminal(terminal, after));
            if !matches!(result, Err(LinkError::TooOld)) {
                record.terminal_sinks.insert(terminal, sink);
            }
            async move { result }
        }

        fn detach_terminal(
            &self,
            terminal: TerminalId,
        ) -> impl Future<Output = Result<(), LinkError>> + Send {
            let result = self
                .record
                .lock()
                .unwrap()
                .take(self.id, Call::DetachTerminal(terminal));
            async move { result }
        }

        fn terminal_input(
            &self,
            terminal: TerminalId,
            bytes: Vec<u8>,
        ) -> impl Future<Output = Result<(), LinkError>> + Send {
            let result = self
                .record
                .lock()
                .unwrap()
                .take(self.id, Call::TerminalInput(terminal, bytes.len()));
            async move { result }
        }
    }

    impl Drop for FakeLink {
        fn drop(&mut self) {
            let mut record = self.record.lock().unwrap();
            record.closed.push(self.id);
        }
    }

    /// Devices over fake links, the view they feed and what it notifies.
    pub(crate) struct Harness {
        pub(crate) devices: Devices<Fake>,
        pub(crate) fake: Fake,
        pub(crate) view: View,
        emitted: Arc<Mutex<Vec<Delta>>>,
        sender: FakeSender,
    }

    impl Harness {
        /// Sends a report the view may keep to itself, as it does a forged one,
        /// so nothing changes to wait on. A change to the link's own line comes
        /// after it on the same channel and shows it has passed.
        pub(crate) fn report_quiet(&self, link: LinkId, event: LinkEvent) {
            let events = self.fake.record().events[&link].clone();
            block_on(events.send(event)).unwrap();
            let mut line = self
                .view
                .read(|merge| merge.link(link).cloned())
                .expect("the link is in the view");
            line.via.push("after the report".into());
            self.report(link, LinkEvent::Link(Box::new(line)));
        }
    }

    /// Starts the devices in `registry` over fake links.
    pub(crate) fn harness(registry: Registry, maxd: Option<PathBuf>) -> Harness {
        let (view, emitted, sender) = recording();
        let fake = Fake::default();
        let devices = Devices::start(runtime(), fake.clone(), registry, view.clone(), maxd);
        Harness {
            devices,
            fake,
            view,
            emitted,
            sender,
        }
    }

    impl Harness {
        /// Has a link report `event`, and waits until the view shows it.
        pub(crate) fn report(&self, link: LinkId, event: LinkEvent) {
            let before = self.emitted.lock().unwrap().len();
            let events = self.fake.record().events[&link].clone();
            block_on(events.send(event)).unwrap();
            let start = Instant::now();
            while self.emitted.lock().unwrap().len() == before {
                assert!(
                    start.elapsed() < Duration::from_secs(5),
                    "the report never reached the view"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
        }

        /// Connects this machine's link and has it report `entities`.
        fn local_reports(&self, entities: Vec<Entity>) {
            let local = self.local();
            self.report(local.id, link_state(&local, connected()));
            self.report(local.id, LinkEvent::Synced(entities));
            self.fake.record().up.insert(local.id);
        }

        /// This machine's link, which opens first.
        pub(crate) fn local(&self) -> LinkSpec {
            self.fake.record().opened[0].clone()
        }

        /// What the view shows for `key`.
        fn shown(&self, key: EntityKey) -> Option<Entity> {
            self.view.read(|merge| merge.store().get(&key).cloned())
        }
    }

    /// An account a device reports.
    fn account(name: &str) -> Entity {
        Entity::Account(Account {
            id: AccountId::derive(name),
            device: DeviceId::derive("device"),
            vendor: Vendor::Claude,
            label: name.into(),
            config_dir: "~/.claude".into(),
            email: None,
            organization: None,
            plan: None,
            models: Vec::new(),
            usage: None,
            usage_failure: None,
            acp: None,
            login: LoginKind::ConfigDir,
            origin: None,
            model_server: None,
        })
    }

    #[test]
    fn a_link_reaches_only_the_device_it_first_reached_after_a_restart() {
        let dir = std::env::temp_dir().join(format!("maxhq-devices-{}", LinkId::generate()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("maxhq.db");
        let registry = Registry::open(&path).unwrap();
        let spec = registry.add("devbox").unwrap();

        // Nothing is known yet, so any device can say hello.
        let h = harness(registry, None);
        let opened_to = |h: &Harness, link| h.fake.record().opened_to[&link];
        assert_eq!(opened_to(&h, spec.id), None);
        assert_eq!(opened_to(&h, local_link()), None);

        // It connects, as this machine's link does.
        h.report(spec.id, link_state(&spec, connected()));
        let local = h.local();
        h.report(local.id, link_state(&local, connected()));
        let device = DeviceId::derive(&spec.name);
        let start = Instant::now();
        while h.devices.registry().known_device(spec.id).unwrap() != Some(device) {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "the link never kept its device"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        drop(h);

        // After a restart it opens for that device, and this machine's
        // link still takes whichever ID its own maxd has.
        let h = harness(Registry::open(&path).unwrap(), None);
        assert_eq!(opened_to(&h, spec.id), Some(device));
        assert_eq!(opened_to(&h, local_link()), None);

        // Changing its routes opens it again for the same device.
        h.devices
            .set_routes(spec.id, &["devbox.lan".to_owned()])
            .unwrap();
        assert_eq!(opened_to(&h, spec.id), Some(device));

        // Removed and added again, it trusts whichever device answers.
        h.devices.remove_device(spec.id).unwrap();
        let again = h.devices.add_device("devbox").unwrap();
        assert_eq!(opened_to(&h, again), None);

        drop(h);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_phone_opens_each_link_for_the_device_its_book_names() {
        use crate::phone::book_link;
        use crate::phone::tests::{book, entry};

        let h = harness(Registry::in_memory_phone().unwrap(), None);
        h.devices
            .put_phone_book(&book(vec![entry("devbox", &["devbox.local"])]))
            .unwrap();
        let devbox = DeviceId::derive("devbox");
        assert_eq!(
            h.fake.record().opened_to[&book_link(devbox)],
            Some(devbox),
            "no hello is needed first"
        );
    }

    /// Two devices as they start after a restart, each link holding the
    /// device it first reached.
    fn victim_and_hostile() -> (Harness, LinkSpec, LinkSpec) {
        let registry = Registry::in_memory().unwrap();
        let hostile = registry.add("hostile-host").unwrap();
        let victim = registry.add("victim-host").unwrap();
        registry
            .keep_device(hostile.id, DeviceId::derive("hostile"))
            .unwrap();
        registry
            .keep_device(victim.id, DeviceId::derive("victim"))
            .unwrap();
        (harness(registry, None), victim, hostile)
    }

    /// Shows the hostile link still down as the device it's for.
    fn assert_refused(h: &Harness, hostile: &LinkSpec) {
        let Some(Entity::DeviceLink(link)) = h.shown(EntityKey::DeviceLink(hostile.id)) else {
            panic!("the link is shown");
        };
        assert_eq!(link.device, Some(DeviceId::derive("hostile")));
        assert!(matches!(link.state, LinkState::Unreachable { .. }));
    }

    #[test]
    fn a_link_that_claims_another_device_gets_none_of_its_tokens() {
        let (h, victim, hostile) = victim_and_hostile();

        // The victim is off, and the hostile device says it's the victim.
        report_state(&h, &victim, "victim", unreachable());
        hello_as(&h, &hostile, "victim", None);
        assert_refused(&h, &hostile);

        let store = |key| ClientMessage::StoreClaudeToken {
            key,
            origin: "0199a7c2-0000-7000-8000-000000000005".parse().unwrap(),
            label: "Personal".into(),
            token: maxhq_protocol::secret::Secret::new("not-a-real-token".into()),
        };
        let outcomes = block_on(h.devices.act_on_each(&[DeviceId::derive("victim")], store));
        assert!(outcomes[0].is_err(), "the victim is off: {outcomes:?}");
        let sent = h.fake.calls().into_iter().any(|(id, call)| {
            id == hostile.id && matches!(call, Call::Send(ClientMessage::StoreClaudeToken { .. }))
        });
        assert!(!sent, "the token stays off the hostile link");
    }

    #[test]
    fn a_link_that_claims_another_device_gets_none_of_its_actions_with_both_up() {
        let (h, victim, hostile) = victim_and_hostile();
        hello_as(&h, &victim, "victim", None);
        hello_as(&h, &hostile, "victim", None);
        assert_refused(&h, &hostile);

        let action = ClientMessage::WindDown {
            key: IdempotencyKey::generate(),
            window_secs: 300,
        };
        block_on(h.devices.act_on(DeviceId::derive("victim"), action)).unwrap();
        let went: Vec<LinkId> = h
            .fake
            .calls()
            .into_iter()
            .filter(|(_, call)| matches!(call, Call::Send(ClientMessage::WindDown { .. })))
            .map(|(id, _)| id)
            .collect();
        assert_eq!(went, [victim.id]);
    }

    #[test]
    fn every_device_opens_at_startup_with_this_machine_first() {
        let registry = Registry::in_memory().unwrap();
        let nl = registry.add("dev@devbox").unwrap();
        let h = harness(registry, None);
        let opened = h.fake.record().opened.clone();
        assert_eq!(opened.len(), 2);
        assert_eq!(opened[0].id, local_link());
        assert_eq!(opened[0].route, Route::Local);
        assert_eq!(opened[1], nl);
        for spec in &opened {
            assert!(h.shown(EntityKey::DeviceLink(spec.id)).is_some());
        }

        let network = Network {
            id: NetworkId::derive("tailnet"),
            kind: NetworkKind::Tailscale {
                magic_dns_suffix: None,
                client_version: None,
            },
            name: "tailnet".into(),
            state: NetworkState::Up,
        };
        let networks = h.fake.record().networks.take().unwrap();
        networks(vec![network.clone()]);
        assert_eq!(
            h.shown(EntityKey::Network(network.id)),
            Some(Entity::Network(network))
        );
    }

    #[test]
    fn what_a_link_reports_reaches_the_view() {
        let h = harness(Registry::in_memory().unwrap(), None);
        h.local_reports(vec![agent("a", "w", Activity::Busy)]);
        let Some(Entity::Agent(shown)) = h.shown(EntityKey::Agent(AgentId::derive("a"))) else {
            panic!("the agent isn't in the view");
        };
        assert_eq!(shown.activity, Activity::Busy);
    }

    #[test]
    fn requests_go_through_a_connected_link_that_holds_the_agent() {
        let h = harness(Registry::in_memory().unwrap(), None);
        let a = AgentId::derive("a");
        assert_eq!(
            block_on(h.devices.mark_seen(a)),
            Err(gone("This agent")),
            "no link holds it yet"
        );

        h.local_reports(vec![agent("a", "w", Activity::DoneUnread)]);
        let local = h.local();
        block_on(h.devices.mark_seen(a)).unwrap();
        assert_eq!(
            h.fake.calls(),
            [(local.id, Call::Send(ClientMessage::MarkSeen { agent: a }))]
        );

        h.fake.record().up.clear();
        assert_eq!(
            block_on(h.devices.mark_seen(a)),
            Err(CommandError::NotConnected {
                device: local.name.clone()
            })
        );
    }

    #[test]
    fn usage_refreshes_go_to_the_account_or_to_every_device() {
        let registry = Registry::in_memory().unwrap();
        let nl = registry.add("devbox").unwrap();
        let h = harness(registry, None);
        let personal = AccountId::derive("personal");
        assert_eq!(
            block_on(h.devices.refresh_usage(Some(personal))),
            Err(gone("This account"))
        );
        assert_eq!(
            block_on(h.devices.refresh_usage(None)),
            Err(failed("No device is connected".to_owned()))
        );
        h.fake.calls();

        let local = h.local();
        h.local_reports(vec![on(&local.name, account("personal"))]);
        block_on(h.devices.refresh_usage(Some(personal))).unwrap();
        let one = ClientMessage::RefreshUsage {
            account: Some(personal),
        };
        assert_eq!(h.fake.calls(), [(local.id, Call::Send(one))]);

        block_on(h.devices.refresh_usage(None)).unwrap();
        let mut sent: Vec<LinkId> = h.fake.calls().into_iter().map(|(id, _)| id).collect();
        sent.sort();
        let mut every = vec![local.id, nl.id];
        every.sort();
        assert_eq!(sent, every, "a refresh of everything asks every device");
    }

    #[test]
    fn links_refresh_one_at_a_time_or_all_together() {
        let registry = Registry::in_memory().unwrap();
        let nl = registry.add("devbox").unwrap();
        let h = harness(registry, None);
        h.devices.refresh_links(Some(nl.id)).unwrap();
        assert_eq!(h.fake.calls(), [(nl.id, Call::Refresh)]);
        assert_eq!(
            h.devices.refresh_links(Some(LinkId::derive("gone"))),
            Err(gone("This device"))
        );
        h.devices.refresh_links(None).unwrap();
        assert_eq!(h.fake.calls().len(), 2);
    }

    #[test]
    fn devices_you_add_rename_and_remove_follow_the_registry() {
        let h = harness(Registry::in_memory().unwrap(), None);
        let id = h.devices.add_device(" dev@devbox ").unwrap();
        assert_eq!(h.devices.add_device("dev@devbox"), Ok(id));
        assert_eq!(h.fake.record().opened.len(), 2, "one link per device");
        assert_eq!(
            h.devices.add_device("-oProxyCommand=sh"),
            Err(failed("An SSH host can't start with a dash".to_owned()))
        );
        let name = |h: &Harness| h.view.read(|merge| merge.link(id).map(|l| l.name.clone()));
        assert_eq!(name(&h).as_deref(), Some("dev@devbox"));

        h.devices.rename_device(id, "NL").unwrap();
        assert_eq!(name(&h).as_deref(), Some("NL"));
        assert_eq!(
            h.devices.rename_device(id, " "),
            Err(failed("A device needs a name".to_owned()))
        );

        let nl = h.fake.record().opened[1].clone();
        h.report(id, link_state(&nl, connected()));
        h.report(id, LinkEvent::Synced(vec![agent("a", "w", Activity::Busy)]));
        assert!(matches!(
            h.devices.remove_device(local_link()),
            Err(CommandError::Failed { .. })
        ));
        h.devices.remove_device(id).unwrap();
        assert_eq!(h.fake.record().closed, [id]);
        assert_eq!(name(&h), None);
        assert_eq!(h.shown(EntityKey::Agent(AgentId::derive("a"))), None);
        assert_eq!(h.devices.remove_device(id), Err(gone("This device")));
        assert_eq!(h.devices.rename_device(id, "NL"), Err(gone("This device")));
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn restarting_maxd_goes_by_the_links_route_and_probes_it() {
        use crate::restart::tests::{worked, Recorded};
        use maxhq_core::service::{remote_restart_command, Manager};

        let registry = Registry::in_memory().unwrap();
        let nl = registry.add("dev@devbox").unwrap();
        let h = harness(registry, None);
        let mac = Here {
            manager: Some(Manager::Launchd),
            uid: 501,
        };
        let local = h.local();
        h.report(local.id, link_state(&local, connected()));
        h.report(nl.id, link_state(&nl, unreachable()));

        let runner = Recorded::answering(worked);
        block_on(h.devices.restart_maxd_with(local_link(), mac, &runner)).unwrap();
        block_on(h.devices.restart_maxd_with(nl.id, mac, &runner)).unwrap();
        assert_eq!(
            runner.ran(),
            [
                "launchctl kickstart -k gui/501/com.maxagenthq.maxd".to_owned(),
                format!("ssh dev@devbox {}", remote_restart_command()),
            ]
        );
        assert_eq!(
            h.fake.calls(),
            [(local_link(), Call::Refresh), (nl.id, Call::Refresh)],
            "the app connects again right away"
        );

        let failing = Recorded::answering(|| Err(io::Error::other("SSH sign-in was refused")));
        assert!(block_on(h.devices.restart_maxd_with(nl.id, mac, &failing)).is_err());
        assert!(h.fake.calls().is_empty(), "a failed restart probes nothing");
        assert_eq!(
            block_on(
                h.devices
                    .restart_maxd_with(LinkId::generate(), mac, &runner)
            ),
            Err(gone("This device"))
        );
    }

    /// Has a link report that it's connected to `device`.
    fn connect(h: &Harness, spec: &LinkSpec, device: &str) {
        report_state(h, spec, device, connected());
        h.fake.record().up.insert(spec.id);
    }

    /// Has a link to `device` report that it's connected to one that does
    /// what `does` says.
    pub(crate) fn connect_doing(h: &Harness, spec: &LinkSpec, device: &str, does: Capabilities) {
        let LinkEvent::Link(mut link) = link_state(spec, connected()) else {
            unreachable!("link_state reports a link");
        };
        link.device = Some(DeviceId::derive(device));
        link.capabilities = Some(does);
        h.report(spec.id, LinkEvent::Link(link));
        h.fake.record().up.insert(spec.id);
    }

    /// Has a link to `device` report that it dropped.
    pub(crate) fn disconnect(h: &Harness, spec: &LinkSpec, device: &str) {
        h.fake.record().up.remove(&spec.id);
        report_state(h, spec, device, unreachable());
    }

    /// Has a link to `device` report `state`.
    fn report_state(h: &Harness, spec: &LinkSpec, device: &str, state: LinkState) {
        let LinkEvent::Link(mut link) = link_state(spec, state) else {
            unreachable!("link_state reports a link");
        };
        link.device = Some(DeviceId::derive(device));
        h.report(spec.id, LinkEvent::Link(link));
    }

    /// Has a device say hello on a link as `device`, and reports what the
    /// real client does with it: a link opened for another device refuses
    /// it and stays down, still showing the one it's for.
    pub(crate) fn hello_as(
        h: &Harness,
        spec: &LinkSpec,
        device: &str,
        capabilities: Option<Capabilities>,
    ) {
        let said = DeviceId::derive(device);
        let known = h.fake.record().opened_to[&spec.id];
        let refused = known.is_some_and(|known| known != said);
        let state = if refused {
            LinkState::Unreachable {
                reason: "reports a different device ID".into(),
                last_seen: None,
            }
        } else {
            connected()
        };
        let LinkEvent::Link(mut link) = link_state(spec, state) else {
            unreachable!("link_state reports a link");
        };
        link.device = if refused { known } else { Some(said) };
        link.capabilities = capabilities;
        if refused {
            h.fake.record().up.remove(&spec.id);
        } else {
            h.fake.record().up.insert(spec.id);
        }
        h.report(spec.id, LinkEvent::Link(link));
    }

    /// Puts a workspace or an account on `device`.
    pub(crate) fn on(device: &str, mut entity: Entity) -> Entity {
        match &mut entity {
            Entity::Workspace(workspace) => workspace.device = DeviceId::derive(device),
            Entity::Account(account) => account.device = DeviceId::derive(device),
            _ => {}
        }
        entity
    }

    #[test]
    fn a_remote_link_that_claims_this_mac_s_id_gets_no_traffic() {
        let (h, local, devbox) = two_devices();
        let mac = DeviceId::derive("mac");
        let send = |h: &Harness| {
            h.fake.calls();
            let outcome = block_on(h.devices.act_on(
                mac,
                ClientMessage::StoreClaudeToken {
                    key: IdempotencyKey::generate(),
                    origin: AccountId::derive("personal"),
                    label: "work".to_owned(),
                    token: maxhq_protocol::secret::Secret::new("owner's token".to_owned()),
                },
            ));
            (outcome, h.fake.calls())
        };
        // Its hello says it's this Mac, and a remote link's ID sorts first.
        connect(&h, &devbox, "mac");
        assert!(devbox.id < local.id);

        let (_, calls) = send(&h);
        assert!(calls.iter().any(|(link, _)| *link == local.id));
        assert!(!calls.iter().any(|(link, _)| *link == devbox.id));
        assert_eq!(h.devices.route(mac), Some(Route::Local));

        // Not even while this Mac's own link is down.
        disconnect(&h, &local, "mac");
        let (outcome, calls) = send(&h);
        assert!(outcome.is_err(), "{outcome:?}");
        assert!(!calls.iter().any(|(link, _)| *link == devbox.id));
    }

    /// A question from an agent.
    fn question(name: &str, agent: &str) -> Entity {
        Entity::Attention(AttentionEvent {
            id: AttentionId::derive(name),
            agent: AgentId::derive(agent),
            kind: AttentionKind::Question,
            summary: "Which one?".into(),
            options: Vec::new(),
            accepts_text: true,
            created_at: Timestamp(1),
            expires_at: None,
        })
    }

    /// This machine and devbox, each connected and reporting a workspace
    /// with an agent in it. This machine also has an account, and devbox's
    /// agent asks a question.
    pub(crate) fn two_devices() -> (Harness, LinkSpec, LinkSpec) {
        let registry = Registry::in_memory().unwrap();
        let nl = registry.add("devbox").unwrap();
        let h = harness(registry, None);
        let local = h.local();
        connect(&h, &local, "mac");
        h.report(
            local.id,
            LinkEvent::Synced(vec![
                on("mac", workspace("w", "p")),
                agent("a", "w", Activity::Busy),
                on("mac", account("personal")),
            ]),
        );
        connect(&h, &nl, "nl");
        h.report(
            nl.id,
            LinkEvent::Synced(vec![
                on("nl", workspace("w-nl", "p")),
                agent("b", "w-nl", Activity::Busy),
                question("q", "b"),
            ]),
        );
        h.fake.calls();
        (h, local, nl)
    }

    fn interrupt(agent: &str) -> ClientMessage {
        ClientMessage::Interrupt {
            key: IdempotencyKey::generate(),
            agent: AgentId::derive(agent),
        }
    }

    fn start(place: AgentPlace) -> ClientMessage {
        ClientMessage::StartAgent {
            key: IdempotencyKey::generate(),
            spec: AgentSpec {
                effort: None,
                vendor: Vendor::Claude,
                account: AccountId::derive("personal"),
                place,
                model: None,
                permission: None,
                title: None,
                first: None,
                branch: None,
                origin: None,
            },
        }
    }

    /// Which links the requests since the last call went through.
    fn went_through(h: &Harness) -> Vec<LinkId> {
        h.fake.calls().into_iter().map(|(id, _)| id).collect()
    }

    /// What devbox's settings look like here, with the fixture's values.
    fn settings_on_nl() -> DeviceSettings {
        DeviceSettings {
            device: DeviceId::derive("nl"),
            default_permission: PermissionMode::Ask,
            workspace_network: None,
            workspace_pool: None,
            auto_settle_days: 5,
            snooze_at_limit: false,
            continue_claude_at_limit: false,
            continue_codex_at_limit: false,
            claude_effort: None,
            codex_effort: None,
            placement: Default::default(),
            tool_updates: Vec::new(),
            cursor_usage: false,
            keep_awake: Default::default(),
            test_hosts: Default::default(),
            skill_library: None,
            network: Default::default(),
            idle_release_minutes: 60,
            memory_items: 256,
            history_mib: 256,
            claude_setup_token: false,
            shell_integration: true,
            read_other_agents: true,
            dispatcher_acts: true,
            chat_gap_minutes: 30,
            local_models: Default::default(),
            claude_artifacts: true,
        }
    }

    #[test]
    fn a_settings_save_sends_what_changed_and_leaves_the_rest() {
        let (h, _, nl) = two_devices();
        let shown = settings_on_nl();
        let row = Entity::DeviceSettings(shown.clone());
        h.report(
            nl.id,
            LinkEvent::Changed(vec![Change::Upsert { value: row }]),
        );
        h.fake.calls();

        let same = block_on(h.devices.save_settings(shown.clone()));
        assert_eq!(same, Ok(ActionOutcome::Done));
        assert_eq!(h.fake.calls(), [], "nothing changed, so nothing goes");

        let new = DeviceSettings {
            snooze_at_limit: true,
            continue_claude_at_limit: false,
            continue_codex_at_limit: false,
            claude_effort: None,
            codex_effort: None,
            ..shown
        };
        block_on(h.devices.save_settings(new)).unwrap();
        let calls = h.fake.calls();
        let [(
            link,
            Call::Send(ClientMessage::UpdateDeviceSettings {
                device, changes, ..
            }),
        )] = &calls[..]
        else {
            panic!("only the change goes: {calls:?}");
        };
        assert_eq!((*link, *device), (nl.id, DeviceId::derive("nl")));
        assert_eq!(
            changes,
            &[maxhq_protocol::model::DeviceSettingChange::SnoozeAtLimit { value: true }]
        );
    }

    #[test]
    fn an_older_maxd_gets_a_whole_settings_save() {
        let (h, _, nl) = two_devices();
        let shown = settings_on_nl();
        let row = Entity::DeviceSettings(shown.clone());
        h.report(
            nl.id,
            LinkEvent::Changed(vec![Change::Upsert { value: row }]),
        );
        h.fake.calls();
        h.fake.record().oldest.insert(nl.id);
        let new = DeviceSettings {
            snooze_at_limit: true,
            continue_claude_at_limit: false,
            continue_codex_at_limit: false,
            claude_effort: None,
            codex_effort: None,
            ..shown
        };
        block_on(h.devices.save_settings(new.clone())).unwrap();
        let calls = h.fake.calls();
        assert!(
            matches!(
                calls.last(),
                Some((link, Call::Send(ClientMessage::SetDeviceSettings { settings, .. })))
                    if *link == nl.id && *settings == new
            ),
            "{calls:?}"
        );
    }

    #[test]
    fn actions_go_to_the_device_they_are_for() {
        let (h, local, nl) = two_devices();
        let act = |action| block_on(h.devices.act(action));

        let action = interrupt("a");
        assert_eq!(act(action.clone()), Ok(ActionOutcome::Done));
        assert_eq!(h.fake.calls(), [(local.id, Call::Send(action))]);
        act(interrupt("b")).unwrap();
        assert_eq!(went_through(&h), [nl.id], "the device of b's workspace");
        assert_eq!(act(interrupt("nobody")), Err(gone("This agent")));

        act(start(AgentPlace::Workspace {
            workspace: WorkspaceId::derive("w-nl"),
        }))
        .unwrap();
        assert_eq!(went_through(&h), [nl.id]);
        act(start(AgentPlace::Folder {
            path: "/tmp/scratch".into(),
        }))
        .unwrap();
        assert_eq!(went_through(&h), [local.id], "the account's device");
        assert_eq!(
            act(start(AgentPlace::Workspace {
                workspace: WorkspaceId::derive("gone"),
            })),
            Err(gone("This workspace"))
        );

        let settings = |device: &str| ClientMessage::SetDeviceSettings {
            key: IdempotencyKey::generate(),
            settings: DeviceSettings {
                device: DeviceId::derive(device),
                default_permission: PermissionMode::Ask,
                workspace_network: None,
                workspace_pool: None,
                auto_settle_days: 5,
                snooze_at_limit: false,
                continue_claude_at_limit: false,
                continue_codex_at_limit: false,
                claude_effort: None,
                codex_effort: None,
                placement: Default::default(),
                tool_updates: Vec::new(),
                cursor_usage: false,
                keep_awake: Default::default(),
                test_hosts: Default::default(),
                skill_library: None,
                network: Default::default(),
                idle_release_minutes: 60,
                memory_items: 256,
                history_mib: 256,
                claude_setup_token: false,
                shell_integration: true,
                read_other_agents: true,
                dispatcher_acts: true,
                chat_gap_minutes: 30,
                local_models: Default::default(),
                claude_artifacts: true,
            },
        };
        act(settings("nl")).unwrap();
        assert_eq!(went_through(&h), [nl.id]);
        assert_eq!(act(settings("elsewhere")), Err(gone("This device")));
        assert!(matches!(
            act(ClientMessage::MarkSeen {
                agent: AgentId::derive("a")
            }),
            Err(CommandError::Failed { .. })
        ));

        let path = AttachmentSource::Path {
            path: "/tmp/notes.md".into(),
        };
        block_on(h.devices.stage(DeviceId::derive("nl"), path.clone())).unwrap();
        let calls = h.fake.calls();
        let [(id, Call::Send(ClientMessage::StageAttachment { source, .. }))] = calls.as_slice()
        else {
            panic!("staging sent {calls:?}");
        };
        assert_eq!((*id, source), (nl.id, &path));

        // What the `@` menu found goes to the device of the agent whose
        // folder the search looked in.
        let found = |agent: &str| ClientMessage::StageAttachment {
            key: IdempotencyKey::generate(),
            source: AttachmentSource::Found {
                agent: AgentId::derive(agent),
                path: "src/main.rs".into(),
            },
        };
        act(found("b")).unwrap();
        assert_eq!(went_through(&h), [nl.id]);
        assert_eq!(act(found("nobody")), Err(gone("This agent")));
    }

    #[test]
    fn terminal_requests_go_where_the_terminals_agent_runs() {
        use maxhq_protocol::terminal::{Terminal, TerminalKind, TerminalStatus};

        let (h, _, nl) = two_devices();
        let act = |action| block_on(h.devices.act(action));
        let b = AgentId::derive("b");
        let terminal = TerminalId::derive("terminal");
        act(ClientMessage::TerminalOpen {
            key: IdempotencyKey::generate(),
            agent: b,
            kind: TerminalKind::Shell,
            cols: 80,
            rows: 24,
        })
        .unwrap();
        assert_eq!(went_through(&h), [nl.id], "the device of b's workspace");

        let close = || ClientMessage::TerminalClose {
            key: IdempotencyKey::generate(),
            terminal,
            force: false,
        };
        let attach = ClientMessage::TerminalAttach {
            terminal,
            after: None,
        };
        let send = |message| block_on(h.devices.send_terminal(terminal, message));
        assert_eq!(act(close()), Err(gone("This terminal")));
        assert_eq!(send(attach.clone()), Err(gone("This terminal")));

        let shell = Terminal {
            id: terminal,
            agent: b,
            device: DeviceId::derive("nl"),
            kind: TerminalKind::Shell,
            moved: false,
            title: "zsh".into(),
            cwd: "~/code/example-app".into(),
            status: TerminalStatus::Running,
            busy: false,
            marks: false,
            password_prompt: false,
            reads_keys: false,
            full_screen: false,
            app_cursor: false,
            waiting: false,
            answers: false,
            started_at: Timestamp(1_793_368_920_000),
            cols: 80,
            rows: 24,
        };
        h.report(
            nl.id,
            LinkEvent::Changed(vec![Change::Upsert {
                value: Entity::Terminal(shell),
            }]),
        );
        h.fake.calls();
        act(close()).unwrap();
        assert_eq!(went_through(&h), [nl.id]);
        send(attach.clone()).unwrap();
        assert_eq!(h.fake.calls(), [(nl.id, Call::Send(attach))]);
    }

    #[test]
    fn workspace_actions_go_to_the_workspaces_device() {
        let (h, local, nl) = two_devices();
        let act = |action| block_on(h.devices.act(action));
        let key = IdempotencyKey::generate;

        act(ClientMessage::CreateWorkspace {
            key: key(),
            spec: WorkspaceSpec {
                project: ProjectId::derive("p"),
                device: DeviceId::derive("nl"),
                name: None,
                start_at_boot: false,
            },
        })
        .unwrap();
        assert_eq!(went_through(&h), [nl.id], "the device it's made on");

        let w_nl = WorkspaceId::derive("w-nl");
        act(ClientMessage::StartWorkspace {
            key: key(),
            workspace: w_nl,
        })
        .unwrap();
        act(ClientMessage::StopWorkspace {
            key: key(),
            workspace: w_nl,
        })
        .unwrap();
        act(ClientMessage::DeleteWorkspace {
            key: key(),
            workspace: w_nl,
            discard_unpushed: false,
        })
        .unwrap();
        assert_eq!(went_through(&h), [nl.id, nl.id, nl.id]);
        assert_eq!(
            act(ClientMessage::StartWorkspace {
                key: key(),
                workspace: WorkspaceId::derive("gone"),
            }),
            Err(gone("This workspace"))
        );

        act(ClientMessage::SetProjectSettings {
            key: key(),
            settings: ProjectSettings {
                project: ProjectId::derive("p"),
                device: DeviceId::derive("mac"),
                resume_on_restart: true,
                ignored_ports: vec![5432],
                git_author: None,
                auto_settle_days: None,
                settle_on_merge: true,
                workflow_env: true,
                memory_soft: None,
                workflow_env_allowed: None,
            },
        })
        .unwrap();
        assert_eq!(went_through(&h), [local.id], "the device it's for");
    }

    #[test]
    fn copy_actions_go_to_the_workspaces_device_and_a_clone_where_it_goes() {
        let (h, local, nl) = two_devices();
        let act = |action| block_on(h.devices.act(action));
        let key = IdempotencyKey::generate;
        let w_nl = WorkspaceId::derive("w-nl");

        act(ClientMessage::ListIgnoredFiles {
            key: key(),
            workspace: w_nl,
        })
        .unwrap();
        act(ClientMessage::CloneFolder {
            key: key(),
            project: ProjectId::derive("p"),
            device: DeviceId::derive("mac"),
            parent: "/Users/dev/src".into(),
            name: "site".into(),
        })
        .unwrap();
        act(ClientMessage::CloneCancel {
            key: key(),
            workspace: w_nl,
        })
        .unwrap();
        assert_eq!(went_through(&h), [nl.id, local.id, nl.id]);

        let quiet: OnProgress = Arc::new(|_| {});
        let paths = vec![".env".to_owned()];
        block_on(h.devices.copy_files(w_nl, paths.clone(), quiet)).unwrap();
        assert_eq!(h.fake.calls(), [(nl.id, Call::CopyFiles(w_nl, paths))]);
    }

    #[test]
    fn folder_requests_go_to_the_device_they_name_and_an_old_maxd_lists_none() {
        let (h, local, nl) = two_devices();
        let key = IdempotencyKey::generate;
        let list = || ClientMessage::ListFolder {
            key: key(),
            device: DeviceId::derive("nl"),
            path: "~/code".into(),
            files: false,
            hidden: false,
            filter: None,
        };
        assert!(matches!(
            block_on(h.devices.act_unless_old(list())),
            Ok(Some(ActionOutcome::FolderListed { .. }))
        ));
        block_on(h.devices.act(ClientMessage::MakeFolder {
            key: key(),
            device: DeviceId::derive("mac"),
            path: "/Users/dev/code/new".into(),
        }))
        .unwrap();
        assert_eq!(went_through(&h), [nl.id, local.id]);

        h.fake.record().old.insert(nl.id);
        assert_eq!(block_on(h.devices.act_unless_old(list())), Ok(None));
    }

    #[test]
    fn device_actions_go_where_you_say_each_under_its_own_key() {
        let (h, local, nl) = two_devices();
        let wind_down = |key| ClientMessage::WindDown {
            key,
            window_secs: 300,
        };
        let nl_device = DeviceId::derive("nl");
        block_on(
            h.devices
                .act_on(nl_device, wind_down(IdempotencyKey::generate())),
        )
        .unwrap();
        assert_eq!(went_through(&h), [nl.id]);

        let devices = [
            DeviceId::derive("mac"),
            DeviceId::derive("elsewhere"),
            nl_device,
        ];
        let outcomes = block_on(h.devices.act_on_each(&devices, wind_down));
        assert_eq!(
            outcomes,
            [
                Ok(ActionOutcome::Done),
                Err(gone("This device")),
                Ok(ActionOutcome::Done)
            ]
        );
        let calls = h.fake.calls();
        let keys: Vec<_> = calls
            .iter()
            .map(|(id, call)| match call {
                Call::Send(ClientMessage::WindDown { key, .. }) => (*id, *key),
                other => panic!("expected a wind-down, got {other:?}"),
            })
            .collect();
        let [(first, a), (second, b)] = keys[..] else {
            panic!("one call for each device it found: {keys:?}");
        };
        assert_eq!((first, second), (local.id, nl.id));
        assert_ne!(a, b, "each device gets its own key");
    }

    #[test]
    fn a_github_check_goes_only_to_a_device_that_does_them() {
        let (h, _, nl) = two_devices();
        let nl_device = DeviceId::derive("nl");
        let does = |github_checks| Capabilities {
            transfers: true,
            tunnels: true,
            workspaces: true,
            github_checks,
            inbox: false,
            ..Capabilities::default()
        };
        connect_doing(&h, &nl, "nl", does(true));
        h.fake.calls();
        assert_eq!(
            block_on(h.devices.check_github_app(nl_device)),
            Ok(ActionOutcome::Done)
        );
        let calls = h.fake.calls();
        assert!(
            matches!(
                calls[..],
                [(id, Call::Send(ClientMessage::CheckGithubApp { .. }))] if id == nl.id
            ),
            "{calls:?}"
        );

        // An older maxd would never answer, so it's refused here instead.
        connect_doing(&h, &nl, "nl", does(false));
        h.fake.calls();
        assert_eq!(
            block_on(h.devices.check_github_app(nl_device)),
            Err(CommandError::Failed {
                message: "devbox's maxd is too old to check when asked. Update it, or wait for it to check again by itself".into()
            })
        );
        assert!(h.fake.calls().is_empty());
    }

    #[test]
    fn an_answer_goes_to_the_asking_agents_device() {
        let (h, _, nl) = two_devices();
        let q = AttentionId::derive("q");
        let yes = AttentionAnswer::Text { text: "yes".into() };
        let pill = InlineRef::Agent {
            agent: AgentId::derive("other"),
            title: "cache fix".into(),
            vendor: Vendor::Claude,
            device: DeviceId::derive("nl"),
            excerpt: None,
        };
        let like = AttentionAnswer::Text {
            text: "Like \u{FFFC}".into(),
        };
        assert_eq!(
            block_on(h.devices.answer(q, like.clone(), vec![pill.clone()])),
            Ok(ActionOutcome::Done)
        );
        let calls = h.fake.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, nl.id);
        assert!(matches!(
            &calls[0].1,
            Call::Send(ClientMessage::Answer { answer, refs, .. })
                if *answer == like && *refs == [pill.clone()]
        ));
        assert_eq!(
            block_on(h.devices.answer(q, yes.clone(), Vec::new())),
            Ok(ActionOutcome::Done)
        );
        assert_eq!(went_through(&h), [nl.id]);

        h.fake.record().answer = Some(AnswerOutcome::AlreadyAnswered {
            answered_at: Timestamp(5),
            answered_by: ClientId::derive("phone"),
        });
        assert_eq!(
            block_on(h.devices.answer(q, yes.clone(), Vec::new())),
            Ok(ActionOutcome::Refused {
                reason: "Someone else answered first".into()
            })
        );
        assert_eq!(
            block_on(
                h.devices
                    .answer(AttentionId::derive("other"), yes, Vec::new())
            ),
            Err(gone("This request"))
        );
    }

    #[test]
    fn an_admission_answer_goes_to_the_device_that_raised_the_card() {
        let (h, _, nl) = two_devices();
        let card = AdmissionId::derive("card");
        let raised = Entity::Admission(Admission {
            id: card,
            device: DeviceId::derive("nl"),
            question: maxhq_protocol::timeline::Question {
                id: "card".into(),
                header: None,
                text: "Let this program control devbox?".into(),
                options: Vec::new(),
                multi: false,
                other: false,
                secret: false,
                consent: None,
            },
            raised_at: Timestamp(1),
            expires_at: Timestamp(2),
        });
        h.report(
            nl.id,
            LinkEvent::Changed(vec![Change::Upsert { value: raised }]),
        );
        let answer = |admission| ClientMessage::AnswerAdmission {
            key: IdempotencyKey::generate(),
            admission,
            allow: true,
        };

        assert_eq!(
            block_on(h.devices.act(answer(card))),
            Ok(ActionOutcome::Done)
        );
        assert_eq!(went_through(&h), [nl.id]);
        assert_eq!(
            block_on(h.devices.act(answer(AdmissionId::derive("gone")))),
            Err(gone("That card"))
        );
        assert_eq!(went_through(&h), Vec::<LinkId>::new());
    }

    #[test]
    fn an_action_tries_each_link_to_the_device_and_reports_silence_or_age() {
        let registry = Registry::in_memory().unwrap();
        let tailnet = registry.add("devbox").unwrap();
        let lan = registry.add("devbox.lan").unwrap();
        // Both routes are known to reach the device, as after you said so.
        for spec in [&tailnet, &lan] {
            registry
                .keep_device(spec.id, DeviceId::derive("nl"))
                .unwrap();
        }
        let h = harness(registry, None);
        connect(&h, &tailnet, "nl");
        connect(&h, &lan, "nl");
        h.report(
            lan.id,
            LinkEvent::Synced(vec![
                on("nl", workspace("w", "p")),
                agent("a", "w", Activity::Busy),
            ]),
        );
        h.fake.calls();

        block_on(h.devices.act(interrupt("a"))).unwrap();
        let [first] = went_through(&h)[..] else {
            panic!("one link is enough");
        };
        h.fake.record().up.remove(&first);
        block_on(h.devices.act(interrupt("a"))).unwrap();
        let calls = h.fake.calls();
        let ids: Vec<LinkId> = calls.iter().map(|(id, _)| *id).collect();
        let second = if first == lan.id { tailnet.id } else { lan.id };
        assert_eq!(ids, [first, second], "the first link refused it");
        assert_eq!(calls[0].1, calls[1].1, "the same action, under one key");

        h.fake.record().up.clear();
        assert!(matches!(
            block_on(h.devices.act(interrupt("a"))),
            Err(CommandError::NotConnected { .. })
        ));

        h.fake.record().up.insert(lan.id);
        h.fake.record().silent.insert(lan.id);
        assert_eq!(
            block_on(h.devices.act(interrupt("a"))),
            Err(failed("devbox.lan didn't answer in time".to_owned()))
        );

        h.fake.record().silent.clear();
        h.fake.record().old.insert(lan.id);
        assert_eq!(
            block_on(h.devices.act(interrupt("a"))),
            Err(failed(
                "maxd on devbox.lan is too old for this; update it".to_owned()
            ))
        );
        // A request whose answer can say so hears none instead.
        let list = ClientMessage::ListProcesses {
            key: IdempotencyKey::generate(),
            agent: AgentId::derive("a"),
        };
        assert_eq!(block_on(h.devices.act_unless_old(list.clone())), Ok(None));
        h.fake.record().old.clear();
        assert!(matches!(
            block_on(h.devices.act_unless_old(list)),
            Ok(Some(ActionOutcome::Processes { .. }))
        ));
    }

    #[test]
    fn a_paste_over_the_limit_never_leaves_the_app() {
        let (h, _, nl) = two_devices();
        let paste = |data: String| AttachmentSource::Bytes {
            name: "paste.png".into(),
            mime: "image/png".into(),
            data,
        };
        let nl_device = DeviceId::derive("nl");
        // 8 MiB is two bytes past a whole number of base64 groups.
        let at_limit = "A".repeat((MAX_INLINE_ATTACHMENT / 3 + 1) * 4 - 1) + "=";
        assert_eq!(decoded_len(&at_limit), Some(MAX_INLINE_ATTACHMENT));
        let over = "A".repeat((MAX_INLINE_ATTACHMENT + 1) / 3 * 4);
        assert_eq!(decoded_len(&over), Some(MAX_INLINE_ATTACHMENT + 1));
        assert_eq!(
            block_on(h.devices.stage(nl_device, paste(over))),
            Err(failed("A paste can be at most 8 MiB".to_owned()))
        );
        assert_eq!(
            block_on(h.devices.stage(nl_device, paste("abc".into()))),
            Err(failed("The pasted data isn't base64".to_owned()))
        );
        assert!(h.fake.calls().is_empty());

        block_on(h.devices.stage(nl_device, paste(at_limit))).unwrap();
        assert_eq!(went_through(&h), [nl.id]);
        assert_eq!(decoded_len("aGk="), Some(2));
        assert_eq!(decoded_len(""), Some(0));
        assert_eq!(decoded_len("a==="), None);
    }

    #[test]
    fn windows_share_a_timeline_that_closes_when_the_last_one_leaves() {
        let (h, _, nl) = two_devices();
        let b = AgentId::derive("b");
        let (main, other) = (Recorder::new(1), Recorder::new(2));
        assert_eq!(
            block_on(h.devices.open_timeline("main", main.clone(), b)),
            Ok(())
        );
        assert_eq!(h.fake.calls(), [(nl.id, Call::Open(b, None, None))]);
        block_on(h.devices.open_timeline("main", main.clone(), b)).unwrap();
        block_on(h.devices.open_timeline("other", other.clone(), b)).unwrap();
        assert!(
            h.fake.calls().is_empty(),
            "one subscription serves them all"
        );
        assert_eq!(
            block_on(
                h.devices
                    .open_timeline("main", main.clone(), AgentId::derive("x"))
            ),
            Err(gone("This agent"))
        );

        let reset = TimelineUpdate::Reset {
            seq: Seq(1),
            items: Vec::new(),
            has_older: true,
        };
        let events = h.fake.record().events[&nl.id].clone();
        block_on(events.send(LinkEvent::Timeline {
            agent: b,
            update: reset.clone(),
            live: false,
        }))
        .unwrap();
        let wanted = [TimelineMessage {
            agent: b,
            update: reset,
            live: false,
        }];
        let started = Instant::now();
        while main.messages() != wanted || other.messages() != wanted {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "the reset never came"
            );
            std::thread::sleep(Duration::from_millis(2));
        }

        block_on(h.devices.close_timeline("main", b));
        block_on(h.devices.release_window("main")).unwrap();
        assert!(h.fake.calls().is_empty(), "the other window still watches");
        block_on(h.devices.close_timeline("other", b));
        assert_eq!(h.fake.calls(), [(nl.id, Call::Close(b))]);
    }

    /// `name` on `nl`, under the lead `parent`.
    fn sub_agent(name: &str, parent: &str) -> Entity {
        match agent(name, "w-nl", Activity::Busy) {
            Entity::Agent(mut agent) => {
                agent.parent = Some(AgentId::derive(parent));
                Entity::Agent(agent)
            }
            _ => unreachable!(),
        }
    }

    /// A run of `workflow` on `nl` that `lead` leads, with `agents` bound
    /// to its nodes.
    fn led_run(workflow: &str, lead: &str, agents: &[&str]) -> Entity {
        let key = maxhq_protocol::workflow::RunKey {
            device: DeviceId::derive("nl"),
            project: ProjectId::derive("p"),
            workflow: workflow.to_owned(),
            run: lead.to_owned(),
        };
        Entity::WorkflowRun(maxhq_protocol::workflow::WorkflowRun {
            id: key.id(),
            key,
            label: "Sub-agents".to_owned(),
            progress: maxhq_protocol::workflow::Progress { done: 1, total: 1 },
            current: Vec::new(),
            needs_you: false,
            orchestrators: vec![AgentId::derive(lead)],
            last_activity: None,
            done: false,
            failed: false,
            agents: agents.iter().map(|name| AgentId::derive(name)).collect(),
        })
    }

    #[test]
    fn a_finished_sub_agent_opens_through_its_lead() {
        let (h, _, nl) = two_devices();
        let [lead, running, done, nested] = ["b", "s", "done", "n"].map(AgentId::derive);
        let listed = |subs: Vec<Entity>| {
            let mut synced = vec![
                on("nl", workspace("w-nl", "p")),
                agent("b", "w-nl", Activity::Busy),
                led_run(SUB_AGENTS, "b", &["s", "done"]),
                led_run("pipeline", "b", &["x"]),
            ];
            synced.extend(subs);
            LinkEvent::Synced(synced)
        };
        h.report(
            nl.id,
            listed(vec![sub_agent("s", "b"), sub_agent("n", "s")]),
        );
        let window = Recorder::new(1);

        // A listed sub-agent names the lead at the top of its tree, so a
        // link that reconnects names it again.
        block_on(h.devices.open_timeline("main", window.clone(), nested)).unwrap();
        block_on(h.devices.open_timeline("main", window.clone(), lead)).unwrap();
        assert_eq!(
            h.fake.calls(),
            [
                (nl.id, Call::Open(nested, None, Some(lead))),
                (nl.id, Call::Open(lead, None, None)),
            ]
        );

        // The nested one finished and left the view, as did its parent.
        // It still opens through the lead a window saw it under, and one
        // a window never opened through the run it's bound to.
        block_on(h.devices.close_timeline("main", nested));
        h.report(nl.id, listed(Vec::new()));
        h.fake.calls();
        block_on(h.devices.open_timeline("main", window.clone(), nested)).unwrap();
        block_on(h.devices.open_timeline("main", window.clone(), done)).unwrap();
        block_on(h.devices.open_timeline("main", window.clone(), running)).unwrap();
        assert_eq!(
            h.fake.calls(),
            [
                (nl.id, Call::Open(nested, None, Some(lead))),
                (nl.id, Call::Open(done, None, Some(lead))),
                (nl.id, Call::Open(running, None, Some(lead))),
            ]
        );
        assert_eq!(
            block_on(
                h.devices
                    .open_timeline("main", window.clone(), AgentId::derive("x"))
            ),
            Err(gone("This agent")),
            "only a sub-agents run names a lead"
        );

        // Once the lead goes, so do its sub-agents.
        h.report(
            nl.id,
            LinkEvent::Synced(vec![on("nl", workspace("w-nl", "p"))]),
        );
        assert_eq!(
            block_on(
                h.devices
                    .open_timeline("other", Recorder::new(2), AgentId::derive("done"))
            ),
            Err(gone("This agent"))
        );
    }

    #[test]
    fn a_timeline_whose_window_went_away_closes_on_its_link() {
        let (h, _, nl) = two_devices();
        let b = AgentId::derive("b");
        let window = Recorder::new(1);
        block_on(h.devices.open_timeline("main", window.clone(), b)).unwrap();
        h.fake.calls();

        window.broken.store(true, Ordering::SeqCst);
        let events = h.fake.record().events[&nl.id].clone();
        let reset = LinkEvent::Timeline {
            agent: b,
            update: TimelineUpdate::Reset {
                seq: Seq(1),
                items: Vec::new(),
                has_older: false,
            },
            live: false,
        };
        block_on(events.send(reset.clone())).unwrap();
        let started = Instant::now();
        while h.devices.core.timelines.link_of(b).is_some() {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "the window stayed"
            );
            std::thread::sleep(Duration::from_millis(2));
        }

        // The next report from the link closes it.
        block_on(events.send(reset)).unwrap();
        let started = Instant::now();
        loop {
            let calls = h.fake.calls();
            if !calls.is_empty() {
                assert_eq!(calls, [(nl.id, Call::Close(b))]);
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "it never closed"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Has a link report `event`, without waiting for it to land.
    fn tell(h: &Harness, link: LinkId, event: LinkEvent) {
        let events = h.fake.record().events[&link].clone();
        block_on(events.send(event)).unwrap();
    }

    /// Has a link send a timeline update.
    fn update(h: &Harness, link: LinkId, agent: AgentId, update: TimelineUpdate) {
        let live = matches!(update, TimelineUpdate::Events { .. });
        tell(
            h,
            link,
            LinkEvent::Timeline {
                agent,
                update,
                live,
            },
        );
    }

    /// The updates `window` got.
    fn got(window: &Recorder) -> Vec<TimelineUpdate> {
        let messages = window.messages().into_iter();
        messages.map(|message| message.update).collect()
    }

    fn item(id: &str, ordinal: u64) -> TimelineItem {
        TimelineItem {
            id: ItemId(id.into()),
            agent: AgentId::derive("b"),
            turn: None,
            parent: None,
            ordinal,
            status: ItemStatus::Completed,
            started_at: None,
            completed_at: None,
            body: ItemBody::AssistantMessage {
                text: id.into(),
                streaming: false,
            },
        }
    }

    #[test]
    fn a_page_of_older_items_goes_to_the_window_that_asked() {
        let (h, _, nl) = two_devices();
        let b = AgentId::derive("b");
        let (main, other) = (Recorder::new(1), Recorder::new(2));
        block_on(h.devices.open_timeline("main", main.clone(), b)).unwrap();
        block_on(h.devices.open_timeline("other", other.clone(), b)).unwrap();
        let reset = TimelineUpdate::Reset {
            seq: Seq(1),
            items: vec![item("r", 40)],
            has_older: true,
        };
        update(&h, nl.id, b, reset.clone());
        wait_until("the reset never came", || {
            got(&main) == [reset.clone()] && got(&other) == [reset.clone()]
        });
        h.fake.calls();

        block_on(h.devices.load_older("main", b, 40)).unwrap();
        assert_eq!(
            h.fake.calls(),
            [(
                nl.id,
                Call::Send(ClientMessage::LoadOlder {
                    agent: b,
                    before: 40,
                    limit: PAGE
                })
            )]
        );
        for (window, agent) in [("main", "a"), ("nobody", "b")] {
            assert_eq!(
                block_on(h.devices.load_older(window, AgentId::derive(agent), 40)),
                Err(gone("This timeline")),
                "{window} doesn't watch {agent}"
            );
        }

        let page = TimelineUpdate::Older {
            items: vec![item("q", 39)],
            has_older: true,
        };
        let next = TimelineUpdate::Events {
            seq: Seq(2),
            events: Vec::new(),
        };
        update(&h, nl.id, b, page.clone());
        update(&h, nl.id, b, next.clone());
        wait_until("the updates never came", || {
            got(&main).len() == 3 && got(&other).len() == 2
        });
        assert_eq!(got(&main), [reset.clone(), page, next.clone()]);
        assert_eq!(got(&other), [reset, next]);

        h.fake.record().up.remove(&nl.id);
        assert!(matches!(
            block_on(h.devices.load_older("main", b, 39)),
            Err(CommandError::NotConnected { .. })
        ));
        let stray = TimelineUpdate::Older {
            items: Vec::new(),
            has_older: false,
        };
        update(&h, nl.id, b, stray);
        let late = Recorder::new(3);
        block_on(h.devices.open_timeline("late", late.clone(), b)).unwrap();
        wait_until("the late window got nothing", || !got(&late).is_empty());
        assert_eq!(
            got(&late),
            [TimelineUpdate::Reset {
                seq: Seq(2),
                items: vec![item("q", 39), item("r", 40)],
                has_older: true,
            }],
            "the app keeps the page, and one nobody asked for changes nothing"
        );
        assert_eq!(got(&main).len(), 3, "the refused ask got no page");
    }

    #[test]
    fn a_timeline_moves_to_another_link_when_its_own_drops() {
        let registry = Registry::in_memory().unwrap();
        let tailnet = registry.add("devbox").unwrap();
        let lan = registry.add("devbox.lan").unwrap();
        // Both routes are known to reach the device, as after you said so.
        for spec in [&tailnet, &lan] {
            registry
                .keep_device(spec.id, DeviceId::derive("nl"))
                .unwrap();
        }
        let h = harness(registry, None);
        let synced = vec![
            on("nl", workspace("w", "p")),
            agent("b", "w", Activity::Busy),
        ];
        connect(&h, &tailnet, "nl");
        connect(&h, &lan, "nl");
        h.report(tailnet.id, LinkEvent::Synced(synced.clone()));
        // Both links reach the same maxd, so they report the same.
        tell(&h, lan.id, LinkEvent::Synced(synced));
        let b = AgentId::derive("b");
        wait_until("the second link never reported", || {
            h.view
                .read(|merge| merge.holders(&EntityKey::Agent(b)).len() == 2)
        });
        let window = Recorder::new(1);
        block_on(h.devices.open_timeline("main", window.clone(), b)).unwrap();
        let first = h.devices.core.timelines.link_of(b).unwrap();
        let (gone_spec, second) = if first == lan.id {
            (&lan, tailnet.id)
        } else {
            (&tailnet, lan.id)
        };
        let reset = TimelineUpdate::Reset {
            seq: Seq(3),
            items: vec![item("r", 1)],
            has_older: true,
        };
        update(&h, first, b, reset.clone());
        wait_until("the reset never came", || got(&window) == [reset.clone()]);
        h.fake.calls();

        disconnect(&h, gone_spec, "nl");
        wait_until("the timeline never moved", || {
            h.fake.record().calls.len() == 2
        });
        assert_eq!(
            h.fake.calls(),
            [
                (second, Call::Open(b, Some(Seq(3)), None)),
                (first, Call::Close(b))
            ],
            "the new link picks up after what the app holds"
        );
        assert_eq!(h.devices.core.timelines.link_of(b), Some(second));

        let append = |seq: u64, text: &str| TimelineUpdate::Events {
            seq: Seq(seq),
            events: vec![TimelineEvent::Append {
                id: ItemId("r".into()),
                field: TextField::Text,
                text: text.into(),
            }],
        };
        update(&h, first, b, append(4, " from the old link"));
        update(&h, second, b, append(3, " again"));
        update(&h, second, b, append(4, " next"));
        wait_until("the next update never came", || got(&window).len() == 2);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(got(&window), [reset, append(4, " next")]);

        // Back up, the old link leaves the timeline where it is.
        connect(&h, gone_spec, "nl");
        std::thread::sleep(Duration::from_millis(50));
        assert!(h.fake.calls().is_empty());
        assert_eq!(h.devices.core.timelines.link_of(b), Some(second));
    }

    /// What a `maxd` that keeps inboxes says it does.
    fn keeps_inbox() -> Capabilities {
        Capabilities {
            inbox: true,
            ..Capabilities::default()
        }
    }

    /// Has a link to `device` report `state`, from a `maxd` that keeps
    /// inboxes. The client keeps what a device does after its link drops,
    /// so this does too.
    fn report_keeping(h: &Harness, spec: &LinkSpec, device: &str, state: LinkState) {
        let LinkEvent::Link(mut link) = link_state(spec, state) else {
            unreachable!("link_state reports a link");
        };
        link.device = Some(DeviceId::derive(device));
        link.capabilities = Some(keeps_inbox());
        h.report(spec.id, LinkEvent::Link(link));
    }

    /// What devbox reports in the inbox tests: a workspace with three
    /// agents in it.
    fn devbox_agents(activity: Activity) -> Vec<Entity> {
        vec![
            on("nl", workspace("w-nl", "p")),
            agent("b", "w-nl", activity.clone()),
            agent("c", "w-nl", activity.clone()),
            agent("d", "w-nl", activity),
        ]
    }

    /// This machine and devbox as [`two_devices`] has them, with devbox's
    /// `maxd` keeping inboxes and three agents, and its link down.
    fn devbox_away() -> (Harness, LinkSpec) {
        let (h, _, nl) = two_devices();
        report_keeping(&h, &nl, "nl", connected());
        h.report(nl.id, LinkEvent::Synced(devbox_agents(Activity::Busy)));
        h.fake.record().up.remove(&nl.id);
        report_keeping(&h, &nl, "nl", unreachable());
        h.fake.calls();
        (h, nl)
    }

    /// Brings devbox's link back, ready for requests by the time it shows
    /// connected.
    fn devbox_back(h: &Harness, nl: &LinkSpec) {
        h.fake.record().up.insert(nl.id);
        report_keeping(h, nl, "nl", connected());
    }

    /// Makes one inbox change for one agent.
    fn set_inbox(
        h: &Harness,
        agent: &str,
        change: InboxChange,
    ) -> Result<ActionOutcome, CommandError> {
        let mut outcomes = block_on(h.devices.set_inbox(&[AgentId::derive(agent)], &change));
        assert_eq!(outcomes.len(), 1, "one result for each agent");
        outcomes.remove(0)
    }

    /// What waits in the outbox.
    fn outbox(h: &Harness) -> Vec<Queued> {
        lock(&h.devices.core.registry).queued().unwrap()
    }

    /// Waits until the outbox is empty.
    fn outbox_empties(h: &Harness) {
        wait_until("the outbox never emptied", || outbox(h).is_empty());
    }

    /// The keys of the inbox changes sent to `link` since the last call.
    fn inbox_keys(h: &Harness, link: LinkId) -> Vec<IdempotencyKey> {
        h.fake
            .calls()
            .into_iter()
            .filter_map(|(id, call)| match call {
                Call::Send(ClientMessage::SetInbox { key, .. }) if id == link => Some(key),
                _ => None,
            })
            .collect()
    }

    /// The agent as the view shows it.
    fn shown_agent(h: &Harness, name: &str) -> maxhq_protocol::model::Agent {
        match h.shown(EntityKey::Agent(AgentId::derive(name))) {
            Some(Entity::Agent(agent)) => agent,
            other => panic!("{name} isn't an agent in the view: {other:?}"),
        }
    }

    /// The agents the view shows waiting for `link`.
    fn waiting_on(h: &Harness, link: LinkId) -> Vec<AgentId> {
        match h.shown(EntityKey::DeviceLink(link)) {
            Some(Entity::DeviceLink(link)) => link.waiting,
            other => panic!("the link isn't in the view: {other:?}"),
        }
    }

    /// The bodies of the outbox's notifications since the last call.
    fn refusals(h: &Harness) -> Vec<(String, String)> {
        h.sender
            .take()
            .into_iter()
            .filter_map(|sent| match sent {
                Sent::Deliver(n) if n.id.starts_with("inbox:") => Some((n.id, n.body)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn an_inbox_change_goes_straight_to_a_connected_device() {
        let (h, nl) = devbox_away();
        devbox_back(&h, &nl);
        assert_eq!(
            set_inbox(&h, "b", InboxChange::Settle),
            Ok(ActionOutcome::Done)
        );
        assert_eq!(inbox_keys(&h, nl.id).len(), 1);

        // A refusal comes back as it is, with nothing kept and nothing to
        // notify, since the window that asked shows it.
        let reason = "it needs your input".to_owned();
        h.fake.record().inbox.insert(
            AgentId::derive("c"),
            ActionOutcome::Refused {
                reason: reason.clone(),
            },
        );
        assert_eq!(
            set_inbox(&h, "c", InboxChange::Settle),
            Ok(ActionOutcome::Refused { reason })
        );
        assert!(outbox(&h).is_empty());
        assert!(refusals(&h).is_empty());
    }

    #[test]
    fn an_inbox_change_for_a_device_that_is_away_waits_and_shows_at_once() {
        let (h, nl) = devbox_away();
        let before = Timestamp::now();
        assert_eq!(
            set_inbox(&h, "b", InboxChange::Settle),
            Ok(ActionOutcome::Done)
        );

        // The link refused it at once, and the outbox kept it under the key
        // it went out with.
        let tried = inbox_keys(&h, nl.id);
        let kept = outbox(&h);
        assert_eq!(kept.len(), 1);
        assert_eq!(tried, [kept[0].key]);
        assert_eq!(
            (kept[0].link, kept[0].device, kept[0].agent),
            (nl.id, DeviceId::derive("nl"), AgentId::derive("b"))
        );
        assert_eq!(kept[0].change, InboxChange::Settle);
        assert_eq!(kept[0].title, "b title");
        assert!(kept[0].at >= before);

        let b = shown_agent(&h, "b");
        assert_eq!(b.activity, Activity::NotAccessible);
        let settled = b.inbox.settled.expect("the view shows it settled");
        assert_eq!((settled.at, settled.by), (kept[0].at, Settler::You));
        assert_eq!(shown_agent(&h, "c").inbox, Inbox::default());
        assert_eq!(waiting_on(&h, nl.id), [AgentId::derive("b")]);
    }

    #[test]
    fn the_outbox_goes_in_order_under_its_keys_once_the_device_is_back() {
        let (h, nl) = devbox_away();
        set_inbox(&h, "b", InboxChange::Settle).unwrap();
        set_inbox(&h, "b", InboxChange::Pin { key: "m".into() }).unwrap();
        set_inbox(
            &h,
            "c",
            InboxChange::Snooze {
                until: Timestamp(u64::MAX),
            },
        )
        .unwrap();
        let kept: Vec<IdempotencyKey> = outbox(&h).iter().map(|queued| queued.key).collect();
        assert_eq!(kept.len(), 3);
        let b = shown_agent(&h, "b").inbox;
        assert!(b.pin.is_some() && b.settled.is_none(), "applied in order");
        assert_eq!(
            waiting_on(&h, nl.id),
            [AgentId::derive("b"), AgentId::derive("c")]
        );
        h.fake.calls();

        devbox_back(&h, &nl);
        h.report(nl.id, LinkEvent::Synced(devbox_agents(Activity::Idle)));
        outbox_empties(&h);
        assert_eq!(inbox_keys(&h, nl.id), kept);
        // The fake device didn't change anything, so the view shows what it
        // reports once the outbox has gone.
        wait_until("the link still shows the outbox", || {
            waiting_on(&h, nl.id).is_empty()
        });
        assert_eq!(shown_agent(&h, "b").inbox, Inbox::default());
        assert_eq!(shown_agent(&h, "c").inbox, Inbox::default());
    }

    #[test]
    fn every_answer_takes_a_change_out_of_the_outbox_and_a_refusal_says_why() {
        let (h, nl) = devbox_away();
        for name in ["b", "d", "c"] {
            set_inbox(&h, name, InboxChange::Settle).unwrap();
        }
        let kept: Vec<IdempotencyKey> = outbox(&h).iter().map(|queued| queued.key).collect();
        let refused = kept[1];
        {
            let mut record = h.fake.record();
            record.inbox.insert(
                AgentId::derive("d"),
                ActionOutcome::Refused {
                    reason: "it needs your input".into(),
                },
            );
            record
                .inbox
                .insert(AgentId::derive("c"), ActionOutcome::UnknownAgent);
        }
        h.sender.take();
        h.fake.calls();

        devbox_back(&h, &nl);
        outbox_empties(&h);
        assert_eq!(
            inbox_keys(&h, nl.id),
            kept,
            "a refusal doesn't stop the rest"
        );
        assert_eq!(
            refusals(&h),
            [(
                format!("inbox:{refused}"),
                "Couldn't settle d title on devbox: it needs your input".to_owned()
            )]
        );
        assert_eq!(shown_agent(&h, "d").inbox, Inbox::default());
    }

    #[test]
    fn a_change_that_may_have_gone_out_goes_again_under_the_same_key() {
        let (h, nl) = devbox_away();
        // The link carried it, then dropped before the answer came.
        h.fake.record().up.insert(nl.id);
        h.fake.record().silent.insert(nl.id);
        assert_eq!(
            set_inbox(&h, "b", InboxChange::Settle),
            Ok(ActionOutcome::Done)
        );
        let first = inbox_keys(&h, nl.id);
        let kept = outbox(&h);
        assert_eq!(first, [kept[0].key]);

        h.fake.record().silent.remove(&nl.id);
        devbox_back(&h, &nl);
        outbox_empties(&h);
        assert_eq!(
            inbox_keys(&h, nl.id),
            first,
            "the same key, so it applies once"
        );

        // A device still connected that doesn't answer may have made the
        // change, so that's an error and nothing waits.
        h.fake.record().silent.insert(nl.id);
        assert_eq!(
            set_inbox(&h, "b", InboxChange::Unsettle),
            Err(failed("devbox didn't answer in time".to_owned()))
        );
        assert!(outbox(&h).is_empty());
    }

    #[test]
    fn a_connected_device_that_does_not_answer_gets_the_outbox_again_in_order() {
        let (h, nl) = devbox_away();
        for name in ["b", "c"] {
            set_inbox(&h, name, InboxChange::Settle).unwrap();
        }
        let kept: Vec<IdempotencyKey> = outbox(&h).iter().map(|queued| queued.key).collect();
        h.fake.calls();

        // The device comes back and its link stays up, but no answer comes.
        h.fake.record().silent.insert(nl.id);
        devbox_back(&h, &nl);
        let tried = RefCell::new(Vec::new());
        wait_until("the outbox never went again", || {
            tried.borrow_mut().extend(inbox_keys(&h, nl.id));
            tried.borrow().len() >= 3
        });
        assert!(
            tried.borrow().iter().all(|key| *key == kept[0]),
            "only the oldest goes, one at a time"
        );
        assert_eq!(outbox(&h).len(), 2);

        h.fake.record().silent.remove(&nl.id);
        outbox_empties(&h);
        let mut tried = tried.into_inner();
        tried.extend(inbox_keys(&h, nl.id));
        assert!(tried.ends_with(&kept), "in order: {tried:?}");
        assert!(refusals(&h).is_empty());
    }

    #[test]
    fn the_wait_before_the_outbox_goes_again_doubles_up_to_the_most() {
        assert_eq!(retry_wait(1), RETRY_WAIT);
        assert_eq!(retry_wait(2), RETRY_WAIT * 2);
        assert_eq!(retry_wait(3), RETRY_WAIT * 4);
        assert_eq!(retry_wait(9), RETRY_MOST);
        assert_eq!(retry_wait(u32::MAX), RETRY_MOST);
    }

    #[test]
    fn a_change_waits_behind_the_ones_before_it() {
        let (h, nl) = devbox_away();
        set_inbox(&h, "b", InboxChange::Settle).unwrap();
        let settle = outbox(&h)[0].key;

        // The link shows connected but can't take requests yet, so the
        // first change is still waiting when the next one comes.
        report_keeping(&h, &nl, "nl", connected());
        h.fake.calls();
        assert_eq!(
            set_inbox(&h, "b", InboxChange::Unsettle),
            Ok(ActionOutcome::Done)
        );
        let kept: Vec<IdempotencyKey> = outbox(&h).iter().map(|queued| queued.key).collect();
        let unsettle = kept[1];
        assert_eq!(kept, [settle, unsettle]);
        assert!(
            !inbox_keys(&h, nl.id).contains(&unsettle),
            "it didn't jump the queue"
        );

        h.fake.record().up.insert(nl.id);
        h.report(nl.id, LinkEvent::Synced(devbox_agents(Activity::Idle)));
        outbox_empties(&h);
        let sent = inbox_keys(&h, nl.id);
        assert!(sent.ends_with(&[settle, unsettle]), "{sent:?}");
        assert_eq!(sent.iter().filter(|key| **key == unsettle).count(), 1);
    }

    #[test]
    fn a_link_that_drops_partway_leaves_the_rest_waiting() {
        let (h, nl) = devbox_away();
        for name in ["b", "c", "d"] {
            set_inbox(&h, name, InboxChange::Settle).unwrap();
        }
        let kept: Vec<IdempotencyKey> = outbox(&h).iter().map(|queued| queued.key).collect();
        h.fake.record().drops.insert(AgentId::derive("c"));
        h.fake.calls();

        devbox_back(&h, &nl);
        wait_until("the link never dropped", || {
            !h.fake.record().up.contains(&nl.id)
        });
        report_keeping(&h, &nl, "nl", unreachable());
        assert_eq!(inbox_keys(&h, nl.id), kept[..2]);
        let left: Vec<IdempotencyKey> = outbox(&h).iter().map(|queued| queued.key).collect();
        assert_eq!(left, kept[1..], "c may not have arrived, so it goes again");
        assert!(shown_agent(&h, "c").inbox.settled.is_some());

        h.fake.record().drops.clear();
        devbox_back(&h, &nl);
        outbox_empties(&h);
        assert_eq!(inbox_keys(&h, nl.id), kept[1..]);
    }

    #[test]
    fn the_outbox_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("maxhq-outbox-{}", LinkId::generate()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("maxhq.db");
        let (nl, kept) = {
            let registry = Registry::open(&path).unwrap();
            let nl = registry.add("devbox").unwrap();
            let h = harness(registry, None);
            report_keeping(&h, &nl, "nl", connected());
            h.report(nl.id, LinkEvent::Synced(devbox_agents(Activity::Busy)));
            report_keeping(&h, &nl, "nl", unreachable());
            set_inbox(&h, "b", InboxChange::Settle).unwrap();
            (nl, outbox(&h))
        };
        assert_eq!(kept.len(), 1);

        let h = harness(Registry::open(&path).unwrap(), None);
        assert_eq!(outbox(&h), kept);
        // The device's agents come back with what waits for them on top.
        report_keeping(&h, &nl, "nl", unreachable());
        h.report(nl.id, LinkEvent::Synced(devbox_agents(Activity::Busy)));
        assert!(shown_agent(&h, "b").inbox.settled.is_some());

        devbox_back(&h, &nl);
        outbox_empties(&h);
        assert_eq!(inbox_keys(&h, nl.id), [kept[0].key]);
        drop(h);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn removing_a_device_drops_what_waits_for_it() {
        let registry = Registry::in_memory().unwrap();
        let tailnet = registry.add("devbox").unwrap();
        let lan = registry.add("devbox.lan").unwrap();
        // Both routes are known to reach the device, as after you said so.
        for spec in [&tailnet, &lan] {
            registry
                .keep_device(spec.id, DeviceId::derive("nl"))
                .unwrap();
        }
        let h = harness(registry, None);
        for spec in [&tailnet, &lan] {
            report_keeping(&h, spec, "nl", connected());
            h.report(spec.id, LinkEvent::Synced(devbox_agents(Activity::Busy)));
            report_keeping(&h, spec, "nl", unreachable());
        }
        set_inbox(&h, "b", InboxChange::Settle).unwrap();
        let through = outbox(&h)[0].link;
        let other = if through == tailnet.id {
            lan.id
        } else {
            tailnet.id
        };

        // Another link still reaches the device, so the change stays.
        h.devices.remove_device(through).unwrap();
        let kept = outbox(&h);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].link, other);
        assert!(shown_agent(&h, "b").inbox.settled.is_some());

        h.devices.remove_device(other).unwrap();
        assert!(outbox(&h).is_empty());
        assert!(h.shown(EntityKey::Agent(AgentId::derive("b"))).is_none());
    }

    #[test]
    fn the_outbox_only_keeps_changes_for_a_device_that_keeps_inboxes() {
        let (h, _, nl) = two_devices();
        // devbox's maxd never said what it does.
        disconnect(&h, &nl, "nl");
        assert_eq!(
            set_inbox(&h, "b", InboxChange::Settle),
            Err(failed(
                "MaxHQ hasn't heard from devbox's maxd yet, so it can't keep the change for later. Try again once it connects".to_owned()
            ))
        );
        assert!(outbox(&h).is_empty());

        // One from before inboxes is refused before anything goes out.
        let old = Capabilities {
            inbox: false,
            copy: false,
            ..keeps_inbox()
        };
        connect_doing(&h, &nl, "nl", old);
        h.fake.calls();
        assert_eq!(
            set_inbox(&h, "b", InboxChange::Settle),
            Err(failed(
                "devbox's maxd is too old to settle, snooze or pin its agents. Update it first"
                    .to_owned()
            ))
        );
        assert!(h.fake.calls().is_empty());
        assert!(outbox(&h).is_empty());
    }

    /// Waits until `done` holds, for up to 5 seconds.
    pub(crate) fn wait_until(what: &str, done: impl Fn() -> bool) {
        let started = Instant::now();
        while !done() {
            assert!(started.elapsed() < Duration::from_secs(5), "{what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn a_close_never_reaches_the_link_before_the_open_it_takes_back() {
        let (h, _, nl) = two_devices();
        let b = AgentId::derive("b");
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        h.fake.record().gate = Some(gate.clone());

        std::thread::scope(|scope| {
            let open =
                scope.spawn(|| block_on(h.devices.open_timeline("main", Recorder::new(1), b)));
            wait_until("the open never reached the link", || {
                h.fake
                    .record()
                    .calls
                    .contains(&(nl.id, Call::Open(b, None, None)))
            });
            // The window lets go while the link is still opening.
            let close = scope.spawn(|| block_on(h.devices.close_timeline("main", b)));
            std::thread::sleep(Duration::from_millis(50));
            gate.add_permits(1);
            open.join().unwrap().unwrap();
            close.join().unwrap();
        });
        let open = h.fake.record().open.get(&(nl.id, b)).copied();
        assert_eq!(
            open, None,
            "no window watches the timeline, so it shouldn't stay open"
        );
    }

    /// The keys the links were given, by link, since the last call.
    #[cfg(feature = "desktop")]
    fn ssh_keys(h: &Harness) -> Vec<(LinkId, String)> {
        h.fake
            .calls()
            .into_iter()
            .filter_map(|(id, call)| match call {
                Call::Send(ClientMessage::SetGuestSshKey { public_key, .. }) => {
                    Some((id, public_key))
                }
                _ => None,
            })
            .collect()
    }

    /// Has `spec`'s link report `state`, from a `maxd` that takes editor
    /// sessions when `guest_ssh` says so.
    #[cfg(feature = "desktop")]
    fn report_ssh(h: &Harness, spec: &LinkSpec, state: LinkState, guest_ssh: bool) {
        let LinkEvent::Link(mut link) = link_state(spec, state) else {
            unreachable!("link_state reports a link");
        };
        link.capabilities = Some(Capabilities {
            guest_ssh,
            ..Capabilities::default()
        });
        h.report(spec.id, LinkEvent::Link(link));
    }

    /// A running container on the device `spec` reaches, with a host key.
    #[cfg(feature = "desktop")]
    fn running_container(spec: &LinkSpec) -> Entity {
        use maxhq_protocol::model::{
            EgressCheck, GuestState, ManagedWorkspace, Workspace, WorkspaceLocation, WorkspaceState,
        };
        Entity::Workspace(Workspace {
            id: WorkspaceId::derive("web"),
            project: None,
            device: DeviceId::derive(&spec.name),
            location: WorkspaceLocation::Container {
                instance: "web".into(),
                path: "/work".into(),
            },
            managed: Some(ManagedWorkspace {
                state: WorkspaceState::Running,
                start_at_boot: false,
                guest: GuestState::Installing,
                egress: EgressCheck::NotRun,
                recipe: 1,
                created_at: Timestamp(1),
                ssh_host_key: Some(
                    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcH"
                        .into(),
                ),
                unsaved: None,
                memory_soft: None,
            }),
            clone: None,
        })
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn a_device_that_takes_editor_sessions_gets_the_key_each_time_it_connects() {
        let registry = Registry::in_memory().unwrap();
        let nl = registry.add("dev@devbox").unwrap();
        let h = harness(registry, None);
        let dir = std::env::temp_dir().join(format!("maxhq-ssh-{}", LinkId::generate()));
        let (ssh, home) = (dir.join("ssh"), dir.join("home"));
        // Before the shell says where the files go, no key goes anywhere.
        h.fake.record().up.insert(nl.id);
        report_ssh(&h, &nl, connected(), true);
        report_ssh(&h, &nl, unreachable(), true);
        h.devices.keep_ssh(ssh.clone(), home.clone()).unwrap();
        assert!(ssh_keys(&h).is_empty());

        report_ssh(&h, &nl, connected(), true);
        wait_until("the key never went", || {
            h.fake
                .record()
                .calls
                .iter()
                .any(|(_, call)| matches!(call, Call::Send(ClientMessage::SetGuestSshKey { .. })))
        });
        let given = ssh_keys(&h);
        assert_eq!(given.len(), 1);
        assert_eq!(given[0].0, nl.id);
        assert!(given[0].1.starts_with("ssh-ed25519 "));

        // A device that doesn't take sessions gets no key.
        let local = h.local();
        h.fake.record().up.insert(local.id);
        report_ssh(&h, &local, connected(), false);
        std::thread::sleep(Duration::from_millis(50));
        assert!(ssh_keys(&h).is_empty());

        // The files follow the running containers the device reports.
        h.report(nl.id, LinkEvent::Synced(vec![running_container(&nl)]));
        let config = ssh.join("config");
        wait_until("the entry never came", || {
            std::fs::read_to_string(&config).is_ok_and(|text| text.contains("Host mhq-web."))
        });
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(text.contains(" dev@devbox 'sh -c "));
        assert!(
            !home.join(".ssh").exists(),
            "your own config waits for a yes"
        );

        // Once the device's link is gone, so is its entry.
        h.devices.remove_device(nl.id).unwrap();
        wait_until("the entry never went", || {
            std::fs::read_to_string(&config).is_ok_and(|text| !text.contains("Host "))
        });

        // Opened again, the files keep the same key.
        let again = SshFiles::open(ssh, home, None).unwrap();
        assert_eq!(again.public_key(), given[0].1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn a_device_already_connected_gets_the_key_once_the_files_are_kept() {
        let registry = Registry::in_memory().unwrap();
        let nl = registry.add("dev@devbox").unwrap();
        let h = harness(registry, None);
        h.fake.record().up.insert(nl.id);
        report_ssh(&h, &nl, connected(), true);
        let dir = std::env::temp_dir().join(format!("maxhq-ssh-{}", LinkId::generate()));
        h.devices
            .keep_ssh(dir.join("ssh"), dir.join("home"))
            .unwrap();
        wait_until("the key never went", || {
            h.fake
                .record()
                .calls
                .iter()
                .any(|(_, call)| matches!(call, Call::Send(ClientMessage::SetGuestSshKey { .. })))
        });
        assert_eq!(ssh_keys(&h).len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn asking_about_ssh_says_why_an_editor_cant_reach_a_workspace() {
        let registry = Registry::in_memory().unwrap();
        let nl = registry.add("dev@devbox").unwrap();
        let h = harness(registry, None);
        let web = WorkspaceId::derive("web");
        assert_eq!(
            block_on(h.devices.ssh_include(web, None)),
            Ok(SshIncludeState::Unavailable {
                reason: "This app doesn't open containers in an editor.".into()
            })
        );
        let dir = std::env::temp_dir().join(format!("maxhq-ssh-{}", LinkId::generate()));
        std::fs::create_dir_all(dir.join("home")).unwrap();
        h.devices
            .keep_ssh(dir.join("ssh"), dir.join("home"))
            .unwrap();
        assert_eq!(
            block_on(h.devices.ssh_include(web, None)),
            Err(gone("This workspace"))
        );
        report_ssh(&h, &nl, connected(), false);
        h.report(nl.id, LinkEvent::Synced(vec![running_container(&nl)]));
        assert!(matches!(
            block_on(h.devices.ssh_include(web, None)),
            Ok(SshIncludeState::Unavailable { reason }) if reason.contains("Update maxd")
        ));
        report_ssh(&h, &nl, connected(), true);
        let Ok(SshIncludeState::Ask { alias, .. }) = block_on(h.devices.ssh_include(web, None))
        else {
            panic!("the app should ask");
        };
        let config = std::fs::read_to_string(dir.join("ssh/config")).unwrap();
        assert!(
            config.contains(&format!("Host {alias}\n")),
            "the entry is there first"
        );
        assert_eq!(
            block_on(h.devices.ssh_include(web, Some(true))),
            Ok(SshIncludeState::Included { alias })
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_phone_takes_its_book_only_from_the_device_that_keeps_it() {
        use crate::phone::book_link;
        use crate::phone::tests::{book, entry};

        let h = harness(Registry::in_memory_phone().unwrap(), None);
        let first = book(vec![
            entry("devbox", &["devbox.local"]),
            entry("buildbox", &["buildbox.local"]),
        ]);
        h.devices.put_phone_book(&first).unwrap();
        let devbox = book_link(DeviceId::derive("devbox"));
        let buildbox = book_link(DeviceId::derive("buildbox"));
        let opened = h.fake.record().opened.clone();
        assert_eq!(opened.len(), 2);
        report_state(&h, &opened[0], "devbox", connected());
        report_state(&h, &opened[1], "buildbox", connected());

        // Each book moves devbox elsewhere and swaps its host key. They
        // differ in who they name as keeper and in which link brings them.
        for (link, keeper, name) in [
            (buildbox, "buildbox", "names the sender"),
            (buildbox, "devbox", "names the keeper"),
            (devbox, "buildbox", "right link, wrong keeper"),
        ] {
            let mut forged = first.clone();
            forged.device = DeviceId::derive(keeper);
            forged.devices[0] = entry("devbox", &["evil.example"]);
            forged.devices[0].name = name.into();
            forged.devices[0].host_key = "ssh-ed25519 ATTACKERKEY".into();
            let change = Change::Upsert {
                value: Entity::PhoneBook(forged),
            };
            h.report_quiet(link, LinkEvent::Changed(vec![change]));
        }
        assert_eq!(
            h.fake.record().opened.len(),
            2,
            "a forged book opened a link"
        );
        assert_eq!(
            h.devices.registry().phone_books().unwrap(),
            std::slice::from_ref(&first)
        );

        // The real keeper still gets through over its own link.
        let mut real = first.clone();
        real.devices[1].name = "buildbox at home".into();
        let change = Change::Upsert {
            value: Entity::PhoneBook(real.clone()),
        };
        h.report_quiet(devbox, LinkEvent::Changed(vec![change]));
        let start = Instant::now();
        while h.devices.registry().phone_books().unwrap() != [real.clone()] {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "the keeper's book never landed"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            h.fake.record().opened.len(),
            2,
            "a forged book opened a link"
        );
    }

    #[test]
    fn a_phone_follows_its_book() {
        use crate::phone::book_link;
        use crate::phone::tests::{book, entry};

        let h = harness(Registry::in_memory_phone().unwrap(), None);
        assert!(h.fake.record().opened.is_empty());
        let first = book(vec![
            entry("devbox", &["devbox.local"]),
            entry("buildbox", &["buildbox.local"]),
        ]);
        h.devices.put_phone_book(&first).unwrap();
        let devbox = book_link(DeviceId::derive("devbox"));
        let buildbox = book_link(DeviceId::derive("buildbox"));
        let opened: Vec<LinkId> = h.fake.record().opened.iter().map(|spec| spec.id).collect();
        assert_eq!(opened, [devbox, buildbox]);

        // A book for another phone changes nothing.
        let mut other = book(vec![entry("devbox", &["devbox.lan"])]);
        other.phone = maxhq_protocol::ids::PhoneId::derive("other");
        let opened = h.fake.record().opened[0].clone();
        report_state(&h, &opened, "devbox", connected());
        h.report(devbox, LinkEvent::Synced(vec![Entity::PhoneBook(other)]));

        // The paired device sends a newer one: buildbox left it and
        // devbox moved.
        let mut second = book(vec![entry("devbox", &["devbox.lan"])]);
        second.devices[0].name = "devbox at home".into();
        let change = Change::Upsert {
            value: Entity::PhoneBook(second.clone()),
        };
        h.report(devbox, LinkEvent::Changed(vec![change]));
        let start = Instant::now();
        while h.fake.record().opened.len() < 3 {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "devbox never reopened"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        let record = h.fake.record();
        assert_eq!(record.opened.len(), 3);
        assert_eq!(
            record.opened[2].route,
            Route::Ssh {
                host: "ssh://dev@devbox.lan:22".into()
            }
        );
        assert!(record.closed.contains(&devbox));
        assert!(record.closed.contains(&buildbox));
        drop(record);
        assert_eq!(h.devices.registry().phone_books().unwrap(), [second]);
    }

    /// Waits until the phone's books are `want`.
    fn books_become(h: &Harness, want: &[PhoneBook]) {
        let start = Instant::now();
        while h.devices.registry().phone_books().unwrap() != want {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "the books never became {want:?}"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn book_change(book: PhoneBook) -> LinkEvent {
        LinkEvent::Changed(vec![Change::Upsert {
            value: Entity::PhoneBook(book),
        }])
    }

    #[test]
    fn each_mac_speaks_only_for_its_own_book() {
        use crate::phone::book_link;
        use crate::phone::tests::{entry, mac_book};

        let h = harness(Registry::in_memory_phone().unwrap(), None);
        let home = mac_book("Home Mac", vec![entry("devbox", &["192.168.1.20"])]);
        let work = mac_book("Work Mac", vec![entry("build-box", &["192.168.1.20"])]);
        h.devices.put_phone_book(&home).unwrap();
        h.devices.put_phone_book(&work).unwrap();
        let link = |name: &str| book_link(DeviceId::derive(name));
        let opened = h.fake.record().opened.clone();
        let ids: Vec<LinkId> = opened.iter().map(|spec| spec.id).collect();
        assert_eq!(
            ids,
            [
                link("Home Mac"),
                link("devbox"),
                link("Work Mac"),
                link("build-box")
            ]
        );
        report_state(&h, &opened[0], "Home Mac", connected());
        report_state(&h, &opened[2], "Work Mac", connected());

        // Work Mac can't speak for Home Mac's book.
        let mut forged = home.clone();
        forged.devices[1] = entry("devbox", &["evil.example"]);
        h.report_quiet(link("Work Mac"), book_change(forged));
        assert_eq!(
            h.devices.registry().phone_books().unwrap(),
            [home.clone(), work.clone()]
        );
        assert_eq!(
            h.fake.record().opened.len(),
            4,
            "a forged book opened a link"
        );

        // Each Mac's newer book takes the place of its own alone.
        let mut newer_home = home.clone();
        newer_home.devices[1].name = "devbox at home".into();
        h.report_quiet(link("Home Mac"), book_change(newer_home.clone()));
        books_become(&h, &[newer_home.clone(), work.clone()]);
        let mut newer_work = work.clone();
        newer_work.devices.push(entry("ci-box", &["ci-box.local"]));
        h.report_quiet(link("Work Mac"), book_change(newer_work.clone()));
        books_become(&h, &[newer_home, newer_work]);
        let start = Instant::now();
        while !h
            .fake
            .record()
            .opened
            .iter()
            .any(|spec| spec.id == link("ci-box"))
        {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "Work Mac's new device never opened"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(h.fake.record().closed.is_empty(), "Home Mac's devices stay");
    }

    #[test]
    fn forgetting_a_mac_closes_only_the_devices_no_other_mac_lists() {
        use crate::phone::book_link;
        use crate::phone::tests::{entry, mac_book};

        let h = harness(Registry::in_memory_phone().unwrap(), None);
        let home = mac_book("Home Mac", vec![entry("devbox", &["192.168.1.20"])]);
        let work = mac_book(
            "Work Mac",
            vec![
                entry("build-box", &["192.168.1.20"]),
                entry("devbox", &["devbox.work"]),
            ],
        );
        h.devices.put_phone_book(&home).unwrap();
        h.devices.put_phone_book(&work).unwrap();
        let device = DeviceId::derive;
        let link = |name: &str| book_link(device(name));

        // Both Macs list devbox, and it comes once, under Home Mac.
        let ids: Vec<LinkId> = h.fake.record().opened.iter().map(|spec| spec.id).collect();
        assert_eq!(
            ids,
            [
                link("Home Mac"),
                link("devbox"),
                link("Work Mac"),
                link("build-box")
            ]
        );
        assert_eq!(
            h.fake.record().opened_to[&link("devbox")],
            Some(device("devbox"))
        );

        let gone = h.devices.forget_phone_book(device("Work Mac")).unwrap();
        assert_eq!(gone, [device("Work Mac"), device("build-box")]);
        let closed = h.fake.record().closed.clone();
        assert_eq!(closed, [link("Work Mac"), link("build-box")]);
        assert_eq!(
            h.devices.registry().phone_books().unwrap(),
            std::slice::from_ref(&home)
        );

        // Without Home Mac, devbox stays, on the route Work Mac gives it.
        h.devices.put_phone_book(&work).unwrap();
        let gone = h.devices.forget_phone_book(device("Home Mac")).unwrap();
        assert_eq!(gone, [device("Home Mac")]);
        let record = h.fake.record();
        assert!(record.closed.contains(&link("Home Mac")));
        let reopened = record
            .opened
            .iter()
            .rev()
            .find(|spec| spec.id == link("devbox"))
            .unwrap();
        assert_eq!(
            reopened.route,
            Route::Ssh {
                host: "ssh://dev@devbox.work:22".into()
            }
        );
        drop(record);
        assert_eq!(h.devices.registry().phone_books().unwrap(), [work]);
    }
}
