// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use maxhq_core::prove::Kind;
use maxhq_protocol::admission::{Admission, AppKeyOrigin};
use maxhq_protocol::ids::{AdmissionId, ClientId, IdempotencyKey, PhoneId};
use maxhq_protocol::message::{ActionOutcome, ErrorCode, ServerHello};
use maxhq_protocol::state::Entity;
use maxhq_protocol::{ClientMessage, ProtocolRange, ServerMessage};
use tokio::time::{sleep, timeout};

use crate::admission::alert::testing::Fake;
use crate::admission::alert::{Alert, Reply};
use crate::admission::Limits;
use crate::app_keys::testing;
use crate::server::proof::unproven;
use crate::server::tests::{hello, serve, Served, TestClient, WAIT};

/// Another app key's seed.
const NEW: [u8; 32] = [9; 32];

/// Short enough for a test, with the expiry and the quiet out of the way.
const QUICK: Limits = Limits {
    delay: Duration::from_millis(100),
    expiry: Duration::from_secs(60),
    quiet: Duration::from_secs(60),
    cap: 3,
};

const NOT_THE_APP: &str = "Only MaxHQ's app, once it has shown mac its key, can answer that card.";
const CLOSED: &str = "That card has closed. Someone answered it, its program disconnected or it \
                      timed out.";

/// A server whose cards come after `limits`, and whose alert is `alert`.
fn served(limits: Limits, alert: Option<Arc<Fake>>) -> Served {
    let served = serve();
    let alert = alert.map(|alert| alert as Arc<dyn Alert>);
    served.keys.admissions().set_for_tests(limits, alert);
    served
}

/// The cards the device shows, oldest first.
fn cards(served: &Served) -> Vec<Admission> {
    let mut cards = served.hub.pick(|entity| match entity {
        Entity::Admission(card) => Some(card.clone()),
        _ => None,
    });
    cards.sort_by_key(|card| card.raised_at);
    cards
}

/// Waits until the device shows `n` cards.
async fn until_cards(served: &Served, n: usize) -> Vec<Admission> {
    let shown = timeout(WAIT, async {
        loop {
            let cards = cards(served);
            if cards.len() == n {
                return cards;
            }
            sleep(Duration::from_millis(10)).await;
        }
    });
    shown
        .await
        .unwrap_or_else(|_| panic!("{n} cards in time, got {:?}", cards(served)))
}

/// Waits until `fake` has shown `n` alerts.
async fn until_shown(fake: &Fake, n: usize) {
    let shown = timeout(WAIT, async {
        while fake.shown().len() < n {
            sleep(Duration::from_millis(10)).await;
        }
    });
    shown.await.expect("the alert in time");
    assert_eq!(fake.shown().len(), n);
}

/// Waits until `done` says so.
async fn until(what: &str, mut done: impl FnMut() -> bool) {
    let waited = timeout(WAIT, async {
        while !done() {
            sleep(Duration::from_millis(10)).await;
        }
    });
    waited.await.unwrap_or_else(|_| panic!("{what} in time"));
}

/// Whether every card `served` shows says `waiting` about the programs
/// that wait.
fn all_say(served: &Served, waiting: Option<&str>) -> bool {
    cards(served)
        .iter()
        .all(|card| fact(card, "Also waiting") == waiting)
}

/// What `card` says about `label`.
fn fact<'a>(card: &'a Admission, label: &str) -> Option<&'a str> {
    let consent = card.question.consent.as_ref()?;
    let fact = consent.facts.iter().find(|fact| fact.label == label)?;
    Some(fact.value.as_str())
}

/// Sends `message`, an action, and waits for its outcome.
async fn ask(client: &mut TestClient, message: ClientMessage) -> ActionOutcome {
    let key = message.action_key().expect("an action");
    client.send(&message).await;
    client.outcome(key).await
}

async fn answer(client: &mut TestClient, card: AdmissionId, allow: bool) -> ActionOutcome {
    let message = ClientMessage::AnswerAdmission {
        key: IdempotencyKey::generate(),
        admission: card,
        allow,
    };
    ask(client, message).await
}

