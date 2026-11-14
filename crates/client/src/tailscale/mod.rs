// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Watches Tailscale through its LocalAPI: the IPN bus for state and peer
//! changes as they happen, and a status poll now and then for what the bus
//! doesn't patch, such as each peer's path. The tailnet's name never goes
//! to a log, since a personal tailnet is named after an email address.

mod api;
mod wire;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::BodyExt;
use maxhq_protocol::ids::NetworkId;
use maxhq_protocol::link::{Network, NetworkKind, NetworkState, Peer, PeerPath};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::backoff::{Backoff, BackoffLimits};
use crate::tailnet::{Tailnet, TailnetPeer};
use wire::{parse_time, Node, Notify, PeerChange, PeerStatus, Status};

/// Retries for the watcher itself. Tailscale is local, so it retries
/// sooner than a device would.
const WATCH_LIMITS: BackoffLimits = BackoffLimits {
    base: Duration::from_secs(1),
    cap: Duration::from_secs(30),
    long_cap: Duration::from_secs(30),
    long_after: Duration::MAX,
};

/// How often the status is fetched while the bus is open.
const STATUS_POLL: Duration = Duration::from_secs(30);

/// How often the CLI is run when the LocalAPI can't be reached.
const CLI_POLL: Duration = Duration::from_secs(10);

/// Where the watcher publishes.
pub(crate) struct Publisher {
    /// The view links read.
    pub tailnet: watch::Sender<Arc<Tailnet>>,
    /// What [`crate::Client::networks`] hands out.
    pub networks: watch::Sender<Vec<Network>>,
}

impl Publisher {
    /// Sends the model's view to whichever watch it changes.
    fn publish(&self, model: &Model) {
        let view = model.view();
        let networks: Vec<Network> = view.network.iter().cloned().collect();
        self.networks.send_if_modified(|current| {
            let changed = *current != networks;
            if changed {
                *current = networks;
            }
            changed
        });
        self.tailnet.send_if_modified(|current| {
            let changed = **current != view;
            if changed {
                *current = Arc::new(view);
            }
            changed
        });
    }
}

/// How one round of watching went.
enum Outcome {
    /// The bus delivered at least one message before it closed.
    Watched,
    /// Only the CLI answered.
    Polled,
    /// Nothing answered.
    Failed,
}

/// This machine's MagicDNS name, such as `devbox.example.ts.net`, from the
/// LocalAPI or else the CLI. `None` when Tailscale doesn't answer or has
/// no name for it.
pub async fn own_name() -> Option<String> {
    let status = match api::discover().await {
        Some(endpoint) => api::status(&endpoint).await.ok(),
        None => None,
    };
    let status = match status {
        Some(status) => status,
        None => api::cli_status().await?,
    };
    own_dns_name(&status)
}

/// What a run of the Tailscale CLI gave.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CliRun {
    /// Whether it exited zero.
    pub ok: bool,
    /// What it printed.
    pub stdout: Vec<u8>,
    /// What it printed as errors.
    pub stderr: String,
}

/// The first Tailscale CLI on this machine, for a caller outside a runtime.
pub fn cli_path() -> Option<&'static str> {
    api::find_cli_now()
}

/// Runs `tailscale <args>` with the first CLI on this machine, for at most
/// `timeout`. An error says, in words for a person, why it didn't run.
pub async fn run_cli(args: &[&str], timeout: Duration) -> Result<CliRun, String> {
    let cli = api::find_cli()
        .await
        .ok_or_else(|| "Tailscale isn't installed on this device.".to_owned())?;
    let output = api::run_cli(cli, args, timeout)
        .await
        .ok_or_else(|| "The Tailscale CLI didn't answer.".to_owned())?;
    Ok(CliRun {
        ok: output.status.success(),
        stdout: output.stdout,
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    })
}

/// What Funnel needs from this machine's tailnet, as `tailscale status
/// --json` tells it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunnelNeeds {
    /// This machine's MagicDNS name, such as `devbox.example.ts.net`.
    pub name: Option<String>,
    /// Whether the tailnet has HTTPS certificates on.
    pub https: bool,
    /// Whether the tailnet policy gives this machine the `funnel` attribute.
    pub funnel: bool,
}

