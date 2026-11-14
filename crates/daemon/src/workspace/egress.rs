// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! W-7's egress check. A workspace may reach the internet and nothing
//! private: no RFC 1918 range, no link-local, no CGNAT or Tailscale
//! address, no ULA, and none of the host's own addresses or ports. The
//! host builds the targets here from what it knows about itself, the
//! guest's `maxd guest probe` tries them, and [`judge`] reads the answers.
//!
//! In M3 the block itself comes from the device's guard, such as
//! `guest-egress` on devbox. [`acl`] builds the same rules as an
//! Incus network ACL, which nothing applies yet.
//!
//! The probe never targets an address inside a managed Incus bridge other
//! than the host's own, so it never connects to another container.

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use maxhq_protocol::model::EgressCheck;
use maxhq_protocol::time::Timestamp;
use serde_json::{json, Value};

use super::probe::{is_link_local, Report, Target, Targets};
use crate::incus::Network;

/// W-7's blocked ranges.
#[cfg_attr(not(test), allow(dead_code))]
pub const BLOCKED: &[&str] = &[
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "169.254.0.0/16",
    "100.64.0.0/10",
    "0.0.0.0/8",
    "192.0.0.0/24",
    "198.18.0.0/15",
    "224.0.0.0/4",
    "240.0.0.0/4",
    "fc00::/7",
    "fe80::/10",
];

/// One address in each blocked range and the ones that matter most:
/// Tailscale's DNS at 100.100.100.100, the cloud metadata service, and a
/// ULA. Each is tried on [`SAMPLE_PORTS`]. 0.0.0.0/8 and 224.0.0.0/4 have
/// none, since the kernel refuses a TCP connect to them before it leaves.
const SAMPLES: &[&str] = &[
    "10.0.0.1",
    "172.16.0.1",
    "192.168.0.1",
    "192.168.1.1",
    "100.64.0.1",
    "100.100.100.100",
    "169.254.169.254",
    "192.0.0.1",
    "198.18.0.1",
    "240.0.0.1",
    "fd00::1",
    "fd7a:115c:a1e0::53",
];

/// The ports tried on sample addresses and gateways.
const SAMPLE_PORTS: &[u16] = &[22, 53, 80, 443];

/// The ports tried on every host address, besides the ones it listens on.
const HOST_PORTS: &[u16] = &[22];

/// DNS, which the guard lets containers reach on the host. The probe
/// skips it on host addresses.
const DNS: u16 = 53;

/// The places a workspace must reach.
pub const REQUIRED: &[(&str, u16)] = &[("github.com", 443), ("deb.debian.org", 443)];

/// How long each connect may take.
pub const TIMEOUT_MS: u64 = 2000;

/// The most blocked targets tried, so a host with many addresses and
/// ports doesn't make the check run for minutes.
const MAX_TARGETS: usize = 1024;

/// An address range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Net {
    /// The first address.
    pub addr: IpAddr,
    /// The prefix length.
    pub prefix: u8,
}

impl Net {
    /// Reads `10.0.0.0/8` or `10.100.2.1/24`. A bare address is one host.
    pub fn parse(text: &str) -> Option<Self> {
        let (addr, prefix) = match text.split_once('/') {
            Some((addr, prefix)) => (addr.parse().ok()?, Some(prefix.parse().ok()?)),
            None => (text.parse().ok()?, None),
        };
        let max = if matches!(addr, IpAddr::V4(_)) {
            32
        } else {
            128
        };
        let prefix = prefix.unwrap_or(max);
        (prefix <= max).then_some(Self { addr, prefix })
    }

    /// True when `ip` is in the range.
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix))
                    .unwrap_or(0);
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix))
                    .unwrap_or(0);
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

/// True when `ip` is in one of W-7's ranges.
#[cfg_attr(not(test), allow(dead_code))]
pub fn is_blocked(ip: IpAddr) -> bool {
    BLOCKED
        .iter()
        .filter_map(|range| Net::parse(range))
        .any(|net| net.contains(ip))
}

/// An IPv6 address the host holds, with its interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostV6 {
    /// The address.
    pub address: Ipv6Addr,
    /// The interface, such as `tailscale0`.
    pub interface: String,
}

