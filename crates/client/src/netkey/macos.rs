// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The default gateway on macOS, from `route -n get default` and `arp -n`.

use std::time::Duration;

use tokio::process::Command;

/// How long each command may take.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

/// The default gateway's IP and MAC address, or none when there's no
/// default route or `route` failed. A gateway `arp` doesn't know has no MAC.
pub(super) async fn gateway() -> Option<(String, Option<String>)> {
    let route = run("route", &["-n", "get", "default"]).await?;
    let gateway = gateway_in(&route)?;
    let arp = run("arp", &["-n", &gateway]).await.unwrap_or_default();
    let mac = mac(&arp, &gateway);
    Some((gateway, mac))
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
fn gateway_in(route: &str) -> Option<String> {
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
        assert_eq!(gateway_in(ROUTE).as_deref(), Some("192.168.1.1"));
        assert_eq!(
            gateway_in("route: writing to routing socket: not in table\n"),
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
}