/// Reads [`FunnelNeeds`] from the output of `tailscale status --json`.
/// `None` when it isn't a status, or Tailscale isn't logged in.
pub fn funnel_needs(status: &[u8]) -> Option<FunnelNeeds> {
    let status: Status = serde_json::from_slice(status).ok()?;
    let own = status.own.as_ref()?;
    let has = |cap: &str| own.cap_map.contains_key(cap);
    Some(FunnelNeeds {
        name: own_dns_name(&status),
        https: !status.cert_domains.is_empty() || has("https"),
        funnel: has("funnel"),
    })
}

/// The name a status gives this machine, without its trailing dot.
fn own_dns_name(status: &Status) -> Option<String> {
    let name = status.own.as_ref()?.dns_name.trim_end_matches('.');
    (!name.is_empty()).then(|| name.to_owned())
}

/// Watches until cancelled.
pub(crate) async fn run(publisher: Publisher, cancel: CancellationToken) {
    let mut model = Model::default();
    let mut backoff = Backoff::new(WATCH_LIMITS);
    loop {
        let outcome = tokio::select! {
            () = cancel.cancelled() => return,
            outcome = watch_once(&mut model, &publisher) => outcome,
        };
        let delay = match outcome {
            Outcome::Watched => {
                backoff.reset();
                backoff.next(Duration::ZERO)
            }
            Outcome::Polled => CLI_POLL,
            Outcome::Failed => backoff.next(Duration::ZERO),
        };
        tokio::select! {
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(delay) => {}
        }
    }
}

/// Opens the bus and follows it until it closes.
async fn watch_once(model: &mut Model, publisher: &Publisher) -> Outcome {
    let Some(endpoint) = api::discover().await else {
        return fall_back(model, publisher).await;
    };
    let mut reply = match api::get(&endpoint, api::BUS_PATH).await {
        Ok(reply) => reply,
        Err(err) => {
            debug!(%err, "the tailscale bus didn't open");
            return fall_back(model, publisher).await;
        }
    };

    let mut lines = api::Lines::default();
    let mut poll = tokio::time::interval(STATUS_POLL);
    poll.tick().await;
    let mut watched = false;
    loop {
        tokio::select! {
            frame = reply.response.body_mut().frame() => {
                let data = match frame {
                    Some(Ok(frame)) => match frame.into_data() {
                        Ok(data) => data,
                        Err(_) => continue,
                    },
                    Some(Err(err)) => {
                        debug!(%err, "the tailscale bus failed");
                        break;
                    }
                    None => break,
                };
                for line in lines.push(&data) {
                    let notify = match serde_json::from_slice::<Notify>(&line) {
                        Ok(notify) => notify,
                        Err(err) => {
                            debug!(%err, "skipped a tailscale bus message");
                            continue;
                        }
                    };
                    if notify.is_empty() {
                        continue;
                    }
                    watched = true;
                    if model.apply_notify(notify) {
                        refresh(model, &endpoint).await;
                    }
                    publisher.publish(model);
                }
            }
            _ = poll.tick() => {
                refresh(model, &endpoint).await;
                publisher.publish(model);
            }
        }
    }
    if watched {
        Outcome::Watched
    } else {
        Outcome::Failed
    }
}

/// Fetches the status and folds it in.
async fn refresh(model: &mut Model, endpoint: &api::Endpoint) {
    match api::status(endpoint).await {
        Ok(status) => model.apply_status(status),
        Err(err) => debug!(%err, "the tailscale status didn't load"),
    }
}

/// Tries the CLI, and otherwise reports Tailscale as not running.
async fn fall_back(model: &mut Model, publisher: &Publisher) -> Outcome {
    if let Some(status) = api::cli_status().await {
        model.apply_status(status);
        publisher.publish(model);
        return Outcome::Polled;
    }
    model.lost(api::installed().await);
    publisher.publish(model);
    Outcome::Failed
}

