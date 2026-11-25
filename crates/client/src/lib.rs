// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The client side of the protocol: connecting to `maxd`, the hello,
//! reconnecting with backoff and keeping a state replica current. The desktop
//! app uses it first, and a CLI can later.
//!
//! A [`Client`] watches the overlay networks this machine is on and opens
//! one link per device. Each link reports its own state, and the device's
//! state as full syncs followed by gap-free changes, so the owner never
//! deals with sequence numbers.
//!
//! A link also carries requests and their results. An action waits for its
//! result through reconnects, sent again under the same key each time, so
//! `maxd` carries it out once. A timeline opened on a link stays open across
//! reconnects and picks up after the last version it saw.
//!
//! Uploads and downloads go over the same link, behind requests, and live
//! through reconnects too. Each picks up where the device says it got to.
//!
//! So do port forwards, as [`forward`] tells: a port on this machine that
//! reaches one an agent listens on. A forward stays through reconnects but
//! not past the link.
//!
//! The desktop reaches devices with the system `ssh` (the `system-ssh`
//! feature) and watches Tailscale (the `tailscale` feature), both on by
//! default. The phone builds without them and with `russh`, whose
//! [`PhoneSsh`] carries its own key and each device's pinned host key,
//! through [`Client::app`]. With `ws`, [`PhoneWs`] reaches a device's
//! network access instead, over TLS pinned to its certificate.

#![warn(missing_docs)]
// A build with neither SSH reaches devices only over given streams.
#![cfg_attr(not(any(feature = "system-ssh", feature = "russh")), allow(dead_code))]

mod backoff;
pub mod forward;
mod link;
pub mod media;
pub mod netkey;
pub mod pinned;
pub mod proof;
mod requests;
#[cfg(feature = "system-ssh")]
pub mod setup;
mod ssh_config;
mod tailnet;
#[cfg(feature = "tailscale")]
// Only `Client::new`, with the system ssh, starts the watcher.
#[cfg_attr(not(feature = "system-ssh"), allow(dead_code))]
mod tailscale;
mod terminals;
mod transfer;
mod transport;

// The link tests stand maxd up behind Unix sockets.
#[cfg(all(test, unix))]
mod tests;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(feature = "system-ssh")]
use maxhq_core::paths;
use maxhq_protocol::agents::{ClientRole, PeerRequest};
use maxhq_protocol::attention::{AnswerOutcome, AttentionAnswer};
use maxhq_protocol::context::InlineRef;
use maxhq_protocol::frame;
use maxhq_protocol::ids::{
    AgentId, AttentionId, ClientId, DeviceId, IdempotencyKey, LinkId, PortId, StreamId, TerminalId,
    TransferId, WorkspaceId,
};
use maxhq_protocol::link::{DeviceLink, LinkState, Network, Route, RouteChoice};
use maxhq_protocol::message::{
    ActionOutcome, Capabilities, ClientInfo, UploadTarget, SECRET_INPUT_SINCE, SHOW_ANYWAY_SINCE,
    TERMINALS_SINCE,
};
use maxhq_protocol::model::ListeningPort;
use maxhq_protocol::skills::TreeHash;
use maxhq_protocol::state::{Change, Entity, Seq};
use maxhq_protocol::timeline::TimelineUpdate;
use maxhq_protocol::workflow::{RunKey, WorkflowUpdate};
use maxhq_protocol::ClientMessage;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, watch, Notify};
use tokio_util::sync::DropGuard;

use crate::forward::{ForwardError, Forwards, LocalForward};
use crate::link::{Pings, Shared, Timing};
use crate::media::Media;
use crate::pinned::PeerDial;
#[cfg(feature = "system-ssh")]
use crate::pinned::Pins;
use crate::requests::{Book, Reply};
use crate::tailnet::Tailnet;
use crate::terminals::Terminals;
use crate::transfer::{Item, Places, Transfers};
use crate::transport::Dialed;
#[cfg(feature = "system-ssh")]
use crate::transport::{Pinned, System};

pub use crate::ssh_config::is_safe_destination;
#[cfg(feature = "tailscale")]
pub use crate::tailscale::own_name as tailscale_name;
#[cfg(feature = "tailscale")]
pub use crate::tailscale::{
    cli_path as tailscale_cli, funnel_needs as tailscale_funnel_needs, run_cli as run_tailscale,
    CliRun as TailscaleRun, FunnelNeeds as TailscaleFunnelNeeds,
};
pub use crate::terminals::TerminalSink;
pub use crate::transfer::{Copied, Saved, TransferError, TransferProgress, TransferState};
#[cfg(feature = "russh")]
pub use crate::transport::PhoneSsh;
pub use crate::transport::{phone_route_host, ws_route_host};
#[cfg(feature = "ws")]
pub use crate::transport::{Ended, Headers, PhoneWs, Posted, WsRoute, SIGN_IN_ENDED};

/// How many events a link can hold before its task waits for the owner.
const EVENT_BUFFER: usize = 256;

