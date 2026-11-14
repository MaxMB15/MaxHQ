// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use maxhq_core::prove::Challenge;
use maxhq_protocol::admission::AppKeyOrigin;
use maxhq_protocol::ids::{
    AdmissionId, AgentId, ClientId, DeviceId, IdempotencyKey, InstanceId, StreamId, WorkspaceId,
};
use maxhq_protocol::message::{ActionOutcome, ErrorCode, ServerHello, TunnelEnd};
use maxhq_protocol::{ClientMessage, ProtocolRange, ServerMessage};

use super::{open, open_to_root, unproven};
use crate::app_keys::testing::{self, SEED};
use crate::server::tests::{hello, hub, serve, serve_bare, TestClient, WAIT};

mod app;
mod phone;

/// Another app key's seed.
const NEW: [u8; 32] = [9; 32];

/// The client ID every test hello carries.
fn me() -> ClientId {
    ClientId::derive("test")
}

/// Whether `client` may read the state, which only a proven app may.
async fn may_sync(client: &mut TestClient) -> bool {
    client.send(&ClientMessage::Sync { since: None }).await;
    loop {
        match client.recv().await {
            ServerMessage::Snapshot(_) => return true,
            ServerMessage::Error {
                code: ErrorCode::NotAllowed,
                message,
            } => {
                assert_eq!(message, unproven("mac"));
                return false;
            }
            _ => {}
        }
    }
}

/// Sends `message`, an action, and waits for its outcome.
async fn ask(client: &mut TestClient, message: ClientMessage) -> ActionOutcome {
    let key = message.action_key().expect("an action");
    client.send(&message).await;
    client.outcome(key).await
}

async fn enroll(client: &mut TestClient, label: &str) -> ActionOutcome {
    let message = ClientMessage::EnrollAppKey {
        key: IdempotencyKey::generate(),
        label: label.into(),
    };
    ask(client, message).await
}

async fn remove(client: &mut TestClient, public_key: String) -> ActionOutcome {
    let message = ClientMessage::RemoveAppKey {
        key: IdempotencyKey::generate(),
        public_key,
    };
    ask(client, message).await
}

fn refused(reason: &str) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.into(),
    }
}

const UNKNOWN: &str = "Unknown key: mac doesn't trust this app key yet.";
const MISMATCH: &str = "Bad proof: the signature doesn't match.";

#[test]
fn only_what_grants_nothing_is_open() {
    let stream = StreamId::derive("stream");
    let key = IdempotencyKey::derive;
    let open_set = [
        ClientMessage::Ping { nonce: 1 },
        ClientMessage::Prove {
            key: key("prove"),
            public_key: String::new(),
            signature: String::new(),
        },
        ClientMessage::EnrollAppKey {
            key: key("enroll"),
            label: String::new(),
        },
        ClientMessage::GuestSshOpen {
            stream,
            workspace: WorkspaceId::derive("workspace"),
        },
        ClientMessage::TunnelCredit { stream, granted: 1 },
        ClientMessage::TunnelClose {
            stream,
            how: TunnelEnd::Reset,
        },
    ];
    for message in &open_set {
        assert!(open(message), "{message:?}");
        assert!(open_to_root(message), "{message:?}");
    }
    // Winding the agents down stops their turns, so only root, which the
    // shutdown unit runs as, gets it unproven.
    let wind_down = ClientMessage::WindDown {
        key: key("wind-down"),
        window_secs: 60,
    };
    assert!(!open(&wind_down));
    assert!(open_to_root(&wind_down));
    let closed = [
        ClientMessage::Sync { since: None },
        ClientMessage::StopAgent {
            key: key("stop"),
            agent: AgentId::derive("agent"),
        },
        ClientMessage::RemoveAppKey {
            key: key("remove"),
            public_key: testing::public_key(),
        },
        ClientMessage::AnswerAdmission {
            key: key("admit"),
            admission: AdmissionId::derive("admission"),
            allow: true,
        },
    ];
    for message in &closed {
        assert!(!open(message), "{message:?}");
        assert!(!open_to_root(message), "{message:?}");
    }
}

