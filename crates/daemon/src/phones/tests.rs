// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::fs;
use std::path::Path;

use maxhq_protocol::netaccess::PhoneNetwork;
use maxhq_protocol::phones::PhoneRoute;

use super::*;
use crate::secrets::testing::hub_for;
use crate::session::clock::FakeClock;
use crate::testing::TempDir;

/// An ed25519 key line from a seed of `n`, which is no one's key.
fn key(n: u8) -> String {
    PublicKey::from(Ed25519Keypair::from_seed(&[n; 32]).public)
        .to_openssh()
        .unwrap()
}

fn push_key() -> String {
    STANDARD.encode([7u8; 32])
}

/// The Mac that makes the test phone's book.
fn home() -> PhoneMac {
    PhoneMac {
        device: DeviceId::derive("Home Mac"),
        name: "Home Mac".into(),
    }
}

fn entry(device: DeviceId, name: &str) -> PhoneBookEntry {
    PhoneBookEntry {
        device,
        name: name.into(),
        routes: vec![PhoneRoute {
            user: "dev".into(),
            host: format!("{name}.local"),
            port: 22,
        }],
        host_key: key(9),
        network: None,
    }
}

struct Harness {
    temp: TempDir,
    hub: Hub,
    clock: Arc<FakeClock>,
    phones: Phones,
}

impl Harness {
    fn new() -> Self {
        let temp = TempDir::new();
        let hub = hub_for("devbox");
        let clock = Arc::new(FakeClock::new());
        let phones = open(&temp, &hub, &clock);
        Self {
            temp,
            hub,
            clock,
            phones,
        }
    }

    fn keys(&self) -> String {
        fs::read_to_string(keys_path(self.temp.path())).unwrap_or_default()
    }

    fn book(&self) -> Vec<PhoneBookEntry> {
        vec![
            entry(self.hub.device().id, "devbox"),
            entry(DeviceId::derive("ws-alpha"), "ws-alpha"),
        ]
    }

    fn begin(&self) -> (PairId, String) {
        match self.phones.begin(self.book(), Some(home())) {
            ActionOutcome::PhonePair {
                pair,
                seed,
                host_key,
                expires_at,
            } => {
                assert_eq!(host_key, key(9));
                assert_eq!(expires_at, self.clock.wall().after(PAIR_WITHIN));
                (pair, seed.expose().to_owned())
            }
            other => panic!("{other:?}"),
        }
    }

    /// Pairs a phone with key `n` through a pairing, and says its ID.
    fn paired(&self, n: u8) -> PhoneId {
        let (pair, _) = self.begin();
        let pairing = self.phones.take(pair).unwrap();
        match self.phones.pair(pairing, "iPhone", &key(n), &push_key()) {
            ActionOutcome::PhonePaired { phone, book, mac } => {
                assert_eq!(book, self.book());
                assert_eq!(mac, Some(home()));
                phone
            }
            other => panic!("{other:?}"),
        }
    }
}

fn keys_path(dir: &Path) -> PathBuf {
    dir.join("home").join(".ssh").join("authorized_keys")
}

fn db_path(dir: &Path) -> PathBuf {
    dir.join("maxd.db")
}

fn open(temp: &TempDir, hub: &Hub, clock: &Arc<FakeClock>) -> Phones {
    Phones::open(
        hub.clone(),
        Folder::open(temp.path().join(DIR)).unwrap(),
        Db::open(&db_path(temp.path())).unwrap(),
        keys_path(temp.path()),
        clock.clone(),
    )
}

