// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A key for the network this machine is on, so a device with several
//! routes tries first the one that last worked here (decision N-7).
//!
//! The key hashes the default gateway's IP and MAC address, from
//! `route -n get default` and `arp -n`. Neither needs location access,
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
    use tokio::process::Command;

    /// How long a key is reused before the gateway is read again.
    const KEEP: Duration = Duration::from_secs(10);

    /// How long each command may take.
    const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

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
        let route = run("route", &["-n", "get", "default"]).await?;
        let gateway = gateway(&route)?;
        let arp = run("arp", &["-n", &gateway]).await.unwrap_or_default();
        Some(key(&gateway, mac(&arp, &gateway).as_deref()))
    }

    /// Runs a command and returns what it wrote, or none if it failed.
    async fn run(program: &str, args: &[&str]) -> Option<String> {
        let output = tokio::time::timeout(
            COMMAND_TIMEOUT,
            Command::new(program).args(args).kill_on_drop(true).output(),
        )
        .await
        .ok()?
        .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// The gateway IP from `route -n get default`.
    fn gateway(route: &str) -> Option<String> {
        route.lines().find_map(|line| {
            let value = line.trim().strip_prefix("gateway:")?.trim();
            (!value.is_empty()).then(|| value.to_owned())
        })
    }

    /// The MAC address `arp -n <ip>` gives for `ip`, in lower case.
    fn mac(arp: &str, ip: &str) -> Option<String> {
        let wanted = format!("({ip})");
        arp.lines().find_map(|line| {
            let mut words = line.split_whitespace();
            words.find(|word| *word == wanted)?;
            let mac = words.skip_while(|word| *word != "at").nth(1)?;
            mac.contains(':').then(|| mac.to_ascii_lowercase())
        })
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

        const ROUTE: &str = "   route to: default
    destination: default
           mask: default
        gateway: 192.168.1.1
      interface: en0
          flags: <UP,GATEWAY,DONE,STATIC,PRCLONING,GLOBAL>
     recvpipe  sendpipe  ssthresh  rtt,msec    rttvar  hopcount      mtu     expire
           0         0         0         0         0         0      1500         0
    ";

        const ARP: &str = "? (192.168.1.1) at a4:2b:B0:12:34:56 on en0 ifscope [ethernet]\n";

        #[test]
        fn the_gateway_comes_from_the_default_route() {
            assert_eq!(gateway(ROUTE).as_deref(), Some("192.168.1.1"));
            assert_eq!(
                gateway("route: writing to routing socket: not in table\n"),
                None
            );
        }

        #[test]
        fn the_mac_comes_from_the_gateways_arp_line() {
            assert_eq!(
                mac(ARP, "192.168.1.1").as_deref(),
                Some("a4:2b:b0:12:34:56")
            );
            assert_eq!(mac(ARP, "192.168.1.10"), None);
            let incomplete = "? (192.168.1.1) at (incomplete) on en0 ifscope [ethernet]\n";
            assert_eq!(mac(incomplete, "192.168.1.1"), None);
        }

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
