// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_NO_DATA, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
    IP_ADAPTER_ADDRESSES_LH,
};
use windows_sys::Win32::Networking::WinSock::{
    AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_IN, SOCKADDR_IN6,
};

/// How many times the list is asked for when it grows between the call
/// that sizes it and the one that fills it.
const TRIES: usize = 4;

/// The first size tried, which Microsoft suggests as enough for most.
const FIRST_SIZE: u32 = 15 * 1024;

/// Every unicast address of every adapter, as `GetAdaptersAddresses`
/// lists them.
#[allow(unsafe_code)]
pub fn addresses() -> io::Result<Vec<IpAddr>> {
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size = FIRST_SIZE;
    for _ in 0..TRIES {
        // u64s, so the list is aligned as its structs need.
        let mut buffer = vec![0u64; usize::try_from(size).map_err(io::Error::other)?.div_ceil(8)];
        let head = buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        // SAFETY: `head` points at `size` bytes or more, which the call
        // fills, and `size` is a valid place for the size it needs.
        let code = unsafe {
            GetAdaptersAddresses(
                u32::from(AF_UNSPEC),
                flags,
                std::ptr::null(),
                head,
                &mut size,
            )
        };
        match code {
            NO_ERROR => {
                // SAFETY: the call filled the buffer with a list that
                // starts at `head`, and it lives as long as `buffer`.
                return Ok(unsafe { walk(head) });
            }
            // No adapter has an address.
            ERROR_NO_DATA => return Ok(Vec::new()),
            ERROR_BUFFER_OVERFLOW => continue,
            code => {
                return Err(io::Error::from_raw_os_error(
                    i32::try_from(code).unwrap_or(i32::MAX),
                ));
            }
        }
    }
    Err(io::Error::other(
        "the list of network adapters kept growing while it was read",
    ))
}

/// The unicast addresses in the adapter list at `head`.
///
/// # Safety
///
/// `head` is null or the start of a list `GetAdaptersAddresses` filled,
/// which lives while this runs.
#[allow(unsafe_code)]
unsafe fn walk(head: *const IP_ADAPTER_ADDRESSES_LH) -> Vec<IpAddr> {
    let mut found = Vec::new();
    let mut adapter = head;
    // SAFETY: each pointer is null or an entry of the list, and each
    // address points at a sockaddr of its family.
    unsafe {
        while let Some(entry) = adapter.as_ref() {
            let mut unicast = entry.FirstUnicastAddress.cast_const();
            while let Some(address) = unicast.as_ref() {
                let sockaddr = address.Address.lpSockaddr.cast_const();
                if let Some(family) = sockaddr.as_ref().map(|sockaddr| sockaddr.sa_family) {
                    if family == AF_INET {
                        let v4 = &*sockaddr.cast::<SOCKADDR_IN>();
                        found.push(IpAddr::V4(Ipv4Addr::from(u32::from_be(
                            v4.sin_addr.S_un.S_addr,
                        ))));
                    } else if family == AF_INET6 {
                        let v6 = &*sockaddr.cast::<SOCKADDR_IN6>();
                        found.push(IpAddr::V6(Ipv6Addr::from(v6.sin6_addr.u.Byte)));
                    }
                }
                unicast = address.Next.cast_const();
            }
            adapter = entry.Next.cast_const();
        }
    }
    found
}
