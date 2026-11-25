// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Every address of every interface, as `getifaddrs` lists them.
#[allow(unsafe_code)]
pub fn addresses() -> io::Result<Vec<IpAddr>> {
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
