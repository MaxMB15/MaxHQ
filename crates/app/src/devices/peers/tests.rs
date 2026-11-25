// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::sync::Mutex;

use maxhq_client::LinkEvent;
use maxhq_protocol::ids::{AccountId, ModelServerId};
use maxhq_protocol::link::LinkState;
use maxhq_protocol::local_models::ModelLoad;
use maxhq_protocol::message::Capabilities;
use maxhq_protocol::model::Activity;

use super::*;
use crate::devices::tests::{harness, hello_as, Call, Harness};
use crate::merge::tests::{connected, link_state, unreachable};
use crate::registry::Registry;
use crate::tests::block_on;

/// The host key setup pinned for devbox.
const HOST_KEY: &str = "ssh-ed25519 host-key-of-devbox";

/// Routes and host keys a test sets: every host is reached straight at
/// `<host>.example.ts.net`, apart from `jumped`, which needs a jump host.
#[derive(Clone, Default)]
struct Facts(Arc<Mutex<BTreeMap<DeviceId, String>>>);

impl PeerFacts for Facts {
    fn route(&self, host: String) -> BoxFuture<'static, Option<PeerRoute>> {
        Box::pin(async move {
            (host != "jumped").then(|| PeerRoute {
                host: format!("{host}.example.ts.net"),
                port: 22,
                user: "dev".to_owned(),
            })
        })
    }

    fn host_key(&self, device: DeviceId) -> Option<String> {
        self.0.lock().unwrap().get(&device).cloned()
    }
}

