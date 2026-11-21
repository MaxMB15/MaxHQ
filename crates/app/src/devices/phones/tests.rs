// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::collections::BTreeSet;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures::future::BoxFuture;
use maxhq_client::{LinkEvent, LinkSpec};
use maxhq_protocol::agents::PeerRoute;
use maxhq_protocol::ids::PhoneSessionId;
use maxhq_protocol::link::LinkState;
use maxhq_protocol::message::Capabilities;
use maxhq_protocol::netaccess::{NetworkAccess, PhoneSession};
use maxhq_protocol::phones::PhoneBook;
use maxhq_protocol::secret::Secret;
use maxhq_protocol::state::{Change, EntityKey};

use super::*;
use crate::devices::tests::{harness, Call, Harness};
use crate::merge::tests::{connected, link_state, unreachable};
use crate::registry::Registry;
use crate::tests::block_on;

/// A made-up Ed25519 host key, the same for every device.
const HOST_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g";

/// `ssh -G` as a test has it: each alias is `<alias>.local` on port 2222
/// as `dev`, apart from `jumped`, which needs a jump host. Every device
/// has a host key but `unpinned`.
struct Facts;

impl PeerFacts for Facts {
    fn route(&self, host: String) -> BoxFuture<'static, Option<PeerRoute>> {
        Box::pin(async move {
            (host != "jumped").then(|| PeerRoute {
                host: format!("{host}.local"),
                port: 2222,
                user: "dev".to_owned(),
            })
        })
    }

    fn host_key(&self, device: DeviceId) -> Option<String> {
        (device != DeviceId::derive("unpinned")).then(|| HOST_KEY.to_owned())
    }
}

/// Devices added by their SSH aliases, each up and taking phones.
fn devices(names: &[&str]) -> (Harness, Vec<LinkSpec>) {
    let registry = Registry::in_memory().unwrap();
    let specs: Vec<LinkSpec> = names
        .iter()
        .map(|name| registry.add(name).unwrap())
        .collect();
    let h = harness(registry, None);
    for (spec, name) in specs.iter().zip(names) {
        up(&h, spec, name);
    }
    settle(&h);
    (h, specs)
}

/// Has every link say what it can, so the first pass over the phones takes
/// in the ones the view shows by then.
fn settle(h: &Harness) {
    let local = h.local();
    h.report(local.id, link_state(&local, unreachable()));
    let connected: Vec<LinkId> = h.fake.record().up.iter().copied().collect();
    lock(&h.devices.core.phone_synced).extend(connected);
}

fn up(h: &Harness, spec: &LinkSpec, device: &str) {
    h.fake.record().up.insert(spec.id);
    report(h, spec, device, connected());
}

fn down(h: &Harness, spec: &LinkSpec, device: &str) {
    h.fake.record().up.remove(&spec.id);
    report(h, spec, device, unreachable());
}

fn report(h: &Harness, spec: &LinkSpec, device: &str, state: LinkState) {
    let LinkEvent::Link(mut link) = link_state(spec, state) else {
        unreachable!("link_state reports a link");
    };
    link.device = Some(DeviceId::derive(device));
    link.capabilities = Some(Capabilities {
        phones: true,
        ..Capabilities::default()
    });
    h.report(spec.id, LinkEvent::Link(link));
}

fn phone() -> PhoneId {
    PhoneId::derive("phone")
}

/// The phone as `device` has it.
fn phone_on(device: &str) -> Entity {
    Entity::Phone(Phone {
        id: phone(),
        device: DeviceId::derive(device),
        name: "iPhone".into(),
        ssh_public: "ssh-ed25519 AAAAphone".into(),
        push_public: STANDARD.encode([3; 32]),
        paired_at: Timestamp::now(),
        push: Vec::new(),
        everything: false,
    })
}

/// The phone as `device` has it, with its switch on or off.
fn phone_switched(device: &str, everything: bool) -> Entity {
    let Entity::Phone(phone) = phone_on(device) else {
        unreachable!("phone_on makes a phone");
    };
    Entity::Phone(Phone {
        everything,
        ..phone
    })
}

/// This Mac as its own link names it in the tests.
fn this_mac() -> PhoneMac {
    // The local link's name, which isn't "This Mac" on Linux.
    let name = crate::registry::LOCAL_NAME;
    PhoneMac {
        device: DeviceId::derive(name),
        name: name.into(),
    }
}

/// The phone's book, kept by `device`, with each of `names`. It names this
/// Mac.
fn book_on(device: &str, names: &[&str]) -> Entity {
    Entity::PhoneBook(PhoneBook {
        phone: phone(),
        device: DeviceId::derive(device),
        devices: names
            .iter()
            .map(|name| PhoneBookEntry {
                device: DeviceId::derive(name),
                name: (*name).into(),
                routes: Vec::new(),
                host_key: HOST_KEY.into(),
                network: None,
            })
            .collect(),
        mac: Some(this_mac()),
    })
}

/// `book_on`, naming `mac`.
fn book_naming(device: &str, names: &[&str], mac: Option<PhoneMac>) -> Entity {
    let mut book = book_on(device, names);
    if let Entity::PhoneBook(found) = &mut book {
        found.mac = mac;
    }
    book
}

/// How many devices each book sent lists, and the Mac it names.
fn macs_sent(h: &Harness) -> Vec<(usize, Option<PhoneMac>)> {
    h.fake
        .calls()
        .into_iter()
        .filter_map(|(_, call)| match call {
            Call::Send(ClientMessage::SetPhoneBook { book, mac, .. }) => Some((book.len(), mac)),
            _ => None,
        })
        .collect()
}

fn publish(h: &Harness, spec: &LinkSpec, entities: Vec<Entity>) {
    let changes = entities
        .into_iter()
        .map(|value| Change::Upsert { value })
        .collect();
    h.report(spec.id, LinkEvent::Changed(changes));
}

/// The devices each phone action went to since the last look, by name.
fn sent(h: &Harness, specs: &[LinkSpec], names: &[&str]) -> BTreeSet<(&'static str, String)> {
    h.fake
        .calls()
        .into_iter()
        .filter_map(|(id, call)| {
            let at = specs.iter().position(|spec| spec.id == id)?;
            let kind = match call {
                Call::Send(ClientMessage::AddPhone { phone: to, .. }) if to == phone() => "add",
                Call::Send(ClientMessage::RemovePhone { phone: to, .. }) if to == phone() => {
                    "remove"
                }
                _ => return None,
            };
            Some((kind, names[at].to_owned()))
        })
        .collect()
}

