// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The addresses network access listens on: this device's private ones,
//! never a public or loopback address, and never every address at once.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

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

/// Every address of every interface, as `getifaddrs` lists them.
#[allow(unsafe_code)]
fn interfaces() -> io::Result<Vec<IpAddr>> {
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: `head` is a valid place for the list, which is freed below.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut found = Vec::new();
    let mut at = head;
    while !at.is_null() {
        // SAFETY: `at` is an entry of the list, which lives until it's
        // freed, and each address points at a sockaddr of its family.
        let entry = unsafe { &*at };
        if let Some(address) = unsafe { entry.ifa_addr.as_ref() } {
            match i32::from(address.sa_family) {
                libc::AF_INET => {
                    let v4 = unsafe { &*entry.ifa_addr.cast::<libc::sockaddr_in>() };
                    found.push(IpAddr::V4(Ipv4Addr::from(u32::from_be(v4.sin_addr.s_addr))));
                }
                libc::AF_INET6 => {
                    let v6 = unsafe { &*entry.ifa_addr.cast::<libc::sockaddr_in6>() };
                    found.push(IpAddr::V6(Ipv6Addr::from(v6.sin6_addr.s6_addr)));
                }
                _ => {}
            }
        }
        at = entry.ifa_next;
    }
    // SAFETY: `head` came from `getifaddrs`, and nothing above outlives it.
    unsafe { libc::freeifaddrs(head) };
    Ok(found)
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