/// A TCP socket the host listens on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Listener {
    /// The local address, which may be the unspecified one.
    pub address: IpAddr,
    /// The port.
    pub port: u16,
}

/// What the host knows about its own network.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostFacts {
    /// Its IPv4 addresses, without loopback.
    pub v4: Vec<Ipv4Addr>,
    /// Its IPv6 addresses, without loopback.
    pub v6: Vec<HostV6>,
    /// The gateways its routes use, such as the LAN router.
    pub gateways: Vec<IpAddr>,
    /// Its listening TCP sockets.
    pub listeners: Vec<Listener>,
}

impl HostFacts {
    /// Reads the host's facts from `/proc`. A file that's missing, as on
    /// macOS, adds nothing.
    pub fn read() -> Self {
        let read = |path: &str| std::fs::read_to_string(path).unwrap_or_default();
        let mut listeners = parse_tcp4(&read("/proc/net/tcp"));
        listeners.extend(parse_tcp6(&read("/proc/net/tcp6")));
        let mut gateways = parse_route(&read("/proc/net/route"));
        gateways.extend(parse_ipv6_route(&read("/proc/net/ipv6_route")));
        Self {
            v4: parse_fib_trie(&read("/proc/net/fib_trie")),
            v6: parse_if_inet6(&read("/proc/net/if_inet6")),
            gateways,
            listeners,
        }
    }
}

/// The host's local IPv4 addresses from `/proc/net/fib_trie`, which marks
/// each with `/32 host LOCAL` on the line after it.
pub fn parse_fib_trie(text: &str) -> Vec<Ipv4Addr> {
    let mut found = BTreeSet::new();
    let mut last = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(addr) = line.strip_prefix("|-- ") {
            last = addr.trim().parse::<Ipv4Addr>().ok();
        } else if line == "/32 host LOCAL"
            && let Some(addr) = last.filter(|addr| !addr.is_loopback())
        {
            found.insert(addr);
        }
    }
    found.into_iter().collect()
}

/// The host's IPv6 addresses from `/proc/net/if_inet6`.
pub fn parse_if_inet6(text: &str) -> Vec<HostV6> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (hex, interface) = (fields.first()?, fields.get(5)?);
            let address = Ipv6Addr::from(u128::from_str_radix(hex, 16).ok()?);
            (*interface != "lo" && !address.is_loopback()).then(|| HostV6 {
                address,
                interface: (*interface).to_owned(),
            })
        })
        .collect()
}

/// The IPv4 gateways in `/proc/net/route`, whose addresses are hex in the
/// kernel's byte order.
pub fn parse_route(text: &str) -> Vec<IpAddr> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (interface, gateway) = (fields.first()?, fields.get(2)?);
            let value = u32::from_str_radix(gateway, 16).ok()?;
            let addr = Ipv4Addr::from(value.to_le_bytes());
            (*interface != "lo" && !addr.is_unspecified() && !addr.is_loopback())
                .then_some(IpAddr::V4(addr))
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The IPv6 gateways in `/proc/net/ipv6_route`. A link-local next hop is
/// on another link than the container's, so it's left out.
pub fn parse_ipv6_route(text: &str) -> Vec<IpAddr> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (hop, interface) = (fields.get(4)?, fields.get(9)?);
            let addr = Ipv6Addr::from(u128::from_str_radix(hop, 16).ok()?);
            (*interface != "lo"
                && !addr.is_unspecified()
                && !addr.is_loopback()
                && !is_link_local(&addr))
            .then_some(IpAddr::V6(addr))
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The `0A` state in `/proc/net/tcp`, a listening socket.
const LISTEN: &str = "0A";

/// Listening sockets in `/proc/net/tcp`.
pub fn parse_tcp4(text: &str) -> Vec<Listener> {
    listeners(text, |hex| {
        let value = u32::from_str_radix(hex, 16).ok()?;
        Some(IpAddr::V4(Ipv4Addr::from(value.to_le_bytes())))
    })
}

