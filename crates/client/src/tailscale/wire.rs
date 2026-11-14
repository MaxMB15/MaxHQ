// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The parts of Tailscale's LocalAPI JSON the watcher reads. Go writes nil
//! slices and pointers as `null`, so every field reads `null` as its
//! default, and fields the watcher doesn't name are skipped.

use std::collections::BTreeMap;

use maxhq_protocol::time::Timestamp;
use serde::{Deserialize, Deserializer};

/// Reads `null` as the type's default.
fn nullable<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// `/localapi/v0/status`, and the `InitialStatus` the IPN bus starts with.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct Status {
    /// The client's version, such as `1.102.3-t1234abcd-g5678`.
    #[serde(rename = "Version", default, deserialize_with = "nullable")]
    pub version: String,
    /// Such as `Running`, `Stopped` or `NeedsLogin`.
    #[serde(rename = "BackendState", default, deserialize_with = "nullable")]
    pub backend_state: String,
    /// The MagicDNS suffix, on versions that put it at the top level.
    #[serde(rename = "MagicDNSSuffix", default, deserialize_with = "nullable")]
    pub magic_dns_suffix: String,
    /// The tailnet this machine is logged in to.
    #[serde(rename = "CurrentTailnet", default)]
    pub current_tailnet: Option<CurrentTailnet>,
    /// Every peer, keyed by node key.
    #[serde(rename = "Peer", default, deserialize_with = "nullable")]
    pub peer: BTreeMap<String, PeerStatus>,
    /// This machine, as its peers see it.
    #[serde(rename = "Self", default)]
    pub own: Option<PeerStatus>,
    /// The names the tailnet gets HTTPS certificates for. Empty while HTTPS
    /// is off.
    #[serde(rename = "CertDomains", default, deserialize_with = "nullable")]
    pub cert_domains: Vec<String>,
}

/// The tailnet in a [`Status`].
#[derive(Debug, Default, Deserialize)]
pub(crate) struct CurrentTailnet {
    /// The tailnet's name.
    #[serde(rename = "Name", default, deserialize_with = "nullable")]
    pub name: String,
    /// The suffix MagicDNS names end in.
    #[serde(rename = "MagicDNSSuffix", default, deserialize_with = "nullable")]
    pub magic_dns_suffix: String,
}

/// One peer in a [`Status`].
#[derive(Debug, Default, Deserialize)]
pub(crate) struct PeerStatus {
    /// The stable node ID.
    #[serde(rename = "ID", default, deserialize_with = "nullable")]
    pub id: String,
    /// The numeric node ID that bus patches use.
    #[serde(rename = "NodeID", default, deserialize_with = "nullable")]
    pub node_id: i64,
    /// The machine's own host name, which needn't be unique.
    #[serde(rename = "HostName", default, deserialize_with = "nullable")]
    pub host_name: String,
    /// The MagicDNS name, with a trailing dot.
    #[serde(rename = "DNSName", default, deserialize_with = "nullable")]
    pub dns_name: String,
    /// The peer's tailnet addresses.
    #[serde(rename = "TailscaleIPs", default, deserialize_with = "nullable")]
    pub tailscale_ips: Vec<String>,
    /// The direct address in use, or empty.
    #[serde(rename = "CurAddr", default, deserialize_with = "nullable")]
    pub cur_addr: String,
    /// The peer's home DERP region.
    #[serde(rename = "Relay", default, deserialize_with = "nullable")]
    pub relay: String,
    /// Whether traffic has flowed lately.
    #[serde(rename = "Active", default, deserialize_with = "nullable")]
    pub active: bool,
    /// Whether the coordinator sees the peer online.
    #[serde(rename = "Online", default, deserialize_with = "nullable")]
    pub online: bool,
    /// When the peer was last online. The zero time while it's online.
    #[serde(rename = "LastSeen", default, deserialize_with = "nullable")]
    pub last_seen: String,
    /// When the peer's key expires. Missing when expiry is off.
    #[serde(rename = "KeyExpiry", default, deserialize_with = "nullable")]
    pub key_expiry: String,
    /// The node's capabilities and attributes, such as `funnel`. Only this
    /// machine's are sent.
    #[serde(rename = "CapMap", default, deserialize_with = "nullable")]
    pub cap_map: BTreeMap<String, serde::de::IgnoredAny>,
}