/// Whether `client` may read the state, which a connection only may once
/// it's proven or let in. A refusal asks for a card.
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

/// Connects without proving anything, and asks for more than that allows,
/// which asks for a card.
async fn stranger(served: &Served) -> TestClient {
    let (mut client, _) = TestClient::unproven(&served.socket).await;
    assert!(!may_sync(&mut client).await);
    client
}

fn refused(reason: &str) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.into(),
    }
}

#[tokio::test]
async fn a_program_without_the_key_gets_a_card_after_the_delay() {
    let fake = Fake::new();
    let served = served(
        Limits {
            delay: Duration::from_millis(300),
            ..QUICK
        },
        Some(fake.clone()),
    );
    let (mut app, _) = TestClient::greeted(&served.socket).await;
    let mut other = stranger(&served).await;
    // Nothing shows until the delay passes, so an app that's only updating
    // proves its key first.
    assert!(cards(&served).is_empty());
    let card = until_cards(&served, 1).await.remove(0);

    assert_eq!(card.device, served.hub.device().id);
    assert!(card.expires_at > card.raised_at);
    let question = &card.question;
    assert_eq!(question.id, card.id.to_string());
    let options: Vec<_> = question.options.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(options, ["Don't allow", "Allow until it disconnects"]);
    let consent = question.consent.as_ref().expect("a consent card");
    assert_eq!(consent.title, "Let this program control mac?");
    assert!(consent.remember.is_none());
    assert!(question
        .text
        .starts_with("Let this program control mac?\n\n"));
    assert_eq!(fact(&card, "Device"), Some("mac"));
    assert_eq!(fact(&card, "Says it is"), Some("maxd-test 0"));
    if cfg!(target_os = "macos") {
        assert_eq!(
            fact(&card, "How it reached maxd"),
            Some("This Mac's socket")
        );
        // The app is connected through the same socket.
        assert!(consent.risk.ends_with(
            "MaxHQ is already connected from this Mac, so this is most likely not MaxHQ."
        ));
        // The test binary has no team to check against.
        assert_eq!(
            fact(&card, "Signature"),
            Some("Not checked, since this maxd isn't signed")
        );
        assert!(fact(&card, "Program").is_some());
    } else {
        assert_eq!(
            fact(&card, "How it reached maxd"),
            Some("The socket on mac")
        );
        assert!(consent.risk.ends_with("until it disconnects."));
        assert_eq!(fact(&card, "Program"), None);
    }
    // The proven app shows the card, so the alert doesn't.
    assert!(fake.shown().is_empty());

    // The app sees it too.
    app.send(&ClientMessage::Sync { since: None }).await;
    loop {
        if let ServerMessage::Snapshot(snapshot) = app.recv().await {
            assert!(snapshot.entities.contains(&Entity::Admission(card.clone())));
            break;
        }
    }

    // A yes lets in that connection, and only it.
    let mut another = stranger(&served).await;
    assert_eq!(answer(&mut app, card.id, true).await, ActionOutcome::Done);
    assert!(may_sync(&mut other).await);
    assert!(!may_sync(&mut another).await);
    let left = until_cards(&served, 1).await;
    assert_ne!(left[0].id, card.id, "the answered card goes");
    // Nothing is remembered: the same program connecting again is refused.
    let mut again = stranger(&served).await;
    assert!(!may_sync(&mut again).await);
    // And a card answered once can't be answered again.
    assert_eq!(answer(&mut app, card.id, false).await, refused(CLOSED));
}