/// Listening sockets in `/proc/net/tcp6`, whose addresses are four words
/// in the kernel's byte order.
pub fn parse_tcp6(text: &str) -> Vec<Listener> {
    listeners(text, |hex| {
        if hex.len() != 32 {
            return None;
        }
        let mut bytes = [0u8; 16];
        for (i, word) in bytes.chunks_mut(4).enumerate() {
            let value = u32::from_str_radix(hex.get(i * 8..i * 8 + 8)?, 16).ok()?;
            word.copy_from_slice(&value.to_le_bytes());
        }
        Some(IpAddr::V6(Ipv6Addr::from(bytes)))
    })
}

fn listeners(text: &str, address: impl Fn(&str) -> Option<IpAddr>) -> Vec<Listener> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (local, state) = (fields.get(1)?, fields.get(3)?);
            if *state != LISTEN {
                return None;
            }
            let (addr, port) = local.split_once(':')?;
            Some(Listener {
                address: address(addr)?,
                port: u16::from_str_radix(port, 16).ok()?,
            })
        })
        .collect()
}

/// A managed Incus bridge, as the check needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bridge {
    /// The bridge's name, which is also its host interface.
    pub name: String,
    /// The host's address on it, with the bridge's prefix.
    pub v4: Option<Net>,
    /// The same for IPv6.
    pub v6: Option<Net>,
}

impl Bridge {
    /// The bridge behind an Incus network, if it's a managed bridge.
    pub fn from_network(network: &Network) -> Option<Self> {
        network.is_managed_bridge().then(|| Self {
            name: network.name.clone(),
            v4: network.ipv4().and_then(Net::parse),
            v6: network.ipv6().and_then(Net::parse),
        })
    }

    fn contains(&self, ip: IpAddr) -> bool {
        self.v4.is_some_and(|net| net.contains(ip)) || self.v6.is_some_and(|net| net.contains(ip))
    }
}

/// The targets a workspace on the bridge `own` probes.
pub fn targets(facts: &HostFacts, bridges: &[Bridge], own: &str) -> Targets {
    let own_bridge = bridges.iter().find(|bridge| bridge.name == own);

    // The host's own addresses, each with the ports it listens on there.
    let mut host: Vec<(IpAddr, Option<&str>)> = Vec::new();
    let mut add_host = |ip: IpAddr, interface: Option<&'static str>| {
        if !host.iter().any(|(known, _)| *known == ip) {
            host.push((ip, interface));
        }
    };
    for bridge in bridges {
        for net in [bridge.v4, bridge.v6].into_iter().flatten() {
            add_host(net.addr, None);
        }
    }
    for ip in &facts.v4 {
        add_host(IpAddr::V4(*ip), None);
    }
    for v6 in &facts.v6 {
        if !is_link_local(&v6.address) {
            add_host(IpAddr::V6(v6.address), None);
        } else if own_bridge.is_some_and(|bridge| bridge.name == v6.interface) {
            // The host's link-local address on the container's own link,
            // reached through the container's eth0.
            add_host(IpAddr::V6(v6.address), Some("eth0"));
        }
    }

    let mut blocked = Vec::new();
    for (ip, interface) in &host {
        let mut ports: BTreeSet<u16> = HOST_PORTS.iter().copied().collect();
        for listener in &facts.listeners {
            if listens_on(listener.address, *ip) {
                ports.insert(listener.port);
            }
        }
        ports.remove(&DNS);
        for port in ports {
            let mut target = Target::blocked(ip.to_string(), port);
            target.interface = interface.map(str::to_owned);
            blocked.push(target);
        }
    }

    let host_ips: Vec<IpAddr> = host.iter().map(|(ip, _)| *ip).collect();
    let others = facts.gateways.iter().copied().chain(
        SAMPLES
            .iter()
            .filter_map(|sample| sample.parse::<IpAddr>().ok()),
    );
    for ip in others {
        // Another address on a bridge is a container's, maybe a ws-*
        // one, and the probe never connects to those.
        if host_ips.contains(&ip) || bridges.iter().any(|bridge| bridge.contains(ip)) {
            continue;
        }
        for port in SAMPLE_PORTS {
            blocked.push(Target::blocked(ip.to_string(), *port));
        }
    }

    let mut seen = BTreeSet::new();
    blocked.retain(|target| seen.insert(target.label()));
    blocked.truncate(MAX_TARGETS);
    blocked.extend(
        REQUIRED
            .iter()
            .map(|(host, port)| Target::required(*host, *port)),
    );
    Targets {
        timeout_ms: TIMEOUT_MS,
        targets: blocked,
    }
}

