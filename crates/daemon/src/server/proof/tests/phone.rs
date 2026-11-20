// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Phones and pairings at the gate: a Phone or Pair first frame grants
//! nothing until the connection proves the key the frame names, and the
//! phone's own client proves it through its prover.

use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use maxhq_app::phone::{PhoneProver, NO_LONGER_KNOWN};
use maxhq_client::proof::Prover;
use maxhq_client::LinkEvent;
use maxhq_core::prove::Kind;
use maxhq_protocol::ids::{AgentId, DeviceId, IdempotencyKey, PairId, PhoneId};
use maxhq_protocol::link::LinkState;
use maxhq_protocol::message::{ActionOutcome, ErrorCode, ServerHello};
use maxhq_protocol::{ClientMessage, ProtocolRange, ServerMessage};

use super::app::{App, Dial};
use super::{ask, may_sync, refused, unproven, MISMATCH};
use crate::app_keys::testing::SEED;
use crate::phones::{book_for_tests, for_tests, openssh_for_tests};
use crate::server::tests::{hello, serve, Served, TestClient};

// The phones service is one for the whole test run, so each test lets in
// phones of its own.
const ALONE: [u8; 32] = [221; 32];
const PROVES: [u8; 32] = [222; 32];
const OTHER: [u8; 32] = [223; 32];
const PAIRS: [u8; 32] = [224; 32];
const OWN: [u8; 32] = [225; 32];
const PAIRS_ITSELF: [u8; 32] = [226; 32];
/// A key no phone here has.
const STRANGER: [u8; 32] = [227; 32];
const SQUATTED: [u8; 32] = [228; 32];

const UNKNOWN_PHONE: &str = "Unknown key: mac doesn't know this phone's key.";
const UNKNOWN_PAIRING: &str = "Unknown key: that isn't this pairing's key.";

/// Lets in the phone whose key comes from `seed`, as the Mac does, and
/// gives its ID.
fn known(seed: &[u8; 32]) -> PhoneId {
    let ssh_public = openssh_for_tests(seed);
    let phone = PhoneId::derive(&ssh_public);
    let added = for_tests().add(phone, "iPhone", &ssh_public, &STANDARD.encode([8u8; 32]));
    // With network access on, the phone gets a code too.
    assert!(
        matches!(
            added,
            ActionOutcome::Done | ActionOutcome::PhoneAdded { .. }
        ),
        "{added:?}"
    );
    phone
}

/// A pairing, and its one-time key's seed, as the Mac's code carries them.
fn pairing() -> (PairId, [u8; 32]) {
    let ActionOutcome::PhonePair { pair, seed, .. } = for_tests().begin(book_for_tests(), None)
    else {
        panic!("expected a pairing");
    };
    let seed = STANDARD.decode(seed.expose()).unwrap().try_into().unwrap();
    (pair, seed)
}

/// Connects with `first` ahead, as `maxd proxy` sends it, and says hello
/// as the app, without proving anything.
async fn after(served: &Served, first: ClientMessage) -> (TestClient, ServerHello) {
    let mut client = TestClient::connect(&served.socket).await;
    client.send(&first).await;
    client.send(&hello(ProtocolRange::OURS)).await;
    match client.recv().await {
        ServerMessage::Hello(server) => (client, server),
        other => panic!("expected a hello, got {other:?}"),
    }
}

/// Proves `client`, which got `server`'s hello, holds `seed`'s key as a
/// `kind` connection.
async fn prove(
    client: &mut TestClient,
    kind: Kind,
    seed: &[u8; 32],
    server: &ServerHello,
) -> ActionOutcome {
    let hello = hello(ProtocolRange::OURS);
    client.prove_as(kind, seed, server, &hello).await
}

fn pair_phone(seed: &[u8; 32]) -> ClientMessage {
    ClientMessage::PairPhone {
        key: IdempotencyKey::generate(),
        name: "iPhone".into(),
        ssh_public: openssh_for_tests(seed),
        push_public: STANDARD.encode([8u8; 32]),
    }
}

#[tokio::test]
async fn a_phone_frame_alone_gets_only_what_grants_nothing() {
    let served = serve();
    let phone = known(&ALONE);
    let (mut client, _) = after(&served, ClientMessage::Phone { phone }).await;
    // A phone app that doesn't prove reads nothing and does nothing.
    assert!(!may_sync(&mut client).await);
    let stop = ClientMessage::StopAgent {
        key: IdempotencyKey::generate(),
        agent: AgentId::derive("agent"),
    };
    assert_eq!(ask(&mut client, stop).await, refused(&unproven("mac")));
    client.send(&ClientMessage::Ping { nonce: 7 }).await;
    assert!(matches!(
        client.recv().await,
        ServerMessage::Pong { nonce: 7 }
    ));
}