#[tokio::test]
async fn a_card_is_never_answered_by_its_own_or_an_unproven_connection() {
    let served = served(QUICK, None);
    let (mut app, _) = TestClient::greeted(&served.socket).await;
    // One at a time, so each card's program is known: two raised in the
    // same millisecond have no order.
    let mut own = stranger(&served).await;
    let mine = until_cards(&served, 1).await[0].id;
    let mut other = stranger(&served).await;
    let cards = until_cards(&served, 2).await;
    let theirs = cards
        .iter()
        .map(|card| card.id)
        .find(|id| *id != mine)
        .unwrap();

    // Its own connection and any other unproven one are turned away at
    // the gate.
    for client in [&mut own, &mut other] {
        for card in [mine, theirs] {
            assert_eq!(answer(client, card, true).await, refused(&unproven("mac")));
        }
    }
    // A connection you let in can do what the app can, but not answer
    // cards: it never showed a key.
    assert_eq!(answer(&mut app, theirs, true).await, ActionOutcome::Done);
    assert!(may_sync(&mut other).await);
    assert_eq!(answer(&mut other, mine, true).await, refused(NOT_THE_APP));
    assert!(!may_sync(&mut own).await);
    assert_eq!(self::cards(&served).len(), 1);

    // Don't allow takes the card down, and the program stays out.
    assert_eq!(answer(&mut app, mine, false).await, ActionOutcome::Done);
    until_cards(&served, 0).await;
    assert!(!may_sync(&mut own).await);
    // It asks no more until it reconnects.
    sleep(QUICK.delay * 3).await;
    assert!(self::cards(&served).is_empty());
}

#[tokio::test]
async fn a_disconnect_drops_the_card() {
    let served = served(
        Limits {
            delay: Duration::from_millis(300),
            ..QUICK
        },
        None,
    );
    let (mut app, _) = TestClient::greeted(&served.socket).await;

    // Gone before the delay: no card ever shows.
    drop(stranger(&served).await);
    sleep(Duration::from_millis(600)).await;
    assert!(cards(&served).is_empty());

    // Gone after it: the card goes with it.
    let other = stranger(&served).await;
    let card = until_cards(&served, 1).await.remove(0);
    drop(other);
    until_cards(&served, 0).await;
    assert_eq!(answer(&mut app, card.id, true).await, refused(CLOSED));
}

#[tokio::test]
async fn at_most_three_cards_and_the_rest_wait_without_one() {
    let served = served(QUICK, None);
    let (mut app, _) = TestClient::greeted(&served.socket).await;
    let _first = stranger(&served).await;
    let _second = stranger(&served).await;
    let _third = stranger(&served).await;
    let fourth = stranger(&served).await;
    let fifth = stranger(&served).await;

    let cards = until_cards(&served, 3).await;
    let two = Some("2 more programs, with no card of their own");
    until("the cards count two waiting", || all_say(&served, two)).await;
    sleep(QUICK.delay * 3).await;
    assert_eq!(self::cards(&served).len(), 3, "no card past the cap");

    // A key to trust is refused too, without a card.
    let (mut sixth, server) = TestClient::unproven(&served.socket).await;
    let proof = testing::prove_with(&NEW, &server, ClientId::derive("test"));
    assert!(matches!(
        ask(&mut sixth, proof).await,
        ActionOutcome::Refused { .. }
    ));
    let enroll = ClientMessage::EnrollAppKey {
        key: IdempotencyKey::generate(),
        label: "MaxHQ".into(),
    };
    assert_eq!(
        ask(&mut sixth, enroll).await,
        refused(
            "mac already has 3 programs waiting for an answer, so it didn't ask about this one. \
             Answer their cards, then reconnect."
        )
    );

    let three = Some("3 more programs, with no card of their own");
    until("the cards count three waiting", || all_say(&served, three)).await;
    assert_eq!(self::cards(&served).len(), 3);

    // As programs leave, the count on the cards goes down.
    drop(fourth);
    drop(sixth);
    let one = Some("1 more program, with no card of its own");
    until("the cards count one waiting", || all_say(&served, one)).await;
    drop(fifth);
    until("the cards count none waiting", || all_say(&served, None)).await;

    // Answering one makes room, and the next program to ask gets a card.
    assert_eq!(
        answer(&mut app, cards[0].id, false).await,
        ActionOutcome::Done
    );
    until_cards(&served, 2).await;
    let _later = stranger(&served).await;
    until_cards(&served, 3).await;
}