/// True when a socket bound to `bound` takes connections to `ip`. A
/// socket on `::` takes IPv4 too unless it's IPv6 only, which `/proc`
/// doesn't say, so the check assumes it does.
fn listens_on(bound: IpAddr, ip: IpAddr) -> bool {
    let bound = match bound {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(bound, IpAddr::V4),
        v4 => v4,
    };
    if bound.is_loopback() {
        return false;
    }
    match bound {
        IpAddr::V4(v4) if v4.is_unspecified() => ip.is_ipv4(),
        IpAddr::V6(v6) if v6.is_unspecified() => true,
        _ => bound == ip,
    }
}

/// What the probe's answers mean.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Verdict {
    /// Blocked targets that answered.
    pub reached: Vec<String>,
    /// Required targets that didn't connect.
    pub missing: Vec<String>,
    /// Targets the report doesn't mention.
    pub unchecked: Vec<String>,
}

/// Reads the probe's report against the targets it was given.
pub fn judge(targets: &Targets, report: &Report) -> Verdict {
    let mut verdict = Verdict::default();
    for target in &targets.targets {
        let label = target.label();
        let Some(tried) = report.results.iter().find(|tried| tried.target == label) else {
            verdict.unchecked.push(label);
            continue;
        };
        if target.required {
            if tried.outcome != super::probe::Outcome::Connected {
                verdict.missing.push(label);
            }
        } else if tried.outcome.answered() {
            verdict.reached.push(label);
        }
    }
    verdict
}

impl Verdict {
    /// True when nothing blocked answered and everything required did.
    pub fn passed(&self) -> bool {
        self.reached.is_empty() && self.missing.is_empty() && self.unchecked.is_empty()
    }

    /// The check to show, and a note when it failed with nothing reached.
    pub fn check(&self, at: Timestamp) -> (EgressCheck, Option<String>) {
        if self.passed() {
            return (EgressCheck::Passed { at }, None);
        }
        let mut notes = Vec::new();
        if !self.missing.is_empty() {
            notes.push(format!("{} didn't connect", self.missing.join(" and ")));
        }
        match self.unchecked.len() {
            0 => {}
            1 => notes.push(format!("the probe skipped {}", self.unchecked[0])),
            n => notes.push(format!("the probe skipped {n} targets")),
        }
        let note = (!notes.is_empty()).then(|| notes.join(", and "));
        (
            EgressCheck::Failed {
                at,
                reached: self.reached.clone(),
            },
            note,
        )
    }
}

/// W-7's ranges and the host's addresses as an Incus network ACL that
/// drops them, letting DNS through to the bridge's gateway. M3 applies no
/// ACL, per open question 2, so only tests call this.
#[cfg_attr(not(test), allow(dead_code))]
pub fn acl(name: &str, gateway: Option<IpAddr>, host: &[IpAddr]) -> Value {
    let mut egress = Vec::new();
    if let Some(gateway) = gateway {
        for protocol in ["udp", "tcp"] {
            egress.push(json!({
                "action": "allow",
                "state": "enabled",
                "protocol": protocol,
                "destination": host_net(gateway),
                "destination_port": "53",
                "description": "DNS through the bridge",
            }));
        }
    }
    for range in BLOCKED {
        egress.push(json!({
            "action": "drop",
            "state": "enabled",
            "destination": range,
            "description": "No private networks",
        }));
    }
    let mut seen = BTreeSet::new();
    for ip in host {
        if is_blocked(*ip) || !seen.insert(*ip) {
            continue;
        }
        egress.push(json!({
            "action": "drop",
            "state": "enabled",
            "destination": host_net(*ip),
            "description": "Not the host",
        }));
    }
    json!({
        "name": name,
        "description": "MaxHQ workspaces reach the internet and nothing private",
        "egress": egress,
        "ingress": [],
    })
}

