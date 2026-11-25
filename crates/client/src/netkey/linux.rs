// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The default gateway on Linux, from the kernel's own tables in
//! `/proc/net/route` and `/proc/net/arp`. Reading two small files starts no
//! process and needs no tool that a minimal system might lack.

use std::net::Ipv4Addr;

/// The flag on a route that is up.
const RTF_UP: u32 = 0x1;

/// The flag on a route that goes through a gateway.
const RTF_GATEWAY: u32 = 0x2;

/// The default gateway's IP and MAC address, or none when there's no
/// default route through a gateway. A gateway the neighbour table doesn't
/// hold yet has no MAC.
pub(super) async fn gateway() -> Option<(String, Option<String>)> {
    let route = tokio::fs::read_to_string("/proc/net/route").await.ok()?;
    let (gateway, device) = gateway_in(&route)?;
    let arp = tokio::fs::read_to_string("/proc/net/arp")
        .await
        .unwrap_or_default();
    let mac = mac(&arp, &gateway, &device);
    Some((gateway, mac))
}

/// The default route's gateway IP and the interface it leaves through,
/// from `/proc/net/route`. With several default routes, as with both cable
/// and Wi-Fi up, the one with the lowest metric wins, as it does in the
/// kernel.
fn gateway_in(route: &str) -> Option<(String, String)> {
    route
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let [device, destination, gateway, flags, _refs, _uses, metric, mask, ..] = fields[..]
            else {
                return None;
            };
            let flags = u32::from_str_radix(flags, 16).ok()?;
            let default = destination == "00000000" && mask == "00000000";
            let through_gateway = flags & (RTF_UP | RTF_GATEWAY) == RTF_UP | RTF_GATEWAY;
            if !default || !through_gateway {
                return None;
            }
            let gateway = address(gateway)?;
            let metric: u32 = metric.parse().ok()?;
            Some((metric, gateway.to_string(), device.to_owned()))
        })
        .min_by_key(|(metric, ..)| *metric)
        .map(|(_, gateway, device)| (gateway, device))
}

/// An IPv4 address as `/proc/net/route` prints it: the address's four
/// bytes, in network order, read as one native-endian number and printed
/// in hex. So `0101A8C0` is 192.168.1.1 on a little-endian machine.
fn address(hex: &str) -> Option<Ipv4Addr> {
    let number = u32::from_str_radix(hex, 16).ok()?;
    Some(Ipv4Addr::from(number.to_ne_bytes()))
}

/// The MAC address `/proc/net/arp` gives for `ip` on `device`, in lower
/// case. An entry that is still being resolved shows all zeros, which
/// counts as none.
fn mac(arp: &str, ip: &str, device: &str) -> Option<String> {
    arp.lines().skip(1).find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [address, _kind, _flags, mac, _mask, on] = fields[..] else {
            return None;
        };
        let known = mac.contains(':') && mac.chars().any(|c| c != '0' && c != ':');
        (address == ip && on == device && known).then(|| mac.to_ascii_lowercase())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A laptop with cable and Wi-Fi both up, so two default routes, and a
    /// container bridge with no gateway.
    const ROUTE: &str = "\
Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT
wlp2s0\t00000000\t0101A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0
enp0s31f6\t00000000\t0100000A\t0003\t0\t0\t100\t00000000\t0\t0\t0
enp0s31f6\t0000000A\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0
wlp2s0\t0001A8C0\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0
docker0\t000011AC\t00000000\t0001\t0\t0\t0\t0000FFFF\t0\t0\t0
";

    const ARP: &str = "\
IP address       HW type     Flags       HW address            Mask     Device
192.168.1.1      0x1         0x2         a4:2b:B0:12:34:56     *        wlp2s0
10.0.0.1         0x1         0x2         00:11:22:33:44:55     *        enp0s31f6
10.0.0.1         0x1         0x2         66:77:88:99:aa:bb     *        docker0
192.168.1.20     0x1         0x0         00:00:00:00:00:00     *        wlp2s0
";

    #[test]
    fn addresses_are_read_in_network_order() {
        // The kernel prints the address's bytes as one native number, so
        // the fixture is built the same way on whatever runs the test.
        let hex = format!("{:08X}", u32::from_ne_bytes([192, 168, 1, 1]));
        assert_eq!(address(&hex), Some(Ipv4Addr::new(192, 168, 1, 1)));
        assert_eq!(address("not hex"), None);
    }

    #[cfg(target_endian = "little")]
    #[test]
    fn the_default_route_with_the_lowest_metric_wins() {
        assert_eq!(
            gateway_in(ROUTE),
            Some(("10.0.0.1".to_owned(), "enp0s31f6".to_owned()))
        );
        let wifi_only: String = ROUTE
            .lines()
            .filter(|line| !line.starts_with("enp0s31f6"))
            .map(|line| format!("{line}\n"))
            .collect();
        assert_eq!(
            gateway_in(&wifi_only),
            Some(("192.168.1.1".to_owned(), "wlp2s0".to_owned()))
        );
    }

    #[test]
    fn a_route_without_a_gateway_isnt_the_default() {
        let header = ROUTE.lines().next().unwrap();
        // A default route straight onto a link, as a point-to-point VPN
        // adds, has no gateway to key on.
        let direct =
            format!("{header}\ntun0\t00000000\t00000000\t0001\t0\t0\t50\t00000000\t0\t0\t0\n");
        assert_eq!(gateway_in(&direct), None);
        // A route that is down doesn't count either.
        let down =
            format!("{header}\neth0\t00000000\t0101A8C0\t0002\t0\t0\t50\t00000000\t0\t0\t0\n");
        assert_eq!(gateway_in(&down), None);
        assert_eq!(gateway_in(header), None);
        assert_eq!(gateway_in(""), None);
    }

    #[test]
    fn the_mac_comes_from_the_gateways_neighbour_entry() {
        assert_eq!(
            mac(ARP, "192.168.1.1", "wlp2s0").as_deref(),
            Some("a4:2b:b0:12:34:56")
        );
        assert_eq!(
            mac(ARP, "10.0.0.1", "enp0s31f6").as_deref(),
            Some("00:11:22:33:44:55"),
            "the same IP on another interface is another machine"
        );
        assert_eq!(mac(ARP, "192.168.1.20", "wlp2s0"), None, "still resolving");
        assert_eq!(mac(ARP, "192.168.1.10", "wlp2s0"), None);
        assert_eq!(mac("", "192.168.1.1", "wlp2s0"), None);
    }
}