#[tokio::test]
async fn a_card_nobody_answers_counts_as_dont_allow() {
    let served = served(
        Limits {
            expiry: Duration::from_millis(300),
            ..QUICK
        },
        None,
    );
    let (mut app, _) = TestClient::greeted(&served.socket).await;
    let mut other = stranger(&served).await;
    let card = until_cards(&served, 1).await.remove(0);
    until_cards(&served, 0).await;
    assert_eq!(answer(&mut app, card.id, true).await, refused(CLOSED));
    // The program stays out, and asks no more until it reconnects.
    assert!(!may_sync(&mut other).await);
    sleep(QUICK.delay * 3).await;
    assert!(cards(&served).is_empty());
}

#[tokio::test]
async fn the_alert_shows_only_while_no_proven_app_is_connected() {
    let fake = Fake::new();
    let served = served(QUICK, Some(fake.clone()));

    // No app: the alert shows the card's words.
    let mut first = stranger(&served).await;
    let card = until_cards(&served, 1).await.remove(0);
    until_shown(&fake, 1).await;
    let shown = &fake.shown()[0];
    let consent = card.question.consent.as_ref().unwrap();
    assert_eq!(shown.title, consent.title);
    assert_eq!(shown.risk, consent.risk);
    assert_eq!(shown.allow, "Allow until it disconnects");
    assert_eq!(shown.text(), card.question.text);

    // Its yes lets that program in.
    fake.reply(Reply::Allow);
    until_cards(&served, 0).await;
    assert!(may_sync(&mut first).await);

    // An app proving its key takes the alert down, and shows the card
    // itself.
    let _second = stranger(&served).await;
    until_shown(&fake, 2).await;
    let (mut app, _) = TestClient::greeted(&served.socket).await;
    let closed = timeout(WAIT, async {
        while fake.closed() == 0 {
            sleep(Duration::from_millis(10)).await;
        }
    });
    closed.await.expect("the alert closes");
    let card = until_cards(&served, 1).await.remove(0);

    // While it's connected, no card shows as an alert.
    let _third = stranger(&served).await;
    until_cards(&served, 2).await;
    sleep(QUICK.delay * 3).await;
    assert_eq!(fake.shown().len(), 2);

    // Once the last app is gone, the alert shows the card left.
    assert_eq!(answer(&mut app, card.id, true).await, ActionOutcome::Done);
    let left = until_cards(&served, 1).await.remove(0);
    drop(app);
    until_shown(&fake, 3).await;
    assert_eq!(fake.shown()[2].text(), left.question.text);
}

#[tokio::test]
async fn the_alert_stays_quiet_after_dont_allow() {
    let fake = Fake::new();
    let served = served(QUICK, Some(fake.clone()));
    let mut first = stranger(&served).await;
    until_shown(&fake, 1).await;
    fake.reply(Reply::DontAllow);
    until_cards(&served, 0).await;
    assert!(!may_sync(&mut first).await);

    // The next card waits for the app instead.
    let _second = stranger(&served).await;
    until_cards(&served, 1).await;
    sleep(QUICK.delay * 3).await;
    assert_eq!(fake.shown().len(), 1);
}

#[tokio::test]
async fn the_alert_closes_as_dont_allow_when_the_card_expires() {
    let fake = Fake::new();
    let served = served(
        Limits {
            expiry: Duration::from_millis(300),
            ..QUICK
        },
        Some(fake.clone()),
    );
    let mut first = stranger(&served).await;
    until_shown(&fake, 1).await;
    until_cards(&served, 0).await;
    assert!(!may_sync(&mut first).await);
    // An expired card counts as Don't allow, so the alert stays quiet too.
    let _second = stranger(&served).await;
    until_cards(&served, 1).await;
    sleep(QUICK.delay * 3).await;
    assert_eq!(fake.shown().len(), 1);
}