/// The books sent since the last look, by the device that keeps each.
fn books_sent(h: &Harness, specs: &[LinkSpec]) -> Vec<(usize, Vec<PhoneBookEntry>)> {
    h.fake
        .calls()
        .into_iter()
        .filter_map(|(id, call)| match call {
            Call::Send(ClientMessage::SetPhoneBook { book, .. }) => {
                Some((specs.iter().position(|spec| spec.id == id)?, book))
            }
            _ => None,
        })
        .collect()
}

/// `device`'s network access as it publishes it, listening on one
/// address while it's on.
fn access(device: &str, on: bool, pin: Option<&str>) -> Entity {
    Entity::NetworkAccess(NetworkAccess {
        device: DeviceId::derive(device),
        on,
        port: 7466,
        hosts: if on {
            vec!["192.168.1.20".into()]
        } else {
            Vec::new()
        },
        pin: pin.map(str::to_owned),
        problem: None,
    })
}

/// A pin in the shape a device publishes.
fn pin() -> String {
    URL_SAFE_NO_PAD.encode([7; 32])
}

/// The network part a phone gets for a device whose access is on.
fn network(code: Option<&str>) -> PhoneNetwork {
    PhoneNetwork {
        hosts: vec!["192.168.1.20".into()],
        port: 7466,
        pin: pin(),
        code: code.map(|code| Secret::new(code.to_owned())),
    }
}

/// The session `device` holds for the phone.
fn session_on(device: &str) -> Entity {
    Entity::PhoneSession(PhoneSession {
        id: PhoneSessionId::derive(device),
        device: DeviceId::derive(device),
        phone: phone(),
        created: Timestamp::now(),
        expires: Timestamp::now(),
        last_used: None,
    })
}

fn pair_outcome() -> ActionOutcome {
    ActionOutcome::PhonePair {
        pair: PairId::derive("pairing"),
        seed: Secret::new(STANDARD.encode([9; 32])),
        host_key: HOST_KEY.into(),
        expires_at: Timestamp::now(),
    }
}

#[test]
fn aliases_resolve_to_plain_addresses_with_the_one_for_the_phone_last() {
    let names = ["devbox", "ws-alpha", "jumped"];
    let (h, specs) = devices(&names);
    h.devices.link_peers(Facts);
    let (devbox, alpha) = (DeviceId::derive("devbox"), DeviceId::derive("ws-alpha"));
    h.devices
        .set_phone_address(devbox, Some("home.example.net:2200"))
        .unwrap();
    assert!(h.devices.set_phone_address(devbox, Some("a b")).is_err());
    h.fake.record().acts.push_back(pair_outcome());

    let pairing = block_on(h.devices.begin_phone_pair(devbox, &[alpha])).unwrap();
    assert_eq!(pairing.device, devbox);
    assert!(!pairing.qr.path.is_empty());
    let book = h
        .fake
        .calls()
        .into_iter()
        .find_map(|(id, call)| match call {
            Call::Send(ClientMessage::BeginPhonePair { book, .. }) => {
                assert_eq!(id, specs[0].id, "the pairing device starts it");
                Some(book)
            }
            _ => None,
        })
        .unwrap();
    let route = |host: &str, port| PhoneRoute {
        user: "dev".into(),
        host: host.into(),
        port,
    };
    assert_eq!(
        book.iter()
            .map(|entry| (entry.device, entry.routes.clone()))
            .collect::<Vec<_>>(),
        [
            (
                devbox,
                vec![route("devbox.local", 2222), route("home.example.net", 2200)]
            ),
            (alpha, vec![route("ws-alpha.local", 2222)]),
        ]
    );

    // One the phone can't dial isn't paired with.
    let jumped = DeviceId::derive("jumped");
    let refused = block_on(h.devices.begin_phone_pair(devbox, &[jumped])).unwrap_err();
    assert!(
        refused.to_string().contains("address for the phone"),
        "{refused}"
    );
    let listed = block_on(h.devices.phone_devices()).unwrap();
    let notes: Vec<_> = listed
        .iter()
        .map(|found| (found.name.as_str(), found.note.is_some()))
        .collect();
    assert_eq!(
        notes,
        [("devbox", false), ("jumped", true), ("ws-alpha", false)]
    );
    assert_eq!(listed[0].address.as_deref(), Some("home.example.net:2200"));
}

#[test]
fn the_phones_key_reaches_every_ticked_device_and_only_those() {
    let names = ["devbox", "ws-alpha", "ws-bravo"];
    let (h, specs) = devices(&names);
    publish(
        &h,
        &specs[0],
        vec![
            phone_on("devbox"),
            book_on("devbox", &["devbox", "ws-alpha"]),
        ],
    );
    h.devices.link_peers(Facts);

    block_on(h.devices.core.phone_pass());
    assert_eq!(
        sent(&h, &specs, &names),
        BTreeSet::from([("add", "ws-alpha".to_owned())])
    );

    // Once it has the phone, a pass sends nothing more.
    publish(&h, &specs[1], vec![phone_on("ws-alpha")]);
    h.fake.record().calls.clear();
    block_on(h.devices.core.phone_pass());
    assert!(sent(&h, &specs, &names).is_empty());
}

#[test]
fn remove_reaches_every_device_that_has_the_phone() {
    let names = ["devbox", "ws-alpha", "ws-bravo"];
    let (h, specs) = devices(&names);
    publish(
        &h,
        &specs[0],
        vec![
            phone_on("devbox"),
            book_on("devbox", &["devbox", "ws-alpha"]),
        ],
    );
    publish(&h, &specs[1], vec![phone_on("ws-alpha")]);
    h.devices.link_peers(Facts);
    down(&h, &specs[1], "ws-alpha");

    block_on(h.devices.remove_phone(phone())).unwrap();
    let first = sent(&h, &specs, &names);
    assert!(
        first.contains(&("remove", "devbox".to_owned())),
        "{first:?}"
    );
    assert!(!first.iter().any(|(kind, _)| *kind == "add"), "{first:?}");
    assert!(!first.contains(&("remove", "ws-bravo".to_owned())));
    assert_eq!(
        h.devices.core.removals()[&phone()].devices,
        BTreeSet::from([DeviceId::derive("ws-alpha")]),
        "the device that's away still has it to take"
    );

    h.fake.record().calls.clear();
    up(&h, &specs[1], "ws-alpha");
    block_on(h.devices.core.phone_pass());
    assert!(sent(&h, &specs, &names).contains(&("remove", "ws-alpha".to_owned())));
    assert!(h.devices.core.removals()[&phone()].devices.is_empty());

    // Until each device drops it, a pass doesn't bring it back.
    h.fake.record().calls.clear();
    let gone = |device: &str| Change::Remove {
        key: EntityKey::Phone {
            phone: phone(),
            device: DeviceId::derive(device),
        },
    };
    h.report(specs[1].id, LinkEvent::Changed(vec![gone("ws-alpha")]));
    block_on(h.devices.core.phone_pass());
    assert!(sent(&h, &specs, &names).is_empty());
    h.report(
        specs[0].id,
        LinkEvent::Changed(vec![
            gone("devbox"),
            Change::Remove {
                key: EntityKey::PhoneBook(phone()),
            },
        ]),
    );
    block_on(h.devices.core.phone_pass());
    assert!(h.devices.core.removals().is_empty());
}