#[cfg_attr(not(test), allow(dead_code))]
fn host_net(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(_) => format!("{ip}/32"),
        IpAddr::V6(_) => format!("{ip}/128"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::probe::{Outcome, Tried};
    use super::*;

    const FIB_TRIE: &str = "Main:
  +-- 0.0.0.0/0 3 0 5
     |-- 0.0.0.0
        /0 universe UNICAST
     +-- 10.100.2.0/24 2 0 2
        |-- 10.100.2.0
           /24 link UNICAST
        |-- 10.100.2.1
           /32 host LOCAL
     +-- 127.0.0.0/8 2 0 2
        |-- 127.0.0.1
           /32 host LOCAL
     |-- 100.101.102.103
        /32 host LOCAL
     |-- 192.168.1.20
        /32 host LOCAL
Local:
     |-- 192.168.1.20
        /32 host LOCAL
";

    const IF_INET6: &str = "\
00000000000000000000000000000001 01 80 10 80       lo
fd7a115ca1e000000000000000000001 05 80 00 80 tailscale0
fe800000000000000000000000000001 07 40 20 80 agentbr0
fe800000000000000000000000000002 02 40 20 80 enp3s0
";

    const ROUTE: &str = "\
Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT
enp3s0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0
enp3s0\t0001A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0
";

    const IPV6_ROUTE: &str = "\
00000000000000000000000000000000 00 00000000000000000000000000000000 00 fe800000000000000000000000000009 00000400 00000001 00000000 00000003 enp3s0
20010db8000000000000000000000000 30 00000000000000000000000000000000 00 20010db8000000000000000000000001 00000400 00000001 00000000 00000003 enp3s0
";

    const TCP: &str = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1
   1: 0100007F:0CEA 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 2
   2: 1401A8C0:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 3
   3: 1401A8C0:0016 0201A8C0:C350 01 00000000:00000000 00:00000000 00000000     0        0 4
   4: 0102640A:0035 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 5
";

    const TCP6: &str = "\
  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000000000000:1F40 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 6
   1: 00000000000000000000000001000000:0277 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 7
";

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn facts() -> HostFacts {
        let mut listeners = parse_tcp4(TCP);
        listeners.extend(parse_tcp6(TCP6));
        let mut gateways = parse_route(ROUTE);
        gateways.extend(parse_ipv6_route(IPV6_ROUTE));
        HostFacts {
            v4: parse_fib_trie(FIB_TRIE),
            v6: parse_if_inet6(IF_INET6),
            gateways,
            listeners,
        }
    }

    fn bridges() -> Vec<Bridge> {
        vec![
            Bridge {
                name: "agentbr0".to_owned(),
                v4: Net::parse("10.100.2.1/24"),
                v6: None,
            },
            Bridge {
                name: "incusbr0".to_owned(),
                v4: Net::parse("10.100.0.1/24"),
                v6: None,
            },
        ]
    }

    #[test]
    fn ranges_hold_what_w7_blocks() {
        for blocked in [
            "10.1.2.3",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "100.100.100.100",
            "fd7a:115c:a1e0::53",
            "fe80::1",
            "0.1.2.3",
            "192.0.0.170",
            "198.18.0.1",
            "198.19.255.255",
            "224.0.0.251",
            "239.255.255.250",
            "240.0.0.1",
            "255.255.255.255",
        ] {
            assert!(is_blocked(ip(blocked)), "{blocked}");
        }
        for open in [
            "8.8.8.8",
            "1.0.0.1",
            "172.32.0.1",
            "100.128.0.1",
            "192.0.1.1",
            "198.17.255.255",
            "198.20.0.1",
            "223.255.255.255",
            "2001:db8::1",
        ] {
            assert!(!is_blocked(ip(open)), "{open}");
        }
        assert_eq!(Net::parse("10.0.0.0/33"), None);
        assert!(Net::parse("0.0.0.0/0").unwrap().contains(ip("1.2.3.4")));
    }

    /// The host's firewall loads these into an interval set, which refuses
    /// ranges that overlap.
    #[test]
    fn no_blocked_range_overlaps_another() {
        let nets: Vec<Net> = BLOCKED
            .iter()
            .map(|range| Net::parse(range).unwrap())
            .collect();
        for (at, net) in nets.iter().enumerate() {
            for (other_at, other) in nets.iter().enumerate() {
                assert!(
                    at == other_at || !net.contains(other.addr),
                    "{net:?} holds {other:?}"
                );
            }
        }
    }

    #[test]
    fn reads_the_host_from_proc() {
        let facts = facts();
        assert_eq!(
            facts.v4,
            [ip("10.100.2.1"), ip("100.101.102.103"), ip("192.168.1.20")]
                .map(|ip| match ip {
                    IpAddr::V4(v4) => v4,
                    IpAddr::V6(_) => unreachable!(),
                })
                .to_vec()
        );
        let v6: Vec<_> = facts
            .v6
            .iter()
            .map(|v6| (v6.address.to_string(), v6.interface.as_str()))
            .collect();
        assert_eq!(
            v6,
            [
                ("fd7a:115c:a1e0::1".to_owned(), "tailscale0"),
                ("fe80::1".to_owned(), "agentbr0"),
                ("fe80::2".to_owned(), "enp3s0"),
            ]
        );
        assert_eq!(facts.gateways, [ip("192.168.1.1"), ip("2001:db8::1")]);
        assert_eq!(
            facts.listeners,
            [
                Listener {
                    address: ip("0.0.0.0"),
                    port: 22
                },
                Listener {
                    address: ip("127.0.0.1"),
                    port: 3306
                },
                Listener {
                    address: ip("192.168.1.20"),
                    port: 8080
                },
                Listener {
                    address: ip("10.100.2.1"),
                    port: 53
                },
                Listener {
                    address: ip("::"),
                    port: 8000
                },
                Listener {
                    address: ip("::1"),
                    port: 631
                },
            ]
        );
    }

    #[test]
    fn targets_cover_the_host_and_the_ranges() {
        let targets = targets(&facts(), &bridges(), "agentbr0");
        let labels: Vec<String> = targets.targets.iter().map(Target::label).collect();
        let has = |label: &str| labels.iter().any(|l| l == label);

        // The host's addresses, with every port it listens on there.
        for label in [
            "10.100.2.1:22",
            "10.100.2.1:8000",
            "10.100.0.1:22",
            "100.101.102.103:22",
            "192.168.1.20:22",
            "192.168.1.20:8080",
            "192.168.1.20:8000",
            "[fd7a:115c:a1e0::1]:22",
            "[fd7a:115c:a1e0::1]:8000",
            "[fe80::1%eth0]:8000",
        ] {
            assert!(has(label), "{label} in {labels:?}");
        }
        // Loopback sockets aren't reachable, the guard lets DNS through,
        // and a link-local address on another link isn't on the path.
        assert!(!has("127.0.0.1:3306"));
        assert!(!has("10.100.2.1:53"));
        assert!(!labels.iter().any(|l| l.contains(":631")));
        assert!(!labels.iter().any(|l| l.contains("fe80::2")));
        // The LAN gateway and the samples, on the sample ports.
        for label in [
            "192.168.1.1:22",
            "192.168.1.1:443",
            "[2001:db8::1]:80",
            "100.100.100.100:53",
            "169.254.169.254:80",
            "192.0.0.1:443",
            "198.18.0.1:443",
            "240.0.0.1:443",
            "[fd00::1]:22",
        ] {
            assert!(has(label), "{label}");
        }
        // The required targets come last.
        let required: Vec<_> = targets.targets.iter().filter(|t| t.required).collect();
        assert_eq!(required.len(), 2);
        assert_eq!(
            labels[labels.len() - 2..],
            ["github.com:443", "deb.debian.org:443"]
        );
        // Each target once.
        let unique: BTreeSet<_> = labels.iter().collect();
        assert_eq!(unique.len(), labels.len());
    }

    #[test]
    fn targets_never_include_another_container() {
        let mut facts = facts();
        // A route through a container and a sample inside a bridge.
        facts.gateways.push(ip("10.100.2.57"));
        facts.gateways.push(ip("10.100.0.9"));
        let mut bridges = bridges();
        bridges.push(Bridge {
            name: "incusbr-lab".to_owned(),
            v4: Net::parse("10.0.0.254/24"),
            v6: Net::parse("fd00::fe/64"),
        });
        let targets = targets(&facts, &bridges, "agentbr0");
        for target in &targets.targets {
            let Ok(addr) = target.host.parse::<IpAddr>() else {
                assert!(target.required);
                continue;
            };
            let on_bridge = bridges.iter().any(|b| b.contains(addr));
            let is_gateway = bridges
                .iter()
                .any(|b| [b.v4, b.v6].into_iter().flatten().any(|n| n.addr == addr));
            assert!(
                !on_bridge || is_gateway,
                "{} is on a bridge and isn't the host",
                target.label()
            );
        }
        let labels: Vec<String> = targets.targets.iter().map(Target::label).collect();
        assert!(labels.iter().any(|l| l == "10.0.0.254:22"));
        assert!(!labels.iter().any(|l| l.starts_with("10.0.0.1:")));
        assert!(!labels.iter().any(|l| l.starts_with("[fd00::1]")));
    }

    fn tried(target: &str, outcome: Outcome) -> Tried {
        Tried {
            target: target.to_owned(),
            outcome,
            detail: None,
        }
    }

    #[test]
    fn judges_the_answers() {
        let targets = Targets {
            timeout_ms: 1,
            targets: vec![
                Target::blocked("10.100.2.1", 22),
                Target::blocked("10.0.0.1", 80),
                Target::blocked("192.168.1.1", 443),
                Target::required("github.com", 443),
                Target::required("deb.debian.org", 443),
            ],
        };
        let clean = Report {
            results: vec![
                tried("10.100.2.1:22", Outcome::TimedOut),
                tried("10.0.0.1:80", Outcome::Unreachable),
                tried("192.168.1.1:443", Outcome::TimedOut),
                tried("github.com:443", Outcome::Connected),
                tried("deb.debian.org:443", Outcome::Connected),
            ],
        };
        let verdict = judge(&targets, &clean);
        assert!(verdict.passed());
        assert_eq!(
            verdict.check(Timestamp(5)),
            (EgressCheck::Passed { at: Timestamp(5) }, None)
        );

        let mut leaky = clean.clone();
        leaky.results[0].outcome = Outcome::Refused;
        leaky.results[2].outcome = Outcome::Connected;
        leaky.results[4].outcome = Outcome::TimedOut;
        leaky.results.remove(1);
        let verdict = judge(&targets, &leaky);
        assert_eq!(verdict.reached, ["10.100.2.1:22", "192.168.1.1:443"]);
        assert_eq!(verdict.missing, ["deb.debian.org:443"]);
        assert_eq!(verdict.unchecked, ["10.0.0.1:80"]);
        let (check, note) = verdict.check(Timestamp(5));
        assert_eq!(
            check,
            EgressCheck::Failed {
                at: Timestamp(5),
                reached: vec!["10.100.2.1:22".to_owned(), "192.168.1.1:443".to_owned()],
            }
        );
        let note = note.unwrap();
        assert!(note.contains("deb.debian.org:443 didn't connect"), "{note}");
    }

    #[test]
    fn builds_an_acl_from_the_ranges() {
        let acl = acl(
            "maxhq-egress",
            Some(ip("10.100.2.1")),
            &[
                ip("10.100.2.1"),
                ip("203.0.113.7"),
                ip("203.0.113.7"),
                ip("2001:db8::5"),
            ],
        );
        assert_eq!(acl["name"], "maxhq-egress");
        let rules = acl["egress"].as_array().unwrap();
        let allows: Vec<_> = rules.iter().filter(|r| r["action"] == "allow").collect();
        assert_eq!(allows.len(), 2);
        assert!(allows
            .iter()
            .all(|r| r["destination"] == "10.100.2.1/32" && r["destination_port"] == "53"));
        let drops: Vec<&str> = rules
            .iter()
            .filter(|r| r["action"] == "drop")
            .map(|r| r["destination"].as_str().unwrap())
            .collect();
        let mut expected: Vec<&str> = BLOCKED.to_vec();
        expected.extend(["203.0.113.7/32", "2001:db8::5/128"]);
        assert_eq!(drops, expected);
        assert_eq!(acl["ingress"], json!([]));
    }
}