#[tokio::test]
async fn a_yes_trusts_the_key_a_card_asked_about() {
    let served = served(QUICK, None);
    let (mut app, _) = TestClient::greeted(&served.socket).await;
    let (mut other, server) = TestClient::unproven(&served.socket).await;
    let proof = testing::prove_with(&NEW, &server, ClientId::derive("test"));
    assert!(matches!(
        ask(&mut other, proof).await,
        ActionOutcome::Refused { .. }
    ));
    let enroll = ClientMessage::EnrollAppKey {
        key: IdempotencyKey::generate(),
        label: "MaxHQ on laptop".into(),
    };
    assert_eq!(ask(&mut other, enroll).await, ActionOutcome::Asked);

    let card = until_cards(&served, 1).await.remove(0);
    let options: Vec<_> = card
        .question
        .options
        .iter()
        .map(|o| o.label.as_str())
        .collect();
    assert_eq!(options, ["Don't allow", "Trust its key"]);
    let new = testing::public_key_of(&NEW);
    assert_eq!(fact(&card, "Key"), Some(new.as_str()));
    assert_eq!(fact(&card, "Key name"), Some("MaxHQ on laptop"));

    assert_eq!(answer(&mut app, card.id, true).await, ActionOutcome::Done);
    let keys = served.keys.keys().unwrap();
    let added = keys
        .iter()
        .find(|key| key.public_key == new)
        .expect("the key is trusted");
    assert_eq!(added.origin, AppKeyOrigin::Approved);
    assert_eq!(added.label, "MaxHQ on laptop");
    assert!(added.proved_at.is_some());
    assert!(may_sync(&mut other).await);

    // It's proven with its key now, so it answers cards itself.
    let _next = stranger(&served).await;
    let next = until_cards(&served, 1).await.remove(0);
    assert_eq!(answer(&mut other, next.id, true).await, ActionOutcome::Done);
}

#[tokio::test]
async fn a_key_nobody_allowed_stays_untrusted() {
    let served = served(QUICK, None);
    let (mut app, _) = TestClient::greeted(&served.socket).await;
    let (mut other, server) = TestClient::unproven(&served.socket).await;
    let proof = testing::prove_with(&NEW, &server, ClientId::derive("test"));
    assert!(matches!(
        ask(&mut other, proof).await,
        ActionOutcome::Refused { .. }
    ));
    let enroll = || ClientMessage::EnrollAppKey {
        key: IdempotencyKey::generate(),
        label: "MaxHQ".into(),
    };
    assert_eq!(ask(&mut other, enroll()).await, ActionOutcome::Asked);
    let card = until_cards(&served, 1).await.remove(0);
    assert_eq!(answer(&mut app, card.id, false).await, ActionOutcome::Done);
    assert_eq!(served.keys.keys().unwrap().len(), 1);
    assert!(!may_sync(&mut other).await);
    assert_eq!(
        ask(&mut other, enroll()).await,
        refused(
            "mac asked about this program and the answer was Don't allow, or nobody answered in \
             time. Reconnect to ask again."
        )
    );
}