#[tokio::test]
async fn each_hello_carries_its_own_challenge() {
    let served = serve();
    let (_, first) = TestClient::unproven(&served.socket).await;
    let (_, second) = TestClient::unproven(&served.socket).await;
    let challenge = |hello: &ServerHello| {
        let challenge = hello.challenge.as_deref().expect("a challenge");
        Challenge::parse(challenge).expect("32 bytes in base64url")
    };
    assert_ne!(challenge(&first), challenge(&second));
}

#[tokio::test]
async fn an_unproven_program_gets_only_what_grants_nothing() {
    assert_eq!(
        unproven("mac"),
        "This program hasn't shown mac that it's MaxHQ, so mac won't do that. If it's an older \
         MaxHQ, update it, or allow it from the card on another device."
    );
    let served = serve();
    let (mut client, server) = TestClient::unproven(&served.socket).await;

    // A message without a key hears it as an error, an action as its
    // outcome.
    assert!(!may_sync(&mut client).await);
    let stop = ClientMessage::StopAgent {
        key: IdempotencyKey::generate(),
        agent: AgentId::derive("agent"),
    };
    assert_eq!(ask(&mut client, stop).await, refused(&unproven("mac")));
    assert_eq!(
        remove(&mut client, testing::public_key()).await,
        refused(&unproven("mac"))
    );
    // Nor may it wind the agents down, which only root may unproven.
    let wind_down = ClientMessage::WindDown {
        key: IdempotencyKey::generate(),
        window_secs: 60,
    };
    assert_eq!(ask(&mut client, wind_down).await, refused(&unproven("mac")));
    assert_eq!(served.keys.keys().unwrap().len(), 1);

    // What grants nothing still works.
    client.send(&ClientMessage::Ping { nonce: 3 }).await;
    assert_eq!(client.recv().await, ServerMessage::Pong { nonce: 3 });
    // The test device runs no containers, which the guests' pipe says, and
    // not the gate.
    let stream = StreamId::derive("stream");
    let open = ClientMessage::GuestSshOpen {
        stream,
        workspace: WorkspaceId::derive("workspace"),
    };
    client.send(&open).await;
    assert!(matches!(
        client.recv().await,
        ServerMessage::TunnelRefused {
            stream: refused,
            code: ErrorCode::NoSuchWorkspace,
            ..
        } if refused == stream
    ));

    // Proving the trusted key opens the rest.
    client.prove(&server, &hello(ProtocolRange::OURS)).await;
    assert!(may_sync(&mut client).await);
}

#[tokio::test]
async fn a_proof_counts_only_for_the_connection_it_signs() {
    let served = serve();
    let (_, other) = TestClient::unproven(&served.socket).await;
    let (mut client, server) = TestClient::unproven(&served.socket).await;

    let another_device = ServerHello {
        device: DeviceId::derive("elsewhere"),
        ..server.clone()
    };
    let another_run = ServerHello {
        instance: InstanceId::generate(),
        ..server.clone()
    };
    for proof in [
        // Another connection's challenge.
        testing::prove(&other, me()),
        // Another client's ID.
        testing::prove(&server, ClientId::derive("other")),
        testing::prove(&another_device, me()),
        testing::prove(&another_run, me()),
    ] {
        assert_eq!(ask(&mut client, proof).await, refused(MISMATCH));
    }
    let garbled = ClientMessage::Prove {
        key: IdempotencyKey::generate(),
        public_key: "not a key".into(),
        signature: "not a signature".into(),
    };
    assert_eq!(
        ask(&mut client, garbled).await,
        refused("Bad proof: the key or the signature isn't Ed25519.")
    );
    assert!(!may_sync(&mut client).await);
    // None of them showed a key this connection holds.
    assert_eq!(
        enroll(&mut client, "MaxHQ").await,
        refused("Prove the key first. This connection hasn't shown it holds one.")
    );

    client.prove(&server, &hello(ProtocolRange::OURS)).await;
    assert!(may_sync(&mut client).await);
}

