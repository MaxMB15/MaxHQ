// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The client's view of the tailnet, and matching SSH hosts to its peers.

use std::net::IpAddr;

use maxhq_protocol::link::{Network, NetworkState, Peer};

/// What the Tailscale watcher knows, shared with every link.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Tailnet {
    /// The tailnet, or none when Tailscale isn't installed.
    pub network: Option<Network>,
    /// The peers as last reported. They're kept while the network is down
    /// so links stay grouped under it.
    pub peers: Vec<TailnetPeer>,
}

/// One peer, with what matching needs beyond the protocol's [`Peer`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TailnetPeer {
    /// Tailscale's stable node ID, which keys the peer table.
    pub stable_id: String,
    /// The numeric node ID that bus patches name, or 0 when unknown.
    pub node_id: i64,
    /// The peer's tailnet addresses.
    pub addresses: Vec<IpAddr>,
    /// What the link reports.
    pub peer: Peer,
}

impl Tailnet {
    /// Whether the tailnet is up, so its peers' online flags are current.
    pub fn is_up(&self) -> bool {
        self.network
            .as_ref()
            .is_some_and(|network| network.state == NetworkState::Up)
    }

    /// The peer an SSH host name is, matched against each peer's MagicDNS
    /// name, its short MagicDNS name, its tailnet addresses and its
    /// `HostName`, best match first. `resolved` holds addresses the name
    /// resolved to, if it was looked up.
    pub fn find(&self, host: &str, resolved: &[IpAddr]) -> Option<&TailnetPeer> {
        let host = normalize(host);
        let literal = host.parse::<IpAddr>().ok();
        self.peers
            .iter()
            .filter_map(|peer| {
                let score = match_score(peer, &host, literal, resolved)?;
                Some(((score, peer.peer.online), peer))
            })
            .max_by_key(|(rank, _)| *rank)
            .map(|(_, peer)| peer)
    }

    /// Whether a host name is on this tailnet by its form alone: a tailnet
    /// address or a MagicDNS name.
    pub fn owns(&self, host: &str) -> bool {
        let host = normalize(host);
        if let Ok(address) = host.parse::<IpAddr>() {
            return is_tailnet_address(address);
        }
        let suffix = self
            .network
            .as_ref()
            .and_then(|network| match &network.kind {
                maxhq_protocol::link::NetworkKind::Tailscale {
                    magic_dns_suffix, ..
                } => magic_dns_suffix.as_deref(),
            });
        host.ends_with(".ts.net")
            || suffix.is_some_and(|suffix| {
                host.strip_suffix(&suffix.to_ascii_lowercase())
                    .is_some_and(|rest| rest.ends_with('.'))
            })
    }
}

/// How well a peer matches a host, higher is better, or none.
fn match_score(
    peer: &TailnetPeer,
    host: &str,
    literal: Option<IpAddr>,
    resolved: &[IpAddr],
) -> Option<u8> {
    if let Some(dns_name) = peer.peer.dns_name.as_deref() {
        let dns_name = normalize(dns_name);
        if dns_name == host {
            return Some(4);
        }
        if dns_name.split('.').next() == Some(host) {
            return Some(3);
        }
    }
    if literal.is_some_and(|address| peer.addresses.contains(&address)) {
        return Some(4);
    }
    if resolved
        .iter()
        .any(|address| peer.addresses.contains(address))
    {
        return Some(2);
    }
    (normalize(&peer.peer.host_name) == host).then_some(1)
}

/// Lowercases a host and drops a trailing dot or IPv6 brackets.
fn normalize(host: &str) -> String {
    let host = host.trim().trim_end_matches('.');
    let host = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host);
    host.to_ascii_lowercase()
}