/// A device to connect to, as you added it.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkSpec {
    /// Assigned when you added the device.
    pub id: LinkId,
    /// The name the app shows.
    pub name: String,
    /// How to reach it: the first route you set.
    pub route: Route,
    /// The routes to try after `route`, in your order.
    pub fallbacks: Vec<Route>,
    /// The route that last worked on each network, keyed by the network's
    /// key: the default gateway's on the desktop, or what the caller gave
    /// [`Client::set_network`]. It's tried first on that network.
    pub last_good: BTreeMap<String, Route>,
}

/// What a link reports, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkEvent {
    /// The link itself changed: its state, network, peer or hops. Carries
    /// the whole link, boxed because it's far larger than the other events.
    Link(Box<DeviceLink>),
    /// Everything the device holds now. It replaces whatever this link
    /// reported before, and comes after any connect that couldn't catch up
    /// from deltas, such as the first one or one after `maxd` restarted.
    Synced(Vec<Entity>),
    /// Changes since the last [`LinkEvent::Synced`], in order and with no
    /// gaps.
    Changed(Vec<Change>),
    /// A route reached the device on network `net`, so it goes first
    /// there next time. Only a device with several routes reports it.
    RouteWorked {
        /// The network's key, as in [`LinkSpec::last_good`].
        net: String,
        /// The route.
        route: Route,
    },
    /// An update to a timeline open on this link, in the order `maxd` sent
    /// it. None come for a timeline once its last open is closed.
    Timeline {
        /// The agent whose timeline it is.
        agent: AgentId,
        /// What changed.
        update: TimelineUpdate,
        /// Set for events that just happened on the device. Clear for a
        /// reset, a page, and the events that answer an open or a
        /// reconnect, which hold what came before.
        live: bool,
    },
    /// An update to a workflow run open on this link, in the order `maxd`
    /// sent it. None come for a run once its last open is closed.
    Workflow {
        /// The run.
        run: RunKey,
        /// What changed, boxed because a reset is large.
        update: Box<WorkflowUpdate>,
    },
    /// The device asked this side of a peer link for something. Answer it
    /// with [`ClientMessage::PeerAnswer`] under the same key. It may come
    /// again after a reconnect.
    PeerRequest {
        /// The key to answer under.
        key: IdempotencyKey,
        /// The request.
        request: PeerRequest,
    },
}

/// Why a request to a device didn't go out or got no result.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum LinkError {
    /// The link isn't connected now. It keeps retrying on its own.
    #[error("not connected")]
    NotConnected,
    /// The link was closed.
    #[error("link closed")]
    Closed,
    /// An action got no result in time, through any reconnects. It may
    /// still have been carried out.
    #[error("no result in time")]
    TimedOut,
    /// The message isn't an action, so it has no result to wait for.
    #[error("not an action")]
    NotAnAction,
    /// The device's `maxd` is older than the message, so it didn't go.
    #[error("maxd is too old for this")]
    TooOld,
}

/// How an App-role client from [`Client::app`] reaches devices.
pub enum Transport {
    /// The phone's own SSH: its key, and each device's routes with the
    /// host key pinned at pairing.
    #[cfg(feature = "russh")]
    Russh(PhoneSsh),
    /// The phone's network access: a WebSocket over TLS pinned to each
    /// device's certificate.
    #[cfg(feature = "ws")]
    Ws(PhoneWs),
    /// Both, in one link: a device's network routes and its SSH routes,
    /// with the stagger across all of them.
    #[cfg(all(feature = "russh", feature = "ws"))]
    Phone {
        /// For the `ssh://` routes.
        ssh: PhoneSsh,
        /// For the `wss://` routes.
        ws: PhoneWs,
    },
    /// Streams from `dial`, in place of SSH.
    Streams(Arc<dyn PeerDial>),
}

/// Watches networks and opens links. Create it inside a Tokio runtime,
/// since it starts tasks.
pub struct Client {
    networks: watch::Receiver<Vec<Network>>,
    /// What every link uses.
    shared: Arc<Shared>,
    /// Whether the app is in the background, for every link.
    life: watch::Sender<bool>,
    /// The network the caller named, for [`Client::app`].
    network: watch::Sender<Option<String>>,
    /// Asks every open connection to prove itself again.
    reprove: watch::Sender<()>,
    /// Stops the Tailscale watcher when the client drops.
    _watcher: Option<DropGuard>,
}

impl Client {
    /// Starts watching the overlay networks this machine is on.
    #[cfg(feature = "system-ssh")]
    pub fn new() -> Self {
        let transport = System::new(
            paths::socket(),
            paths::data_dir().map(|dir| dir.join("ssh")),
        );
        let transport = Arc::new(transport);
        #[cfg(feature = "tailscale")]
        {
            let (tailnet_tx, tailnet) = watch::channel(Arc::new(Tailnet::default()));
            let (networks_tx, networks) = watch::channel(Vec::new());
            let cancel = tokio_util::sync::CancellationToken::new();
            let publisher = tailscale::Publisher {
                tailnet: tailnet_tx,
                networks: networks_tx,
            };
            tokio::spawn(tailscale::run(publisher, cancel.clone()));
            let mut client = Self::with_parts(transport, tailnet, networks, Timing::DEFAULT);
            client._watcher = Some(cancel.drop_guard());
            client
        }
        #[cfg(not(feature = "tailscale"))]
        Self::without_tailnet(transport, Timing::DEFAULT)
    }

