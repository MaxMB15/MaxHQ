// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A connection proving itself after a hello that carries a challenge.

use maxhq_core::prove::{self, Challenge, Statement};
use maxhq_protocol::message::ErrorCode;

use super::*;
use crate::proof::{Kind, Prover, Proving, Verdict};

/// The seed the test prover signs with.
const SEED: [u8; 32] = [7; 32];

/// The seed of a key the test prover moves away from.
const OLD_SEED: [u8; 32] = [3; 32];

/// The label the test prover enrolls under.
const LABEL: &str = "MaxHQ on studio-mac";

/// A prover that proves its key, enrolls it when the device doesn't know
/// it, and with an old key, proves that first and removes it after.
struct TestProver {
    /// The kind of connection it proves.
    kind: Kind,
    /// Whether the Sync goes behind the first proof.
    then_sync: bool,
    /// The key it moves away from, if any.
    old: Option<[u8; 32]>,
}

impl Prover for TestProver {
    fn kind(&self) -> Kind {
        self.kind
    }

    fn prove(&self, proving: Proving) -> BoxFuture<'static, Verdict> {
        let then_sync = self.then_sync;
        let old = self.old;
        Box::pin(async move {
            match run(&proving, then_sync, old).await {
                Ok(verdict) => verdict,
                Err(_) => Verdict::Lost,
            }
        })
    }
}

/// The test prover's steps.
async fn run(
    proving: &Proving,
    then_sync: bool,
    old: Option<[u8; 32]>,
) -> Result<Verdict, LinkError> {
    if let Some(old) = old {
        proving.prove(&old, then_sync).await?;
    }
    if proving.prove(&SEED, then_sync).await? != ActionOutcome::Done {
        match proving.enroll(LABEL).await? {
            ActionOutcome::Done => {}
            ActionOutcome::Refused { reason } => {
                let name = proving.name();
                return Ok(Verdict::Refused(format!(
                    "{name} doesn't trust this app's key: {reason}"
                )));
            }
            other => panic!("unexpected enrollment outcome {other:?}"),
        }
    }
    if let Some(old) = old {
        // The public key doesn't depend on what's signed.
        let any = Statement {
            kind: proving.kind(),
            device: proving.device(),
            instance: InstanceId::derive("any"),
            client: ClientId::derive("any"),
            challenge: Challenge([0; 32]),
        };
        let public_key = prove::sign(&old, &any).unwrap().public_key;
        proving.remove(&public_key).await?;
    }
    Ok(Verdict::Proven)
}

/// A rig whose client proves with a [`TestProver`].
fn proving_rig(timing: Timing, then_sync: bool, old: Option<[u8; 32]>) -> Rig {
    rig_as(Kind::App, timing, then_sync, old)
}

/// A rig whose client proves as a `kind` connection.
fn rig_as(kind: Kind, timing: Timing, then_sync: bool, old: Option<[u8; 32]>) -> Rig {
    let Rig {
        client,
        fake,
        servers,
        tailnet,
    } = Rig::new(timing, Tailnet::default(), &[]);
    Rig {
        client: client.proving(Arc::new(TestProver {
            kind,
            then_sync,
            old,
        })),
        fake,
        servers,
        tailnet,
    }
}

/// Takes the client's hello and answers with a challenge, giving what the
/// client's proofs must sign.
async fn challenge(maxd: &mut FakeMaxd, instance: InstanceId) -> Statement {
    let hello = match maxd.recv().await {
        Some(ClientMessage::Hello(hello)) => hello,
        other => panic!("expected a hello, got {other:?}"),
    };
    let challenge = Challenge([9; 32]);
    maxd.send(&ServerMessage::Hello(ServerHello {
        protocol: ProtocolRange::OURS,
        maxd_version: "0.1.0".into(),
        device: device(),
        boot_id: BootId("boot-1".into()),
        instance,
        capabilities: Capabilities::default(),
        build: None,
        target: None,
        challenge: Some(challenge.encode()),
    }))
    .await;
    Statement {
        kind: Kind::App,
        device: device(),
        instance,
        client: hello.client_id,
        challenge,
    }
}