/// Whether an address is in Tailscale's ranges, 100.64.0.0/10 or
/// fd7a:115c:a1e0::/48.
pub(crate) fn is_tailnet_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => {
            let [first, second, ..] = v4.octets();
            first == 100 && second & 0xc0 == 64
        }
        IpAddr::V6(v6) => v6.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use maxhq_protocol::ids::NetworkId;
    use maxhq_protocol::link::{NetworkKind, PeerPath};

    /// A peer for tests.
    pub(crate) fn peer(
        host_name: &str,
        dns_name: &str,
        address: &str,
        online: bool,
    ) -> TailnetPeer {
        TailnetPeer {
            stable_id: format!("n{host_name}"),
            node_id: 0,
            addresses: vec![address.parse().unwrap()],
            peer: Peer {
                host_name: host_name.into(),
                dns_name: Some(dns_name.into()),
                online,
                last_seen: None,
                path: PeerPath::Idle,
                key_expiry: None,
            },
        }
    }

    /// A tailnet for tests, up, with the given peers.
    pub(crate) fn tailnet(peers: Vec<TailnetPeer>) -> Tailnet {
        Tailnet {
            network: Some(Network {
                id: NetworkId::derive("tailscale:example"),
                kind: NetworkKind::Tailscale {
                    magic_dns_suffix: Some("tail1234.ts.net".into()),
                    client_version: Some("1.102.3".into()),
                },
                name: "example".into(),
                state: NetworkState::Up,
            }),
            peers,
        }
    }

    fn sample() -> Tailnet {
        tailnet(vec![
            peer("devbox", "devbox.tail1234.ts.net", "100.101.1.2", true),
            peer(
                "Dev-MacBook",
                "dev-mac.tail1234.ts.net",
                "100.101.1.3",
                true,
            ),
            peer("localhost", "box-a.tail1234.ts.net", "100.101.1.4", false),
            peer("localhost", "box-b.tail1234.ts.net", "100.101.1.5", true),
        ])
    }

    fn found(tailnet: &Tailnet, host: &str, resolved: &[IpAddr]) -> Option<String> {
        tailnet
            .find(host, resolved)
            .and_then(|peer| peer.peer.dns_name.clone())
    }

    #[test]
    fn hosts_match_peers_by_name() {
        let tailnet = sample();
        assert_eq!(
            found(&tailnet, "devbox", &[]).as_deref(),
            Some("devbox.tail1234.ts.net")
        );
        assert_eq!(
            found(&tailnet, "DevBox.tail1234.ts.net.", &[]).as_deref(),
            Some("devbox.tail1234.ts.net")
        );
        // The short MagicDNS name, which the admin console can set apart
        // from the machine's own host name.
        assert_eq!(
            found(&tailnet, "dev-mac", &[]).as_deref(),
            Some("dev-mac.tail1234.ts.net")
        );
        assert_eq!(
            found(&tailnet, "dev-macbook", &[]).as_deref(),
            Some("dev-mac.tail1234.ts.net")
        );
    }

    #[test]
    fn hosts_match_peers_by_address() {
        let tailnet = sample();
        assert_eq!(
            found(&tailnet, "100.101.1.2", &[]).as_deref(),
            Some("devbox.tail1234.ts.net")
        );
        let resolved = ["100.101.1.3".parse().unwrap()];
        assert_eq!(
            found(&tailnet, "mac.example.com", &resolved).as_deref(),
            Some("dev-mac.tail1234.ts.net")
        );
    }

    #[test]
    fn shared_host_names_prefer_the_peer_thats_online() {
        assert_eq!(
            found(&sample(), "localhost", &[]).as_deref(),
            Some("box-b.tail1234.ts.net")
        );
    }

    #[test]
    fn other_hosts_match_nothing() {
        let tailnet = sample();
        assert_eq!(found(&tailnet, "ws-bravo", &[]), None);
        assert_eq!(found(&tailnet, "192.168.1.54", &[]), None);
        assert_eq!(found(&tailnet, "box", &[]), None);
    }

    #[test]
    fn tailnet_addresses_and_names_belong_to_it() {
        let tailnet = sample();
        assert!(tailnet.owns("100.64.0.1"));
        assert!(tailnet.owns("100.127.255.254"));
        assert!(!tailnet.owns("100.128.0.1"));
        assert!(!tailnet.owns("100.63.255.255"));
        assert!(tailnet.owns("fd7a:115c:a1e0::1"));
        assert!(tailnet.owns("[fd7a:115c:a1e0:ab12::1]"));
        assert!(!tailnet.owns("fd7a:115c:a1e1::1"));
        assert!(tailnet.owns("gone.tail1234.ts.net"));
        assert!(tailnet.owns("box.other.ts.net"));
        assert!(!tailnet.owns("devbox"));
        assert!(!tailnet.owns("example.com"));
    }
}