    /// An App-role client over `transport`, the phone's way in. It watches
    /// no tailnet, and names the network as [`Client::set_network`] last
    /// said.
    pub fn app(transport: Transport) -> Self {
        let transport: Arc<dyn transport::Transport> = match transport {
            #[cfg(feature = "russh")]
            Transport::Russh(ssh) => Arc::new(ssh),
            #[cfg(feature = "ws")]
            Transport::Ws(ws) => Arc::new(ws),
            #[cfg(all(feature = "russh", feature = "ws"))]
            Transport::Phone { ssh, ws } => Arc::new(transport::Both { ssh, ws }),
            Transport::Streams(dial) => Arc::new(Dialed(dial)),
        };
        let mut client = Self::without_tailnet(transport, Timing::DEFAULT);
        client.shared = Arc::new(Shared {
            netkey: netkey::given(client.network.subscribe()),
            ..(*client.shared).clone()
        });
        client
    }

    /// A client for `maxd`'s links to other devices. It dials only the
    /// pinned logins in `pins`, by the route host that names each, watches
    /// no tailnet and syncs nothing. An action waits out the longest agent
    /// wait, across reconnects.
    #[cfg(feature = "system-ssh")]
    pub fn peers(pins: Pins, control_dir: Option<PathBuf>) -> Self {
        Self::peer_parts(Arc::new(Pinned::new(pins, control_dir)))
    }

    /// [`Client::peers`] over streams from `dial`, in place of SSH.
    pub fn peers_over(dial: Arc<dyn PeerDial>) -> Self {
        Self::peer_parts(Arc::new(Dialed(dial)))
    }

    /// Closes every link's connection and stops its timers, until
    /// [`Client::wake`]. For an app going to the background, where the
    /// system would cut the connections anyway.
    pub fn suspend(&self) {
        self.life.send_replace(true);
    }

    /// Ends a [`Client::suspend`]: every link that isn't connected dials
    /// now, past any backoff wait, and every one that is checks its
    /// connection still answers. For an app coming back to the foreground.
    pub fn wake(&self) {
        self.life.send_replace(false);
    }

    /// Names the network this machine is on now, such as `wifi` or
    /// `cellular`, for a link with several routes to try the one that last
    /// worked there first. None means it can't tell. A new network wakes
    /// every link, as [`Client::wake`] does. Only a [`Client::app`] client
    /// reads it; the others read the default gateway.
    pub fn set_network(&self, key: Option<String>) {
        self.network.send_replace(key);
        self.wake();
    }

    /// Has links of `prover`'s kind prove themselves with it to each `maxd`
    /// that asks, right after its hello, in place of any prover of that
    /// kind before. A link opens only once its proof passes. Links opened
    /// before this prove nothing.
    pub fn proving(mut self, prover: Arc<dyn proof::Prover>) -> Self {
        let mut shared = (*self.shared).clone();
        shared.provers.retain(|given| given.kind() != prover.kind());
        shared.provers.push(prover);
        self.shared = Arc::new(shared);
        self
    }

    /// Has each open connection that proved itself prove itself again, such
    /// as after the app made a new key.
    pub fn reprove(&self) {
        self.reprove.send_replace(());
    }

    fn peer_parts(transport: Arc<dyn transport::Transport>) -> Self {
        let mut client = Self::without_tailnet(transport, Timing::PEER);
        client.shared = Arc::new(Shared {
            role: ClientRole::Peer,
            ..(*client.shared).clone()
        });
        client
    }

    /// A client that watches no tailnet.
    fn without_tailnet(transport: Arc<dyn transport::Transport>, timing: Timing) -> Self {
        let (_, tailnet) = watch::channel(Arc::new(Tailnet::default()));
        let (_, networks) = watch::channel(Vec::new());
        Self::with_parts(transport, tailnet, networks, timing)
    }

    /// Has downloads land in `folder` in place of `~/Downloads/MaxHQ`.
    /// Links opened before this keep the folder they had.
    pub fn downloading_into(self, folder: PathBuf) -> Self {
        self.placing(|places| places.downloads = folder)
    }

    /// Copies uploads into `folder` before they go, and lands copies fetched
    /// to hand on there, in place of the user's cache folder. Links opened
    /// before this keep the folder they had.
    pub fn staging_in(self, folder: PathBuf) -> Self {
        self.placing(|places| places.staging = folder)
    }

    /// Keeps pictures and videos for the timeline in `folder` in place of
    /// the user's cache folder. Links opened before this keep the folder
    /// they had.
    pub fn caching_media_in(self, folder: PathBuf) -> Self {
        self.placing(|places| places.media = folder)
    }

