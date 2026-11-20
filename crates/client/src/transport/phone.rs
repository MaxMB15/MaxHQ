// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The phone's own SSH, through russh, since a phone can't run `ssh`. It
//! logs in with the phone's key and trusts only the host key pinned at
//! pairing. The phone's line in the device's `authorized_keys` forces
//! `maxd proxy --phone <phone>`, which tells `maxd` which phone this is
//! before anything the link sends, so the channel's bytes are the link's
//! from the first.

use std::fmt;
use std::io;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use maxhq_protocol::ids::DeviceId;
use maxhq_protocol::link::Route;
use maxhq_protocol::phones::PhoneBookEntry;
use russh::client::{self, Handle, Msg};
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::{PrivateKey, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh::{Channel, ChannelMsg, Disconnect};
use tokio::io::AsyncWriteExt;
use tokio::task::JoinHandle;

use super::reach::{self, Connect, Io, Logins, Reasons, STAGGER};
use super::{Conn, DialError, Rest, StderrTail, Transport, LOGIN_TIMEOUT, PROXY_COMMAND};
use crate::ssh_config::SshConfig;

/// How many bytes from `maxd` wait for the link to read them.
const PIPE: usize = 256 * 1024;

/// How many bytes of stderr wait for the tail to read them.
const STDERR_PIPE: usize = 8 * 1024;

/// What SSH says when it can't reach a port.
const REASONS: Reasons = Reasons {
    refused: "SSH connection refused",
    timed_out: "SSH timed out",
    closed: "the SSH connection closed",
};

/// The phone's SSH: its own ed25519 key, and the devices it may reach,
/// each route with its device's pinned host key. Clones share the devices,
/// so the caller can keep one to add devices as the book changes.
#[derive(Clone)]
pub struct PhoneSsh {
    key: Arc<PrivateKey>,
    logins: Logins<Login>,
    connect: Connect,
}

/// One route's login.
#[derive(Clone, Debug)]
struct Login {
    user: String,
    host: String,
    port: u16,
    /// The device's host key, as an OpenSSH line.
    host_key: String,
}

impl fmt::Debug for PhoneSsh {
    /// Shows the routes it knows, never the key.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PhoneSsh")
            .field("routes", &self.logins.names())
            .finish()
    }
}

impl PhoneSsh {
    /// Logs in with the ed25519 key made from `seed`, such as the phone's
    /// own key from the Keychain or the one-time key from a pairing code.
    pub fn new(seed: &[u8; 32]) -> Self {
        Self {
            key: Arc::new(PrivateKey::from(Ed25519Keypair::from_seed(seed))),
            logins: Logins::default(),
            connect: reach::tcp(),
        }
    }

    /// The key's public half as an OpenSSH line, for `authorized_keys`.
    pub fn public_key(&self) -> String {
        self.key
            .public_key()
            .to_openssh()
            .expect("an ed25519 key encodes")
    }

    /// Lets the phone reach `entry`'s device, trusting only its pinned
    /// host key, and returns its routes in their order, for a
    /// [`crate::LinkSpec`]. A device trusted again replaces what it had.
    pub fn trust(&self, entry: &PhoneBookEntry) -> Vec<Route> {
        self.logins.trust(
            entry.device,
            entry.routes.iter().map(|route| {
                let login = Login {
                    user: route.user.clone(),
                    host: route.host.clone(),
                    port: route.port,
                    host_key: entry.host_key.clone(),
                };
                (super::phone_route_host(route), login)
            }),
        )
    }

    /// Reaches hosts through `connect` in place of TCP.
    #[cfg(test)]
    pub(crate) fn connecting_with(mut self, connect: Connect) -> Self {
        self.connect = connect;
        self
    }

    /// Logs in on the route `name` to `device` and starts `maxd proxy`.
    async fn open(&self, device: Option<DeviceId>, name: &str) -> Result<Conn, DialError> {
        let login = self.logins.get(device, name)?;
        let pin = PublicKey::from_openssh(&login.host_key)
            .map_err(|_| DialError::refused("the device's host key pin doesn't read"))?;
        let stream = reach::open(&self.connect, &login.host, login.port, &REASONS).await?;
        match tokio::time::timeout(LOGIN_TIMEOUT, self.log_in(stream, &login.user, pin)).await {
            Ok(conn) => conn,
            Err(_) => Err(DialError::refused("SSH login didn't finish")),
        }
    }