/// One message from `/localapi/v0/watch-ipn-bus`.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct Notify {
    /// The backend state as a number, when it changed.
    #[serde(rename = "State", default)]
    pub state: Option<i64>,
    /// The whole status, in the first message.
    #[serde(rename = "InitialStatus", default)]
    pub initial_status: Option<Status>,
    /// Small changes to peers.
    #[serde(rename = "PeerChangedPatch", default, deserialize_with = "nullable")]
    pub peer_changed_patch: Vec<PeerChange>,
    /// Peers that changed in full.
    #[serde(rename = "PeersChanged", default, deserialize_with = "nullable")]
    pub peers_changed: Vec<Node>,
    /// Numeric node IDs of peers that left.
    #[serde(rename = "PeersRemoved", default, deserialize_with = "nullable")]
    pub peers_removed: Vec<i64>,
}

impl Notify {
    /// Whether the message says nothing the watcher uses, such as the
    /// engine stats that arrive about once a second.
    pub fn is_empty(&self) -> bool {
        self.state.is_none()
            && self.initial_status.is_none()
            && self.peer_changed_patch.is_empty()
            && self.peers_changed.is_empty()
            && self.peers_removed.is_empty()
    }
}

/// A patch to one peer. Unset fields didn't change.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct PeerChange {
    /// The peer's numeric node ID.
    #[serde(rename = "NodeID", default, deserialize_with = "nullable")]
    pub node_id: i64,
    /// Whether it's online now.
    #[serde(rename = "Online", default)]
    pub online: Option<bool>,
    /// When it was last online.
    #[serde(rename = "LastSeen", default)]
    pub last_seen: Option<String>,
    /// When its key expires.
    #[serde(rename = "KeyExpiry", default)]
    pub key_expiry: Option<String>,
}

/// A peer as the control plane describes it, in `PeersChanged`.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct Node {
    /// The numeric node ID.
    #[serde(rename = "ID", default, deserialize_with = "nullable")]
    pub id: i64,
    /// The stable node ID.
    #[serde(rename = "StableID", default, deserialize_with = "nullable")]
    pub stable_id: String,
    /// The MagicDNS name, with a trailing dot.
    #[serde(rename = "Name", default, deserialize_with = "nullable")]
    pub name: String,
    /// When the key expires, or the zero time when it doesn't.
    #[serde(rename = "KeyExpiry", default, deserialize_with = "nullable")]
    pub key_expiry: String,
    /// Tailnet addresses as prefixes, such as `100.101.1.2/32`.
    #[serde(rename = "Addresses", default, deserialize_with = "nullable")]
    pub addresses: Vec<String>,
    /// What the machine says about itself.
    #[serde(rename = "Hostinfo", default, deserialize_with = "nullable")]
    pub hostinfo: Hostinfo,
    /// Whether it's online, if the control plane said.
    #[serde(rename = "Online", default)]
    pub online: Option<bool>,
    /// When it was last online, if the control plane said.
    #[serde(rename = "LastSeen", default)]
    pub last_seen: Option<String>,
}

/// The part of a node's host info the watcher reads.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct Hostinfo {
    /// The machine's own host name.
    #[serde(rename = "Hostname", default, deserialize_with = "nullable")]
    pub hostname: String,
}

