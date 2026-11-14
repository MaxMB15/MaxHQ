// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Holds a port back until [`SHOW_AFTER`] polls in a row have seen it, so
//! a socket that opens and closes between two polls never shows.

use std::collections::HashMap;

use maxhq_protocol::guest::GuestPort;

use super::SHOW_AFTER;

/// The ports seen in recent polls, by port number.
#[derive(Default)]
pub struct Debounce {
    seen: HashMap<u16, u32>,
}

impl Debounce {
    /// Starts with nothing seen.
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes one poll and gives the ports to show, as the poll found them.
    /// A port missing from a poll starts over.
    pub fn poll(&mut self, ports: Vec<GuestPort>) -> Vec<GuestPort> {
        let mut seen = HashMap::with_capacity(ports.len());
        let mut shown = Vec::new();
        for port in ports {
            if seen.contains_key(&port.port) {
                continue;
            }
            let count = self
                .seen
                .get(&port.port)
                .map_or(1, |count| count.saturating_add(1));
            seen.insert(port.port, count);
            if count >= SHOW_AFTER {
                shown.push(port);
            }
        }
        self.seen = seen;
        shown
    }
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::model::BindAddress;

    use super::*;

    fn ports(numbers: &[u16]) -> Vec<GuestPort> {
        numbers
            .iter()
            .map(|port| GuestPort {
                port: *port,
                bind: BindAddress::Loopback,
                session: None,
                process: None,
            })
            .collect()
    }

    fn numbers(ports: &[GuestPort]) -> Vec<u16> {
        ports.iter().map(|port| port.port).collect()
    }

    #[test]
    fn a_port_shows_after_two_polls_in_a_row() {
        let mut debounce = Debounce::new();
        assert!(debounce.poll(ports(&[3000])).is_empty());
        assert_eq!(numbers(&debounce.poll(ports(&[3000, 5173]))), [3000]);
        assert_eq!(numbers(&debounce.poll(ports(&[3000, 5173]))), [3000, 5173]);
    }

    #[test]
    fn a_missed_poll_starts_over() {
        let mut debounce = Debounce::new();
        debounce.poll(ports(&[3000]));
        debounce.poll(ports(&[]));
        assert!(debounce.poll(ports(&[3000])).is_empty());
        assert_eq!(numbers(&debounce.poll(ports(&[3000]))), [3000]);
        assert!(debounce.poll(ports(&[])).is_empty());
    }

    #[test]
    fn the_latest_poll_gives_the_details() {
        let mut debounce = Debounce::new();
        debounce.poll(ports(&[3000]));
        let mut wide = ports(&[3000]);
        wide[0].bind = BindAddress::Any;
        assert_eq!(debounce.poll(wide)[0].bind, BindAddress::Any);
    }
}