/// What the watcher has pieced together.
#[derive(Debug, Default)]
pub(crate) struct Model {
    /// Whether Tailscale looked installed the last time nothing answered.
    installed: bool,
    /// Whether the LocalAPI or the CLI answered last time.
    reachable: bool,
    /// The backend state, if known.
    state: Option<NetworkState>,
    /// The tailnet's name, kept after logging out so the network keeps
    /// its ID.
    tailnet_name: Option<String>,
    /// The MagicDNS suffix.
    magic_dns_suffix: Option<String>,
    /// The client's version, without its build suffix.
    client_version: Option<String>,
    /// Peers by stable node ID.
    peers: BTreeMap<String, TailnetPeer>,
}

impl Model {
    /// Folds in a full status.
    pub fn apply_status(&mut self, status: Status) {
        self.reachable = true;
        self.state = Some(state_from_name(&status.backend_state));
        if let Some(version) = status.version.split('-').next().filter(|v| !v.is_empty()) {
            self.client_version = Some(version.to_owned());
        }
        let mut suffix = status.magic_dns_suffix;
        if let Some(tailnet) = status.current_tailnet {
            if !tailnet.name.is_empty() {
                self.tailnet_name = Some(tailnet.name);
            }
            if !tailnet.magic_dns_suffix.is_empty() {
                suffix = tailnet.magic_dns_suffix;
            }
        }
        if !suffix.is_empty() {
            self.magic_dns_suffix = Some(suffix);
        }
        let peers: BTreeMap<String, TailnetPeer> = status
            .peer
            .into_iter()
            .map(|(key, peer)| {
                let peer = peer_from_status(key, peer);
                (peer.stable_id.clone(), peer)
            })
            .collect();
        // A stopped client may report no peers. Keep the last ones so links
        // stay grouped under the network.
        if !peers.is_empty() || self.state == Some(NetworkState::Up) {
            self.peers = peers;
        }
    }

    /// Folds in a bus message, and says whether the status should be
    /// fetched because the backend just came up.
    pub fn apply_notify(&mut self, notify: Notify) -> bool {
        self.reachable = true;
        if let Some(status) = notify.initial_status {
            self.apply_status(status);
        }
        let mut came_up = false;
        if let Some(state) = notify.state {
            let state = state_from_number(state);
            came_up = state == NetworkState::Up && self.state != Some(NetworkState::Up);
            self.state = Some(state);
        }
        for change in notify.peer_changed_patch {
            self.patch(change);
        }
        for node in notify.peers_changed {
            self.replace(node);
        }
        for removed in notify.peers_removed {
            self.peers.retain(|_, peer| peer.node_id != removed);
        }
        came_up
    }

    /// Applies a patch to the peer it names.
    fn patch(&mut self, change: PeerChange) {
        let Some(peer) = self
            .peers
            .values_mut()
            .find(|peer| peer.node_id == change.node_id && change.node_id != 0)
        else {
            return;
        };
        if let Some(online) = change.online {
            peer.peer.online = online;
            if online {
                peer.peer.last_seen = None;
            }
        }
        if let Some(last_seen) = change.last_seen
            && !peer.peer.online
        {
            peer.peer.last_seen = parse_time(&last_seen);
        }
        if let Some(key_expiry) = change.key_expiry {
            peer.peer.key_expiry = parse_time(&key_expiry);
        }
    }

    /// Replaces a peer with a full node, keeping the path the node doesn't
    /// carry.
    fn replace(&mut self, node: Node) {
        let key = if node.stable_id.is_empty() {
            format!("node:{}", node.id)
        } else {
            node.stable_id.clone()
        };
        let old = self.peers.remove(&key).or_else(|| {
            let old = self
                .peers
                .iter()
                .find(|(_, peer)| peer.node_id == node.id)?;
            let old_key = old.0.clone();
            self.peers.remove(&old_key)
        });
        let online = node
            .online
            .or(old.as_ref().map(|old| old.peer.online))
            .unwrap_or(false);
        let last_seen = if online {
            None
        } else {
            node.last_seen.as_deref().and_then(parse_time)
        };
        let dns_name = node.name.trim_end_matches('.');
        let peer = TailnetPeer {
            stable_id: key.clone(),
            node_id: node.id,
            addresses: node
                .addresses
                .iter()
                .filter_map(|prefix| prefix.split('/').next()?.parse().ok())
                .collect(),
            peer: Peer {
                host_name: node.hostinfo.hostname,
                dns_name: (!dns_name.is_empty()).then(|| dns_name.to_owned()),
                online,
                last_seen,
                path: old.map_or(PeerPath::Idle, |old| old.peer.path),
                key_expiry: parse_time(&node.key_expiry),
            },
        };
        self.peers.insert(key, peer);
    }

