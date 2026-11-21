// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A connection another device's `maxd` opened, through `maxd proxy
//! --peer`. It says hello as a peer and proves it holds that device's key,
//! the one `authorized_keys` lets in for it. Then it sends agent calls and
//! wakes, and answers the ones this device sends back over it. Nothing
//! else is let through: no sync, secrets, workspace or settings. The one
//! exception is the model streams a request on the link opened, whose
//! bytes and credit come in tunnel frames and messages.
//!
//! The frame that names the device proves nothing, since any program on
//! this device can write one to the socket. Until the proof, the link
//! isn't one: its calls are refused and nothing goes back over it.

use std::sync::Arc;

use maxhq_core::prove::{self, Challenge, Kind, Statement};
use maxhq_protocol::agents::ClientRole;
use maxhq_protocol::frame::{self, Frame, FrameDecoder};
use maxhq_protocol::ids::{ClientId, DeviceId};
use maxhq_protocol::message::{ActionOutcome, ErrorCode, ServerHello};
use maxhq_protocol::{ClientMessage, ProtocolRange, ServerMessage};
use maxhq_wire::{Queue, Sender};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

use super::{Peers, Via, MAX_REQUESTS};

/// How much is read from the socket at once.
const READ_CHUNK: usize = 64 * 1024;

/// What a peer connection is refused for anything but its own messages.
const ONLY_PEER: &str = "a link from another device carries only agent calls and wakes";

/// What a call on a link that hasn't proved its key is refused with. A
/// `maxd` from before peer proofs sends calls without one.
pub(super) const UNPROVEN: &str = "this device takes another device's calls only once that \
                                   device proves it holds its key. Update maxd on the device \
                                   that called";

/// Where a peer's connection is.
enum Stage {
    /// Waiting for its hello.
    Hello,
    /// It said hello with this client ID, which its proof signs.
    Proving(ClientId),
    /// It proved its key, and its link counts, by this serial, until it
    /// closes.
    Linked(ClientId, u64),
}

/// Serves the link `device` dialed until it closes. `decoder` holds what
/// came after the frame that named the device, and `hello` is what this
/// `maxd` says hello with, its challenge included.
pub async fn serve(
    peers: Peers,
    device: DeviceId,
    mut reader: impl AsyncRead + Unpin,
    mut decoder: FrameDecoder,
    wire: Sender,
    hello: ServerHello,
) {
    let mut stage = Stage::Hello;
    let mut buf = vec![0; READ_CHUNK];
    let running = Arc::new(Semaphore::new(MAX_REQUESTS));
    'read: loop {
        loop {
            let message = match decoder.next_frame() {
                Ok(None) => break,
                Ok(Some(Frame::Message(json))) => serde_json::from_slice::<ClientMessage>(json),
                Ok(Some(Frame::TunnelData { stream, data })) => {
                    if let Stage::Linked(_, serial) = stage {
                        peers.tunnel_data(device, serial, stream, data);
                    }
                    continue;
                }
                Ok(Some(_)) => continue,
                Err(err) => {
                    debug!(%err, %device, "a peer sent a broken frame");
                    break 'read;
                }
            };
            let message = match message {
                Ok(message) => message,
                Err(err) => {
                    refuse(&wire, ErrorCode::BadMessage, err.to_string()).await;
                    continue;
                }
            };
            match stage {
                Stage::Hello => match greet(&peers, device, &wire, message, &hello).await {
                    Greeted::Yes(client) => stage = Stage::Proving(client),
                    Greeted::No => {}
                    Greeted::Never => break 'read,
                },
                Stage::Proving(client) => {
                    let asked = Asked {
                        device,
                        client,
                        hello: &hello,
                    };
                    if before_proof(&peers, &asked, &wire, message).await {
                        info!(%device, "a device proved its key, so its link counts");
                        stage = Stage::Linked(client, peers.attach(device, wire.clone()));
                    }
                }
                Stage::Linked(client, serial) => {
                    let asked = Asked {
                        device,
                        client,
                        hello: &hello,
                    };
                    if !handle(&peers, &asked, serial, &wire, message, &running).await {
                        warn!(%device, "a peer asked too much at once, so its link closes");
                        break 'read;
                    }
                }
            }
        }
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => decoder.push(&buf[..n]),
        }
    }
    if let Stage::Linked(_, serial) = stage {
        peers.detach(device, serial);
    }
    wire.close();
}

enum Greeted {
    Yes(ClientId),
    No,
    Never,
}

/// Answers what came before the hello.
async fn greet(
    peers: &Peers,
    device: DeviceId,
    wire: &Sender,
    message: ClientMessage,
    hello: &ServerHello,
) -> Greeted {
    let ClientMessage::Hello(asked) = message else {
        refuse(wire, ErrorCode::NoHello, "say hello first".into()).await;
        return Greeted::No;
    };
    if asked.role != ClientRole::Peer {
        refuse(wire, ErrorCode::NotAllowed, ONLY_PEER.into()).await;
        return Greeted::Never;
    }
    if !peers.knows(device) {
        let reason = "this device doesn't link to that one. Open MaxHQ to update its devices";
        refuse(wire, ErrorCode::NotAllowed, reason.into()).await;
        return Greeted::Never;
    }
    if ProtocolRange::OURS.agree(asked.protocol).is_none() {
        let reason = "this maxd and that device's speak no protocol version in common";
        refuse(wire, ErrorCode::UnsupportedProtocol, reason.into()).await;
        return Greeted::Never;
    }
    reply(wire, &ServerMessage::Hello(hello.clone())).await;
    Greeted::Yes(asked.client_id)
}

