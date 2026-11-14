// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A forward's local port: 127.0.0.1 and ::1 bound on one port number, so
//! `localhost` reaches it whichever address a browser tries first.
//!
//! The sockets don't set `SO_REUSEADDR`. On macOS a socket with it may bind
//! 127.0.0.1 on a port another program holds on 0.0.0.0, and then take
//! that program's local connections. Without it the bind fails, and the
//! forward moves to the next free port.

use std::io::{self, ErrorKind};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

use tokio::net::{TcpListener, TcpSocket};

/// How many ports after the wanted one are tried before the system picks.
const TRIES: u16 = 100;
/// How many ports the system picks before giving up, each one taken on
/// ::1 already.
const PICKS: usize = 16;
/// How many connections wait to be accepted.
const BACKLOG: u32 = 128;

/// The ports browsers refuse to load, from the Fetch standard's list of
/// bad ports, in order.
const REFUSED: &[u16] = &[
    1, 7, 9, 11, 13, 15, 17, 19, 20, 21, 22, 23, 25, 37, 42, 43, 53, 69, 77, 79, 87, 95, 101, 102,
    103, 104, 109, 110, 111, 113, 115, 117, 119, 123, 135, 137, 139, 143, 161, 179, 389, 427, 465,
    512, 513, 514, 515, 526, 530, 531, 532, 540, 548, 554, 556, 563, 587, 601, 636, 989, 990, 993,
    995, 1719, 1720, 1723, 2049, 3659, 4045, 4190, 5060, 5061, 6000, 6566, 6665, 6666, 6667, 6668,
    6669, 6679, 6697, 10080,
];

/// Whether browsers refuse to load `port`.
pub(super) fn refused_by_browsers(port: u16) -> bool {
    REFUSED.binary_search(&port).is_ok()
}

/// The listeners for one local port.
pub(super) struct Bound {
    /// The port both listen on.
    pub(super) port: u16,
    /// 127.0.0.1, then ::1 when this machine has it.
    pub(super) listeners: Vec<TcpListener>,
}

/// Binds 127.0.0.1 and ::1 on `wanted`, or the next free port above it
/// that browsers load, or failing those, one the system picks.
pub(super) fn bind(wanted: u16) -> io::Result<Bound> {
    let first = wanted.max(1);
    for port in (first..=first.saturating_add(TRIES)).filter(|port| !refused_by_browsers(*port)) {
        match pair(port) {
            Ok(bound) => return Ok(bound),
            Err(err) if taken(&err) => continue,
            Err(err) => return Err(err),
        }
    }
    let mut last = io::Error::new(ErrorKind::AddrInUse, "no free local port");
    for _ in 0..PICKS {
        match pair(0) {
            Ok(bound) => return Ok(bound),
            Err(err) if taken(&err) => last = err,
            Err(err) => return Err(err),
        }
    }
    Err(last)
}

/// Whether a bind failed because something else holds the port, or this
/// user may not bind it.
fn taken(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        ErrorKind::AddrInUse | ErrorKind::PermissionDenied
    )
}

/// A socket for `addr` without `SO_REUSEADDR`.
fn socket(addr: SocketAddr) -> io::Result<TcpSocket> {
    let socket = if addr.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };
    socket.set_reuseaddr(false)?;
    Ok(socket)
}

/// Binds both loopback addresses on `port`, or on a port the system picks
/// for 127.0.0.1 when it's 0. A machine without ::1 gets 127.0.0.1 alone.
fn pair(port: u16) -> io::Result<Bound> {
    let v4_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port);
    let v4 = socket(v4_addr)?;
    v4.bind(v4_addr)?;
    let port = v4.local_addr()?.port();
    let v6_addr = SocketAddr::new(Ipv6Addr::LOCALHOST.into(), port);
    let v6 = match socket(v6_addr).and_then(|v6| v6.bind(v6_addr).map(|()| v6)) {
        Ok(v6) => Some(v6),
        Err(err) if taken(&err) => return Err(err),
        // No IPv6 loopback here.
        Err(_) => None,
    };
    let mut listeners = vec![v4.listen(BACKLOG)?];
    if let Some(v6) = v6 {
        listeners.push(v6.listen(BACKLOG)?);
    }
    Ok(Bound { port, listeners })
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener as StdListener;

    use tokio::net::TcpStream;

    use super::*;

    /// A port free on both loopbacks a moment ago.
    fn free_port() -> u16 {
        for _ in 0..20 {
            let Ok(Bound { port, .. }) = pair(0) else {
                continue;
            };
            return port;
        }
        panic!("no free port");
    }

    fn addrs(bound: &Bound) -> Vec<SocketAddr> {
        bound
            .listeners
            .iter()
            .map(|listener| listener.local_addr().unwrap())
            .collect()
    }

    #[tokio::test]
    async fn both_loopbacks_bind_on_one_port() {
        // Another test may take the free port before this binds it, and
        // then the bind rightly moves on, so it tries a few.
        let (wanted, bound) = std::iter::repeat_with(|| {
            let wanted = free_port();
            (wanted, bind(wanted).unwrap())
        })
        .take(20)
        .find(|(wanted, bound)| bound.port == *wanted)
        .expect("never bound the port it wanted");
        assert_eq!(
            addrs(&bound),
            [
                SocketAddr::new(Ipv4Addr::LOCALHOST.into(), wanted),
                SocketAddr::new(Ipv6Addr::LOCALHOST.into(), wanted),
            ]
        );
        for addr in addrs(&bound) {
            TcpStream::connect(addr).await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_port_taken_on_either_loopback_moves_to_the_next() {
        for held in ["127.0.0.1", "::1"] {
            // Another program, which sets SO_REUSEADDR as most do. It holds
            // a port the system picks, so no other test takes it first.
            let other = StdListener::bind((held.parse::<std::net::IpAddr>().unwrap(), 0)).unwrap();
            let wanted = other.local_addr().unwrap().port();
            let bound = bind(wanted).unwrap();
            assert_ne!(bound.port, wanted, "{held}");
            assert_eq!(bound.listeners.len(), 2);
        }
    }

    #[tokio::test]
    async fn ports_browsers_refuse_are_skipped() {
        assert!(refused_by_browsers(6000));
        assert!(refused_by_browsers(10080));
        assert!(!refused_by_browsers(5173));
        assert!(REFUSED.windows(2).all(|pair| pair[0] < pair[1]));
        let bound = bind(6665).unwrap();
        assert!(!refused_by_browsers(bound.port), "{}", bound.port);
        assert_ne!(bound.port, 6665);
    }

    #[test]
    fn the_sockets_never_reuse_an_address() {
        for addr in ["127.0.0.1:0", "[::1]:0"] {
            let socket = socket(addr.parse().unwrap()).unwrap();
            assert!(!socket.reuseaddr().unwrap(), "{addr}");
        }
    }

    /// S18's other half. It binds 0.0.0.0 for a moment, which may ask
    /// the user about incoming connections, so it runs only when asked.
    #[tokio::test]
    #[ignore = "binds every address; run with --ignored for S18"]
    async fn a_wildcard_listener_on_the_port_moves_to_the_next() {
        let wanted = free_port();
        let _other = StdListener::bind((Ipv4Addr::UNSPECIFIED, wanted)).unwrap();
        let bound = bind(wanted).unwrap();
        assert_ne!(bound.port, wanted);
    }
}
