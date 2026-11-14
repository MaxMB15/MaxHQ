// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! N-6's port detection. On Linux it polls `/proc/net/tcp` and `tcp6`,
//! maps each socket to a process and walks up to a holder. A port shows
//! after two polls in a row and stays a while after it goes. The guest
//! runs this for its container, and host `maxd` for its folder agents.
//!
//! A [`Scanner`] finds what listens now: [`Procfs`] on Linux, and
//! `Libproc` on macOS, which looks only in the trees of the holders it's
//! given. The same walk lists an agent's processes for the Running list,
//! through [`processes`]. [`Debounce`] holds a port back until two polls
//! see it. On the host, [`HostPorts`] turns each workspace's ports into
//! `ListeningPort` entities, keeps a gone port [`GONE_AFTER`], and closes
//! its forwards when it goes for good. [`Links`] feeds it each guest's
//! reports and tells each guest what to watch. In the guest, [`guest`]
//! watches for the host.
//!
//! A fixed list is never shown: ports 22 and 5355 anywhere, and 53 on
//! systemd-resolved's 127.0.0.53 and 127.0.0.54. Neither are the guest's
//! own sockets, nor a CLI's own listeners, nor a project's
//! `ignored_ports`.

pub mod guest;
mod host;
#[cfg(target_os = "macos")]
mod libproc;
mod links;
pub mod processes;
mod procfs;
mod track;

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use maxhq_protocol::guest::{GuestPort, Pace, WatchedHolder};
use maxhq_protocol::ids::SessionId;
use maxhq_protocol::model::{BindAddress, PortProcess};
use maxhq_protocol::processes::{AgentProcess, AgentRunning};

pub use self::host::{local_scanner, run_local, HostPorts, LocalHolder};
#[cfg(target_os = "macos")]
pub use self::libproc::Libproc;
pub use self::links::{HolderWatch, Holding, Holdings, Links};
pub use self::procfs::Procfs;
pub use self::track::Debounce;

/// How often ports are polled while a client is connected and an agent
/// runs.
pub const FAST: Duration = Duration::from_secs(2);
/// How often ports are polled otherwise.
pub const SLOW: Duration = Duration::from_secs(30);
/// How many polls in a row must see a port before it shows.
pub const SHOW_AFTER: u32 = 2;
/// How long a port stays after it goes.
pub const GONE_AFTER: Duration = Duration::from_secs(10);

/// The longest process name shown, in bytes.
const MAX_NAME_LEN: usize = 64;
/// The longest command line shown, in bytes.
const MAX_COMMAND_LEN: usize = 1024;

/// The poll interval for a pace.
pub fn interval(pace: Pace) -> Duration {
    match pace {
        Pace::Fast => FAST,
        Pace::Slow => SLOW,
    }
}

/// The pace to poll at: fast while a client is connected and an agent
/// runs.
pub fn pace(client_connected: bool, agent_running: bool) -> Pace {
    if client_connected && agent_running {
        Pace::Fast
    } else {
        Pace::Slow
    }
}

/// Finds the ports listening now, and the processes in a holder's tree.
pub trait Scanner: Send + 'static {
    /// One poll: the ports in the trees of `holders`, merged so each port
    /// shows once, in port order.
    fn scan(&mut self, holders: &[WatchedHolder]) -> Vec<GuestPort>;

    /// The processes in `holder`'s tree, oldest first, without the holder
    /// itself. A scanner that can't walk a tree finds none.
    fn processes(&mut self, holder: &WatchedHolder) -> Vec<AgentProcess> {
        let _ = holder;
        Vec::new()
    }
}

impl<S: Scanner + ?Sized> Scanner for Box<S> {
    fn scan(&mut self, holders: &[WatchedHolder]) -> Vec<GuestPort> {
        (**self).scan(holders)
    }

    fn processes(&mut self, holder: &WatchedHolder) -> Vec<AgentProcess> {
        (**self).processes(holder)
    }
}

/// The one scanner for this device's holders, which the port poll and
/// the Running list share, so a process only an earlier poll saw still
/// counts in both.
pub type SharedScanner = Arc<Mutex<Box<dyn Scanner>>>;

/// What each holder of one agent on this device runs besides its CLI, by
/// the holder's session, as of the last port poll. A holder with nothing
/// running has no entry.
pub type Jobs = HashMap<SessionId, AgentRunning>;

/// The port numbers of a process's listening sockets, as port detection
/// counts them.
pub(crate) fn port_numbers(sockets: impl IntoIterator<Item = (IpAddr, u16)>) -> Vec<u16> {
    sockets
        .into_iter()
        .filter(|(ip, port)| *port != 0 && !ignored(*ip, *port))
        .map(|(_, port)| port)
        .collect()
}

/// Whether a socket on `ip` and `port` is on the fixed ignore list.
pub fn ignored(ip: IpAddr, port: u16) -> bool {
    const RESOLVED: [Ipv4Addr; 2] = [Ipv4Addr::new(127, 0, 0, 53), Ipv4Addr::new(127, 0, 0, 54)];
    match (port, ip.to_canonical()) {
        (22 | 5355, _) => true,
        (53, IpAddr::V4(ip)) => RESOLVED.contains(&ip),
        _ => false,
    }
}

/// How a socket's address shows. Only 127.0.0.1 and ::1 count as
/// loopback, since a tunnel connects to one of them.
pub fn bind_of(ip: IpAddr) -> BindAddress {
    let ip = ip.to_canonical();
    if ip.is_unspecified() {
        BindAddress::Any
    } else if ip == IpAddr::V4(Ipv4Addr::LOCALHOST) || ip == IpAddr::V6(Ipv6Addr::LOCALHOST) {
        BindAddress::Loopback
    } else {
        BindAddress::Address { ip: ip.to_string() }
    }
}

