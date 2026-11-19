// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The phone's SSH against an in-process russh server, whose sessions
//! play the forced `maxd proxy --phone`: they send the `Phone` frame on to
//! the fake `maxd`, then pass the channel's bytes through.

use std::collections::HashMap as Channels;

use maxhq_protocol::ids::{IdempotencyKey, PhoneId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::phones::{PhoneBookEntry, PhoneRoute};
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::{PrivateKey, PublicKey};
use russh::server::{self, Auth, ChannelOpenHandle, Msg, PublicKeyOffer, VerifiedPublicKey};
use russh::{Channel, ChannelId, MethodKind, MethodSet};

use super::requests::{interrupt, result};
use super::*;
use crate::transport::PhoneSsh;

/// The host that answers.
const DEVBOX: &str = "devbox.lan";

/// A host that takes the connection and then says nothing.
const SILENT: &str = "devbox-tail.example";

fn key(seed: u8) -> PrivateKey {
    PrivateKey::from(Ed25519Keypair::from_seed(&[seed; 32]))
}

fn openssh(key: &PrivateKey) -> String {
    key.public_key().to_openssh().unwrap()
}

fn phone() -> PhoneId {
    PhoneId::derive("test-phone")
}

/// The device's book entry: its routes, in order, and the host key the
/// phone pins.
fn entry(hosts: &[&str], host_key: &PrivateKey) -> PhoneBookEntry {
    PhoneBookEntry {
        device: device(),
        name: "Test box".into(),
        routes: hosts
            .iter()
            .map(|host| PhoneRoute {
                user: "dev".into(),
                host: (*host).into(),
                port: 22,
            })
            .collect(),
        host_key: openssh(host_key),
        network: None,
    }
}

/// The far side: an SSH server with `host_key` that lets in `allowed`.
struct Server {
    config: Arc<server::Config>,
    allowed: PublicKey,
    /// Where each session's `maxd` end goes.
    maxds: mpsc::UnboundedSender<UnixStream>,
}

/// One session on the far side.
struct Session {
    allowed: PublicKey,
    maxds: mpsc::UnboundedSender<UnixStream>,
    channels: Channels<ChannelId, Channel<Msg>>,
}

impl server::Handler for Session {
    type Error = russh::Error;

    async fn auth_publickey_offered(
        &mut self,
        _user: &str,
        offer: PublicKeyOffer<'_>,
    ) -> Result<Auth, Self::Error> {
        Ok(self.check(offer.public_key))
    }

    async fn auth_publickey(
        &mut self,
        _user: &str,
        key: VerifiedPublicKey<'_>,
    ) -> Result<Auth, Self::Error> {
        Ok(self.check(key.public_key))
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    /// Runs the forced command, whatever the client asked for.
    async fn exec_request(
        &mut self,
        id: ChannelId,
        _data: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let Some(channel) = self.channels.remove(&id) else {
            return session.channel_failure(id);
        };
        session.channel_success(id)?;
        let (mut ours, theirs) = UnixStream::pair().expect("a socket pair");
        let _ = self.maxds.send(theirs);
        tokio::spawn(async move {
            let mut first = Vec::new();
            frame::encode_message(&mut first, &ClientMessage::Phone { phone: phone() }).unwrap();
            if ours.write_all(&first).await.is_err() {
                return;
            }
            let mut stream = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut stream, &mut ours).await;
        });
        Ok(())
    }
}

impl Session {
    fn check(&self, key: &PublicKey) -> Auth {
        if key.key_data() == self.allowed.key_data() {
            Auth::Accept
        } else {
            Auth::reject()
        }
    }
}

/// A phone client whose SSH reaches [`DEVBOX`] and [`SILENT`] over pipes.
struct PhoneRig {
    client: Client,
    ssh: PhoneSsh,
    maxds: mpsc::UnboundedReceiver<UnixStream>,
}

impl PhoneRig {
    /// The phone logs in with `phone_key` to a server with `host_key` that
    /// lets in `allowed`.
    fn new(phone_key: u8, host_key: &PrivateKey, allowed: &PrivateKey) -> Self {
        let (maxds_tx, maxds) = mpsc::unbounded_channel();
        let server = Arc::new(Server {
            config: Arc::new(server::Config {
                methods: MethodSet::from(&[MethodKind::PublicKey][..]),
                keys: vec![host_key.clone()],
                inactivity_timeout: None,
                ..Default::default()
            }),
            allowed: allowed.public_key().clone(),
            maxds: maxds_tx,
        });
        let ssh = PhoneSsh::new(&[phone_key; 32]).connecting_with(Arc::new(move |host, _port| {
            let server = server.clone();
            let host = host.to_owned();
            Box::pin(async move {
                if host != DEVBOX {
                    // Takes the bytes and never answers.
                    let (ours, theirs) = tokio::io::duplex(1024);
                    std::mem::forget(theirs);
                    return Ok(Box::new(ours) as Box<dyn crate::transport::Io>);
                }
                let (ours, theirs) = tokio::io::duplex(64 * 1024);
                let session = Session {
                    allowed: server.allowed.clone(),
                    maxds: server.maxds.clone(),
                    channels: Channels::new(),
                };
                let config = server.config.clone();
                tokio::spawn(async move {
                    if let Ok(running) = server::run_stream(config, theirs, session).await {
                        let _ = running.await;
                    }
                });
                Ok(Box::new(ours) as Box<dyn crate::transport::Io>)
            })
        }));
        let client = Client::app(crate::Transport::Russh(ssh.clone())).timed(FAST);
        Self { client, ssh, maxds }
    }

    /// A link to `entry`'s device over its routes.
    fn link(&self, entry: &PhoneBookEntry) -> (LinkHandle, mpsc::Receiver<LinkEvent>) {
        let mut routes = self.ssh.trust(entry).into_iter();
        let route = routes.next().expect("a route");
        self.client.link_to(
            LinkSpec {
                fallbacks: routes.collect(),
                ..spec(route)
            },
            entry.device,
        )
    }

    /// The `maxd` behind the next session, past the `Phone` frame the
    /// forced command sends.
    async fn next_maxd(&mut self) -> FakeMaxd {
        let stream = tokio::time::timeout(PATIENCE, self.maxds.recv())
            .await
            .expect("a session in time")
            .expect("the server is there");
        let mut maxd = FakeMaxd::new(stream);
        assert_eq!(
            maxd.recv().await,
            Some(ClientMessage::Phone { phone: phone() })
        );
        maxd
    }
}

#[tokio::test]
async fn the_phone_says_hello_through_its_own_ssh() {
    let host = key(7);
    let mut rig = PhoneRig::new(1, &host, &key(1));
    let (_handle, mut events) = rig.link(&entry(&[DEVBOX], &host));
    let mut maxd = rig.next_maxd().await;
    maxd.accept(InstanceId::generate(), "boot-1").await;
    assert!(is_connected(&settled(&mut events).await));
}

#[tokio::test]
async fn a_host_showing_another_key_is_refused() {
    let mut rig = PhoneRig::new(1, &key(8), &key(1));
    let (_handle, mut events) = rig.link(&entry(&[DEVBOX], &key(7)));
    assert_eq!(
        refused_with(&mut events).await,
        "the device showed a different host key"
    );
    assert!(rig.maxds.try_recv().is_err(), "nothing ran");
}

#[tokio::test]
async fn a_phone_the_device_doesnt_know_is_refused() {
    let host = key(7);
    let mut rig = PhoneRig::new(2, &host, &key(1));
    let (_handle, mut events) = rig.link(&entry(&[DEVBOX], &host));
    assert_eq!(refused_with(&mut events).await, "SSH sign-in was refused");
    assert!(rig.maxds.try_recv().is_err(), "nothing ran");
}

#[tokio::test]
async fn a_route_that_hangs_gives_way_to_the_next_and_is_remembered() {
    let host = key(7);
    let mut rig = PhoneRig::new(1, &host, &key(1));
    rig.client.set_network(Some("wifi".into()));
    let (_handle, mut events) = rig.link(&entry(&[SILENT, DEVBOX], &host));
    // The silent route takes far longer than this to time out.
    let mut maxd = rig.next_maxd().await;
    maxd.accept(InstanceId::generate(), "boot-1").await;
    loop {
        if let LinkEvent::RouteWorked { net, route } = next(&mut events).await {
            assert_eq!(net, "wifi");
            assert_eq!(route, ssh(&format!("ssh://dev@{DEVBOX}:22")));
            break;
        }
    }
}

#[tokio::test]
async fn after_a_cut_the_phone_sends_its_action_again_under_its_key() {
    let host = key(7);
    let mut rig = PhoneRig::new(1, &host, &key(1));
    let (handle, mut events) = rig.link(&entry(&[DEVBOX], &host));
    let instance = InstanceId::generate();
    let mut maxd = rig.next_maxd().await;
    maxd.accept(instance, "boot-1").await;
    assert!(is_connected(&settled(&mut events).await));

    let key = IdempotencyKey::generate();
    let action = interrupt(key);
    let (outcome, ()) = tokio::join!(handle.act(action.clone()), async {
        assert_eq!(maxd.recv().await, Some(action.clone()));
        drop(maxd);
        let mut maxd = rig.next_maxd().await;
        maxd.accept(instance, "boot-1").await;
        assert_eq!(maxd.recv().await, Some(action.clone()));
        maxd.send(&result(key, ActionOutcome::Done)).await;
    });
    assert_eq!(outcome, Ok(ActionOutcome::Done));
}