/// Reads an RFC 3339 time as Go writes it, such as
/// `2026-10-25T13:02:11.123456789Z`. Go's zero time, and anything before
/// 1970 or malformed, reads as none.
pub(crate) fn parse_time(text: &str) -> Option<Timestamp> {
    let bytes = text.as_bytes();
    if bytes.len() < 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't' | b' ')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<u32> {
        let digits = text.get(range)?;
        digits
            .bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| digits.parse().ok())?
    };
    let year = number(0..4)?;
    let (month, day) = (number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if year < 1970 || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let mut rest = &text[19..];
    let mut millis = 0u64;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        let first_three = format!("{:0<3}", &fraction[..digits.min(3)]);
        millis = first_three.parse().ok()?;
        rest = &fraction[digits..];
    }
    let offset_seconds: i64 = match rest.as_bytes() {
        [b'Z' | b'z'] => 0,
        [sign @ (b'+' | b'-'), h1, h2, b':', m1, m2] => {
            let hours = i64::from(digit(*h1)? * 10 + digit(*h2)?);
            let minutes = i64::from(digit(*m1)? * 10 + digit(*m2)?);
            let offset = hours * 3600 + minutes * 60;
            if *sign == b'+' {
                offset
            } else {
                -offset
            }
        }
        _ => return None,
    };

    let days = days_from_civil(i64::from(year), month, day);
    let seconds =
        days * 86_400 + i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second)
            - offset_seconds;
    let seconds = u64::try_from(seconds).ok()?;
    Some(Timestamp(seconds * 1000 + millis))
}

/// One ASCII digit's value.
fn digit(byte: u8) -> Option<u32> {
    byte.is_ascii_digit().then(|| u32::from(byte - b'0'))
}

/// Days from 1970-01-01 to a date in the proleptic Gregorian calendar, after
/// Howard Hinnant's `days_from_civil`.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month_from_march = i64::from((month + 9) % 12);
    let day_of_year = (153 * month_from_march + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_as_milliseconds() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(Timestamp(0)));
        assert_eq!(
            parse_time("2025-10-25T00:00:00Z"),
            Some(Timestamp(1_761_350_400_000))
        );
        assert_eq!(
            parse_time("2025-10-25T00:00:00.123456789Z"),
            Some(Timestamp(1_761_350_400_123))
        );
        assert_eq!(
            parse_time("2025-10-25T02:00:00.5+02:00"),
            Some(Timestamp(1_761_350_400_500))
        );
        assert_eq!(
            parse_time("2025-10-24T17:00:00-07:00"),
            Some(Timestamp(1_761_350_400_000))
        );
        assert_eq!(
            parse_time("2024-02-29T12:00:00Z"),
            Some(Timestamp(1_709_208_000_000))
        );
    }

    #[test]
    fn zero_and_broken_times_read_as_none() {
        assert_eq!(parse_time("0001-01-01T00:00:00Z"), None);
        assert_eq!(parse_time(""), None);
        assert_eq!(parse_time("2025-10-25"), None);
        assert_eq!(parse_time("2025-13-25T00:00:00Z"), None);
        assert_eq!(parse_time("2025-10-25T00:00:00"), None);
        assert_eq!(parse_time("2025-10-25T00:00:00.Z"), None);
        assert_eq!(parse_time("2025-1x-25T00:00:00Z"), None);
    }

    #[test]
    fn nulls_read_as_defaults() {
        let status: Status = serde_json::from_str(
            r#"{"Version":"1.102.3-t1","BackendState":"Running","CurrentTailnet":null,
                "Peer":{"nodekey:1":{"ID":"n1","NodeID":7,"HostName":"devbox",
                "DNSName":"devbox.tail1234.ts.net.","TailscaleIPs":null,"Online":true,
                "LastSeen":"0001-01-01T00:00:00Z","KeyExpiry":null,"Extra":{"x":1}}},
                "Health":null}"#,
        )
        .unwrap();
        let peer = &status.peer["nodekey:1"];
        assert_eq!(peer.node_id, 7);
        assert!(peer.tailscale_ips.is_empty());
        assert!(peer.key_expiry.is_empty());
        assert!(status.current_tailnet.is_none());

        let notify: Notify = serde_json::from_str(
            r#"{"Version":"1.102.3","State":null,"NetMap":null,"Engine":{"RBytes":1}}"#,
        )
        .unwrap();
        assert!(notify.is_empty());
    }
}
