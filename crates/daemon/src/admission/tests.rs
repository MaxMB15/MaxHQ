// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::path::PathBuf;

use jiff::tz::TimeZone;
use maxhq_protocol::ids::AdmissionId;
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::ConsentFact;

use super::facts::{classify, Facts, Transport};
use super::words::{card, Host, DONT_ALLOW};
use super::Ask;
use crate::codesign::{Program, Signed};

/// 2026-11-08 14:03:07 UTC.
const ASKED: Timestamp = Timestamp(1_794_146_587_000);

fn host(mac: bool) -> Host<'static> {
    Host {
        device: "devbox",
        mac,
        zone: TimeZone::UTC,
    }
}

fn facts() -> Facts {
    Facts {
        says: Some("MaxHQ 0.9.0".to_owned()),
        transport: Transport::Socket,
        phone: false,
        program: None,
        asked_at: ASKED,
        same_mac: false,
    }
}

fn pairs(facts: &[ConsentFact]) -> Vec<(&str, &str)> {
    facts
        .iter()
        .map(|fact| (fact.label.as_str(), fact.value.as_str()))
        .collect()
}

#[test]
fn the_card_asks_in_the_designs_words_with_dont_allow_first() {
    let words = card(&host(false), &Ask::Admit, &facts(), 0);
    assert_eq!(words.title, "Let this program control devbox?");
    assert_eq!(
        words.risk,
        "A program connected to maxd on devbox without MaxHQ's app key. It may be an older MaxHQ \
         that hasn't updated. It may also be an agent on devbox trying to answer its own \
         questions, change what it may run or make keys. If you allow it, it can do anything the \
         app can until it disconnects."
    );
    assert_eq!(
        pairs(&words.facts),
        [
            ("Device", "devbox"),
            ("Says it is", "MaxHQ 0.9.0"),
            ("How it reached maxd", "The socket on devbox"),
            ("Asked", "14:03:07 UTC"),
        ]
    );

    let id = AdmissionId::generate();
    let question = words.question(id);
    assert_eq!(question.id, id.to_string());
    let options: Vec<_> = question.options.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(options, [DONT_ALLOW, "Allow until it disconnects"]);
    assert!(!question.multi && !question.other && !question.secret);
    let consent = question.consent.expect("the card lays out its consent");
    assert_eq!(consent.title, words.title);
    assert_eq!(consent.risk, words.risk);
    assert_eq!(consent.facts, words.facts);
    assert!(consent.remember.is_none(), "a yes is never remembered");
    assert!(consent.rows.is_empty() && consent.changes.is_empty());
    // The plain words say the same, for the alert and older clients.
    assert_eq!(
        question.text,
        format!(
            "Let this program control devbox?\n\n{}\n\nDevice: devbox\nSays it is: MaxHQ 0.9.0\nHow \
             it reached maxd: The socket on devbox\nAsked: 14:03:07 UTC",
            words.risk
        )
    );
}

#[test]
fn a_program_beside_the_proven_app_on_a_mac_is_most_likely_not_maxhq() {
    let mut facts = facts();
    facts.same_mac = true;
    facts.program = Some(Program {
        path: Some(PathBuf::from("/usr/local/bin/agent")),
        signed: Signed::NotYours,
    });
    let words = card(&host(true), &Ask::Admit, &facts, 0);
    assert!(words.risk.ends_with(
        "until it disconnects. MaxHQ is already connected from this Mac, so this is most likely \
         not MaxHQ."
    ));
    assert_eq!(
        pairs(&words.facts),
        [
            ("Device", "devbox"),
            ("Says it is", "MaxHQ 0.9.0"),
            ("How it reached maxd", "This Mac's socket"),
            ("Program", "/usr/local/bin/agent"),
            ("Signature", "Your certificate didn't sign it"),
            ("Asked", "14:03:07 UTC"),
        ]
    );

    for (signed, says) in [
        (Signed::Yours, "Your certificate signed it"),
        (
            Signed::Unchecked,
            "Not checked, since this maxd isn't signed",
        ),
    ] {
        facts.program = Some(Program { path: None, signed });
        let words = card(&host(true), &Ask::Admit, &facts, 0);
        let got = pairs(&words.facts);
        assert!(got.contains(&("Program", "Couldn't tell")), "{got:?}");
        assert!(got.contains(&("Signature", says)), "{got:?}");
    }
}