/// Lets in the phone whose key comes from `seed`, as the Mac does through
/// `app`, and gives its ID. The phones store is one for the whole test
/// run, so each test's phone is one no other test lets in.
async fn phone_in(app: &mut TestClient, seed: &[u8; 32]) -> PhoneId {
    // The device lets phones in only once its phones store is up.
    crate::phones::for_tests();
    let ssh_public = crate::phones::openssh_for_tests(seed);
    let phone = PhoneId::derive(&ssh_public);
    let add = ClientMessage::AddPhone {
        key: IdempotencyKey::generate(),
        phone,
        name: "iPhone".into(),
        ssh_public,
        push_public: base64::engine::general_purpose::STANDARD.encode([8u8; 32]),
    };
    let added = ask(app, add).await;
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

/// Connects as `phone`, as `maxd proxy` does for it, and says hello
/// without proving its key.
async fn as_phone(served: &Served, phone: PhoneId) -> (TestClient, ServerHello) {
    let mut client = TestClient::connect(&served.socket).await;
    client.send(&ClientMessage::Phone { phone }).await;
    client.send(&hello(ProtocolRange::OURS)).await;
    match client.recv().await {
        ServerMessage::Hello(server) => (client, server),
        other => panic!("expected a hello, got {other:?}"),
    }
}

const NO_KEYS_FROM_A_PHONE: &str = "A phone can't let other keys in. Do that from the Mac.";

#[tokio::test]
async fn a_phone_cannot_answer_a_card() {
    let served = served(QUICK, None);
    let (mut app, _) = TestClient::greeted(&served.socket).await;
    let phone = phone_in(&mut app, &[213; 32]).await;
    let (mut own, server) = as_phone(&served, phone).await;
    let hello = hello(ProtocolRange::OURS);
    let proved = own.prove_as(Kind::Phone, &[213; 32], &server, &hello).await;
    assert_eq!(proved, ActionOutcome::Done);

    let _other = stranger(&served).await;
    let card = until_cards(&served, 1).await.remove(0);
    assert_eq!(
        answer(&mut own, card.id, true).await,
        refused(NO_KEYS_FROM_A_PHONE)
    );
    assert_eq!(cards(&served), [card]);
}

#[tokio::test]
async fn a_phone_that_doesnt_prove_gets_a_card_and_a_yes_lets_it_in_as_a_phone() {
    let served = served(QUICK, None);
    let (mut app, _) = TestClient::greeted(&served.socket).await;
    let phone = phone_in(&mut app, &[214; 32]).await;
    let (mut own, _) = as_phone(&served, phone).await;
    assert!(!may_sync(&mut own).await);

    let card = until_cards(&served, 1).await.remove(0);
    let consent = card.question.consent.as_ref().expect("a consent card");
    assert!(consent
        .risk
        .contains("as a paired phone, without showing it holds that phone's key"));
    assert!(consent
        .risk
        .ends_with("If you allow it, it can do what a paired phone can until it disconnects."));
    // The test's phone dials from maxd's own process, as network access does.
    assert_eq!(
        fact(&card, "How it reached maxd"),
        Some("Network access, as a paired phone")
    );

    assert_eq!(answer(&mut app, card.id, true).await, ActionOutcome::Done);
    assert!(may_sync(&mut own).await);
    until_cards(&served, 0).await;
    // Let in, it may still do only what a phone may.
    assert_eq!(
        answer(&mut own, card.id, true).await,
        refused(NO_KEYS_FROM_A_PHONE)
    );
}

#[tokio::test]
async fn a_phone_that_proves_its_key_drops_its_card() {
    let served = served(QUICK, None);
    let (mut app, _) = TestClient::greeted(&served.socket).await;
    let phone = phone_in(&mut app, &[215; 32]).await;
    let (mut own, server) = as_phone(&served, phone).await;
    assert!(!may_sync(&mut own).await);
    until_cards(&served, 1).await;

    let hello = hello(ProtocolRange::OURS);
    let proved = own.prove_as(Kind::Phone, &[215; 32], &server, &hello).await;
    assert_eq!(proved, ActionOutcome::Done);
    until_cards(&served, 0).await;
    assert!(may_sync(&mut own).await);
}

#[tokio::test]
async fn a_pairing_that_doesnt_prove_gets_no_card() {
    let served = served(QUICK, None);
    let ActionOutcome::PhonePair { pair, .. } =
        crate::phones::for_tests().begin(crate::phones::book_for_tests())
    else {
        panic!("expected a pairing");
    };
    let mut pairing = TestClient::connect(&served.socket).await;
    pairing.send(&ClientMessage::Pair { pair }).await;
    pairing.send(&hello(ProtocolRange::OURS)).await;
    assert!(matches!(pairing.recv().await, ServerMessage::Hello(_)));
    let pair_phone = ClientMessage::PairPhone {
        key: IdempotencyKey::generate(),
        name: "iPhone".into(),
        ssh_public: crate::phones::openssh_for_tests(&[216; 32]),
        push_public: base64::engine::general_purpose::STANDARD.encode([8u8; 32]),
    };
    assert_eq!(
        ask(&mut pairing, pair_phone).await,
        refused(&unproven("mac"))
    );

    // A program refused after it gets its card, so the pairing's would
    // have shown first.
    let _other = stranger(&served).await;
    until_cards(&served, 1).await;
    sleep(QUICK.delay * 2).await;
    assert_eq!(cards(&served).len(), 1);
}