/// Takes a proof, checks it signs `statement`, and gives its key and the
/// public key it proved.
async fn proof(maxd: &mut FakeMaxd, statement: &Statement) -> (IdempotencyKey, String) {
    match maxd.recv().await {
        Some(ClientMessage::Prove {
            key,
            public_key,
            signature,
        }) => {
            prove::verify(&public_key, &signature, statement).expect("a good signature");
            (key, public_key)
        }
        other => panic!("expected a proof, got {other:?}"),
    }
}

/// The public key of `seed`.
fn public(seed: &[u8; 32], statement: &Statement) -> String {
    prove::sign(seed, statement).unwrap().public_key
}

async fn answer(maxd: &mut FakeMaxd, key: IdempotencyKey, outcome: ActionOutcome) {
    maxd.send(&ServerMessage::ActionResult { key, outcome })
        .await;
}

fn refused(reason: &str) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.into(),
    }
}

/// The link's states until one matches `wanted`.
async fn until_state(
    events: &mut mpsc::Receiver<LinkEvent>,
    wanted: impl Fn(&LinkState) -> bool,
) -> LinkState {
    loop {
        let state = next_state(events).await;
        if wanted(&state) {
            return state;
        }
    }
}

#[tokio::test]
async fn a_trusted_key_proves_with_the_sync_right_behind() {
    let mut rig = proving_rig(FAST, true, None);
    let (_handle, mut events) = rig.client.link(spec(Route::Local));
    let mut maxd = rig.next_maxd().await;
    let statement = challenge(&mut maxd, InstanceId::generate()).await;
    let (key, public_key) = proof(&mut maxd, &statement).await;
    assert_eq!(public_key, public(&SEED, &statement));
    // The Sync doesn't wait for the answer.
    assert_eq!(maxd.recv().await, Some(ClientMessage::Sync { since: None }));
    answer(&mut maxd, key, ActionOutcome::Done).await;
    maxd.send(&snapshot(5, &[1])).await;
    until_state(&mut events, is_connected).await;
    match next(&mut events).await {
        LinkEvent::Synced(entities) => assert!(same(&entities, &[entity(1)])),
        other => panic!("expected a sync, got {other:?}"),
    }
    // Proving again sends only the proof: the Sync went once.
    rig.client.reprove();
    let (again, _) = proof(&mut maxd, &statement).await;
    answer(&mut maxd, again, ActionOutcome::Done).await;
}

#[tokio::test]
async fn an_unknown_key_enrolls_and_then_syncs() {
    let mut rig = proving_rig(FAST, true, None);
    let (_handle, mut events) = rig.client.link(spec(Route::Local));
    let mut maxd = rig.next_maxd().await;
    let statement = challenge(&mut maxd, InstanceId::generate()).await;
    let (key, _) = proof(&mut maxd, &statement).await;
    assert_eq!(maxd.recv().await, Some(ClientMessage::Sync { since: None }));
    answer(&mut maxd, key, refused("unknown key")).await;
    maxd.send(&ServerMessage::Error {
        code: ErrorCode::NotAllowed,
        message: "prove this connection first".into(),
    })
    .await;
    let enroll = match maxd.recv().await {
        Some(ClientMessage::EnrollAppKey { key, label }) => {
            assert_eq!(label, LABEL);
            key
        }
        other => panic!("expected an enrollment, got {other:?}"),
    };
    answer(&mut maxd, enroll, ActionOutcome::Done).await;
    // The first Sync got no answer, so it goes again.
    assert_eq!(maxd.recv().await, Some(ClientMessage::Sync { since: None }));
    until_state(&mut events, is_connected).await;
}