    /// The tailnet's peers by the names ssh can reach them at, their
    /// MagicDNS names where they have one, for setup to offer.
    pub fn tailnet_hosts(&self) -> Vec<String> {
        let tailnet = self.shared.tailnet.borrow().clone();
        let mut hosts: Vec<String> = tailnet
            .peers
            .iter()
            .map(|peer| match &peer.peer.dns_name {
                Some(name) if !name.is_empty() => name.trim_end_matches('.').to_owned(),
                _ => peer.peer.host_name.clone(),
            })
            .collect();
        hosts.sort();
        hosts.dedup();
        hosts
    }

    /// Runs one fixed command on an SSH host over the master the links
    /// share, logging in first if there's none, and waits for it. Never
    /// build `command` from anything a user typed: the host's login shell
    /// reads it.
    #[cfg(feature = "system-ssh")]
    pub async fn run_over_ssh(&self, host: &str, command: &str) -> std::io::Result<setup::Output> {
        let transport = Arc::clone(&self.shared.transport);
        transport.run(host, command).await
    }

    fn placing(mut self, change: impl FnOnce(&mut Places)) -> Self {
        let mut shared = (*self.shared).clone();
        change(&mut shared.places);
        self.shared = Arc::new(shared);
        self
    }

    /// Has links name the network with `netkey` in place of reading the
    /// default gateway, which is how tests pick the network.
    #[cfg(all(test, unix))]
    fn on_network(mut self, key: Option<&str>) -> Self {
        let key = key.map(str::to_owned);
        self.shared = Arc::new(Shared {
            netkey: Arc::new(move || {
                let key = key.clone();
                Box::pin(async move { key })
            }),
            ..(*self.shared).clone()
        });
        self
    }

    /// The same client with links on `timing`, so tests needn't wait.
    #[cfg(all(test, any(feature = "russh", feature = "ws")))]
    fn timed(mut self, timing: Timing) -> Self {
        self.shared = Arc::new(Shared {
            timing,
            ..(*self.shared).clone()
        });
        self
    }

    /// A client over any transport and tailnet view, which is how tests
    /// fake both.
    fn with_parts(
        transport: Arc<dyn transport::Transport>,
        tailnet: watch::Receiver<Arc<Tailnet>>,
        networks: watch::Receiver<Vec<Network>>,
        timing: Timing,
    ) -> Self {
        let (life, suspended) = watch::channel(false);
        let (network, given) = watch::channel(None);
        let (reprove, reproving) = watch::channel(());
        #[cfg(feature = "system-ssh")]
        let netkey: link::NetKey = {
            drop(given);
            Arc::new(|| Box::pin(netkey::current()))
        };
        #[cfg(not(feature = "system-ssh"))]
        let netkey = netkey::given(given);
        let shared = Shared {
            transport,
            tailnet,
            client: ClientInfo {
                name: env!("CARGO_PKG_NAME").to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            client_id: ClientId::generate(),
            timing,
            places: Places::new(),
            netkey,
            role: ClientRole::App,
            life: suspended,
            provers: Vec::new(),
            reprove: reproving,
        };
        Self {
            networks,
            shared: Arc::new(shared),
            life,
            network,
            reprove,
            _watcher: None,
        }
    }

    /// The networks this machine is on, kept current.
    pub fn networks(&self) -> watch::Receiver<Vec<Network>> {
        self.networks.clone()
    }

    /// Opens a link to a device. It connects, reconnects with backoff and
    /// reports through the returned receiver until the handle drops.
    pub fn link(&self, spec: LinkSpec) -> (LinkHandle, mpsc::Receiver<LinkEvent>) {
        self.open(spec, None)
    }

    /// Opens a link to `device` only: a hello from any other device is
    /// refused, on every route.
    pub fn link_to(
        &self,
        spec: LinkSpec,
        device: DeviceId,
    ) -> (LinkHandle, mpsc::Receiver<LinkEvent>) {
        self.open(spec, Some(device))
    }

    fn open(
        &self,
        spec: LinkSpec,
        device: Option<DeviceId>,
    ) -> (LinkHandle, mpsc::Receiver<LinkEvent>) {
        let (events_tx, events) = mpsc::channel(EVENT_BUFFER);
        let link = DeviceLink {
            routes: choices(&spec),
            id: spec.id,
            name: spec.name.clone(),
            device,
            route: spec.route.clone(),
            state: LinkState::Unknown,
            network: None,
            peer: None,
            via: Vec::new(),
            capabilities: None,
            protocol: None,
            protocol_mismatch: None,
            waiting: Vec::new(),
            claim: None,
        };
        // The channel is empty, so this can't fail.
        let _ = events_tx.try_send(LinkEvent::Link(Box::new(link.clone())));
        let routes = link::Routes {
            yours: std::iter::once(spec.route.clone())
                .chain(spec.fallbacks.iter().cloned())
                .collect(),
            last_good: spec.last_good.clone(),
        };
        let control = link::spawn(Arc::clone(&self.shared), link, routes, events_tx);
        let handle = LinkHandle {
            id: spec.id,
            outbox: control.outbox,
            wire: control.wire,
            refresh: control.refresh,
            book: control.book,
            capabilities: control.capabilities,
            protocol: control.protocol,
            transfers: control.transfers,
            pings: control.pings,
            forwards: control.forwards,
            terminals: control.terminals,
            reply: self.shared.timing.reply,
            _task: control.guard,
        };
        (handle, events)
    }
}

/// The routes a link shows: none for a device with one, and each of yours
/// in order otherwise.
fn choices(spec: &LinkSpec) -> Vec<RouteChoice> {
    if spec.fallbacks.is_empty() {
        return Vec::new();
    }
    std::iter::once(&spec.route)
        .chain(&spec.fallbacks)
        .map(|route| RouteChoice {
            route: route.clone(),
            last_good_here: false,
        })
        .collect()
}

#[cfg(feature = "system-ssh")]
impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

/// Controls one link. Dropping it closes the link.
pub struct LinkHandle {
    id: LinkId,
    /// Where requests go while the link is connected.
    outbox: watch::Receiver<Option<mpsc::Sender<ClientMessage>>>,
    /// The connection's writer while connected, for frames that aren't
    /// messages, such as what you type into a terminal.
    wire: watch::Receiver<Option<maxhq_wire::Sender>>,
    /// Wakes the link's task for a probe.
    refresh: Arc<Notify>,
    /// What the link waits on, shared with its task.
    book: Arc<Book>,
    /// What the device said it does at the last hello.
    capabilities: watch::Receiver<Capabilities>,
    /// The protocol version the device's `maxd` said it speaks at the last
    /// hello.
    protocol: watch::Receiver<Option<u32>>,
    /// The uploads and downloads in flight, shared with the link's task.
    transfers: Arc<Transfers>,
    /// Pings that wait for their pong, shared with the link's task.
    pings: Arc<Pings>,
    /// The link's port forwards, shared with its task.
    forwards: Arc<Forwards>,
    /// The terminals the link attached, shared with its task.
    terminals: Arc<Terminals>,
    /// How long an action waits for its result.
    reply: Duration,
    /// Stops the link's task when the handle drops.
    _task: DropGuard,
}

impl LinkHandle {
    /// The link's ID.
    pub fn id(&self) -> LinkId {
        self.id
    }

