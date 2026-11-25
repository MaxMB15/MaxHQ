// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! `maxd guest probe TARGETS.json`, which runs inside a container. It tries
//! a TCP connect to each target and prints what happened as JSON. The host
//! builds the targets and judges the answers in [`super::egress`].

use std::io::{self, Write};
use std::net::{IpAddr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The most connects tried at once.
const WORKERS: usize = 64;

/// What the host asks the probe to try.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Targets {
    /// How long each connect may take, in milliseconds.
    pub timeout_ms: u64,
    /// The targets, in order.
    pub targets: Vec<Target>,
}

/// One address and port to try.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// An IP address or a host name.
    pub host: String,
    /// The TCP port.
    pub port: u16,
    /// The container's interface for a link-local address, such as `eth0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,
    /// True for a target that must connect, such as `github.com:443`.
    /// Every other target must not answer.
    #[serde(default)]
    pub required: bool,
}

impl Target {
    /// A target that must not answer.
    pub fn blocked(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            interface: None,
            required: false,
        }
    }

    /// A target that must connect.
    pub fn required(host: impl Into<String>, port: u16) -> Self {
        Self {
            required: true,
            ..Self::blocked(host, port)
        }
    }

    /// How the target is named in a result, such as `10.0.0.1:22` or
    /// `[fe80::1%eth0]:22`.
    pub fn label(&self) -> String {
        let scoped = match &self.interface {
            Some(interface) => format!("{}%{interface}", self.host),
            None => self.host.clone(),
        };
        if self.host.contains(':') {
            format!("[{scoped}]:{}", self.port)
        } else {
            format!("{scoped}:{}", self.port)
        }
    }
}

/// What one connect did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The connect went through.
    Connected,
    /// Something answered with a reset, so the target is reachable.
    Refused,
    /// Nothing answered in time.
    TimedOut,
    /// The container has no route there, or a guard rejected it.
    Unreachable,
    /// Anything else, such as a name that doesn't resolve.
    Failed,
}

impl Outcome {
    /// True when something on the far side answered.
    pub fn answered(self) -> bool {
        matches!(self, Outcome::Connected | Outcome::Refused)
    }
}

/// What happened to one target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tried {
    /// The target's label.
    pub target: String,
    /// What the connect did.
    pub outcome: Outcome,
    /// The error, when there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// What the probe prints.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// One result per target, in the targets' order.
    pub results: Vec<Tried>,
}

/// Reads the targets at `path`, tries them, and prints the report.
pub fn run(path: &Path) -> ExitCode {
    run_to(path, &mut io::stdout().lock())
}

/// Reads the targets at `path`, tries them, and writes the report to `out`.
fn run_to(path: &Path, out: &mut impl Write) -> ExitCode {
    let targets = match std::fs::read(path)
        .map_err(|e| e.to_string())
        .and_then(|bytes| serde_json::from_slice::<Targets>(&bytes).map_err(|e| e.to_string()))
    {
        Ok(targets) => targets,
        Err(error) => {
            eprintln!(
                "maxd guest probe: couldn't read {}: {error}",
                path.display()
            );
            return ExitCode::FAILURE;
        }
    };
    let report = probe(&targets);
    match serde_json::to_writer(&mut *out, &report)
        .map_err(io::Error::from)
        .and_then(|()| out.write_all(b"\n"))
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("maxd guest probe: couldn't print the report: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Tries every target, [`WORKERS`] at a time.
pub fn probe(targets: &Targets) -> Report {
    let timeout = Duration::from_millis(targets.timeout_ms.max(1));
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<Tried>>> = Mutex::new(vec![None; targets.targets.len()]);
    let workers = WORKERS.min(targets.targets.len()).max(1);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(target) = targets.targets.get(i) else {
                    break;
                };
                let tried = try_one(target, timeout);
                if let Ok(mut results) = results.lock() {
                    results[i] = Some(tried);
                }
            });
        }
    });
    let results = results.into_inner().unwrap_or_default();
    Report {
        results: results.into_iter().flatten().collect(),
    }
}

/// Tries one target. A host name is tried at each address it resolves
/// to, and the best answer counts.
fn try_one(target: &Target, timeout: Duration) -> Tried {
    let label = target.label();
    let addrs = match addresses(target) {
        Ok(addrs) if !addrs.is_empty() => addrs,
        Ok(_) => return failed(label, "it resolved to no address".to_owned()),
        Err(error) => return failed(label, error.to_string()),
    };
    let mut best: Option<(Outcome, Option<String>)> = None;
    for addr in addrs {
        let (outcome, detail) = match TcpStream::connect_timeout(&addr, timeout) {
            Ok(_) => (Outcome::Connected, None),
            Err(error) => (classify(&error), Some(error.to_string())),
        };
        if outcome == Outcome::Connected {
            return Tried {
                target: label,
                outcome,
                detail: None,
            };
        }
        if best.as_ref().is_none_or(|(b, _)| rank(outcome) > rank(*b)) {
            best = Some((outcome, detail));
        }
    }
    let (outcome, detail) = best.unwrap_or((Outcome::Failed, None));
    Tried {
        target: label,
        outcome,
        detail,
    }
}

/// Orders outcomes so the one that says most about reachability wins.
fn rank(outcome: Outcome) -> u8 {
    match outcome {
        Outcome::Connected => 4,
        Outcome::Refused => 3,
        Outcome::TimedOut => 2,
        Outcome::Unreachable => 1,
        Outcome::Failed => 0,
    }
}