#[test]
fn a_phone_that_pairs_again_spreads_again() {
    let names = ["devbox", "ws-alpha"];
    let (h, specs) = devices(&names);
    publish(
        &h,
        &specs[0],
        vec![
            phone_on("devbox"),
            book_on("devbox", &["devbox", "ws-alpha"]),
        ],
    );
    h.devices.link_peers(Facts);
    block_on(h.devices.remove_phone(phone())).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    h.fake.record().acts.push_back(pair_outcome());
    block_on(
        h.devices
            .begin_phone_pair(DeviceId::derive("devbox"), &[DeviceId::derive("ws-alpha")]),
    )
    .unwrap();
    h.fake.record().calls.clear();
    publish(&h, &specs[0], vec![phone_on("devbox")]);
    block_on(h.devices.core.phone_pass());
    assert!(h.devices.core.removals().is_empty());
    assert_eq!(
        sent(&h, &specs, &names),
        BTreeSet::from([("add", "ws-alpha".to_owned())])
    );
}

#[test]
fn network_access_goes_in_the_book_while_its_on() {
    let names = ["devbox", "jumped"];
    let (h, specs) = devices(&names);
    h.devices.link_peers(Facts);
    let (devbox, jumped) = (DeviceId::derive("devbox"), DeviceId::derive("jumped"));
    let note = |h: &Harness| {
        block_on(h.devices.phone_devices())
            .unwrap()
            .into_iter()
            .find(|found| found.device == jumped)
            .and_then(|found| found.note)
    };

    // A device that publishes no network access is an older one, so off.
    assert!(note(&h).unwrap().contains("address for the phone"));
    publish(&h, &specs[1], vec![access("jumped", false, Some(&pin()))]);
    assert_eq!(
        note(&h).as_deref(),
        Some("Turn on Network access on its tab to pair a phone with it.")
    );
    // On without a pin is no way in yet.
    publish(&h, &specs[1], vec![access("jumped", true, None)]);
    assert!(note(&h).unwrap().contains("address for the phone"));

    // On with a pin is one, even with no SSH route, and it rides in the
    // code to scan.
    publish(&h, &specs[1], vec![access("jumped", true, Some(&pin()))]);
    assert_eq!(note(&h), None);
    h.fake.record().acts.push_back(ActionOutcome::PhonePair {
        pair: PairId::derive("pairing"),
        seed: Secret::new(STANDARD.encode([9; 32])),
        host_key: String::new(),
        expires_at: Timestamp::now(),
    });
    let pairing = block_on(h.devices.begin_phone_pair(jumped, &[devbox])).unwrap();
    let book = h
        .fake
        .calls()
        .into_iter()
        .find_map(|(id, call)| match call {
            Call::Send(ClientMessage::BeginPhonePair { book, .. }) => {
                assert_eq!(id, specs[1].id);
                Some(book)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(book[0].device, jumped);
    assert!(book[0].routes.is_empty());
    assert_eq!(book[0].network, Some(network(None)));
    assert_eq!(book[1].device, devbox);
    assert_eq!(book[1].network, None, "devbox publishes none");
    assert!(!book[1].routes.is_empty());
    assert!(
        !pairing.qr.path.is_empty(),
        "a network part alone makes a code"
    );
}

#[test]
fn a_devices_code_rides_in_the_book_until_it_lists_a_session() {
    let names = ["devbox", "ws-alpha"];
    let (h, specs) = devices(&names);
    publish(
        &h,
        &specs[0],
        vec![
            phone_on("devbox"),
            book_on("devbox", &["devbox", "ws-alpha"]),
        ],
    );
    publish(&h, &specs[1], vec![access("ws-alpha", true, Some(&pin()))]);
    h.devices.link_peers(Facts);
    h.fake.record().acts.push_back(ActionOutcome::PhoneAdded {
        code: Secret::new("one-time".into()),
    });

    block_on(h.devices.core.phone_pass());
    let sent_books = books_sent(&h, &specs);
    assert_eq!(sent_books.len(), 1, "one book, to the device that keeps it");
    let (at, book) = &sent_books[0];
    assert_eq!(*at, 0);
    assert_eq!(book[0].network, None);
    assert_eq!(book[1].network, Some(network(Some("one-time"))));

    // Once the book shows it, a pass sends nothing more, even when the
    // Mac no longer holds the code.
    let mut kept = book_on("devbox", &["devbox", "ws-alpha"]);
    if let Entity::PhoneBook(found) = &mut kept {
        found.devices = book.clone();
    }
    publish(&h, &specs[0], vec![kept]);
    publish(&h, &specs[1], vec![phone_on("ws-alpha")]);
    h.devices.core.phone_codes.lock().unwrap().clear();
    h.fake.record().calls.clear();
    block_on(h.devices.core.phone_pass());
    assert!(books_sent(&h, &specs).is_empty());

    // The device lists a session: the next book drops the code.
    publish(&h, &specs[1], vec![session_on("ws-alpha")]);
    block_on(h.devices.core.phone_pass());
    let sent_books = books_sent(&h, &specs);
    assert!(!sent_books.is_empty());
    for (_, book) in sent_books {
        assert_eq!(book[1].network, Some(network(None)));
    }
}

#[test]
fn a_book_gets_this_macs_name_once_a_run_and_another_macs_name_stays() {
    let names = ["devbox"];
    let (h, specs) = devices(&names);
    publish(
        &h,
        &specs[0],
        vec![phone_on("devbox"), book_naming("devbox", &names, None)],
    );
    h.devices.link_peers(Facts);
    block_on(h.devices.core.phone_pass());
    assert_eq!(macs_sent(&h), vec![(1, Some(this_mac()))]);

    // A keeper too old to keep the name still shows none, and isn't sent
    // it again this run.
    block_on(h.devices.core.phone_pass());
    assert!(macs_sent(&h).is_empty());

    // This Mac by an old name gets the new one; another Mac keeps its own.
    let old = PhoneMac {
        name: "Old name".into(),
        ..this_mac()
    };
    publish(
        &h,
        &specs[0],
        vec![book_naming("devbox", &names, Some(old))],
    );
    lock(&h.devices.core.phone_named).clear();
    block_on(h.devices.core.phone_pass());
    assert_eq!(macs_sent(&h), vec![(1, Some(this_mac()))]);
    let work = PhoneMac {
        device: DeviceId::derive("Work Mac"),
        name: "Work Mac".into(),
    };
    publish(
        &h,
        &specs[0],
        vec![book_naming("devbox", &names, Some(work))],
    );
    lock(&h.devices.core.phone_named).clear();
    block_on(h.devices.core.phone_pass());
    assert!(macs_sent(&h).is_empty());
}

#[test]
fn a_code_waits_for_the_book_and_goes_with_the_session() {
    let names = ["devbox", "ws-alpha"];
    let (h, specs) = devices(&names);
    publish(
        &h,
        &specs[0],
        vec![
            phone_on("devbox"),
            book_on("devbox", &["devbox", "ws-alpha"]),
        ],
    );
    publish(&h, &specs[1], vec![access("ws-alpha", true, Some(&pin()))]);
    h.devices.link_peers(Facts);
    h.fake.record().acts.push_back(ActionOutcome::PhoneAdded {
        code: Secret::new("one-time".into()),
    });
    // The book's device is away, so the code waits.
    down(&h, &specs[0], "devbox");
    block_on(h.devices.core.phone_pass());
    assert!(books_sent(&h, &specs).is_empty());
    assert_eq!(h.devices.core.phone_codes.lock().unwrap().len(), 1);

    up(&h, &specs[0], "devbox");
    h.fake.record().calls.clear();
    block_on(h.devices.core.phone_pass());
    let sent_books = books_sent(&h, &specs);
    assert_eq!(sent_books[0].1[1].network, Some(network(Some("one-time"))));

    publish(&h, &specs[1], vec![session_on("ws-alpha")]);
    block_on(h.devices.core.phone_pass());
    assert!(h.devices.core.phone_codes.lock().unwrap().is_empty());
}

#[test]
fn a_new_code_goes_to_each_device_the_phone_isnt_signed_in_to() {
    let names = ["devbox", "ws-alpha", "ws-bravo", "ws-charlie"];
    let (h, specs) = devices(&names);
    publish(
        &h,
        &specs[0],
        vec![phone_on("devbox"), book_on("devbox", &names)],
    );
    // ws-alpha waits for the phone, ws-bravo has its session, and
    // ws-charlie is away. devbox has no network access.
    for (spec, name) in specs.iter().zip(names).skip(1) {
        publish(
            &h,
            spec,
            vec![phone_on(name), access(name, true, Some(&pin()))],
        );
    }
    publish(&h, &specs[2], vec![session_on("ws-bravo")]);
    down(&h, &specs[3], "ws-charlie");
    link_quietly(&h);
    block_on(h.devices.core.phone_pass());
    h.fake.record().calls.clear();
    h.fake.record().acts.push_back(ActionOutcome::PhoneAdded {
        code: Secret::new("fresh".into()),
    });

    let missed = block_on(h.devices.renew_phone_codes(phone())).unwrap();
    assert_eq!(missed, ["ws-charlie"]);
    let codes: Vec<_> = lock(&h.devices.core.phone_codes).keys().copied().collect();
    assert_eq!(codes, [(phone(), DeviceId::derive("ws-alpha"))]);
    let sent_books = books_sent(&h, &specs);
    assert_eq!(sent_books.len(), 1, "{sent_books:?}");
    assert_eq!(
        sent_books[0].0, 0,
        "the book goes to devbox, which keeps it"
    );
    assert_eq!(sent_books[0].1[1].network, Some(network(Some("fresh"))));
    assert_eq!(sent_books[0].1[2].network, Some(network(None)));

    // A device that doesn't make one is named too.
    h.fake.record().calls.clear();
    h.fake.record().acts.push_back(ActionOutcome::Refused {
        reason: "This device couldn't keep the phone's code".into(),
    });
    let missed = block_on(h.devices.renew_phone_codes(phone())).unwrap();
    assert_eq!(missed, ["ws-alpha", "ws-charlie"]);
    assert_eq!(
        sent(&h, &specs, &names),
        BTreeSet::from([("add", "ws-alpha".to_owned())])
    );

    // With devbox away the book can't take a code, so nothing goes.
    down(&h, &specs[0], "devbox");
    h.fake.record().calls.clear();
    let missed = block_on(h.devices.renew_phone_codes(phone())).unwrap();
    assert_eq!(missed, ["devbox", "ws-alpha", "ws-charlie"]);
    assert!(sent(&h, &specs, &names).is_empty());

    assert!(block_on(h.devices.renew_phone_codes(PhoneId::derive("other"))).is_err());
}

#[test]
fn remove_reaches_every_ticked_device() {
    let names = ["devbox", "ws-alpha", "ws-bravo", "ws-charlie"];
    let (h, specs) = devices(&names);
    // ws-bravo is ticked but hasn't shown the phone yet.
    publish(
        &h,
        &specs[0],
        vec![
            phone_on("devbox"),
            book_on("devbox", &["devbox", "ws-alpha", "ws-bravo"]),
        ],
    );
    publish(&h, &specs[1], vec![phone_on("ws-alpha")]);
    h.devices.link_peers(Facts);
    h.fake.record().calls.clear();

    block_on(h.devices.remove_phone(phone())).unwrap();
    assert_eq!(
        sent(&h, &specs, &names),
        BTreeSet::from([
            ("remove", "devbox".to_owned()),
            ("remove", "ws-alpha".to_owned()),
            ("remove", "ws-bravo".to_owned()),
        ])
    );
}

/// The devices each switch went to since the last look, by name, with
/// which way it went.
fn switches_sent(h: &Harness, specs: &[LinkSpec], names: &[&str]) -> BTreeSet<(String, bool)> {
    h.fake
        .calls()
        .into_iter()
        .filter_map(|(id, call)| match call {
            Call::Send(ClientMessage::SetPhoneEverything {
                phone: to,
                everything,
                ..
            }) if to == phone() => {
                let at = specs.iter().position(|spec| spec.id == id)?;
                Some((names[at].to_owned(), everything))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn the_switch_goes_to_each_connected_device_and_names_the_rest() {
    let names = ["devbox", "ws-alpha", "ws-bravo", "ws-charlie"];
    let (h, specs) = devices(&names);
    publish(
        &h,
        &specs[0],
        vec![
            phone_on("devbox"),
            book_on("devbox", &["devbox", "ws-alpha", "ws-bravo"]),
        ],
    );
    publish(&h, &specs[1], vec![phone_on("ws-alpha")]);
    down(&h, &specs[2], "ws-bravo");
    h.devices.link_peers(Facts);
    h.fake.record().calls.clear();

    let missed = block_on(h.devices.set_phone_everything(phone(), true)).unwrap();
    assert_eq!(missed, vec!["ws-bravo".to_owned()]);
    assert_eq!(
        switches_sent(&h, &specs, &names),
        BTreeSet::from([("devbox".to_owned(), true), ("ws-alpha".to_owned(), true)]),
        "nothing goes to a device the phone's book doesn't list"
    );
}

#[test]
fn a_device_that_refuses_or_is_away_with_the_switch_on_is_named() {
    let names = ["devbox", "ws-alpha", "ws-bravo"];
    let (h, specs) = devices(&names);
    publish(
        &h,
        &specs[0],
        vec![phone_switched("devbox", true), book_on("devbox", &names)],
    );
    publish(&h, &specs[1], vec![phone_switched("ws-alpha", true)]);
    down(&h, &specs[1], "ws-alpha");
    down(&h, &specs[2], "ws-bravo");
    link_quietly(&h);
    h.fake.record().acts.push_back(ActionOutcome::Refused {
        reason: "This device couldn't keep the switch".into(),
    });
    h.fake.record().calls.clear();

    let mut missed = block_on(h.devices.set_phone_everything(phone(), false)).unwrap();
    missed.sort();
    assert_eq!(
        missed,
        ["devbox", "ws-alpha"],
        "ws-bravo doesn't have the phone, so there's nothing to turn off"
    );
    assert_eq!(
        switches_sent(&h, &specs, &names),
        BTreeSet::from([("devbox".to_owned(), false)])
    );
}

/// Links the devices, then waits out the pass over them that starts, so
/// it doesn't take an answer the test lines up.
fn link_quietly(h: &Harness) {
    h.devices.link_peers(Facts);
    let core = &h.devices.core;
    for _ in 0..5000 {
        if !core.peers_due.load(Ordering::Acquire) {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        !core.peers_due.load(Ordering::Acquire),
        "the pass never started"
    );
    drop(block_on(core.peer_turn.lock()));
}

const AWAY: [&str; 3] = ["devbox", "ws-alpha", "ws-bravo"];

/// devbox keeps the phone's book, which ticks ws-alpha and ws-bravo.
/// ws-alpha has the phone and ws-bravo doesn't yet, and both are away.
fn both_away() -> (Harness, Vec<LinkSpec>) {
    let (h, specs) = devices(&AWAY);
    publish(
        &h,
        &specs[0],
        vec![phone_on("devbox"), book_on("devbox", &AWAY)],
    );
    publish(&h, &specs[1], vec![phone_on("ws-alpha")]);
    down(&h, &specs[1], "ws-alpha");
    down(&h, &specs[2], "ws-bravo");
    h.devices.link_peers(Facts);
    (h, specs)
}

/// The phone's adds and switches `name` got since the last look, in
/// order.
fn phone_calls(h: &Harness, specs: &[LinkSpec], name: &str) -> Vec<&'static str> {
    let at = AWAY.iter().position(|each| *each == name).unwrap();
    h.fake
        .calls()
        .into_iter()
        .filter(|(id, _)| *id == specs[at].id)
        .filter_map(|(_, call)| match call {
            Call::Send(ClientMessage::AddPhone { .. }) => Some("add"),
            Call::Send(ClientMessage::SetPhoneEverything { everything, .. }) => {
                Some(if everything { "on" } else { "off" })
            }
            _ => None,
        })
        .collect()
}

#[test]
fn a_device_away_gets_the_switch_when_it_connects() {
    let (h, specs) = both_away();
    let mut missed = block_on(h.devices.set_phone_everything(phone(), true)).unwrap();
    missed.sort();
    assert_eq!(missed, ["ws-alpha", "ws-bravo"]);
    assert_eq!(
        switches_sent(&h, &specs, &AWAY),
        BTreeSet::from([("devbox".to_owned(), true)])
    );
    publish(&h, &specs[0], vec![phone_switched("devbox", true)]);
    block_on(h.devices.core.phone_pass());

    h.fake.record().calls.clear();
    up(&h, &specs[1], "ws-alpha");
    block_on(h.devices.core.phone_pass());
    assert_eq!(
        switches_sent(&h, &specs, &AWAY),
        BTreeSet::from([("ws-alpha".to_owned(), true)])
    );

    // Once it shows the switch, a pass sends nothing more.
    publish(&h, &specs[1], vec![phone_switched("ws-alpha", true)]);
    block_on(h.devices.core.phone_pass());
    h.fake.record().calls.clear();
    block_on(h.devices.core.phone_pass());
    assert!(switches_sent(&h, &specs, &AWAY).is_empty());
}

#[test]
fn a_device_that_lets_the_phone_in_later_gets_the_switch_with_it() {
    let (h, specs) = both_away();
    block_on(h.devices.set_phone_everything(phone(), true)).unwrap();

    h.fake.record().calls.clear();
    up(&h, &specs[2], "ws-bravo");
    block_on(h.devices.core.phone_pass());
    let calls = phone_calls(&h, &specs, "ws-bravo");
    assert_eq!(calls.get(..2), Some(&["add", "on"][..]), "{calls:?}");
}

#[test]
fn turning_the_switch_off_reaches_both() {
    let (h, specs) = both_away();
    block_on(h.devices.set_phone_everything(phone(), true)).unwrap();
    up(&h, &specs[1], "ws-alpha");
    up(&h, &specs[2], "ws-bravo");
    block_on(h.devices.core.phone_pass());
    for (spec, name) in specs.iter().zip(AWAY) {
        publish(&h, spec, vec![phone_switched(name, true)]);
    }
    down(&h, &specs[1], "ws-alpha");
    down(&h, &specs[2], "ws-bravo");
    block_on(h.devices.core.phone_pass());
    h.fake.record().calls.clear();

    let mut missed = block_on(h.devices.set_phone_everything(phone(), false)).unwrap();
    missed.sort();
    assert_eq!(missed, ["ws-alpha", "ws-bravo"]);
    assert_eq!(
        switches_sent(&h, &specs, &AWAY),
        BTreeSet::from([("devbox".to_owned(), false)])
    );

    publish(&h, &specs[0], vec![phone_switched("devbox", false)]);
    block_on(h.devices.core.phone_pass());
    h.fake.record().calls.clear();
    up(&h, &specs[1], "ws-alpha");
    up(&h, &specs[2], "ws-bravo");
    block_on(h.devices.core.phone_pass());
    assert_eq!(
        switches_sent(&h, &specs, &AWAY),
        BTreeSet::from([
            ("ws-alpha".to_owned(), false),
            ("ws-bravo".to_owned(), false)
        ])
    );
    assert!(
        sent(&h, &specs, &AWAY).is_empty(),
        "the phone doesn't go in again"
    );
}

#[test]
fn revoke_goes_to_the_device_that_holds_the_session() {
    let names = ["devbox", "ws-alpha"];
    let (h, specs) = devices(&names);
    publish(&h, &specs[1], vec![session_on("ws-alpha")]);
    h.fake.record().calls.clear();
    let revoke = |session| ClientMessage::RevokePhoneSession {
        key: IdempotencyKey::generate(),
        session,
    };

    block_on(h.devices.act(revoke(PhoneSessionId::derive("ws-alpha")))).unwrap();
    let to: Vec<LinkId> = h
        .fake
        .calls()
        .into_iter()
        .filter(|(_, call)| matches!(call, Call::Send(ClientMessage::RevokePhoneSession { .. })))
        .map(|(id, _)| id)
        .collect();
    assert_eq!(to, [specs[1].id]);
    assert!(block_on(h.devices.act(revoke(PhoneSessionId::derive("gone")))).is_err());
}

/// [`Facts`] with no host keys, until a device that comes up gets one
/// pinned. They keep what they were asked to pin.
#[derive(Clone, Default)]
struct Pins {
    asked: Arc<std::sync::Mutex<Vec<(DeviceId, String)>>>,
}

impl PeerFacts for Pins {
    fn route(&self, host: String) -> BoxFuture<'static, Option<PeerRoute>> {
        Facts.route(host)
    }

    fn host_key(&self, device: DeviceId) -> Option<String> {
        let asked = self.asked.lock().unwrap();
        asked
            .iter()
            .any(|(pinned, _)| *pinned == device)
            .then(|| HOST_KEY.to_owned())
    }

    fn pin(&self, device: DeviceId, host: String) -> BoxFuture<'static, bool> {
        self.asked.lock().unwrap().push((device, host));
        Box::pin(async { true })
    }
}

#[test]
fn a_device_pinned_when_it_comes_up_can_pair_a_phone() {
    let (h, specs) = devices(&["devbox"]);
    let pins = Pins::default();
    h.devices.link_peers(pins.clone());
    let note = || block_on(h.devices.phone_devices()).unwrap()[0].note.clone();
    assert_eq!(
        note().as_deref(),
        Some("MaxHQ has no host key for it. Set it up again to pair a phone with it.")
    );

    down(&h, &specs[0], "devbox");
    up(&h, &specs[0], "devbox");
    crate::devices::tests::wait_until("the device's host key is pinned", || {
        !pins.asked.lock().unwrap().is_empty()
    });
    assert_eq!(
        *pins.asked.lock().unwrap(),
        [(DeviceId::derive("devbox"), "devbox".to_owned())]
    );
    // The dialog reads the pin on its next look.
    assert_eq!(note(), None);
}

/// A key some device made up, as the phone's key on `device`.
const MADE_UP: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g";

/// What a device reports for a phone it made up, keeping the book itself.
fn made_up_on(device: &str, names: &[&str]) -> Vec<Entity> {
    let id = PhoneId::derive(MADE_UP);
    let Entity::PhoneBook(book) = book_on(device, names) else {
        unreachable!("book_on makes a book");
    };
    vec![
        Entity::Phone(Phone {
            id,
            device: DeviceId::derive(device),
            name: "iPhone".into(),
            ssh_public: MADE_UP.into(),
            push_public: STANDARD.encode([3; 32]),
            paired_at: Timestamp::now(),
            push: Vec::new(),
            everything: false,
        }),
        Entity::PhoneBook(PhoneBook { phone: id, ..book }),
    ]
}

/// The devices that were sent any phone, and every book sent, by name.
fn phones_and_books(h: &Harness, specs: &[LinkSpec], names: &[&str]) -> (Vec<String>, Vec<String>) {
    let mut added = Vec::new();
    let mut books = Vec::new();
    for (id, call) in h.fake.calls() {
        let Some(at) = specs.iter().position(|spec| spec.id == id) else {
            continue;
        };
        match call {
            Call::Send(ClientMessage::AddPhone { .. }) => added.push(names[at].to_owned()),
            Call::Send(ClientMessage::SetPhoneBook { .. }) => books.push(names[at].to_owned()),
            _ => {}
        }
    }
    (added, books)
}

fn begin_pair(h: &Harness, through: &str, others: &[&str]) {
    h.fake.record().acts.push_back(pair_outcome());
    let others: Vec<DeviceId> = others.iter().map(|name| DeviceId::derive(name)).collect();
    block_on(
        h.devices
            .begin_phone_pair(DeviceId::derive(through), &others),
    )
    .unwrap();
}

/// The phones the Mac has approved.
fn approved(h: &Harness) -> Approved {
    h.devices.core.read_setting(APPROVED)
}

#[test]
fn a_phone_no_pairing_asked_for_gets_nothing() {
    let names = ["evil", "victim", "maclocal"];
    let (h, specs) = devices(&names);
    h.devices.link_peers(Facts);
    block_on(h.devices.core.phone_pass());
    assert!(approved(&h).settled, "the links have all reported");

    publish(&h, &specs[1], vec![access("victim", true, Some(&pin()))]);
    publish(
        &h,
        &specs[0],
        made_up_on("evil", &["evil", "victim", "maclocal"]),
    );
    h.fake.record().acts.push_back(ActionOutcome::PhoneAdded {
        code: Secret::new("one-time-code".into()),
    });
    block_on(h.devices.core.phone_pass());

    let (added, books) = phones_and_books(&h, &specs, &names);
    assert_eq!(added, Vec::<String>::new(), "the key stays off the others");
    assert_eq!(books, Vec::<String>::new(), "no network part goes back");
    assert!(lock(&h.devices.core.phone_codes).is_empty());
}

#[test]
fn a_pairing_lets_in_its_phone_on_every_ticked_device_and_only_once() {
    let names = ["devbox", "ws-alpha", "ws-bravo", "ws-charlie"];
    let (h, specs) = devices(&names);
    h.devices.link_peers(Facts);
    block_on(h.devices.core.phone_pass());

    begin_pair(&h, "devbox", &["ws-alpha", "ws-bravo"]);
    h.fake.record().calls.clear();
    publish(
        &h,
        &specs[0],
        vec![
            phone_on("devbox"),
            book_on("devbox", &["devbox", "ws-alpha", "ws-bravo"]),
        ],
    );
    block_on(h.devices.core.phone_pass());
    assert_eq!(
        sent(&h, &specs, &names),
        BTreeSet::from([
            ("add", "ws-alpha".to_owned()),
            ("add", "ws-bravo".to_owned()),
        ])
    );

    // The pairing is used up, so a second phone from that device gets nothing.
    publish(&h, &specs[1], vec![phone_on("ws-alpha")]);
    publish(&h, &specs[2], vec![phone_on("ws-bravo")]);
    publish(
        &h,
        &specs[0],
        made_up_on("devbox", &["devbox", "ws-alpha", "ws-bravo"]),
    );
    h.fake.record().calls.clear();
    block_on(h.devices.core.phone_pass());
    assert!(!approved(&h).phones.contains_key(&PhoneId::derive(MADE_UP)));
    let (added, _) = phones_and_books(&h, &specs, &names);
    assert_eq!(added, Vec::<String>::new());
}

#[test]
fn a_pairing_answers_only_for_its_own_device_and_ticks() {
    let names = ["devbox", "evil", "ws-alpha", "ws-bravo"];
    let (h, specs) = devices(&names);
    h.devices.link_peers(Facts);
    block_on(h.devices.core.phone_pass());
    begin_pair(&h, "devbox", &["ws-alpha"]);
    h.fake.record().calls.clear();

    // Another device's phone, then the same device naming more than you ticked.
    publish(&h, &specs[1], made_up_on("evil", &["devbox", "ws-alpha"]));
    block_on(h.devices.core.phone_pass());
    publish(
        &h,
        &specs[0],
        made_up_on("devbox", &["devbox", "ws-alpha", "ws-bravo"]),
    );
    block_on(h.devices.core.phone_pass());
    assert!(!approved(&h).phones.contains_key(&PhoneId::derive(MADE_UP)));
    let (added, _) = phones_and_books(&h, &specs, &names);
    assert_eq!(added, Vec::<String>::new());
}

#[test]
fn a_pairing_lapses() {
    let names = ["devbox", "ws-alpha"];
    let (h, specs) = devices(&names);
    h.devices.link_peers(Facts);
    block_on(h.devices.core.phone_pass());
    h.fake.record().acts.push_back(ActionOutcome::PhonePair {
        pair: PairId::derive("pairing"),
        seed: Secret::new(STANDARD.encode([9; 32])),
        host_key: HOST_KEY.into(),
        expires_at: Timestamp(1),
    });
    block_on(
        h.devices
            .begin_phone_pair(DeviceId::derive("devbox"), &[DeviceId::derive("ws-alpha")]),
    )
    .unwrap();
    h.fake.record().calls.clear();
    publish(
        &h,
        &specs[0],
        vec![
            phone_on("devbox"),
            book_on("devbox", &["devbox", "ws-alpha"]),
        ],
    );
    block_on(h.devices.core.phone_pass());
    assert!(sent(&h, &specs, &names).is_empty());
    assert!(!approved(&h).phones.contains_key(&phone()));
}

#[test]
fn a_pairing_survives_an_app_restart() {
    let names = ["devbox", "ws-alpha"];
    let dir = std::env::temp_dir().join(format!("maxhq-pairing-{}", LinkId::generate()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("maxhq.db");
    let specs = {
        let registry = Registry::open(&path).unwrap();
        let specs: Vec<LinkSpec> = names
            .iter()
            .map(|name| registry.add(name).unwrap())
            .collect();
        let h = harness(registry, None);
        for (spec, name) in specs.iter().zip(names) {
            up(&h, spec, name);
        }
        settle(&h);
        h.devices.link_peers(Facts);
        block_on(h.devices.core.phone_pass());
        begin_pair(&h, "devbox", &["ws-alpha"]);
        specs
    };

    let h = harness(Registry::open(&path).unwrap(), None);
    for (spec, name) in specs.iter().zip(names) {
        up(&h, spec, name);
    }
    settle(&h);
    h.devices.link_peers(Facts);
    publish(
        &h,
        &specs[0],
        vec![
            phone_on("devbox"),
            book_on("devbox", &["devbox", "ws-alpha"]),
        ],
    );
    block_on(h.devices.core.phone_pass());
    assert_eq!(
        sent(&h, &specs, &names),
        BTreeSet::from([("add", "ws-alpha".to_owned())])
    );
    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_phone_from_before_the_mac_kept_this_still_gets_its_book_updates() {
    let names = ["devbox", "ws-alpha"];
    let registry = Registry::in_memory().unwrap();
    let specs: Vec<LinkSpec> = names
        .iter()
        .map(|name| registry.add(name).unwrap())
        .collect();
    let h = harness(registry, None);
    h.devices.link_peers(Facts);
    let local = h.local();
    h.report(local.id, link_state(&local, unreachable()));

    // The links come up one at a time, as they do when the app opens, and
    // the first pass doesn't take the view as it stands by then.
    up(&h, &specs[1], "ws-alpha");
    up(&h, &specs[0], "devbox");
    block_on(h.devices.core.phone_pass());
    assert!(!approved(&h).settled, "no snapshot is in yet");
    h.report(
        specs[1].id,
        LinkEvent::Synced(vec![
            phone_on("ws-alpha"),
            access("ws-alpha", true, Some(&pin())),
        ]),
    );
    block_on(h.devices.core.phone_pass());
    assert!(!approved(&h).settled, "one device has yet to report");
    assert!(books_sent(&h, &specs).is_empty());

    h.report(
        specs[0].id,
        LinkEvent::Synced(vec![
            phone_on("devbox"),
            book_on("devbox", &["devbox", "ws-alpha"]),
        ]),
    );
    block_on(h.devices.core.phone_pass());
    assert!(approved(&h).settled);
    let sent_books = books_sent(&h, &specs);
    let (at, book) = sent_books.last().expect("the book goes to its device");
    assert_eq!(*at, 0);
    assert_eq!(book[1].network, Some(network(None)));
}

#[test]
fn a_phone_on_a_device_away_at_the_first_pass_waits_for_a_new_pairing() {
    let names = ["devbox", "ws-alpha"];
    let (h, specs) = devices(&names);
    down(&h, &specs[0], "devbox");
    h.devices.link_peers(Facts);
    block_on(h.devices.core.phone_pass());
    assert!(
        approved(&h).settled,
        "a device that's away counts as settled"
    );

    up(&h, &specs[0], "devbox");
    publish(
        &h,
        &specs[0],
        vec![
            phone_on("devbox"),
            book_on("devbox", &["devbox", "ws-alpha"]),
        ],
    );
    h.fake.record().calls.clear();
    block_on(h.devices.core.phone_pass());
    assert!(sent(&h, &specs, &names).is_empty());

    begin_pair(&h, "devbox", &["ws-alpha"]);
    h.fake.record().calls.clear();
    block_on(h.devices.core.phone_pass());
    assert_eq!(
        sent(&h, &specs, &names),
        BTreeSet::from([("add", "ws-alpha".to_owned())])
    );
}

#[test]
fn removing_a_phone_takes_its_approval_with_it() {
    let names = ["devbox", "ws-alpha"];
    let (h, specs) = devices(&names);
    publish(
        &h,
        &specs[0],
        vec![
            phone_on("devbox"),
            book_on("devbox", &["devbox", "ws-alpha"]),
        ],
    );
    h.devices.link_peers(Facts);
    block_on(h.devices.core.phone_pass());
    assert!(approved(&h).phones.contains_key(&phone()));

    block_on(h.devices.remove_phone(phone())).unwrap();
    assert!(!approved(&h).phones.contains_key(&phone()));

    // The device says it paired again, with no pairing from you.
    std::thread::sleep(std::time::Duration::from_millis(5));
    h.fake.record().calls.clear();
    publish(&h, &specs[0], vec![phone_on("devbox")]);
    block_on(h.devices.core.phone_pass());
    assert!(h.devices.core.removals().is_empty());
    assert!(sent(&h, &specs, &names).is_empty());
}

#[test]
fn a_phone_paired_again_through_another_device_moves_to_it() {
    let names = ["devbox", "buildbox", "ws-alpha"];
    let (h, specs) = devices(&names);
    publish(
        &h,
        &specs[0],
        vec![phone_on("devbox"), book_on("devbox", &["devbox"])],
    );
    publish(&h, &specs[1], vec![access("buildbox", true, Some(&pin()))]);
    publish(&h, &specs[2], vec![access("ws-alpha", true, Some(&pin()))]);
    h.devices.link_peers(Facts);
    block_on(h.devices.core.phone_pass());
    assert_eq!(
        approved(&h).phones[&phone()].keeper,
        DeviceId::derive("devbox")
    );

    h.fake.record().acts.push_back(pair_outcome());
    block_on(h.devices.begin_phone_pair(
        DeviceId::derive("buildbox"),
        &[DeviceId::derive("ws-alpha")],
    ))
    .unwrap();
    h.fake.record().calls.clear();
    publish(
        &h,
        &specs[1],
        vec![
            phone_on("buildbox"),
            book_on("buildbox", &["buildbox", "ws-alpha"]),
        ],
    );
    block_on(h.devices.core.phone_pass());
    assert_eq!(
        approved(&h).phones[&phone()].keeper,
        DeviceId::derive("buildbox"),
        "the new pairing was never used"
    );
    assert!(sent(&h, &specs, &names).contains(&("add", "ws-alpha".to_owned())));
}

#[test]
fn another_device_cant_take_over_an_approved_phones_book() {
    let names = ["devbox", "ws-alpha", "evil"];
    let (h, specs) = devices(&names);
    publish(
        &h,
        &specs[0],
        vec![
            phone_on("devbox"),
            book_on("devbox", &["devbox", "ws-alpha"]),
        ],
    );
    publish(
        &h,
        &specs[1],
        vec![phone_on("ws-alpha"), access("ws-alpha", true, Some(&pin()))],
    );
    h.devices.link_peers(Facts);
    block_on(h.devices.core.phone_pass());
    h.fake.record().calls.clear();

    // The newest report of the book wins in the view, and it names `evil`.
    publish(
        &h,
        &specs[2],
        vec![phone_on("evil"), book_on("evil", &["evil", "ws-alpha"])],
    );
    block_on(h.devices.core.phone_pass());
    let (_, books) = phones_and_books(&h, &specs, &names);
    assert!(!books.contains(&"evil".to_owned()), "{books:?}");
}

#[test]
fn a_keeper_gets_nothing_for_a_device_you_didnt_tick() {
    let names = ["devbox", "ws-alpha", "ws-bravo"];
    let (h, specs) = devices(&names);
    publish(
        &h,
        &specs[0],
        vec![
            phone_on("devbox"),
            book_on("devbox", &["devbox", "ws-alpha"]),
        ],
    );
    publish(&h, &specs[1], vec![phone_on("ws-alpha")]);
    publish(&h, &specs[2], vec![access("ws-bravo", true, Some(&pin()))]);
    h.devices.link_peers(Facts);
    block_on(h.devices.core.phone_pass());
    h.fake.record().calls.clear();

    publish(
        &h,
        &specs[0],
        vec![book_on("devbox", &["devbox", "ws-alpha", "ws-bravo"])],
    );
    block_on(h.devices.core.phone_pass());
    assert!(sent(&h, &specs, &names).is_empty(), "the key stays off it");
    for (_, book) in books_sent(&h, &specs) {
        assert_eq!(book[2].network, None, "its network part stays off the book");
    }
}