    /// What the device said it does at the last hello: transfers, tunnels
    /// and workspaces. Before the first hello, and for an M2 `maxd`, it's
    /// none of them. It stays as it was while the link is down.
    pub fn capabilities(&self) -> Capabilities {
        *self.capabilities.borrow()
    }

    /// Sends a request to the device, such as `MarkSeen`, without waiting
    /// for a result. Hellos, syncs, opening, paging or closing a timeline,
    /// opening or closing a workflow run, transfer messages, closing a forward and tunnel messages are the
    /// link's own business and are refused.
    pub async fn send(&self, message: ClientMessage) -> Result<(), LinkError> {
        if matches!(
            message,
            ClientMessage::Hello(_)
                | ClientMessage::Sync { .. }
                | ClientMessage::Peer { .. }
                | ClientMessage::OpenTimeline { .. }
                | ClientMessage::LoadOlder { .. }
                | ClientMessage::CloseTimeline { .. }
                | ClientMessage::OpenWorkflow { .. }
                | ClientMessage::CloseWorkflow { .. }
                | ClientMessage::UploadBegin { .. }
                | ClientMessage::FetchBegin { .. }
                | ClientMessage::FetchSkillTree { .. }
                | ClientMessage::UploadCancel { .. }
                | ClientMessage::TransferCredit { .. }
                | ClientMessage::CloseForward { .. }
                | ClientMessage::TunnelOpen { .. }
                | ClientMessage::TunnelCredit { .. }
                | ClientMessage::TunnelClose { .. }
        ) {
            // LinkError has no variant for a refused request, and this one
            // at least doesn't tell the owner to drop the link.
            return Err(LinkError::NotConnected);
        }
        self.post(message).await
    }

    /// Sends an action, such as `SendInput`, and waits for its result. Give
    /// each action a fresh key when you create it. If the link drops before
    /// the result comes, the action goes again under the same key after
    /// each reconnect, until a result comes or the reply timeout passes. A
    /// link that isn't connected when you call refuses the action at once,
    /// so you can try another. `OpenForward` is refused like
    /// [`LinkHandle::send`] refuses the link's own messages, since a forward
    /// needs its local port: use [`LinkHandle::open_forward`].
    pub async fn act(&self, action: ClientMessage) -> Result<ActionOutcome, LinkError> {
        if matches!(action, ClientMessage::OpenForward { .. }) {
            return Err(LinkError::NotConnected);
        }
        let key = action.action_key().ok_or(LinkError::NotAnAction)?;
        self.request(key, action, Reply::Action).await
    }

    /// Answers an attention event and waits for what came of it, retried
    /// like an action. Give each answer a fresh key. A `maxd` too old for
    /// the pills in `refs` gets none and fails with [`LinkError::TooOld`].
    pub async fn answer(
        &self,
        key: IdempotencyKey,
        event: AttentionId,
        answer: AttentionAnswer,
        refs: Vec<InlineRef>,
    ) -> Result<AnswerOutcome, LinkError> {
        let message = ClientMessage::Answer {
            idempotency_key: key,
            event,
            answer,
            refs,
        };
        self.request(key, message, Reply::Answer).await
    }