/// Answers one message between the hello and the proof. True once the
/// connection proves the device's key.
async fn before_proof(
    peers: &Peers,
    asked: &Asked<'_>,
    wire: &Sender,
    message: ClientMessage,
) -> bool {
    let device = asked.device;
    match message {
        ClientMessage::Ping { nonce } => reply(wire, &ServerMessage::Pong { nonce }).await,
        ClientMessage::Prove {
            key,
            public_key,
            signature,
        } => {
            let outcome = asked.prove(peers, &public_key, &signature).await;
            let proven = outcome == ActionOutcome::Done;
            reply(wire, &ServerMessage::ActionResult { key, outcome }).await;
            return proven;
        }
        ClientMessage::PeerRequest { key, .. } => {
            debug!(%device, "refused a call from a link that hasn't proved its key");
            let outcome = refused(UNPROVEN);
            reply(wire, &ServerMessage::ActionResult { key, outcome }).await;
        }
        // Nothing went to this connection for it to answer.
        ClientMessage::PeerAnswer { .. } => {
            debug!(%device, "dropped an answer from a link that hasn't proved its key");
        }
        ClientMessage::Hello(_) => {
            let already = "this connection already said hello".into();
            refuse(wire, ErrorCode::BadMessage, already).await;
        }
        _ => refuse(wire, ErrorCode::NotAllowed, ONLY_PEER.into()).await,
    }
    false
}

/// What a proof on this connection signs, but for its challenge.
struct Asked<'a> {
    /// The device its first frame named.
    device: DeviceId,
    /// The client ID its hello carried.
    client: ClientId,
    /// What this `maxd` said hello with.
    hello: &'a ServerHello,
}

impl Asked<'_> {
    /// Checks a proof that the connection holds the device's key: a
    /// signature over this connection's challenge, by a key the device's
    /// lines in `authorized_keys` let in.
    async fn prove(&self, peers: &Peers, public_key: &str, signature: &str) -> ActionOutcome {
        let challenge = self.hello.challenge.as_deref().and_then(Challenge::parse);
        let Some(challenge) = challenge else {
            return refused("maxd had no challenge for this connection. Its log says why.");
        };
        let statement = Statement {
            kind: Kind::Peer,
            device: self.hello.device,
            instance: self.hello.instance,
            client: self.client,
            challenge,
        };
        let key = match prove::verify(public_key, signature, &statement) {
            Ok(key) => key,
            Err(err) => return refused(format!("Bad proof: {err}.")),
        };
        let (check, device) = (peers.clone(), self.device);
        match tokio::task::spawn_blocking(move || check.lets_in(device, key)).await {
            Ok(Ok(true)) => ActionOutcome::Done,
            Ok(Ok(false)) => refused(format!(
                "Unknown key: it isn't the key this device lets in for {}.",
                peers.name(device)
            )),
            Ok(Err(err)) => {
                warn!(%err, "couldn't read authorized_keys for a peer's proof");
                refused("This device couldn't read its authorized_keys. Its log says why.")
            }
            Err(err) => {
                warn!(%err, "a peer's proof check stopped");
                refused("This device couldn't check the key. Its log says why.")
            }
        }
    }
}

/// Answers one message on a link that proved its key. False when the
/// connection has too many requests running to take another.
async fn handle(
    peers: &Peers,
    asked: &Asked<'_>,
    serial: u64,
    wire: &Sender,
    message: ClientMessage,
    running: &Arc<Semaphore>,
) -> bool {
    let device = asked.device;
    match message {
        ClientMessage::Ping { nonce } => reply(wire, &ServerMessage::Pong { nonce }).await,
        ClientMessage::PeerRequest { key, request } => {
            let Ok(place) = Arc::clone(running).try_acquire_owned() else {
                return false;
            };
            let peers = peers.clone();
            let wire = wire.clone();
            tokio::spawn(async move {
                let via = Via::Incoming(serial);
                let outcome = peers.answer_on(device, key, request, via).await;
                reply(&wire, &ServerMessage::ActionResult { key, outcome }).await;
                drop(place);
            });
        }
        ClientMessage::PeerAnswer { key, outcome } => peers.answered(device, key, outcome),
        // A model stream's credit and close, for a stream a request on
        // this link opened.
        ClientMessage::TunnelCredit { stream, granted } => {
            peers.tunnel_credit(device, serial, stream, granted);
        }
        ClientMessage::TunnelClose { stream, how } => {
            peers.tunnel_close(device, serial, stream, how);
        }
        // A proof again gets its answer, and the link stays whatever it
        // is.
        ClientMessage::Prove {
            key,
            public_key,
            signature,
        } => {
            let outcome = asked.prove(peers, &public_key, &signature).await;
            reply(wire, &ServerMessage::ActionResult { key, outcome }).await;
        }
        ClientMessage::Hello(_) => {
            let already = "this connection already said hello".into();
            refuse(wire, ErrorCode::BadMessage, already).await;
        }
        _ => refuse(wire, ErrorCode::NotAllowed, ONLY_PEER.into()).await,
    }
    true
}

fn refused(reason: impl Into<String>) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.into(),
    }
}

async fn refuse(wire: &Sender, code: ErrorCode, message: String) {
    debug!(?code, %message, "refused a peer's message");
    reply(wire, &ServerMessage::Error { code, message }).await;
}

/// Queues one message on the link's control queue and waits for room, so a
/// peer that stops reading stalls its own connection and nothing piles up
/// here. A link that closed drops it.
async fn reply(wire: &Sender, message: &ServerMessage) {
    let mut out = Vec::new();
    if let Err(err) = frame::encode_message(&mut out, message) {
        warn!(%err, "couldn't encode a peer message");
        return;
    }
    let _ = wire.send(Queue::Control, out).await;
}
