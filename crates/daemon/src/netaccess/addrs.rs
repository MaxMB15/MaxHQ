// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The addresses network access listens on: this device's private ones,
//! never a public or loopback address, and never every address at once.

use std::net::IpAddr;

use maxhq_os::net::addresses as interfaces;

/// Whether `ip` is private: in a home or office network (10/8,
/// 172.16/12, 192.168/16, fc00::/7) or a tailnet (100.64/10). Loopback,
/// link-local and public addresses aren't.
pub fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, ..] = ip.octets();
            ip.is_private() || (a == 100 && (b & 0xc0) == 64)
        }
        IpAddr::V6(ip) => (ip.segments()[0] & 0xfe00) == 0xfc00,
    }
}

/// This device's private addresses now, each once.
pub fn private() -> Vec<IpAddr> {
    let mut found: Vec<IpAddr> = match interfaces() {
        Ok(found) => found.into_iter().filter(|ip| is_private(*ip)).collect(),
        Err(err) => {
            tracing::warn!(%err, "couldn't list this device's addresses");
            Vec::new()
        }
    };
    found.sort_unstable();
    found.dedup();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn private(text: &str) -> bool {
        is_private(text.parse().unwrap())
    }

    #[test]
    fn only_private_and_tailnet_addresses_are_private() {
        for yes in [
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.20",
            "100.64.0.1",
            "100.127.255.255",
            "fd7a:115c:a1e0::1",
            "fc00::1",
        ] {
            assert!(private(yes), "{yes}");
        }
        for no in [
            "127.0.0.1",
            "0.0.0.0",
            "8.8.8.8",
            "172.32.0.1",
            "100.63.255.255",
            "100.128.0.1",
            "169.254.1.1",
            "::1",
            "::",
            "fe80::1",
            "2001:db8::1",
            "::ffff:192.168.1.20",
        ] {
            assert!(!private(no), "{no}");
        }
    }

    #[test]
    fn this_device_lists_its_addresses() {
        // Every machine has a loopback address, and none of what's listed
        // as private is loopback.
        assert!(interfaces().unwrap().iter().any(IpAddr::is_loopback));
        assert!(super::private().iter().all(|ip| !ip.is_loopback()));
    }
}