#[test]
fn a_phone_frame_asks_for_what_a_phone_may_do() {
    let mut facts = facts();
    facts.phone = true;
    facts.transport = Transport::Network;
    let words = card(&host(true), &Ask::Admit, &facts, 0);
    // A phone proves its own key, not the app key.
    assert!(words.risk.starts_with(
        "A program connected to maxd on devbox as a paired phone, without showing it holds \
         that phone's key."
    ));
    assert!(!words.risk.contains("app key"));
    assert!(words
        .risk
        .ends_with("If you allow it, it can do what a paired phone can until it disconnects."));
    assert!(
        pairs(&words.facts).contains(&("How it reached maxd", "Network access, as a paired phone"))
    );

    facts.transport = Transport::Proxy;
    let words = card(&host(false), &Ask::Admit, &facts, 0);
    assert!(pairs(&words.facts).contains(&(
        "How it reached maxd",
        "SSH, through maxd proxy, as a paired phone"
    )));
}

#[test]
fn a_key_to_trust_names_the_key_and_lasts_until_you_remove_it() {
    let ask = Ask::Enroll {
        public_key: "AAAA-key".to_owned(),
        label: "MaxHQ on\nlaptop".to_owned(),
    };
    let words = card(&host(false), &ask, &facts(), 0);
    assert_eq!(words.title, "Let this program control devbox?");
    assert_eq!(
        words.risk,
        "A program connected to maxd on devbox with an app key devbox doesn't trust, and asked \
         devbox to trust it. It may be MaxHQ on another Mac of yours, or on a Mac that lost its \
         key. It may also be an agent on devbox trying to answer its own questions, change what \
         it may run or make keys. If you allow it, devbox trusts that key until you remove it, so \
         the program can do anything the app can, now and each time it connects."
    );
    assert_eq!(words.allow, "Trust its key");
    let got = pairs(&words.facts);
    assert_eq!(
        &got[3..],
        [
            ("Key name", "MaxHQ on laptop"),
            ("Key", "AAAA-key"),
            ("Asked", "14:03:07 UTC"),
        ]
    );
}

#[test]
fn the_open_cards_say_how_many_more_wait() {
    let one = card(&host(false), &Ask::Admit, &facts(), 1);
    assert_eq!(
        one.facts
            .last()
            .map(|f| (f.label.as_str(), f.value.as_str())),
        Some(("Also waiting", "1 more program, with no card of its own"))
    );
    let two = card(&host(false), &Ask::Admit, &facts(), 2);
    assert_eq!(
        two.facts.last().map(|f| f.value.as_str()),
        Some("2 more programs, with no card of their own")
    );
}

#[test]
fn what_a_program_says_it_is_stays_on_one_short_line() {
    let mut facts = facts();
    facts.says = Some(format!("  Max\nHQ\t{}  ", "x".repeat(300)));
    let words = card(&host(false), &Ask::Admit, &facts, 0);
    let says = &words.facts[1].value;
    assert!(says.starts_with("Max HQ x"), "{says}");
    assert!(!says.contains('\n') && !says.contains('\t'));
    assert_eq!(says.chars().count(), 100);

    for blank in [None, Some(" \n ".to_owned())] {
        facts.says = blank;
        let words = card(&host(false), &Ask::Admit, &facts, 0);
        assert_eq!(words.facts[1].value, "Couldn't tell");
    }
}

#[test]
fn how_a_program_reached_maxd() {
    let maxd = PathBuf::from("/opt/maxhq/maxd");
    let other = PathBuf::from("/usr/local/bin/agent");
    // A test's client, or network access dialling in for a phone.
    assert_eq!(
        classify(true, false, Some(&maxd), Some(&maxd)),
        Transport::Socket
    );
    assert_eq!(
        classify(true, true, Some(&maxd), Some(&maxd)),
        Transport::Network
    );
    // `maxd proxy`, which SSH runs.
    assert_eq!(
        classify(false, false, Some(&maxd), Some(&maxd)),
        Transport::Proxy
    );
    assert_eq!(
        classify(false, true, Some(&maxd), Some(&maxd)),
        Transport::Proxy
    );
    // Anything else dialled the socket itself.
    assert_eq!(
        classify(false, false, Some(&other), Some(&maxd)),
        Transport::Socket
    );
    assert_eq!(
        classify(false, false, Some(&other), None),
        Transport::Socket
    );
    assert_eq!(
        classify(false, false, None, Some(&maxd)),
        Transport::Unknown
    );
}

#[test]
fn a_test_client_dialled_the_socket() {
    let caller = super::facts::Caller {
        pid: i32::try_from(std::process::id()).ok(),
        token: None,
        says: None,
        phone: false,
        asked_at: ASKED,
    };
    let facts = Facts::find(&caller, &[]);
    assert_eq!(facts.transport, Transport::Socket);
    assert!(!facts.same_mac);
    assert_eq!(facts.program.is_some(), cfg!(target_os = "macos"));

    let nobody = super::facts::Caller {
        pid: None,
        ..caller
    };
    assert_eq!(Facts::find(&nobody, &[]).transport, Transport::Unknown);
}