    /// Notes that nothing answered.
    pub fn lost(&mut self, installed: bool) {
        self.installed = installed;
        self.reachable = false;
        self.state = Some(NetworkState::NotRunning);
    }

    /// The view links and the app see.
    pub fn view(&self) -> Tailnet {
        let Some(state) = self.state else {
            return Tailnet::default();
        };
        if !self.reachable && !self.installed {
            return Tailnet::default();
        }
        let (id, name) = match &self.tailnet_name {
            Some(name) => (
                NetworkId::derive(&format!("tailscale:{name}")),
                name.clone(),
            ),
            None => (NetworkId::derive("tailscale"), "Tailscale".to_owned()),
        };
        Tailnet {
            network: Some(Network {
                id,
                kind: NetworkKind::Tailscale {
                    magic_dns_suffix: self.magic_dns_suffix.clone(),
                    client_version: self.client_version.clone(),
                },
                name,
                state,
            }),
            peers: self.peers.values().cloned().collect(),
        }
    }
}

/// A peer from the status.
fn peer_from_status(key: String, status: PeerStatus) -> TailnetPeer {
    let path = if !status.cur_addr.is_empty() {
        PeerPath::Direct {
            address: status.cur_addr,
        }
    } else if status.active && !status.relay.is_empty() {
        PeerPath::Relayed {
            region: status.relay,
        }
    } else {
        PeerPath::Idle
    };
    let dns_name = status.dns_name.trim_end_matches('.');
    TailnetPeer {
        stable_id: if status.id.is_empty() { key } else { status.id },
        node_id: status.node_id,
        addresses: status
            .tailscale_ips
            .iter()
            .filter_map(|address| address.parse().ok())
            .collect(),
        peer: Peer {
            host_name: status.host_name,
            dns_name: (!dns_name.is_empty()).then(|| dns_name.to_owned()),
            online: status.online,
            last_seen: if status.online {
                None
            } else {
                parse_time(&status.last_seen)
            },
            path,
            key_expiry: parse_time(&status.key_expiry),
        },
    }
}

/// The state for a `BackendState` name.
fn state_from_name(name: &str) -> NetworkState {
    match name {
        "Running" => NetworkState::Up,
        "Starting" | "NoState" => NetworkState::Starting,
        "Stopped" => NetworkState::Off,
        "NeedsLogin" => NetworkState::NeedsLogin,
        "NeedsMachineAuth" => NetworkState::NeedsApproval,
        _ => NetworkState::Unknown,
    }
}

