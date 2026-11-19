// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! How a client reaches each device. A client works this out for itself, so
//! `maxd` never sends these. The desktop app merges them into the state it
//! hands its webview.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::{AgentId, DeviceId, LinkId, NetworkId};
use crate::message::{Capabilities, ProtocolRange};
use crate::time::Timestamp;

/// A client's connection to one device's `maxd`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct DeviceLink {
    /// Assigned when you add the device to the client.
    pub id: LinkId,
    /// Your name for the device, or its SSH host until you rename it.
    pub name: String,
    /// The device's ID, once its `maxd` has answered a hello.
    pub device: Option<DeviceId>,
    /// How the client reaches it, or the route it tries first while down.
    pub route: Route,
    /// Every route you set for the device, in your order. Empty for a
    /// device with only `route`.
    #[serde(default)]
    pub routes: Vec<RouteChoice>,
    /// Where the connection stands now.
    pub state: LinkState,
    /// The network the route goes over, once the client has worked it
    /// out. Unset means a plain network path with nothing to watch.
    pub network: Option<NetworkId>,
    /// The overlay network's view of the device, when the device is a peer
    /// on one. It answers whether the device is up before SSH can.
    pub peer: Option<Peer>,
    /// The SSH hosts the route passes through first, nearest first, from a
    /// `ProxyJump` or the host a `ProxyCommand` dials. A device behind a
    /// host shares that host's problems.
    pub via: Vec<String>,
    /// What the device does, from its last hello. Unset until it has
    /// answered one, and kept while the link is down.
    pub capabilities: Option<Capabilities>,
    /// The newest protocol version the device's `maxd` speaks, from its
    /// last hello. Unset and kept like `capabilities`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<u32>,
    /// The protocol versions the device's `maxd` speaks, when its last
    /// hello found none in common with this client's. Kept while the link
    /// is down, and unset once a hello agrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_mismatch: Option<ProtocolRange>,
    /// The agents with inbox changes waiting in the app's outbox to go out
    /// through this link, in ID order. The client leaves it empty, and the
    /// app's view fills it in.
    pub waiting: Vec<AgentId>,
    /// The device this link's hello named while it waits for you to say
    /// whether that's really the device, because another link already
    /// knows it. `device` stays unset until you do, so nothing goes to the
    /// device through this link. The client leaves it unset, and the app's
    /// view fills it in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<LinkClaim>,
}

/// A device ID a link's hello named that the app hasn't let it speak for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct LinkClaim {
    /// The device the hello named.
    pub device: DeviceId,
    /// The link that already knows that device, when one does. Unset when
    /// only the host keys MaxHQ keeps under its name know it.
    pub known_by: Option<LinkId>,
    /// Whether you've answered.
    pub state: LinkClaimState,
    /// When the link first named it.
    pub since: Timestamp,
}

/// Where a [`LinkClaim`] stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkClaimState {
    /// It waits for your answer.
    Asking,
    /// You said it's a different device, so the link stays cut off until
    /// you ask again.
    Different,
}

/// One of a device's routes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct RouteChoice {
    /// The route.
    pub route: Route,
    /// Whether it last worked on the network this client is on now, so
    /// it's tried first.
    pub last_good_here: bool,
}

/// How a client reaches a device.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Route {
    /// `maxd` runs on the same machine, behind a Unix socket.
    Local,
    /// `ssh <host> maxd proxy`.
    Ssh {
        /// The SSH destination, as `~/.ssh/config` would take it, such as
        /// `dev@devbox`.
        host: String,
    },
}

/// An overlay network the client reaches devices through, such as a
/// tailnet. Never sent by `maxd`. The connections view groups devices under
/// it, and its state explains why every device on it is down at once.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Network {
    /// Derived from the kind and the network's own name.
    pub id: NetworkId,
    /// What kind of network it is.
    pub kind: NetworkKind,
    /// The name the group header shows, such as the tailnet's name.
    pub name: String,
    /// Whether this Mac is on it now.
    pub state: NetworkState,
}

/// The kinds of overlay network MaxHQ watches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NetworkKind {
    /// A Tailscale tailnet, watched through the local client's API.
    Tailscale {
        /// The suffix MagicDNS names end in, such as `tail1234.ts.net`.
        magic_dns_suffix: Option<String>,
        /// The local client's version, such as `1.102.3`.
        client_version: Option<String>,
    },
}

/// Whether this Mac is on a network. Decision N-2 calls this layer L1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NetworkState {
    /// Connected, so peers can be reached.
    Up,
    /// The client is connecting.
    Starting,
    /// You turned it off.
    Off,
    /// The client wants you to log in again.
    NeedsLogin,
    /// An admin has to approve this machine.
    NeedsApproval,
    /// The client isn't running, or MaxHQ can't talk to it.
    NotRunning,
    /// MaxHQ hasn't checked yet.
    Unknown,
}

/// What an overlay network says about one device. Decision N-2 calls this
/// layer L2.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Peer {
    /// The peer's host name on the network, such as `devbox`.
    pub host_name: String,
    /// The peer's full DNS name, such as `devbox.tail1234.ts.net`.
    pub dns_name: Option<String>,
    /// Whether the network's coordinator sees the peer online.
    pub online: bool,
    /// When the peer was last online. Unset while it's online.
    pub last_seen: Option<Timestamp>,
    /// How traffic to the peer travels now.
    pub path: PeerPath,
    /// When the peer's key expires and it drops off the network. Unset when
    /// expiry is off.
    pub key_expiry: Option<Timestamp>,
}

/// How traffic to a peer travels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PeerPath {
    /// Straight to the peer.
    Direct {
        /// The address in use, such as `192.168.1.54:41641`.
        address: String,
    },
    /// Through a relay, which works but adds latency.
    Relayed {
        /// The relay's region, such as `ams`.
        region: String,
    },
    /// No traffic lately, so there's no path to report.
    Idle,
}

/// Where a connection stands. Decision N-2 maps these to the connection dot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LinkState {
    /// `maxd` answered its hello.
    Connected {
        /// When the connection opened.
        since: Timestamp,
    },
    /// An attempt is in flight.
    Connecting {
        /// How far the attempt has got.
        step: ConnectStep,
        /// Counts from 1, and resets once connected.
        attempt: u32,
    },
    /// The last attempt failed. Background retries keep this state.
    Unreachable {
        /// Why, such as "Tailscale is off" or "auth failed".
        reason: String,
        /// When the device was last reachable, if ever.
        last_seen: Option<Timestamp>,
    },
    /// The client hasn't tried yet, or can't tell.
    Unknown,
}

/// How far a connection attempt has got.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConnectStep {
    /// Opening the transport.
    Dialing,
    /// Copying or upgrading `maxd` on the device.
    InstallingMaxd,
    /// Exchanging hellos.
    Handshake,
    /// The device is restarting and the client is waiting for it.
    WaitingForReboot,
    /// The device waits for you to let this app's key in from another
    /// device, and the connection stays up meanwhile.
    WaitingForApproval,
}