    /// Opens an agent's timeline, which then comes as
    /// [`LinkEvent::Timeline`] updates. With no `since`, they start with a
    /// reset to the newest `limit` items. With `since`, a version you hold
    /// already, perhaps from another link to the same device, they start
    /// with the events after it, or with a reset if the device can't go
    /// back that far. Opening one that's open already asks again from
    /// `since`. Each open needs its own close. A link that isn't connected
    /// opens it once it connects. `lead` names the lead a sub-agent runs
    /// under, so the device can still read it once it finished.
    pub async fn open_timeline(
        &self,
        agent: AgentId,
        since: Option<Seq>,
        limit: u32,
        lead: Option<AgentId>,
    ) -> Result<(), LinkError> {
        self.book.lock().open(agent, since, limit, lead)?;
        let open = ClientMessage::OpenTimeline {
            agent,
            since,
            limit,
            lead,
        };
        match self.post(open).await {
            Err(LinkError::Closed) => Err(LinkError::Closed),
            // The next connect opens it.
            _ => Ok(()),
        }
    }

    /// Asks an open timeline for up to `limit` items before the ordinal
    /// `before`, which come as an `Older` update. Each ask that returns Ok
    /// gets one such update while the timeline stays open, in the order
    /// asked, even if the link drops and reconnects in between. A link that
    /// isn't connected refuses at once. A timeline that isn't open gets
    /// nothing.
    pub async fn load_older(
        &self,
        agent: AgentId,
        before: u64,
        limit: u32,
    ) -> Result<(), LinkError> {
        // A new connection opens only with this held, so the queue below
        // belongs to the connection the book counts.
        let _pages = self.book.pages().await;
        let queue = self.outbox.borrow().clone();
        let Some(queue) = queue else {
            return Err(self.not_connected());
        };
        let permit = queue.reserve().await.map_err(|_| LinkError::NotConnected)?;
        // The page is noted and queued together, so the book holds the
        // pages of a connection in the order they went out.
        let mut book = self.book.lock();
        if book.ask_older(agent, before, limit)? {
            permit.send(ClientMessage::LoadOlder {
                agent,
                before,
                limit,
            });
        }
        Ok(())
    }

    /// Closes one open of an agent's timeline. The last close tells the
    /// device to stop sending updates.
    pub async fn close_timeline(&self, agent: AgentId) -> Result<(), LinkError> {
        if !self.book.lock().close(agent) {
            return Ok(());
        }
        match self.post(ClientMessage::CloseTimeline { agent }).await {
            // A new connection opens only what's still open.
            Err(LinkError::NotConnected) => Ok(()),
            result => result,
        }
    }

    /// Opens a workflow run, which then comes as [`LinkEvent::Workflow`]
    /// updates, starting with a reset. Opening one that's open already
    /// asks for a fresh reset. Each open needs its own close. A link that
    /// isn't connected opens it once it connects, and every reconnect
    /// starts it over with a reset.
    pub async fn open_workflow(&self, run: RunKey) -> Result<(), LinkError> {
        self.book.lock().open_run(run.clone())?;
        match self.post(ClientMessage::OpenWorkflow { run }).await {
            Err(LinkError::Closed) => Err(LinkError::Closed),
            // The next connect opens it.
            _ => Ok(()),
        }
    }

    /// Closes one open of a workflow run. The last close tells the device
    /// to stop sending updates.
    pub async fn close_workflow(&self, run: RunKey) -> Result<(), LinkError> {
        if !self.book.lock().close_run(&run) {
            return Ok(());
        }
        match self.post(ClientMessage::CloseWorkflow { run }).await {
            // A new connection opens only what's still open.
            Err(LinkError::NotConnected) => Ok(()),
            result => result,
        }
    }

    /// Probes the device now instead of at the next backoff step. A
    /// connected link pings and drops the connection if no answer comes
    /// within 3 seconds. One that isn't connected dials at once, even past
    /// a peer Tailscale shows offline. Either way the backoff starts over.
    pub fn refresh(&self) {
        self.refresh.notify_one();
    }

    /// Sends files and folders on this machine to the device and gives
    /// what came of each, in order. Each goes as a transfer of its own,
    /// one after another, copied first so they can keep editing the
    /// original. A transfer goes on through reconnects and picks up where
    /// it got to, with no reply timeout, since a large one takes a while.
    /// A link that isn't connected when one starts refuses it at once.
    pub async fn upload(
        &self,
        target: UploadTarget,
        paths: Vec<PathBuf>,
        on_progress: impl Fn(TransferProgress) + Send + Sync,
    ) -> Vec<Result<ActionOutcome, TransferError>> {
        let mut results = Vec::with_capacity(paths.len());
        for path in paths {
            let item = Item::Path(path);
            results.push(
                self.transfers
                    .upload(target.clone(), item, &on_progress)
                    .await,
            );
        }
        results
    }