#[test]
fn a_pairing_key_works_once_and_gives_way_to_the_phones_own() {
    let h = Harness::new();
    let (pair, seed) = h.begin();
    let seed: [u8; 32] = STANDARD.decode(seed).unwrap().try_into().unwrap();
    let one_time = PublicKey::from(Ed25519Keypair::from_seed(&seed).public)
        .to_openssh()
        .unwrap();
    assert_eq!(
        h.keys(),
        format!(
            "restrict,command=\"~/.local/bin/maxd proxy --pair {pair}\" {one_time} maxhq-pair-{pair}\n"
        )
    );

    let pairing = h.phones.take(pair).unwrap();
    assert_eq!(h.keys(), "", "the key is out once it's used");
    assert!(h.phones.take(pair).is_err());

    let phone = match h.phones.pair(pairing, " iPhone ", &key(1), &push_key()) {
        ActionOutcome::PhonePaired { phone, book, mac } => {
            assert_eq!(book, h.book());
            assert_eq!(mac, Some(home()), "the phone hears which Mac paired it");
            phone
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(phone, PhoneId::derive(&key(1)));
    assert!(h.phones.knows(phone));
    assert_eq!(
        h.keys(),
        format!(
            "restrict,command=\"~/.local/bin/maxd proxy --phone {phone}\" {} maxhq-phone-{phone}\n",
            key(1)
        )
    );
    let device = h.hub.device().id;
    let Some(Entity::Phone(published)) = h.hub.get(&EntityKey::Phone { phone, device }) else {
        panic!("no phone");
    };
    assert_eq!(published.name, "iPhone");
    assert_eq!(published.push_public, push_key());
    assert!(published.push.is_empty());
    assert!(matches!(
        h.hub.get(&EntityKey::PhoneBook(phone)),
        Some(Entity::PhoneBook(book)) if book.devices == h.book() && book.mac == Some(home())
    ));

    // After a restart the phone and its book are published again.
    let hub = hub_for("devbox");
    let again = open(&h.temp, &hub, &h.clock);
    assert!(again.knows(phone));
    assert!(hub.get(&EntityKey::PhoneBook(phone)).is_some());
}

#[tokio::test]
async fn a_pairing_nobody_uses_goes_after_ten_minutes() {
    let h = Harness::new();
    let (pair, _) = h.begin();
    assert!(h.keys().contains("--pair"));
    while h.clock.sleepers() == 0 {
        tokio::task::yield_now().await;
    }
    h.clock.advance(PAIR_WITHIN);
    for _ in 0..100 {
        if h.keys().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(h.keys(), "");
    assert!(h.phones.take(pair).is_err());
}

#[test]
fn a_pairing_from_before_a_restart_is_taken_out() {
    let h = Harness::new();
    let (pair, _) = h.begin();
    let phone = h.paired(1);
    let _ = open(&h.temp, &hub_for("devbox"), &h.clock);
    assert!(!h.keys().contains(&pair.to_string()));
    assert!(h.keys().contains(&phone.to_string()));
}

#[test]
fn a_book_must_hold_this_device_and_plain_addresses() {
    let h = Harness::new();
    let refused_with = |book: Vec<PhoneBookEntry>| match h.phones.begin(book, None) {
        ActionOutcome::Refused { reason } => reason,
        other => panic!("{other:?}"),
    };
    let other = entry(DeviceId::derive("ws-alpha"), "ws-alpha");
    assert!(refused_with(vec![other.clone()]).contains("pairs through"));
    assert!(refused_with(Vec::new()).contains("at least one"));
    let mut bad = entry(h.hub.device().id, "devbox");
    bad.routes[0].host = "devbox.local -oProxyCommand=x".into();
    assert!(refused_with(vec![bad]).contains("plain SSH address"));
    let mut bad = entry(h.hub.device().id, "devbox");
    bad.host_key = "ssh-rsa AAAA".into();
    assert!(refused_with(vec![bad]).contains("host key"));
    assert!(refused_with(vec![other.clone(), other]).contains("twice"));
    assert_eq!(h.keys(), "", "nothing was let in");
}

#[test]
fn network_access_alone_is_a_way_in() {
    let h = Harness::new();
    let network = PhoneNetwork {
        hosts: vec!["192.168.1.20".into(), "fd7a:115c:a1e0::1".into()],
        port: 7466,
        pin: "A".repeat(43),
        code: None,
    };
    let mut mac = entry(h.hub.device().id, "devbox");
    mac.routes.clear();
    mac.host_key = String::new();
    let refused = |entry: &PhoneBookEntry| match h.phones.begin(vec![entry.clone()], None) {
        ActionOutcome::Refused { reason } => reason,
        other => panic!("{other:?}"),
    };
    assert!(refused(&mac).contains("no address"));
    mac.network = Some(network.clone());
    match h.phones.begin(vec![mac.clone()], None) {
        ActionOutcome::PhonePair { host_key, .. } => assert_eq!(host_key, ""),
        other => panic!("{other:?}"),
    }
    for bad in [
        PhoneNetwork {
            hosts: Vec::new(),
            ..network.clone()
        },
        PhoneNetwork {
            hosts: vec!["192.168.1.20 -x".into()],
            ..network.clone()
        },
        PhoneNetwork {
            pin: "short".into(),
            ..network.clone()
        },
        PhoneNetwork { port: 0, ..network },
    ] {
        mac.network = Some(bad);
        assert!(refused(&mac).contains("network address"));
    }
}

#[test]
fn only_a_few_pairings_wait_at_once() {
    let h = Harness::new();
    for _ in 0..PAIRS_MAX {
        h.begin();
    }
    assert!(matches!(
        h.phones.begin(h.book(), None),
        ActionOutcome::Refused { .. }
    ));
}

#[test]
fn removing_a_phone_takes_its_key_its_entities_and_its_connections() {
    let h = Harness::new();
    let phone = h.paired(1);
    let kept = h.paired(2);
    let mut removed = h.phones.removed();
    assert_eq!(h.phones.remove(phone), ActionOutcome::Done);
    assert_eq!(removed.try_recv().unwrap(), phone);
    assert!(!h.phones.knows(phone));
    assert!(!h.keys().contains(&phone.to_string()));
    assert!(h.keys().contains(&kept.to_string()));
    let device = h.hub.device().id;
    assert!(h.hub.get(&EntityKey::Phone { phone, device }).is_none());
    assert!(h.hub.get(&EntityKey::PhoneBook(phone)).is_none());
    // Again is done too, since the phone is gone.
    assert_eq!(h.phones.remove(phone), ActionOutcome::Done);
}

#[test]
fn a_phone_added_from_another_device_has_no_book_here() {
    let h = Harness::new();
    let phone = PhoneId::derive(&key(3));
    assert_eq!(
        h.phones.add(phone, "iPhone", &key(3), &push_key()),
        ActionOutcome::Done
    );
    assert!(h.keys().contains(&format!("--phone {phone}")));
    assert!(h.hub.get(&EntityKey::PhoneBook(phone)).is_none());
    assert!(matches!(
        h.phones.set_book(phone, h.book(), None),
        ActionOutcome::Refused { .. }
    ));
    // Adding it again keeps the one line.
    assert_eq!(
        h.phones.add(phone, "iPhone", &key(3), &push_key()),
        ActionOutcome::Done
    );
    assert_eq!(h.keys().lines().count(), 1);
    assert!(matches!(
        h.phones.add(phone, "iPhone", &key(3), "short"),
        ActionOutcome::Refused { .. }
    ));
}

#[test]
fn a_phone_added_under_an_id_its_key_doesnt_give_is_refused() {
    let h = Harness::new();
    let phone = PhoneId::derive(&key(3));
    h.phones.add(phone, "iPhone", &key(3), &push_key());
    let before = h.keys();
    // Another key under that phone's ID would let a second key in as it.
    assert!(matches!(
        h.phones.add(phone, "iPhone", &key(4), &push_key()),
        ActionOutcome::Refused { .. }
    ));
    let stranger = PhoneId::derive(&key(5));
    assert!(matches!(
        h.phones.add(stranger, "iPhone", &key(4), &push_key()),
        ActionOutcome::Refused { .. }
    ));
    assert_eq!(h.keys(), before);
}

#[test]
fn a_book_changes_for_a_phone_that_paired_here() {
    let h = Harness::new();
    let phone = h.paired(1);
    let book = vec![entry(h.hub.device().id, "devbox")];
    assert_eq!(
        h.phones.set_book(phone, book.clone(), None),
        ActionOutcome::Done
    );
    assert!(
        matches!(
            h.hub.get(&EntityKey::PhoneBook(phone)),
            Some(Entity::PhoneBook(published))
                if published.devices == book && published.mac == Some(home())
        ),
        "a book that names no Mac keeps the one before"
    );

    // A Mac that took a new name gives it, and a restart keeps it. A
    // blank one changes nothing.
    let renamed = PhoneMac {
        name: "Studio Mac".into(),
        ..home()
    };
    let blank = PhoneMac {
        name: " ".into(),
        ..home()
    };
    for mac in [renamed.clone(), blank] {
        assert_eq!(
            h.phones.set_book(phone, book.clone(), Some(mac)),
            ActionOutcome::Done
        );
    }
    let hub = hub_for("devbox");
    let _again = open(&h.temp, &hub, &h.clock);
    assert!(matches!(
        hub.get(&EntityKey::PhoneBook(phone)),
        Some(Entity::PhoneBook(published)) if published.mac == Some(renamed)
    ));
}

/// Whether the phone this device publishes for `phone` says its switch is
/// on.
fn published_everything(hub: &Hub, phone: PhoneId) -> bool {
    let device = hub.device().id;
    match hub.get(&EntityKey::Phone { phone, device }) {
        Some(Entity::Phone(published)) => published.everything,
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_switch_is_kept_until_turned_off_or_the_phone_goes() {
    let h = Harness::new();
    let phone = h.paired(1);
    let other = h.paired(2);
    assert!(!h.phones.everything(phone), "a new phone starts off");
    assert!(!published_everything(&h.hub, phone));

    assert_eq!(h.phones.set_everything(phone, true), ActionOutcome::Done);
    assert!(h.phones.everything(phone));
    assert!(!h.phones.everything(other));
    assert!(published_everything(&h.hub, phone));
    // Again is done too.
    assert_eq!(h.phones.set_everything(phone, true), ActionOutcome::Done);

    // A restart keeps it, and letting the phone in again doesn't turn it
    // off.
    let hub = hub_for("devbox");
    let again = open(&h.temp, &hub, &h.clock);
    assert!(again.everything(phone));
    assert!(published_everything(&hub, phone));
    assert_eq!(
        again.add(phone, "iPhone", &key(1), &push_key()),
        ActionOutcome::Done
    );
    assert!(again.everything(phone));

    assert_eq!(again.set_everything(phone, false), ActionOutcome::Done);
    assert!(!again.everything(phone));
    assert!(!published_everything(&hub, phone));
    assert!(!open(&h.temp, &hub_for("devbox"), &h.clock).everything(phone));

    // A phone removed and let in again starts off.
    assert_eq!(again.set_everything(phone, true), ActionOutcome::Done);
    assert_eq!(again.remove(phone), ActionOutcome::Done);
    assert_eq!(
        again.add(phone, "iPhone", &key(1), &push_key()),
        ActionOutcome::Done
    );
    assert!(!again.everything(phone));
    assert!(!open(&h.temp, &hub_for("devbox"), &h.clock).everything(phone));

    // A phone this device doesn't let in has no switch here.
    let stranger = PhoneId::derive(&key(5));
    assert!(matches!(
        again.set_everything(stranger, true),
        ActionOutcome::Refused { .. }
    ));
}

#[test]
fn a_database_from_before_the_switch_opens_with_every_phone_off() {
    let h = Harness::new();
    let phone = h.paired(1);
    assert_eq!(h.phones.set_everything(phone, true), ActionOutcome::Done);
    // An older build's database, without the switch's table.
    fs::remove_file(db_path(h.temp.path())).unwrap();
    {
        let mut conn = rusqlite::Connection::open(db_path(h.temp.path())).unwrap();
        crate::db::migrations().to_version(&mut conn, 28).unwrap();
    }
    let hub = hub_for("devbox");
    let again = open(&h.temp, &hub, &h.clock);
    assert!(again.knows(phone));
    assert!(!again.everything(phone));
    assert!(!published_everything(&hub, phone));
}