#[tokio::test]
async fn a_device_that_asks_elsewhere_shows_as_waiting_and_stays_connected() {
    let mut rig = proving_rig(FAST, false, None);
    let (_handle, mut events) = rig.client.link(spec(Route::Local));
    let mut maxd = rig.next_maxd().await;
    let statement = challenge(&mut maxd, InstanceId::generate()).await;
    let (key, _) = proof(&mut maxd, &statement).await;
    answer(&mut maxd, key, refused("unknown key")).await;
    let enroll = match maxd.recv().await {
        Some(ClientMessage::EnrollAppKey { key, .. }) => key,
        other => panic!("expected an enrollment, got {other:?}"),
    };
    answer(&mut maxd, enroll, ActionOutcome::Asked).await;
    until_state(&mut events, |state| {
        *state == connecting(ConnectStep::WaitingForApproval, 1)
    })
    .await;
    // Longer than any answer may take, and still waiting.
    tokio::time::sleep(FAST.handshake + Duration::from_millis(200)).await;
    answer(&mut maxd, enroll, ActionOutcome::Done).await;
    assert_eq!(maxd.recv().await, Some(ClientMessage::Sync { since: None }));
    until_state(&mut events, is_connected).await;
}

#[tokio::test]
async fn a_refused_key_leaves_the_link_unreachable_with_the_reason() {
    let mut rig = proving_rig(NO_RETRY, false, None);
    let (_handle, mut events) = rig.client.link(spec(Route::Local));
    let mut maxd = rig.next_maxd().await;
    let statement = challenge(&mut maxd, InstanceId::generate()).await;
    let (key, _) = proof(&mut maxd, &statement).await;
    answer(&mut maxd, key, refused("unknown key")).await;
    let enroll = match maxd.recv().await {
        Some(ClientMessage::EnrollAppKey { key, .. }) => key,
        other => panic!("expected an enrollment, got {other:?}"),
    };
    answer(&mut maxd, enroll, refused("you said no")).await;
    let state = until_state(&mut events, |state| {
        matches!(state, LinkState::Unreachable { .. })
    })
    .await;
    assert_eq!(
        state,
        unreachable("Test box doesn't trust this app's key: you said no", None)
    );
    assert_eq!(maxd.recv().await, None, "the connection closes");
}

#[tokio::test]
async fn a_phone_says_hello_as_the_app_and_proves_as_itself() {
    for kind in [Kind::Phone, Kind::Pair] {
        let mut rig = rig_as(kind, FAST, false, None);
        let (_handle, mut events) = rig.client.link(spec(Route::Local));
        let mut maxd = rig.next_maxd().await;
        let statement = Statement {
            kind,
            ..challenge(&mut maxd, InstanceId::generate()).await
        };
        let (key, _) = proof(&mut maxd, &statement).await;
        answer(&mut maxd, key, ActionOutcome::Done).await;
        assert_eq!(maxd.recv().await, Some(ClientMessage::Sync { since: None }));
        until_state(&mut events, is_connected).await;
    }
}

#[tokio::test]
async fn a_proof_with_no_answer_ends_the_attempt() {
    let timing = Timing {
        handshake: Duration::from_millis(300),
        ..NO_RETRY
    };
    let mut rig = proving_rig(timing, false, None);
    let (_handle, mut events) = rig.client.link(spec(Route::Local));
    let mut maxd = rig.next_maxd().await;
    let statement = challenge(&mut maxd, InstanceId::generate()).await;
    proof(&mut maxd, &statement).await;
    let state = until_state(&mut events, |state| {
        matches!(state, LinkState::Unreachable { .. })
    })
    .await;
    assert_eq!(state, unreachable("maxd didn't answer the proof", None));
}