    /// Sends one file's bytes to the device, such as a pasted picture, and
    /// gives what came of it. A name with no extension gets one from
    /// `mime`. It goes like one of [`LinkHandle::upload`]'s.
    pub async fn upload_bytes(
        &self,
        target: UploadTarget,
        name: String,
        mime: &str,
        bytes: Vec<u8>,
        on_progress: impl Fn(TransferProgress) + Send + Sync,
    ) -> Result<ActionOutcome, TransferError> {
        let item = Item::Bytes {
            name,
            mime: mime.to_owned(),
            bytes,
        };
        self.transfers.upload(target, item, &on_progress).await
    }

    /// Fetches a file or folder an agent can see into `~/Downloads/MaxHQ`
    /// and gives where it landed, with the links in it this machine
    /// couldn't make. It takes a free name there, and macOS and Windows
    /// ask before they open what came. It goes on through reconnects like
    /// an upload.
    pub async fn download(
        &self,
        agent: AgentId,
        path: String,
        on_progress: impl Fn(TransferProgress) + Send + Sync,
    ) -> Result<Saved, TransferError> {
        self.transfers.download(agent, path, &on_progress).await
    }

    /// Fetches a file or folder an agent can see into a staging folder of
    /// the client's own, to hand on to a device, and gives the copy. The
    /// folder goes when the copy drops.
    pub async fn copy(
        &self,
        agent: AgentId,
        path: String,
        on_progress: impl Fn(TransferProgress) + Send + Sync,
    ) -> Result<Copied, TransferError> {
        self.transfers.copy(agent, path, &on_progress).await
    }

    /// Fetches some files in a workspace's checkout, by their paths there,
    /// into a staging folder of the client's own, to copy to another
    /// device. They come as one folder that holds them at those paths,
    /// which goes when the copy drops.
    pub async fn copy_files(
        &self,
        workspace: WorkspaceId,
        paths: Vec<String>,
        on_progress: impl Fn(TransferProgress) + Send + Sync,
    ) -> Result<Copied, TransferError> {
        self.transfers
            .copy_files(workspace, paths, &on_progress)
            .await
    }

    /// Fetches a tree in the device's skill store into a staging folder of
    /// the client's own, to copy to another device's store. The folder
    /// goes when the copy drops.
    pub async fn copy_tree(
        &self,
        tree: TreeHash,
        on_progress: impl Fn(TransferProgress) + Send + Sync,
    ) -> Result<Copied, TransferError> {
        self.transfers.copy_tree(tree, &on_progress).await
    }

    /// Fetches a picture or video an agent can see into the media cache,
    /// as [`media`] tells, unless it's there already or is larger than
    /// `limit` bytes. A limit of 0 only asks how large it is. `anyway`
    /// asks for one file outside the agent's folders, from your click on
    /// Show anyway, and needs a `maxd` that's known to read it.
    pub async fn media(
        &self,
        agent: AgentId,
        path: String,
        limit: u64,
        anyway: bool,
        on_progress: impl Fn(TransferProgress) + Send + Sync,
    ) -> Result<Media, TransferError> {
        let known = (*self.protocol.borrow()).is_some_and(|version| version >= SHOW_ANYWAY_SINCE);
        if anyway && !known {
            return Err(LinkError::TooOld.into());
        }
        self.transfers
            .media(agent, path, limit, anyway, &on_progress)
            .await
    }

    /// Cancels an upload or download on this link, which then ends with
    /// [`TransferError::Cancelled`]. One that ended already is left be.
    pub fn cancel_transfer(&self, transfer: TransferId) {
        self.transfers.cancel(transfer);
    }

    /// Opens a forward to a port an agent listens on and gives the port on
    /// this machine that reaches it, on 127.0.0.1 and ::1. That's the same
    /// number when it's free and browsers load it, or the next one that is.
    /// A forward open already gives its port. It stays through reconnects,
    /// and closes when you close it, when the port or its agent goes from
    /// the device's state, or when the link closes. A link that isn't
    /// connected, or a device whose `maxd` doesn't forward ports, refuses
    /// at once.
    pub async fn open_forward(&self, port: &ListeningPort) -> Result<u16, ForwardError> {
        self.forwards.open(port).await
    }

    /// Closes the forward to a port, if one is open.
    pub fn close_forward(&self, port: PortId) {
        self.forwards.close(port);
    }

    /// Pumps one end of an in-memory pipe as `stream`, which `maxd` opens
    /// by a request of its own: see [`Forwards::attach`].
    pub fn attach_stream(
        &self,
        stream: StreamId,
        socket: tokio::io::DuplexStream,
    ) -> Result<(), String> {
        self.forwards.attach(stream, socket)
    }

    /// Drops a stream [`LinkHandle::attach_stream`] took.
    pub fn forget_stream(&self, stream: StreamId) {
        self.forwards.forget(stream);
    }

    /// The forwards open on this link, by local port. Each shows `maxd`'s
    /// forward on the current connection once it has answered.
    pub fn forwards(&self) -> Vec<LocalForward> {
        self.forwards.list()
    }