/// What a fake link was asked about device links, by the device it reaches.
#[derive(Clone, Debug, PartialEq)]
enum Sent {
    Key(&'static str),
    LetIn(&'static str, DeviceId),
    Revoke(&'static str, DeviceId),
    Book(&'static str, PeerBook),
}

/// This machine as "mac" and devbox as "nl", both connected, whose
/// `maxd` links to others when `peers` says so. The links start once
/// they're up, so each test sees one pass's worth.
fn mac_and_devbox(peers: bool, pinned: bool) -> (Harness, LinkSpec, LinkSpec, Facts) {
    let registry = Registry::in_memory().unwrap();
    let nl = registry.add("devbox").unwrap();
    let h = harness(registry, None);
    let facts = Facts::default();
    if pinned {
        facts
            .0
            .lock()
            .unwrap()
            .insert(DeviceId::derive("nl"), HOST_KEY.to_owned());
    }
    let local = h.local();
    join(&h, &local, "mac", true);
    join(&h, &nl, "nl", peers);
    h.devices.link_peers(facts.clone());
    (h, local, nl, facts)
}

/// Has a link come up to `device`, whose `maxd` links to others when
/// `peers` says so.
fn join(h: &Harness, spec: &LinkSpec, device: &str, peers: bool) {
    h.fake.record().up.insert(spec.id);
    report(h, spec, device, connected(), peers);
}

/// Has a link to `device` drop.
fn leave(h: &Harness, spec: &LinkSpec, device: &str) {
    h.fake.record().up.remove(&spec.id);
    report(h, spec, device, unreachable(), true);
}

fn report(h: &Harness, spec: &LinkSpec, device: &str, state: LinkState, peers: bool) {
    let LinkEvent::Link(mut link) = link_state(spec, state) else {
        unreachable!("link_state reports a link");
    };
    link.device = Some(DeviceId::derive(device));
    link.capabilities = Some(Capabilities {
        peers,
        ..Capabilities::default()
    });
    h.report(spec.id, LinkEvent::Link(link));
}

/// Runs a pass now, and gives what it and any before it sent.
fn pass(h: &Harness, names: &[(&LinkSpec, &'static str)]) -> Vec<Sent> {
    block_on(h.devices.core.peer_pass());
    sent(h, names)
}

/// What passes sent since the last look.
fn sent(h: &Harness, names: &[(&LinkSpec, &'static str)]) -> Vec<Sent> {
    h.fake
        .calls()
        .into_iter()
        .filter_map(|(id, call)| {
            let (_, name) = names.iter().find(|(spec, _)| spec.id == id)?;
            match call {
                Call::Send(ClientMessage::PeerKey { .. }) => Some(Sent::Key(name)),
                Call::Send(ClientMessage::AuthorizePeer {
                    device, public_key, ..
                }) => {
                    assert_eq!(public_key, key_of(names, device), "the key of the device");
                    Some(Sent::LetIn(name, device))
                }
                Call::Send(ClientMessage::RevokePeer { device, .. }) => {
                    Some(Sent::Revoke(name, device))
                }
                Call::Send(ClientMessage::SetPeerBook { book, .. }) => Some(Sent::Book(name, book)),
                _ => None,
            }
        })
        .collect()
}

/// The key the fake gives for the device named `device`.
fn key_of(names: &[(&LinkSpec, &'static str)], device: DeviceId) -> String {
    let (spec, _) = names
        .iter()
        .find(|(_, name)| DeviceId::derive(name) == device)
        .expect("a device the test named");
    format!("ssh-ed25519 key-of-{}", spec.id)
}

/// What went to the device named `name`, in order. Devices take their
/// turns in id order, so a test looks at each apart.
fn to(sent: &[Sent], name: &str) -> Vec<Sent> {
    sent.iter()
        .filter(|sent| match sent {
            Sent::Key(to) | Sent::LetIn(to, _) | Sent::Revoke(to, _) | Sent::Book(to, _) => {
                *to == name
            }
        })
        .cloned()
        .collect()
}

fn route(host: &str) -> PeerRoute {
    PeerRoute {
        host: format!("{host}.example.ts.net"),
        port: 22,
        user: "dev".to_owned(),
    }
}

fn states(h: &Harness, device: &str) -> Vec<(String, PeerLinkState)> {
    h.devices
        .device_links(DeviceId::derive(device))
        .unwrap()
        .into_iter()
        .map(|link| (link.name, link.state))
        .collect()
}

#[test]
fn keys_are_asked_once_and_logins_go_only_to_devices_that_take_them() {
    let (h, local, nl, _) = mac_and_devbox(true, true);
    let names = [(&local, "mac"), (&nl, "nl")];
    let (mac, devbox) = (DeviceId::derive("mac"), DeviceId::derive("nl"));

    let sent = pass(&h, &names);
    assert!(
        matches!(&sent[..2], [Sent::Key(_), Sent::Key(_)]),
        "keys come first"
    );
    assert_eq!(
        to(&sent, "mac"),
        [
            Sent::Key("mac"),
            Sent::Book(
                "mac",
                PeerBook {
                    ssh: false,
                    peers: vec![PeerEntry {
                        device: devbox,
                        name: "devbox".into(),
                        routes: vec![route("devbox")],
                        host_key: HOST_KEY.into(),
                        test_kinds: Vec::new(),
                        home: false,
                        model_server: None,
                        models_from: Vec::new(),
                    }],
                }
            ),
        ]
    );
    assert_eq!(
        to(&sent, "nl"),
        [
            Sent::Key("nl"),
            Sent::LetIn("nl", mac),
            Sent::Book(
                "nl",
                PeerBook {
                    ssh: true,
                    // The Mac is never dialed, so it has no host key. It
                    // runs the app, so it may move agents.
                    peers: vec![PeerEntry {
                        device: mac,
                        name: local.name.clone(),
                        routes: Vec::new(),
                        host_key: String::new(),
                        test_kinds: Vec::new(),
                        home: true,
                        model_server: None,
                        models_from: Vec::new(),
                    }],
                }
            ),
        ]
    );
    assert_eq!(
        states(&h, "mac"),
        [("devbox".to_owned(), PeerLinkState::Linked)]
    );
    assert_eq!(
        states(&h, "nl"),
        [(local.name.clone(), PeerLinkState::Linked)]
    );

    assert_eq!(pass(&h, &names), [], "nothing changed");
    leave(&h, &nl, "nl");
    join(&h, &nl, "nl", true);
    assert_eq!(pass(&h, &names), [], "a reconnect with nothing changed");
}

/// Facts that note which hosts a device's key was to be pinned from.
#[derive(Clone, Default)]
struct PinNotes {
    facts: Facts,
    asked: Arc<Mutex<Vec<(DeviceId, String)>>>,
}

impl PeerFacts for PinNotes {
    fn route(&self, host: String) -> BoxFuture<'static, Option<PeerRoute>> {
        self.facts.route(host)
    }

    fn host_key(&self, device: DeviceId) -> Option<String> {
        self.facts.host_key(device)
    }

    fn pin(&self, device: DeviceId, host: String) -> BoxFuture<'static, bool> {
        self.asked.lock().unwrap().push((device, host));
        Box::pin(async { false })
    }
}

#[test]
fn a_refused_hello_reaches_no_key_route_or_device_of_another() {
    let registry = Registry::in_memory().unwrap();
    let nl = registry.add("devbox").unwrap();
    let bad = registry.add("hostile").unwrap();
    let (nl_id, bad_id) = (DeviceId::derive("nl"), DeviceId::derive("bad"));
    registry.keep_device(nl.id, nl_id).unwrap();
    registry.keep_device(bad.id, bad_id).unwrap();
    let h = harness(registry, None);
    let notes = PinNotes::default();
    notes.facts.0.lock().unwrap().extend([
        (nl_id, HOST_KEY.to_owned()),
        (bad_id, "ssh-ed25519 host-key-of-bad".to_owned()),
    ]);
    let takes_peers = Some(Capabilities {
        peers: true,
        ..Capabilities::default()
    });
    let local = h.local();
    let names = [(&local, "mac"), (&nl, "nl"), (&bad, "bad")];
    join(&h, &local, "mac", true);
    hello_as(&h, &nl, "nl", takes_peers);
    hello_as(&h, &bad, "bad", takes_peers);
    h.devices.link_peers(notes.clone());
    pass(&h, &names);

    // devbox goes off, and the hostile device comes back saying it's devbox.
    leave(&h, &nl, "nl");
    leave(&h, &bad, "bad");
    hello_as(&h, &bad, "nl", takes_peers);
    let sent = pass(&h, &names);

    // Each link stays with its own device, so nothing gets let go.
    assert!(
        !sent.iter().any(|sent| matches!(sent, Sent::Revoke(..))),
        "{sent:?}"
    );
    let kept = h.devices.core.kept();
    assert_eq!(
        kept.devices[&nl_id].links,
        BTreeSet::from([nl.id]),
        "the hostile link isn't one of devbox's"
    );
    assert_eq!(kept.devices[&bad_id].links, BTreeSet::from([bad.id]));

    // devbox's routes in others' books stay its own.
    let found = block_on(find(
        &notes,
        nl_id,
        &kept.devices[&nl_id],
        &h.devices.core.specs(),
        Vec::new(),
    ));
    assert_eq!(found.routes, [route("devbox")]);

    // The hostile host's key is never pinned under devbox's ID.
    assert!(!notes
        .asked
        .lock()
        .unwrap()
        .contains(&(nl_id, "hostile".to_owned())));
}

#[test]
fn a_replaced_pin_goes_out_to_the_other_devices() {
    let (h, local, nl, facts) = mac_and_devbox(true, true);
    let names = [(&local, "mac"), (&nl, "nl")];
    let first = pass(&h, &names);
    assert!(matches!(
        &to(&first, "mac")[..],
        [.., Sent::Book(_, book)] if book.peers[0].host_key == HOST_KEY
    ));

    // A setup you confirmed replaced the pin, and says so.
    let rotated = "ssh-ed25519 new-host-key-of-devbox";
    facts
        .0
        .lock()
        .unwrap()
        .insert(DeviceId::derive("nl"), rotated.to_owned());
    h.devices.host_keys_changed();
    let start = std::time::Instant::now();
    let book = loop {
        if let [Sent::Book(_, book)] = &to(&sent(&h, &names), "mac")[..] {
            break book.clone();
        }
        assert!(start.elapsed().as_secs() < 5, "the Mac never got a book");
        std::thread::sleep(std::time::Duration::from_millis(2));
    };
    assert_eq!(book.peers[0].host_key, rotated);
}

#[test]
fn a_link_that_only_says_it_reaches_a_device_cant_count_as_its_only_link() {
    let (h, local, nl, _facts) = mac_and_devbox(true, true);
    let names = [(&local, "mac"), (&nl, "nl")];
    pass(&h, &names);
    let devbox = DeviceId::derive("nl");
    assert!(
        !h.devices.another_link_reaches(devbox, nl.id),
        "its own link"
    );

    // A second host says hello as the same device.
    let other = h.devices.add_device("stand-in").unwrap();
    let other = h.devices.registry().get(other).unwrap();
    join(&h, &other, "nl", true);
    pass(&h, &[(&local, "mac"), (&nl, "nl"), (&other, "stand-in")]);
    assert!(h.devices.another_link_reaches(devbox, other.id));

    let fresh = DeviceId::derive("fresh");
    assert!(
        !h.devices.another_link_reaches(fresh, other.id),
        "nothing reaches it"
    );

    // One whose maxd doesn't link counts too, though it's never kept.
    let old = DeviceId::derive("old");
    let (first, second) = (h.devices.add_device("old-1"), h.devices.add_device("old-2"));
    let first = h.devices.registry().get(first.unwrap()).unwrap();
    let second = h.devices.registry().get(second.unwrap()).unwrap();
    join(&h, &first, "old", false);
    assert!(!h.devices.another_link_reaches(old, first.id));
    join(&h, &second, "old", false);
    assert!(h.devices.another_link_reaches(old, second.id));
}

#[test]
fn a_device_with_no_pinned_host_key_stays_out_of_books() {
    let (h, local, nl, facts) = mac_and_devbox(true, false);
    let names = [(&local, "mac"), (&nl, "nl")];

    let sent = pass(&h, &names);
    assert!(sent.contains(&Sent::Book(
        "mac",
        PeerBook {
            ssh: false,
            peers: Vec::new(),
        }
    )));
    let links = h.devices.device_links(DeviceId::derive("mac")).unwrap();
    assert_eq!(links[0].state, PeerLinkState::Waiting);
    assert!(links[0]
        .note
        .as_deref()
        .unwrap()
        .contains("Set it up again"));

    // Setup pins it, and the next pass sends the Mac a book with it.
    facts
        .0
        .lock()
        .unwrap()
        .insert(DeviceId::derive("nl"), HOST_KEY.to_owned());
    let sent = pass(&h, &names);
    assert!(matches!(&sent[..], [Sent::Book("mac", book)] if book.peers.len() == 1));
}

#[test]
fn devices_whose_maxd_does_not_link_are_skipped() {
    let (h, local, nl, _) = mac_and_devbox(false, true);
    let names = [(&local, "mac"), (&nl, "nl")];

    assert_eq!(
        pass(&h, &names),
        [
            Sent::Key("mac"),
            Sent::Book(
                "mac",
                PeerBook {
                    ssh: false,
                    peers: Vec::new(),
                }
            ),
        ]
    );
    assert!(h.devices.device_links(DeviceId::derive("nl")).is_err());
}

#[test]
fn a_removed_device_comes_off_the_others_even_those_away() {
    let registry = Registry::in_memory().unwrap();
    let nl = registry.add("devbox").unwrap();
    let spare = registry.add("spare").unwrap();
    let h = harness(registry, None);
    let facts = Facts::default();
    for (device, key) in [("nl", HOST_KEY), ("spare", "ssh-ed25519 host-key-of-spare")] {
        facts
            .0
            .lock()
            .unwrap()
            .insert(DeviceId::derive(device), key.to_owned());
    }
    let local = h.local();
    let names = [(&local, "mac"), (&nl, "nl"), (&spare, "spare")];
    join(&h, &local, "mac", true);
    join(&h, &nl, "nl", true);
    join(&h, &spare, "spare", true);
    h.devices.link_peers(facts);
    let sent = pass(&h, &names);
    assert!(sent.contains(&Sent::LetIn("spare", DeviceId::derive("nl"))));
    assert!(sent.contains(&Sent::LetIn("nl", DeviceId::derive("spare"))));
    assert!(!sent
        .iter()
        .any(|sent| matches!(sent, Sent::LetIn("mac", _))));

    leave(&h, &spare, "spare");
    h.devices.remove_device(nl.id).unwrap();
    let devbox = DeviceId::derive("nl");
    let sent = pass(&h, &names);
    assert_eq!(sent.len(), 2, "nothing for the away device: {sent:?}");
    assert_eq!(sent[0], Sent::Revoke("mac", devbox));
    assert!(matches!(&sent[1..], [Sent::Book("mac", book)] if book.peers.len() == 1));

    // The away device gets its revoke once it's back, and only once.
    join(&h, &spare, "spare", true);
    let sent = pass(&h, &names);
    assert_eq!(sent[0], Sent::Revoke("spare", devbox));
    assert!(matches!(&sent[1..], [Sent::Book("spare", book)] if book.peers.len() == 1));
    assert_eq!(pass(&h, &names), []);
}

#[test]
fn a_pair_turned_off_stays_off_until_allowed() {
    let (h, local, nl, _) = mac_and_devbox(true, true);
    let names = [(&local, "mac"), (&nl, "nl")];
    let (mac, devbox) = (DeviceId::derive("mac"), DeviceId::derive("nl"));
    pass(&h, &names);

    block_on(h.devices.set_device_link(mac, devbox, false)).unwrap();
    let empty = |ssh| PeerBook {
        ssh,
        peers: Vec::new(),
    };
    let sent = pass(&h, &names);
    assert_eq!(
        to(&sent, "mac"),
        [Sent::Revoke("mac", devbox), Sent::Book("mac", empty(false))]
    );
    assert_eq!(
        to(&sent, "nl"),
        [Sent::Revoke("nl", mac), Sent::Book("nl", empty(true))]
    );
    assert_eq!(states(&h, "nl"), [(local.name.clone(), PeerLinkState::Off)]);
    leave(&h, &nl, "nl");
    join(&h, &nl, "nl", true);
    assert_eq!(pass(&h, &names), [], "the sync leaves it off");

    block_on(h.devices.set_device_link(devbox, mac, true)).unwrap();
    let sent = pass(&h, &names);
    assert!(sent.contains(&Sent::LetIn("nl", mac)));
    assert_eq!(
        sent.len(),
        3,
        "devbox lets the Mac in, and a book for each: {sent:?}"
    );
    assert_eq!(
        states(&h, "mac"),
        [("devbox".to_owned(), PeerLinkState::Linked)]
    );
}

/// A test host of `kind` on the device named `device`, in `state`.
fn test_host(device: &str, kind: TestKind, state: TestHostState) -> Entity {
    let device = DeviceId::derive(device);
    let backend = kind.backends()[0];
    Entity::TestHost(maxhq_protocol::leases::TestHost {
        id: maxhq_protocol::leases::host_id(device, kind, backend),
        device,
        kind,
        backend,
        state,
        setup: None,
        error: None,
        agent_device: None,
        last_reset: None,
        queue: 0,
        vm: None,
    })
}

#[test]
fn a_book_says_which_kinds_each_device_hosts_once_they_change() {
    let (h, local, nl, _) = mac_and_devbox(true, true);
    let names = [(&local, "mac"), (&nl, "nl")];
    pass(&h, &names);

    // One that lacks what it needs hosts nothing yet.
    let hosts = [
        test_host("nl", TestKind::Linux, TestHostState::Unavailable),
        test_host("nl", TestKind::AndroidEmulator, TestHostState::Warm),
    ];
    let changes = hosts
        .into_iter()
        .map(|value| maxhq_protocol::state::Change::Upsert { value })
        .collect();
    h.report(nl.id, LinkEvent::Changed(changes));
    // The change brings a pass of its own.
    let start = std::time::Instant::now();
    let book = loop {
        if let [Sent::Book(_, book)] = &to(&sent(&h, &names), "mac")[..] {
            break book.clone();
        }
        assert!(start.elapsed().as_secs() < 5, "the Mac never got a book");
        std::thread::sleep(std::time::Duration::from_millis(2));
    };
    assert_eq!(book.peers[0].test_kinds, [TestKind::AndroidEmulator]);

    // A host that only gets busy changes nothing.
    let busy = test_host("nl", TestKind::AndroidEmulator, TestHostState::Leased);
    let changes = vec![maxhq_protocol::state::Change::Upsert { value: busy }];
    h.report(nl.id, LinkEvent::Changed(changes));
    assert_eq!(pass(&h, &names), []);
}

/// The Mac's managed model server, with `load` on its one model.
fn managed_on_mac(load: ModelLoad) -> Entity {
    Entity::ModelServer(maxhq_protocol::local_models::ModelServer {
        id: ModelServerId::derive("model-server:mac:managed"),
        device: DeviceId::derive("mac"),
        kind: ModelServerKind::Managed,
        label: "Mac".into(),
        base_url: None,
        token: false,
        admin_key: false,
        probe: None,
        credits: None,
        models: vec![ServerModel {
            id: "qwen@q4".into(),
            label: "Qwen".into(),
            catalog: None,
            context: Some(32_768),
            load: Some(load),
            speed: Some(maxhq_protocol::local_models::ModelSpeed {
                prompt_per_second: Some(900),
                decode_per_second: Some(40),
            }),
        }],
    })
}

/// The next book the device named `name` gets.
fn next_book(h: &Harness, names: &[(&LinkSpec, &'static str)], name: &str) -> PeerBook {
    let start = std::time::Instant::now();
    loop {
        if let Some(Sent::Book(_, book)) = to(&sent(h, names), name).pop() {
            return book;
        }
        assert!(start.elapsed().as_secs() < 5, "{name} never got a book");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

#[test]
fn a_book_carries_the_macs_models_and_whose_agents_use_them() {
    use maxhq_protocol::model::{Account, LoginKind, Vendor};
    use maxhq_protocol::state::Change;

    let (h, local, nl, _) = mac_and_devbox(true, true);
    let names = [(&local, "mac"), (&nl, "nl")];
    pass(&h, &names);

    // The Mac's server reaches devbox's book, with no load or speed.
    let upsert = |value| LinkEvent::Changed(vec![Change::Upsert { value }]);
    h.report(local.id, upsert(managed_on_mac(ModelLoad::Unloaded)));
    let book = next_book(&h, &names, "nl");
    let shared = book.peers[0]
        .model_server
        .clone()
        .expect("the Mac's models");
    assert_eq!(
        shared.server,
        ModelServerId::derive("model-server:mac:managed")
    );
    assert_eq!(shared.label, "Mac");
    assert_eq!(shared.models[0].id, "qwen@q4");
    assert_eq!(shared.models[0].context, Some(32_768));
    assert_eq!(shared.models[0].load, None);
    assert_eq!(shared.models[0].speed, None);
    assert!(book.peers[0].models_from.is_empty());

    // A model loading sends no book.
    h.report(
        local.id,
        upsert(managed_on_mac(ModelLoad::Loading { percent: Some(40) })),
    );
    assert_eq!(pass(&h, &names), []);

    // An agent on devbox whose account uses the Mac's server tells the
    // Mac to hold a link for it.
    let account = AccountId::derive("devbox local");
    h.report(
        nl.id,
        upsert(Entity::Account(Account {
            id: account,
            device: DeviceId::derive("nl"),
            vendor: Vendor::Claude,
            label: "Mac".into(),
            login: LoginKind::ModelServer,
            origin: None,
            config_dir: String::new(),
            email: None,
            organization: None,
            plan: None,
            models: Vec::new(),
            usage: None,
            usage_failure: None,
            acp: None,
            model_server: Some(ModelServerId::derive("model-server:mac:managed")),
        })),
    );
    let mut agent = crate::merge::tests::agent("local", "w", Activity::Idle);
    if let Entity::Agent(agent) = &mut agent {
        agent.account = Some(account);
    }
    h.report(nl.id, upsert(agent));
    let book = next_book(&h, &names, "mac");
    assert_eq!(book.peers[0].device, DeviceId::derive("nl"));
    assert_eq!(book.peers[0].models_from, [DeviceId::derive("mac")]);
    assert_eq!(book.peers[0].model_server, None, "devbox has no server");
}