#[tokio::test]
async fn a_device_that_sends_too_much_before_the_proof_is_dropped() {
    let timing = Timing {
        handshake: Duration::from_secs(30),
        ..NO_RETRY
    };
    let mut rig = proving_rig(timing, true, None);
    let (_handle, mut events) = rig.client.link(spec(Route::Local));
    let mut maxd = rig.next_maxd().await;
    let statement = challenge(&mut maxd, InstanceId::generate()).await;
    proof(&mut maxd, &statement).await;
    assert_eq!(maxd.recv().await, Some(ClientMessage::Sync { since: None }));
    // Snapshots that keep nothing, each a third of what may come before
    // the proof's answer, which never comes.
    let pad = "x".repeat(crate::link::EARLY_BYTES / 3);
    let large = serde_json::json!({ "type": "snapshot", "seq": 1, "entities": [pad] });
    for _ in 0..3 {
        let mut out = Vec::new();
        frame::encode_message(&mut out, &large).unwrap();
        maxd.stream.write_all(&out).await.unwrap();
    }
    let state = until_state(&mut events, |state| {
        matches!(state, LinkState::Unreachable { .. })
    })
    .await;
    assert_eq!(
        state,
        unreachable("maxd sent too much before the proof", None)
    );
}

#[tokio::test]
async fn an_older_maxd_gets_no_proof() {
    let mut rig = proving_rig(FAST, true, None);
    let (_handle, mut events) = rig.client.link(spec(Route::Local));
    let mut maxd = rig.next_maxd().await;
    // No challenge in the hello, and the Sync comes straight after it.
    assert_eq!(maxd.accept(InstanceId::generate(), "boot-1").await, None);
    until_state(&mut events, is_connected).await;
}

#[tokio::test]
async fn requests_wait_until_the_proof_passes() {
    let mut rig = proving_rig(FAST, false, None);
    let (handle, mut events) = rig.client.link(spec(Route::Local));
    let mut maxd = rig.next_maxd().await;
    let statement = challenge(&mut maxd, InstanceId::generate()).await;
    let (key, _) = proof(&mut maxd, &statement).await;
    assert_eq!(
        handle.send(mark_seen()).await,
        Err(LinkError::NotConnected),
        "nothing goes before the proof passes"
    );
    answer(&mut maxd, key, ActionOutcome::Done).await;
    assert_eq!(maxd.recv().await, Some(ClientMessage::Sync { since: None }));
    until_state(&mut events, is_connected).await;
    handle.send(mark_seen()).await.unwrap();
    assert_eq!(maxd.recv().await, Some(mark_seen()));
}

#[tokio::test]
async fn a_new_key_moves_in_behind_the_old_one() {
    let mut rig = proving_rig(FAST, true, Some(OLD_SEED));
    let (_handle, mut events) = rig.client.link(spec(Route::Local));
    let mut maxd = rig.next_maxd().await;
    let statement = challenge(&mut maxd, InstanceId::generate()).await;
    // The old key proves first, with the Sync behind it.
    let (old, public_key) = proof(&mut maxd, &statement).await;
    assert_eq!(public_key, public(&OLD_SEED, &statement));
    assert_eq!(maxd.recv().await, Some(ClientMessage::Sync { since: None }));
    answer(&mut maxd, old, ActionOutcome::Done).await;
    maxd.send(&snapshot(5, &[1])).await;
    // Then the new one, which the device doesn't know yet.
    let (new, public_key) = proof(&mut maxd, &statement).await;
    assert_eq!(public_key, public(&SEED, &statement));
    answer(&mut maxd, new, refused("unknown key")).await;
    let enroll = match maxd.recv().await {
        Some(ClientMessage::EnrollAppKey { key, .. }) => key,
        other => panic!("expected an enrollment, got {other:?}"),
    };
    answer(&mut maxd, enroll, ActionOutcome::Done).await;
    let remove = match maxd.recv().await {
        Some(ClientMessage::RemoveAppKey { key, public_key }) => {
            assert_eq!(public_key, public(&OLD_SEED, &statement));
            key
        }
        other => panic!("expected a removal, got {other:?}"),
    };
    answer(&mut maxd, remove, ActionOutcome::Done).await;
    until_state(&mut events, is_connected).await;
    // The snapshot that came while proving arrives once open, and the Sync
    // went only once.
    match next(&mut events).await {
        LinkEvent::Synced(entities) => assert!(same(&entities, &[entity(1)])),
        other => panic!("expected a sync, got {other:?}"),
    }
}