#[tokio::test]
async fn a_phone_proves_only_with_its_own_key_for_this_connection() {
    let served = serve();
    let phone = known(&PROVES);
    let (mut client, server) = after(&served, ClientMessage::Phone { phone }).await;
    known(&OTHER);

    // Another phone's key, or one no phone has.
    for seed in [OTHER, STRANGER] {
        assert_eq!(
            prove(&mut client, Kind::Phone, &seed, &server).await,
            refused(UNKNOWN_PHONE)
        );
    }
    // The app key signs for the app, and not for a phone's connection.
    assert_eq!(
        prove(&mut client, Kind::App, &SEED, &server).await,
        refused(MISMATCH)
    );
    // The phone's own key, but for another connection's challenge.
    let (_elsewhere, theirs) = after(&served, ClientMessage::Phone { phone }).await;
    assert_eq!(
        prove(&mut client, Kind::Phone, &PROVES, &theirs).await,
        refused(MISMATCH)
    );
    // Or for another device.
    let moved = ServerHello {
        device: DeviceId::derive("devbox"),
        ..server.clone()
    };
    assert_eq!(
        prove(&mut client, Kind::Phone, &PROVES, &moved).await,
        refused(MISMATCH)
    );
    assert!(!may_sync(&mut client).await);

    assert_eq!(
        prove(&mut client, Kind::Phone, &PROVES, &server).await,
        ActionOutcome::Done
    );
    assert!(may_sync(&mut client).await);
}

#[tokio::test]
async fn a_pair_frame_alone_may_only_prove_and_then_pair() {
    let served = serve();
    // Another pairing, which a connection of its own names.
    let (theirs, their_seed) = pairing();
    let (_taken, _) = after(&served, ClientMessage::Pair { pair: theirs }).await;
    let (pair, seed) = pairing();
    let (mut client, server) = after(&served, ClientMessage::Pair { pair }).await;

    assert_eq!(
        ask(&mut client, pair_phone(&PAIRS)).await,
        refused(&unproven("mac"))
    );
    client.send(&ClientMessage::Sync { since: None }).await;
    assert!(matches!(
        client.recv().await,
        ServerMessage::Error {
            code: ErrorCode::NotAllowed,
            message,
        } if message == "this connection may only pair a phone"
    ));
    assert_eq!(
        prove(&mut client, Kind::Pair, &their_seed, &server).await,
        refused(UNKNOWN_PAIRING)
    );
    assert_eq!(
        prove(&mut client, Kind::Phone, &seed, &server).await,
        refused(MISMATCH)
    );

    assert_eq!(
        prove(&mut client, Kind::Pair, &seed, &server).await,
        ActionOutcome::Done
    );
    let paired = ask(&mut client, pair_phone(&PAIRS)).await;
    let ActionOutcome::PhonePaired { phone, book, .. } = paired else {
        panic!("expected the phone paired, got {paired:?}");
    };
    assert_eq!(phone, PhoneId::derive(&openssh_for_tests(&PAIRS)));
    assert_eq!(book, book_for_tests());
}

#[tokio::test]
async fn a_pair_frame_without_the_key_leaves_the_pairing_to_the_phone() {
    let served = serve();
    let (pair, seed) = pairing();
    // A program that knows only the pairing's ID, as maxd's log shows it.
    let (mut squatter, server) = after(&served, ClientMessage::Pair { pair }).await;
    assert_eq!(
        prove(&mut squatter, Kind::Pair, &STRANGER, &server).await,
        refused(UNKNOWN_PAIRING)
    );
    drop(squatter);

    // The phone with the code still pairs, and proving again holds.
    let (mut client, server) = after(&served, ClientMessage::Pair { pair }).await;
    for _ in 0..2 {
        assert_eq!(
            prove(&mut client, Kind::Pair, &seed, &server).await,
            ActionOutcome::Done
        );
    }
    let paired = ask(&mut client, pair_phone(&SQUATTED)).await;
    assert!(
        matches!(paired, ActionOutcome::PhonePaired { .. }),
        "{paired:?}"
    );

    // Taken now, so a frame naming it is turned away at once.
    let mut late = TestClient::connect(&served.socket).await;
    late.send(&ClientMessage::Pair { pair }).await;
    assert!(matches!(
        late.recv().await,
        ServerMessage::Error {
            code: ErrorCode::NotAllowed,
            ..
        }
    ));
}

/// The phone's own client over `maxd proxy`, which sends `first` ahead,
/// proving with `prover`.
fn phone_app(served: &Served, first: &ClientMessage, prover: Arc<dyn Prover>) -> App {
    let first = crate::proxy::first_frame(first).unwrap();
    let dial = Dial {
        socket: served.socket.clone(),
        first,
    };
    App::through(dial, prover)
}

#[tokio::test]
async fn the_phones_client_proves_its_key_or_says_to_pair_again() {
    let served = serve();
    let phone = known(&OWN);
    let first = ClientMessage::Phone { phone };

    // A phone whose key the device doesn't know for it.
    let mut lost = phone_app(&served, &first, PhoneProver::phone(&STRANGER));
    let reason = loop {
        if let Err(reason) = lost.try_next().await {
            break reason;
        }
    };
    assert_eq!(reason, format!("mac {NO_LONGER_KNOWN} the Mac."));

    // Its own key gets it past the gate.
    let mut own = phone_app(&served, &first, PhoneProver::phone(&OWN));
    own.synced().await;
}

#[tokio::test]
async fn the_phones_client_pairs_with_the_one_time_key() {
    let served = serve();
    let (pair, seed) = pairing();
    let first = ClientMessage::Pair { pair };
    let mut app = phone_app(&served, &first, PhoneProver::pairing(&seed));
    loop {
        if let LinkEvent::Link(link) = app.next().await
            && matches!(link.state, LinkState::Connected { .. })
        {
            break;
        }
    }
    let paired = app.link.act(pair_phone(&PAIRS_ITSELF)).await.unwrap();
    assert!(
        matches!(paired, ActionOutcome::PhonePaired { .. }),
        "{paired:?}"
    );
}