/// The state for the bus's numeric `State`.
fn state_from_number(number: i64) -> NetworkState {
    match number {
        0 | 5 => NetworkState::Starting,
        2 => NetworkState::NeedsLogin,
        3 => NetworkState::NeedsApproval,
        4 => NetworkState::Off,
        6 => NetworkState::Up,
        _ => NetworkState::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maxhq_protocol::time::Timestamp;

    /// A status shaped like the real one, with made-up names.
    const STATUS: &str = r#"{
        "Version": "1.102.3-t1234abcd-g5678",
        "BackendState": "Running",
        "MagicDNSSuffix": "tail1234.ts.net",
        "CurrentTailnet": {"Name": "example.github", "MagicDNSSuffix": "tail1234.ts.net", "MagicDNSEnabled": true},
        "Self": {"HostName": "mac"},
        "Peer": {
            "nodekey:aa": {
                "ID": "nAAA", "NodeID": 11, "HostName": "devbox",
                "DNSName": "devbox.tail1234.ts.net.",
                "TailscaleIPs": ["100.101.1.2", "fd7a:115c:a1e0::2"],
                "CurAddr": "192.168.1.54:41641", "Relay": "nyc", "Active": true,
                "Online": true, "LastSeen": "0001-01-01T00:00:00Z",
                "KeyExpiry": "2026-04-23T09:00:00Z"
            },
            "nodekey:bb": {
                "ID": "nBBB", "NodeID": 12, "HostName": "phone",
                "DNSName": "phone.tail1234.ts.net.", "TailscaleIPs": ["100.101.1.3"],
                "CurAddr": "", "Relay": "fra", "Active": false,
                "Online": false, "LastSeen": "2025-10-25T00:00:00Z"
            }
        }
    }"#;

    #[test]
    fn reads_this_machines_name_without_its_dot() {
        let status: Status = serde_json::from_str(
            r#"{"Self": {"HostName": "devbox", "DNSName": "devbox.example.ts.net."}}"#,
        )
        .unwrap();
        assert_eq!(
            own_dns_name(&status).as_deref(),
            Some("devbox.example.ts.net")
        );
        assert_eq!(own_dns_name(&serde_json::from_str(STATUS).unwrap()), None);
        assert_eq!(own_dns_name(&Status::default()), None);
    }

    #[test]
    fn reads_what_funnel_needs_from_the_status() {
        let ready = br#"{
            "CertDomains": ["devbox.example.ts.net"],
            "Self": {"DNSName": "devbox.example.ts.net.", "CapMap": {"funnel": null, "https": null}}
        }"#;
        assert_eq!(
            funnel_needs(ready),
            Some(FunnelNeeds {
                name: Some("devbox.example.ts.net".into()),
                https: true,
                funnel: true,
            })
        );
        let bare = br#"{"CertDomains": null, "Self": {"DNSName": "devbox.example.ts.net."}}"#;
        let needs = funnel_needs(bare).unwrap();
        assert!(!needs.https && !needs.funnel);
        assert_eq!(funnel_needs(b"{}"), None);
        assert_eq!(funnel_needs(b"not json"), None);
    }

    fn model() -> Model {
        let mut model = Model::default();
        model.apply_status(serde_json::from_str(STATUS).unwrap());
        model
    }

    fn peer<'a>(view: &'a Tailnet, host: &str) -> Option<&'a Peer> {
        view.peers
            .iter()
            .find(|peer| peer.peer.host_name == host)
            .map(|peer| &peer.peer)
    }

    #[test]
    fn the_status_becomes_a_network_and_peers() {
        let view = model().view();
        let network = view.network.clone().unwrap();
        assert_eq!(network.id, NetworkId::derive("tailscale:example.github"));
        assert_eq!(network.name, "example.github");
        assert_eq!(network.state, NetworkState::Up);
        assert_eq!(
            network.kind,
            NetworkKind::Tailscale {
                magic_dns_suffix: Some("tail1234.ts.net".into()),
                client_version: Some("1.102.3".into()),
            }
        );
        assert_eq!(view.peers.len(), 2);

        let node = peer(&view, "devbox").unwrap();
        assert_eq!(node.dns_name.as_deref(), Some("devbox.tail1234.ts.net"));
        assert!(node.online);
        assert_eq!(node.last_seen, None);
        assert_eq!(
            node.path,
            PeerPath::Direct {
                address: "192.168.1.54:41641".into()
            }
        );
        assert!(node.key_expiry.is_some());

        let phone = peer(&view, "phone").unwrap();
        assert!(!phone.online);
        assert_eq!(phone.last_seen, Some(Timestamp(1_761_350_400_000)));
        assert_eq!(phone.path, PeerPath::Idle);
        assert_eq!(phone.key_expiry, None);
    }

    #[test]
    fn patches_flip_peers_online_and_off() {
        let mut model = model();
        let notify: Notify = serde_json::from_str(
            r#"{"PeerChangedPatch":[
                {"NodeID":11,"Online":false,"LastSeen":"2025-10-25T00:00:00Z"},
                {"NodeID":12,"Online":true},
                {"NodeID":99,"Online":true}
            ]}"#,
        )
        .unwrap();
        assert!(!model.apply_notify(notify));
        let view = model.view();
        let node = peer(&view, "devbox").unwrap();
        assert!(!node.online);
        assert_eq!(node.last_seen, Some(Timestamp(1_761_350_400_000)));
        let phone = peer(&view, "phone").unwrap();
        assert!(phone.online);
        assert_eq!(phone.last_seen, None);
    }

    #[test]
    fn changed_and_removed_peers_replace_and_remove() {
        let mut model = model();
        let notify: Notify = serde_json::from_str(
            r#"{"PeersChanged":[{"ID":11,"StableID":"nAAA","Name":"devbox.tail1234.ts.net.",
                "KeyExpiry":"0001-01-01T00:00:00Z","Addresses":["100.101.1.9/32"],
                "Hostinfo":{"Hostname":"devbox","OS":"linux"},"Online":true}],
                "PeersRemoved":[12]}"#,
        )
        .unwrap();
        model.apply_notify(notify);
        let view = model.view();
        assert_eq!(view.peers.len(), 1);
        let node = &view.peers[0];
        assert_eq!(
            node.addresses,
            ["100.101.1.9".parse::<std::net::IpAddr>().unwrap()]
        );
        assert_eq!(node.peer.key_expiry, None);
        // The path came from the status, and a full node doesn't carry one.
        assert!(matches!(node.peer.path, PeerPath::Direct { .. }));
    }

    #[test]
    fn bus_states_map_to_network_states() {
        let mut model = model();
        let stopped: Notify = serde_json::from_str(r#"{"State":4}"#).unwrap();
        assert!(!model.apply_notify(stopped));
        let view = model.view();
        assert_eq!(view.network.unwrap().state, NetworkState::Off);
        // Peers stay, so links keep their network.
        assert_eq!(view.peers.len(), 2);

        let running: Notify = serde_json::from_str(r#"{"State":6}"#).unwrap();
        assert!(model.apply_notify(running), "coming up asks for a status");
        for (number, state) in [
            (0, NetworkState::Starting),
            (2, NetworkState::NeedsLogin),
            (3, NetworkState::NeedsApproval),
            (5, NetworkState::Starting),
            (1, NetworkState::Unknown),
        ] {
            assert_eq!(state_from_number(number), state);
        }
    }

    #[test]
    fn the_first_bus_message_seeds_everything() {
        let mut model = Model::default();
        let notify =
            format!(r#"{{"Version":"1.102.3","State":6,"InitialStatus":{STATUS},"NetMap":null}}"#);
        model.apply_notify(serde_json::from_str(&notify).unwrap());
        let view = model.view();
        assert_eq!(view.network.unwrap().state, NetworkState::Up);
        assert_eq!(view.peers.len(), 2);
    }

    #[test]
    fn a_missing_client_shows_no_network_unless_installed() {
        let mut model = Model::default();
        assert_eq!(model.view(), Tailnet::default());
        model.lost(false);
        assert_eq!(model.view(), Tailnet::default());
        model.lost(true);
        let network = model.view().network.unwrap();
        assert_eq!(network.state, NetworkState::NotRunning);
        assert_eq!(network.id, NetworkId::derive("tailscale"));
    }

    #[test]
    fn a_stopped_client_keeps_the_network_id() {
        let mut model = model();
        model.lost(true);
        let network = model.view().network.unwrap();
        assert_eq!(network.id, NetworkId::derive("tailscale:example.github"));
        assert_eq!(network.state, NetworkState::NotRunning);
    }

    /// Watches the Tailscale client on this machine for a few seconds. It
    /// prints counts only, never names or addresses.
    #[tokio::test]
    #[ignore = "talks to the Tailscale client on this machine"]
    async fn watches_the_tailscale_client_here() {
        let (tailnet, mut view) = watch::channel(Arc::new(Tailnet::default()));
        let (networks, _networks) = watch::channel(Vec::new());
        let cancel = CancellationToken::new();
        let watcher = tokio::spawn(run(Publisher { tailnet, networks }, cancel.clone()));
        let seen = tokio::time::timeout(
            Duration::from_secs(10),
            view.wait_for(|view| view.network.is_some() && !view.peers.is_empty()),
        )
        .await
        .is_ok();
        cancel.cancel();
        let _ = watcher.await;
        assert!(seen, "no tailnet with peers came through");
        let view = view.borrow().clone();
        let network = view.network.as_ref().expect("a network");
        let online = view.peers.iter().filter(|peer| peer.peer.online).count();
        println!(
            "state {:?}, {} peers, {} online",
            network.state,
            view.peers.len(),
            online
        );
    }
}