/// A listening socket a scanner found, before sockets on one port merge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Found {
    pub(crate) ip: IpAddr,
    pub(crate) port: u16,
    /// The watched holder whose tree holds the socket.
    pub(crate) session: Option<SessionId>,
    pub(crate) process: Option<PortProcess>,
}

/// One entry per port, in port order. A port on several addresses shows
/// the widest: every address, then loopback, then a single address. The
/// first socket with a holder or a process gives them.
pub(crate) fn merge(found: Vec<Found>) -> Vec<GuestPort> {
    fn rank(bind: &BindAddress) -> u8 {
        match bind {
            BindAddress::Any => 2,
            BindAddress::Loopback => 1,
            BindAddress::Address { .. } => 0,
        }
    }
    let mut ports: Vec<GuestPort> = Vec::new();
    for socket in found {
        if socket.port == 0 || ignored(socket.ip, socket.port) {
            continue;
        }
        let bind = bind_of(socket.ip);
        match ports.iter_mut().find(|port| port.port == socket.port) {
            Some(port) => {
                if rank(&bind) > rank(&port.bind) {
                    port.bind = bind;
                }
                if port.session.is_none() {
                    port.session = socket.session;
                }
                if port.process.is_none() {
                    port.process = socket.process;
                }
            }
            None => ports.push(GuestPort {
                port: socket.port,
                bind,
                session: socket.session,
                process: socket.process,
            }),
        }
    }
    ports.sort_by_key(|port| port.port);
    ports
}

/// A process as it shows, its name and command cut short.
pub(crate) fn process(pid: u32, name: &str, command: &str) -> PortProcess {
    PortProcess {
        pid,
        name: clip(name, MAX_NAME_LEN),
        command: clip(command, MAX_COMMAND_LEN),
    }
}

/// `text` cut to at most `max` bytes, on a character boundary.
pub(crate) fn clip(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(ip: &str, port: u16) -> Found {
        Found {
            ip: ip.parse().unwrap(),
            port,
            session: None,
            process: None,
        }
    }

    #[test]
    fn the_fixed_list_is_ignored() {
        for (ip, port) in [
            ("0.0.0.0", 22),
            ("::", 22),
            ("127.0.0.1", 5355),
            ("127.0.0.53", 53),
            ("127.0.0.54", 53),
            ("::ffff:127.0.0.53", 53),
        ] {
            assert!(ignored(ip.parse().unwrap(), port), "{ip}:{port}");
        }
        for (ip, port) in [("127.0.0.1", 53), ("0.0.0.0", 53), ("127.0.0.1", 5173)] {
            assert!(!ignored(ip.parse().unwrap(), port), "{ip}:{port}");
        }
    }

    #[test]
    fn binds_show_as_loopback_any_or_one_address() {
        let bind = |ip: &str| bind_of(ip.parse().unwrap());
        assert_eq!(bind("127.0.0.1"), BindAddress::Loopback);
        assert_eq!(bind("::1"), BindAddress::Loopback);
        assert_eq!(bind("::ffff:127.0.0.1"), BindAddress::Loopback);
        assert_eq!(bind("0.0.0.0"), BindAddress::Any);
        assert_eq!(bind("::"), BindAddress::Any);
        assert_eq!(
            bind("127.0.0.2"),
            BindAddress::Address {
                ip: "127.0.0.2".into()
            }
        );
        assert_eq!(
            bind("::ffff:10.0.3.15"),
            BindAddress::Address {
                ip: "10.0.3.15".into()
            }
        );
    }

    #[test]
    fn sockets_on_one_port_merge_to_the_widest() {
        let session = SessionId::derive("s");
        let mut owned = found("::1", 5173);
        owned.session = Some(session);
        owned.process = Some(process(7, "node", "node vite"));
        let ports = merge(vec![
            found("127.0.0.1", 5173),
            owned,
            found("10.0.3.15", 8080),
            found("0.0.0.0", 8080),
            found("127.0.0.53", 53),
            found("0.0.0.0", 0),
            found("127.0.0.1", 3000),
        ]);
        let summary: Vec<(u16, BindAddress, Option<SessionId>)> = ports
            .iter()
            .map(|port| (port.port, port.bind.clone(), port.session))
            .collect();
        assert_eq!(
            summary,
            [
                (3000, BindAddress::Loopback, None),
                (5173, BindAddress::Loopback, Some(session)),
                (8080, BindAddress::Any, None),
            ]
        );
        assert_eq!(ports[1].process.as_ref().unwrap().pid, 7);
    }

    #[test]
    fn names_and_commands_are_cut_on_a_character() {
        assert_eq!(clip("héllo", 2), "h");
        assert_eq!(clip("héllo", 3), "hé");
        assert_eq!(clip("hi", 10), "hi");
        let long = process(1, &"n".repeat(100), &"é".repeat(1000));
        assert_eq!(long.name.len(), MAX_NAME_LEN);
        assert_eq!(long.command.len(), MAX_COMMAND_LEN);
    }

    #[test]
    fn polling_is_fast_only_while_someone_watches_an_agent() {
        assert_eq!(pace(true, true), Pace::Fast);
        assert_eq!(pace(true, false), Pace::Slow);
        assert_eq!(pace(false, true), Pace::Slow);
        assert_eq!(interval(Pace::Fast), FAST);
        assert_eq!(interval(Pace::Slow), SLOW);
    }
}