    /// Logs in over `stream` and runs the proxy command.
    async fn log_in(
        &self,
        stream: Box<dyn Io>,
        user: &str,
        pin: PublicKey,
    ) -> Result<Conn, DialError> {
        let config = Arc::new(client::Config::default());
        let mut handle = client::connect_stream(config, stream, Pinned { pin })
            .await
            .map_err(|err| ssh_failure(&err))?;
        let key = PrivateKeyWithHashAlg::new(Arc::clone(&self.key), None);
        let auth = handle
            .authenticate_publickey(user, key)
            .await
            .map_err(|err| ssh_failure(&err))?;
        if !auth.success() {
            return Err(DialError::refused("SSH sign-in was refused"));
        }
        let channel = handle
            .channel_open_session()
            .await
            .map_err(|err| ssh_failure(&err))?;
        // The forced command runs in its place, but a key without one
        // still reaches `maxd`.
        channel
            .exec(true, PROXY_COMMAND)
            .await
            .map_err(|err| ssh_failure(&err))?;
        Ok(piped(handle, channel))
    }
}

impl Transport for PhoneSsh {
    fn resolve(&self, _destination: &str) -> BoxFuture<'static, io::Result<SshConfig>> {
        Box::pin(async { Err(io::Error::other("the phone reads no ssh config")) })
    }

    fn lookup(&self, _host: &str) -> BoxFuture<'static, Vec<IpAddr>> {
        Box::pin(async { Vec::new() })
    }

    fn dial<'a>(
        &'a self,
        route: &'a Route,
        _config: Option<&'a SshConfig>,
        device: Option<DeviceId>,
    ) -> BoxFuture<'a, Result<Conn, DialError>> {
        Box::pin(async move {
            let Route::Ssh { host } = route else {
                return Err(DialError::refused("the phone reaches devices over SSH"));
            };
            self.open(device, host).await
        })
    }

    /// Each dial logs in afresh, so there's nothing to drop.
    fn forget<'a>(&'a self, _host: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    fn stagger(&self) -> Option<Duration> {
        Some(STAGGER)
    }
}

/// Accepts only the pinned host key.
struct Pinned {
    pin: PublicKey,
}

impl client::Handler for Pinned {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        Ok(matches!(key, PublicKeyOrCertificate::PublicKey { key, .. }
            if key.key_data() == self.pin.key_data()))
    }
}

/// The channel as a stream: its data for the link, its stderr for the
/// tail, and the session kept open until the stream drops.
fn piped(handle: Handle<Pinned>, mut channel: Channel<Msg>) -> Conn {
    let writer = channel.make_writer();
    let (mut out, reader) = tokio::io::duplex(PIPE);
    let (mut err, stderr) = tokio::io::duplex(STDERR_PIPE);
    let pump = tokio::spawn(async move {
        while let Some(message) = channel.wait().await {
            match message {
                ChannelMsg::Data { data } if out.write_all(&data).await.is_err() => break,
                ChannelMsg::ExtendedData { data, ext: 1 } => {
                    let _ = err.write_all(&data).await;
                }
                ChannelMsg::Eof | ChannelMsg::Close => break,
                _ => {}
            }
        }
    });
    Conn {
        reader: Box::new(reader),
        writer: Box::new(Box::pin(writer)),
        rest: Rest {
            stderr: Some(StderrTail::read(stderr)),
            _keep: Some(Box::new(Session {
                handle: Some(handle),
                pump,
            })),
        },
    }
}

/// One login. Dropping it closes the session.
struct Session {
    handle: Option<Handle<Pinned>>,
    pump: JoinHandle<()>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.pump.abort();
        if let Some(handle) = self.handle.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            runtime.spawn(async move {
                let _ = handle.disconnect(Disconnect::ByApplication, "", "en").await;
            });
        }
    }
}

/// What a russh failure means for the app.
fn ssh_failure(err: &russh::Error) -> DialError {
    match err {
        russh::Error::UnknownKey => DialError::refused("the device showed a different host key"),
        russh::Error::IO(err) => reach::io_failure(err, &REASONS),
        _ => DialError::down("the SSH connection closed"),
    }
}