    /// Pings the device and gives how long the pong took. A ping goes
    /// ahead of any file data, so this shows how quick requests are during
    /// a transfer.
    pub async fn ping(&self) -> Result<Duration, LinkError> {
        let (nonce, pong) = self.pings.start();
        let _waiting = Pinging {
            pings: &self.pings,
            nonce,
        };
        let started = Instant::now();
        self.post(ClientMessage::Ping { nonce }).await?;
        match tokio::time::timeout(self.reply, pong).await {
            Ok(Ok(())) => Ok(started.elapsed()),
            // The connection dropped before the pong came.
            Ok(Err(_)) => Err(LinkError::NotConnected),
            Err(_) => Err(LinkError::TimedOut),
        }
    }

    /// Sends a request and waits for its result.
    async fn request<T>(
        &self,
        key: IdempotencyKey,
        message: ClientMessage,
        reply: fn(oneshot::Sender<T>) -> Reply,
    ) -> Result<T, LinkError> {
        let (sender, result) = oneshot::channel();
        self.book.lock().wait(key, message.clone(), reply(sender))?;
        // Stops waiting however this ends, including when the caller drops
        // the future.
        let _waiting = Waiting {
            book: &self.book,
            key,
        };
        self.post(message).await?;
        match tokio::time::timeout(self.reply, result).await {
            Ok(Ok(outcome)) => Ok(outcome),
            Ok(Err(_)) => Err(LinkError::Closed),
            Err(_) => Err(LinkError::TimedOut),
        }
    }

    /// Attaches a terminal, whose output then goes to `sink`, from the byte
    /// after `after`, the last one the caller has, or from the start of
    /// what `maxd` holds. Attaching one that's attached already starts its
    /// output over there. A link that isn't connected attaches it once it
    /// connects, and a reconnect goes on from where the output got to.
    pub async fn attach_terminal(
        &self,
        terminal: TerminalId,
        after: Option<u64>,
        sink: Arc<dyn TerminalSink>,
    ) -> Result<(), LinkError> {
        let attach = ClientMessage::TerminalAttach { terminal, after };
        if !attach.reaches(*self.protocol.borrow()) {
            return Err(LinkError::TooOld);
        }
        self.terminals.attach(terminal, after, sink);
        match self.post(attach).await {
            Err(LinkError::Closed) => Err(LinkError::Closed),
            // The next connect attaches it.
            _ => Ok(()),
        }
    }

    /// Detaches a terminal, so its output stops coming. One that isn't
    /// attached is left alone.
    pub async fn detach_terminal(&self, terminal: TerminalId) -> Result<(), LinkError> {
        if !self.terminals.detach(terminal) {
            return Ok(());
        }
        match self.post(ClientMessage::TerminalDetach { terminal }).await {
            // A new connection attaches only what's still attached.
            Err(LinkError::NotConnected | LinkError::TooOld) => Ok(()),
            result => result,
        }
    }

    /// Writes what you typed to a terminal, ahead of file data. Nothing
    /// keeps it, so with no connection it doesn't go at all. A `secret`
    /// line, from a secret field, goes as such to a `maxd` that reads it,
    /// so a record of it says only that a secret went, and as plain input
    /// to an older one.
    pub async fn terminal_input(
        &self,
        terminal: TerminalId,
        bytes: &[u8],
        secret: bool,
    ) -> Result<(), LinkError> {
        let version = *self.protocol.borrow();
        if version.is_some_and(|version| version < TERMINALS_SINCE) {
            return Err(LinkError::TooOld);
        }
        let secret = secret && version.is_some_and(|version| version >= SECRET_INPUT_SINCE);
        let wire = self.wire.borrow().clone();
        let Some(wire) = wire else {
            return Err(self.not_connected());
        };
        let mut frames = Vec::new();
        frame::encode_terminal_input_as(&mut frames, terminal, bytes, secret);
        if frames.is_empty() {
            return Ok(());
        }
        wire.send(maxhq_wire::Queue::Control, frames)
            .await
            .map_err(|_| LinkError::NotConnected)
    }

    /// Queues a message for the connection, if there is one and its
    /// `maxd` reads the message.
    async fn post(&self, message: ClientMessage) -> Result<(), LinkError> {
        if !message.reaches(*self.protocol.borrow()) {
            return Err(LinkError::TooOld);
        }
        let queue = self.outbox.borrow().clone();
        let Some(queue) = queue else {
            return Err(self.not_connected());
        };
        queue
            .send(message)
            .await
            .map_err(|_| LinkError::NotConnected)
    }

    /// Why a request can't go out with no connection: the link is down, or
    /// its task stopped, which drops its end of the outbox.
    fn not_connected(&self) -> LinkError {
        if self.outbox.has_changed().is_err() {
            LinkError::Closed
        } else {
            LinkError::NotConnected
        }
    }
}

/// Takes a request out of the book when its caller stops waiting.
struct Waiting<'a> {
    book: &'a Book,
    key: IdempotencyKey,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.book.lock().forget(self.key);
    }
}

/// Stops waiting for a pong when its caller stops waiting.
struct Pinging<'a> {
    pings: &'a Pings,
    nonce: u32,
}

impl Drop for Pinging<'_> {
    fn drop(&mut self) {
        self.pings.forget(self.nonce);
    }
}