fn failed(target: String, detail: String) -> Tried {
    Tried {
        target,
        outcome: Outcome::Failed,
        detail: Some(detail),
    }
}

/// The socket addresses a target names.
fn addresses(target: &Target) -> io::Result<Vec<SocketAddr>> {
    match target.host.parse::<IpAddr>() {
        Ok(IpAddr::V6(ip)) => {
            let scope = match &target.interface {
                Some(interface) => scope_id(interface)?,
                None => 0,
            };
            Ok(vec![SocketAddr::V6(std::net::SocketAddrV6::new(
                ip,
                target.port,
                0,
                scope,
            ))])
        }
        Ok(ip) => Ok(vec![SocketAddr::new(ip, target.port)]),
        Err(_) => Ok((target.host.as_str(), target.port)
            .to_socket_addrs()?
            .collect()),
    }
}

/// The index of an interface, which a link-local address needs.
fn scope_id(interface: &str) -> io::Result<u32> {
    if interface.contains('/') || interface.contains("..") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "that isn't an interface name",
        ));
    }
    let text = std::fs::read_to_string(format!("/sys/class/net/{interface}/ifindex"))?;
    text.trim()
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "odd interface index"))
}

/// Sorts a connect error into what it says about the path.
pub fn classify(error: &io::Error) -> Outcome {
    match error.kind() {
        io::ErrorKind::ConnectionRefused | io::ErrorKind::ConnectionReset => Outcome::Refused,
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => Outcome::TimedOut,
        io::ErrorKind::HostUnreachable
        | io::ErrorKind::NetworkUnreachable
        | io::ErrorKind::PermissionDenied
        | io::ErrorKind::AddrNotAvailable => Outcome::Unreachable,
        _ => Outcome::Failed,
    }
}

/// True for an IPv6 address in fe80::/10.
pub fn is_link_local(ip: &Ipv6Addr) -> bool {
    ip.segments()[0] & 0xffc0 == 0xfe80
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use tokio::net::TcpSocket;

    use super::*;
    use crate::testing::TempDir;

    #[test]
    fn labels_name_the_target() {
        assert_eq!(Target::blocked("10.0.0.1", 22).label(), "10.0.0.1:22");
        assert_eq!(Target::blocked("fd00::1", 53).label(), "[fd00::1]:53");
        let mut scoped = Target::blocked("fe80::1", 22);
        scoped.interface = Some("eth0".to_owned());
        assert_eq!(scoped.label(), "[fe80::1%eth0]:22");
        assert_eq!(
            Target::required("github.com", 443).label(),
            "github.com:443"
        );
    }

    #[test]
    fn sorts_connect_errors() {
        let kind = |kind| classify(&io::Error::from(kind));
        assert_eq!(kind(io::ErrorKind::ConnectionRefused), Outcome::Refused);
        assert_eq!(kind(io::ErrorKind::TimedOut), Outcome::TimedOut);
        assert_eq!(kind(io::ErrorKind::HostUnreachable), Outcome::Unreachable);
        assert_eq!(
            kind(io::ErrorKind::NetworkUnreachable),
            Outcome::Unreachable
        );
        assert_eq!(kind(io::ErrorKind::Other), Outcome::Failed);
        assert!(Outcome::Refused.answered());
        assert!(!Outcome::TimedOut.answered());
    }

    #[test]
    fn tries_each_target_and_reports_in_order() {
        let open = TcpListener::bind("127.0.0.1:0").unwrap();
        let open_port = open.local_addr().unwrap().port();
        // The closed target is a socket that holds its port without
        // listening. A dropped listener's port could be taken by another
        // test or test run before the probe got there, and this one can't.
        // Linux refuses a connect to it, and macOS lets it time out. So
        // does Windows, which tries a refused connect again for seconds.
        let closed = TcpSocket::new_v4().unwrap();
        closed.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let closed_port = closed.local_addr().unwrap().port();
        let closed_outcome = if cfg!(any(target_os = "macos", windows)) {
            Outcome::TimedOut
        } else {
            Outcome::Refused
        };

        let targets = Targets {
            timeout_ms: 500,
            targets: vec![
                Target::blocked("127.0.0.1", open_port),
                Target::blocked("127.0.0.1", closed_port),
                Target::required("no-such-host.invalid", 443),
            ],
        };
        let dir = TempDir::new();
        let path = dir.path().join("targets.json");
        std::fs::write(&path, serde_json::to_vec(&targets).unwrap()).unwrap();
        let mut printed = Vec::new();
        assert_eq!(run_to(&path, &mut printed), ExitCode::SUCCESS);
        assert_eq!(printed.last(), Some(&b'\n'));

        let report: Report = serde_json::from_slice(&printed).unwrap();
        let outcomes: Vec<_> = report.results.iter().map(|r| r.outcome).collect();
        assert_eq!(
            outcomes,
            [Outcome::Connected, closed_outcome, Outcome::Failed]
        );
        assert_eq!(report.results[0].target, format!("127.0.0.1:{open_port}"));
        drop((open, closed));
    }

    #[test]
    fn a_missing_targets_file_fails() {
        let dir = TempDir::new();
        assert_eq!(run(&dir.path().join("missing.json")), ExitCode::FAILURE);
    }

    #[test]
    fn knows_link_local_addresses() {
        assert!(is_link_local(&"fe80::1".parse().unwrap()));
        assert!(is_link_local(&"febf::1".parse().unwrap()));
        assert!(!is_link_local(&"fd00::1".parse().unwrap()));
    }
}