#[tokio::test]
async fn a_device_that_trusts_no_key_takes_the_first_and_asks_about_the_next() {
    let served = serve_bare(hub());
    let (mut first, server) = TestClient::unproven(&served.socket).await;
    let proof = testing::prove(&server, me());
    assert_eq!(ask(&mut first, proof).await, refused(UNKNOWN));
    assert!(!may_sync(&mut first).await);

    // The test binary has no team, so the signature check is off, and the
    // first key gets in on its own.
    assert_eq!(
        enroll(&mut first, "MaxHQ on the Mac").await,
        ActionOutcome::Done
    );
    let keys = served.keys.keys().unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].public_key, testing::public_key());
    assert_eq!(keys[0].label, "MaxHQ on the Mac");
    assert_eq!(keys[0].origin, AppKeyOrigin::First);
    assert!(may_sync(&mut first).await);
    // Asking again changes nothing.
    assert_eq!(enroll(&mut first, "again").await, ActionOutcome::Done);
    assert_eq!(served.keys.keys().unwrap(), keys);

    // Any key after it needs someone to say yes.
    let (mut next, server) = TestClient::unproven(&served.socket).await;
    let proof = testing::prove_with(&NEW, &server, me());
    assert_eq!(ask(&mut next, proof).await, refused(UNKNOWN));
    assert_eq!(enroll(&mut next, "MaxHQ").await, ActionOutcome::Asked);
    assert!(!may_sync(&mut next).await);
    assert_eq!(served.keys.keys().unwrap(), keys);
}

#[tokio::test]
async fn a_proven_app_rotates_its_key() {
    let served = serve();
    let (mut app, server) = TestClient::greeted(&served.socket).await;
    let (mut old, _) = TestClient::greeted(&served.socket).await;

    // A new key the device doesn't trust yet leaves the app proven.
    let proof = testing::prove_with(&NEW, &server, me());
    assert_eq!(ask(&mut app, proof).await, refused(UNKNOWN));
    assert!(may_sync(&mut app).await);

    // The app adds it on its own word, with no card.
    assert_eq!(
        enroll(&mut app, "MaxHQ, new key").await,
        ActionOutcome::Done
    );
    let new = testing::public_key_of(&NEW);
    let added = served.keys.keys().unwrap();
    let added = added.iter().find(|key| key.public_key == new).unwrap();
    assert_eq!(added.origin, AppKeyOrigin::App);
    assert_eq!(added.label, "MaxHQ, new key");

    // Dropping the old key closes the connections that proved it, and
    // not the one that moved on.
    assert_eq!(
        remove(&mut app, testing::public_key()).await,
        ActionOutcome::Done
    );
    let closed = tokio::time::timeout(WAIT, async { while old.try_recv().await.is_some() {} });
    closed.await.expect("the old key's connection closes");
    let keys = served.keys.keys().unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].public_key, new);
    assert!(may_sync(&mut app).await);

    // The old key no longer proves anything.
    let (mut again, server) = TestClient::unproven(&served.socket).await;
    let proof = testing::prove_with(&SEED, &server, me());
    assert_eq!(ask(&mut again, proof).await, refused(UNKNOWN));
}

#[tokio::test]
async fn the_last_app_key_stays() {
    let served = serve();
    let (mut app, _) = TestClient::greeted(&served.socket).await;
    assert_eq!(
        remove(&mut app, testing::public_key()).await,
        refused("That's the only app key mac trusts, so it stays. Add another first.")
    );
    assert_eq!(served.keys.keys().unwrap().len(), 1);
    // A key it doesn't trust is as good as removed, so a resend is safe.
    let stranger = testing::public_key_of(&NEW);
    assert_eq!(remove(&mut app, stranger).await, ActionOutcome::Done);
    assert!(may_sync(&mut app).await);
}
