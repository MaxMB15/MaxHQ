// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! This device's network addresses: `getifaddrs` on Unix, and
//! `GetAdaptersAddresses` on Windows.

use std::io;
use std::net::IpAddr;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

/// Every address of every interface, loopback and link-local ones too, as
/// the system lists them. An address on two interfaces is there twice.
pub fn addresses() -> io::Result<Vec<IpAddr>> {
    imp::addresses()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_device_lists_its_loopback_address() {
        assert!(addresses().unwrap().iter().any(IpAddr::is_loopback));
    }
}
