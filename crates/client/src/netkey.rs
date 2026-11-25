// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A key for the network this machine is on, so a device with several
//! routes tries first the one that last worked here (decision N-7).
//!
//! The key hashes the default gateway's IP and MAC address. macOS reads
//! them from `route -n get default` and `arp -n`, Linux from
//! `/proc/net/route` and `/proc/net/arp`. Neither needs location access,
//! unlike the Wi-Fi name. Two networks that hand out the same gateway IP
//! still differ by the gateway's MAC.
//!
//! Without the `system-ssh` feature, as on the phone, nothing is read
//! here: the caller names the network with [`crate::Client::set_network`].

use std::sync::Arc;

use tokio::sync::watch;

use crate::link::NetKey;

#[cfg(feature = "system-ssh")]
pub use gateway::current;

/// The key the caller last gave, read at each dial.
pub(crate) fn given(key: watch::Receiver<Option<String>>) -> NetKey {
    Arc::new(move || {
        let key = key.borrow().clone();
        Box::pin(async move { key })
    })
}

/// The key read from the default gateway.
#[cfg(feature = "system-ssh")]
mod gateway {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use sha2::{Digest, Sha256};

    /// How long a key is reused before the gateway is read again.
    const KEEP: Duration = Duration::from_secs(10);

    /// The last key read, and when.
    static CACHE: Mutex<Option<(Instant, Option<String>)>> = Mutex::new(None);

    /// The key for the network this machine is on now, or none when it has no
    /// default gateway or the commands failed. Reads are cached for 10 seconds.
    pub async fn current() -> Option<String> {
        if let Some((at, key)) = CACHE.lock().ok().and_then(|cache| cache.clone())
            && at.elapsed() < KEEP
        {
            return key;
        }
        let key = read().await;
        if let Ok(mut cache) = CACHE.lock() {
            *cache = Some((Instant::now(), key.clone()));
        }
        key
    }

    /// Reads the gateway and hashes it, with no cache.
    async fn read() -> Option<String> {
        let (gateway, mac) = system_gateway().await?;
        Some(key(&gateway, mac.as_deref()))
    }

    #[cfg(target_os = "macos")]
    use super::macos::gateway as system_gateway;

    #[cfg(target_os = "linux")]
    use super::linux::gateway as system_gateway;

    /// No reader for this system yet, so its devices try their routes in
    /// the usual order on every network.
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    async fn system_gateway() -> Option<(String, Option<String>)> {
        None
    }

    /// The key for a gateway: the first 16 hex characters of a sha256.
    fn key(gateway: &str, mac: Option<&str>) -> String {
        let mut hash = Sha256::new();
        hash.update(gateway.as_bytes());
        hash.update(b"\n");
        hash.update(mac.unwrap_or("").as_bytes());
        hash.finalize()
            .iter()
            .take(8)
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_same_gateway_ip_on_two_networks_gets_two_keys() {
            let home = key("192.168.1.1", Some("a4:2b:b0:12:34:56"));
            let cafe = key("192.168.1.1", Some("00:11:22:33:44:55"));
            assert_ne!(home, cafe);
            assert_eq!(home.len(), 16);
            assert_eq!(home, key("192.168.1.1", Some("a4:2b:b0:12:34:56")));
        }
    }
}

// Each system's reader gives the default gateway's IP and, when the system
// knows it, its MAC address. Both build in every test run, so their parsers
// are tested on any machine.
#[cfg(all(feature = "system-ssh", any(target_os = "macos", test)))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos;

#[cfg(all(feature = "system-ssh", any(target_os = "linux", test)))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod linux;
