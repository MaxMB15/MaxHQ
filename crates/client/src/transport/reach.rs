// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What the phone's two ways in share: the routes each may dial, reaching
//! a host's port, and the stagger. A link with several routes starts the
//! next one [`STAGGER`] after the last if it hasn't answered, so a dead LAN
//! address doesn't cost a full timeout.

use std::collections::HashMap;
use std::io;
#[cfg(all(feature = "russh", feature = "ws"))]
use std::net::IpAddr;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use futures::future::BoxFuture;
use maxhq_protocol::link::Route;
use tokio::io::{AsyncRead, AsyncWrite};

#[cfg(all(feature = "russh", feature = "ws"))]
use super::{Conn, PhoneSsh, PhoneWs, Transport};
use super::{DialError, CONNECT_TIMEOUT};
#[cfg(all(feature = "russh", feature = "ws"))]
use crate::ssh_config::SshConfig;

/// How long a route gets to answer before the next one starts too.
pub(crate) const STAGGER: Duration = Duration::from_millis(300);

/// A byte stream to a device's port.
pub(crate) trait Io: AsyncRead + AsyncWrite + Unpin + Send + 'static {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> Io for T {}

/// Opens a stream to a host's port. Tests swap in pipes.
pub(crate) type Connect =
    Arc<dyn Fn(&str, u16) -> BoxFuture<'static, io::Result<Box<dyn Io>>> + Send + Sync>;

/// Plain TCP, with no delay on small writes.
pub(crate) fn tcp() -> Connect {
    Arc::new(|host, port| {
        let host = host.to_owned();
        Box::pin(async move {
            let stream = tokio::net::TcpStream::connect((host.as_str(), port)).await?;
            stream.set_nodelay(true)?;
            Ok(Box::new(stream) as Box<dyn Io>)
        })
    })
}

/// The routes a transport may dial, by the route host that names each.
/// Clones share them, so the caller can keep one to add devices as the
/// book changes.
pub(crate) struct Logins<T>(Arc<RwLock<HashMap<String, T>>>);

impl<T> Clone for Logins<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> Default for Logins<T> {
    fn default() -> Self {
        Self(Arc::default())
    }
}

impl<T: Clone> Logins<T> {
    /// The login for the route `name`, or a refusal when there's none.
    pub fn get(&self, name: &str) -> Result<T, DialError> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .cloned()
            .ok_or_else(|| DialError::refused("that route isn't in this phone's device book"))
    }

    /// Keeps `logins`, each in place of any login its route had, and
    /// returns their routes in their order.
    pub fn trust(&self, logins: impl IntoIterator<Item = (String, T)>) -> Vec<Route> {
        let mut map = self.0.write().unwrap_or_else(PoisonError::into_inner);
        logins
            .into_iter()
            .map(|(name, login)| {
                map.insert(name.clone(), login);
                Route::Ssh { host: name }
            })
            .collect()
    }

    /// Whether it has a login for the route `name`.
    #[cfg(all(feature = "russh", feature = "ws"))]
    pub fn knows(&self, name: &str) -> bool {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(name)
    }

    /// The routes it knows, sorted, for a `Debug` that shows nothing else.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect();
        names.sort();
        names
    }
}

/// What a transport says when it can't reach a port.
pub(crate) struct Reasons {
    pub refused: &'static str,
    pub timed_out: &'static str,
    pub closed: &'static str,
}

/// Reaches `host:port` through `connect`, within [`CONNECT_TIMEOUT`].
pub(crate) async fn open(
    connect: &Connect,
    host: &str,
    port: u16,
    reasons: &Reasons,
) -> Result<Box<dyn Io>, DialError> {
    match tokio::time::timeout(CONNECT_TIMEOUT, connect(host, port)).await {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(err)) => Err(io_failure(&err, reasons)),
        Err(_) => Err(DialError::down(reasons.timed_out)),
    }
}

/// What a failure to reach or keep a port means for the app.
pub(crate) fn io_failure(err: &io::Error, reasons: &Reasons) -> DialError {
    match err.kind() {
        io::ErrorKind::ConnectionRefused => DialError::down(reasons.refused),
        io::ErrorKind::TimedOut => DialError::down(reasons.timed_out),
        io::ErrorKind::HostUnreachable | io::ErrorKind::NetworkUnreachable => {
            DialError::down("no route to the host")
        }
        io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::BrokenPipe
        | io::ErrorKind::UnexpectedEof => DialError::down(reasons.closed),
        _ => DialError::down("can't reach the host"),
    }
}

/// The phone's SSH and its network access together, so one link can try a
/// device's network routes and then its SSH routes, with the stagger
/// across all of them. Each route goes to the network access that knows
/// it, and any other to SSH.
#[cfg(all(feature = "russh", feature = "ws"))]
pub(crate) struct Both {
    pub ssh: PhoneSsh,
    pub ws: PhoneWs,
}

#[cfg(all(feature = "russh", feature = "ws"))]
impl Transport for Both {
    fn resolve(&self, destination: &str) -> BoxFuture<'static, io::Result<SshConfig>> {
        self.ssh.resolve(destination)
    }

    fn lookup(&self, _host: &str) -> BoxFuture<'static, Vec<IpAddr>> {
        Box::pin(async { Vec::new() })
    }

    fn dial<'a>(
        &'a self,
        route: &'a Route,
        config: Option<&'a SshConfig>,
    ) -> BoxFuture<'a, Result<Conn, DialError>> {
        match route {
            Route::Ssh { host } if self.ws.knows(host) => self.ws.dial(route, config),
            _ => self.ssh.dial(route, config),
        }
    }

    fn forget<'a>(&'a self, _host: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    fn stagger(&self) -> Option<Duration> {
        Some(STAGGER)
    }
}
