// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use maxhq_protocol::admission::AppKeyOrigin;
use maxhq_protocol::ids::DeviceId;
use maxhq_protocol::state::EntityKey;

use super::testing::public_key_of;
use super::*;
use crate::testing::TempDir;

fn store(temp: &TempDir) -> AppKeys {
    AppKeys::open(temp.path().join("phones"), DeviceId::derive("devbox"))
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn the_decision_table() {
    use AppKeyOrigin::{App, Approved, First, Setup, Signature as Signed};
    use Decision::{Add, Ask, Trusted};
    use Signature::{Failed, Off, Passed};
    let rows = [
        // A key the device trusts stays trusted, whoever asks.
        (true, true, Asker::App, Trusted),
        (true, true, Asker::Connection(Failed), Trusted),
        (true, true, Asker::Setup, Trusted),
        (true, true, Asker::Approved, Trusted),
        // A proven app adds keys: that's how it changes its own.
        (false, true, Asker::App, Add(App)),
        // The signed app on a Mac with a signed maxd, with or without a key.
        (false, false, Asker::Connection(Passed), Add(Signed)),
        (false, true, Asker::Connection(Passed), Add(Signed)),
        // Anything else on that Mac asks, even while it trusts no key.
        (false, false, Asker::Connection(Failed), Ask),
        (false, true, Asker::Connection(Failed), Ask),
        // Without the check, the first key offered counts, and only it.
        (false, false, Asker::Connection(Off), Add(First)),
        (false, true, Asker::Connection(Off), Ask),
        // Setup's key counts only while the device trusts none.
        (false, false, Asker::Setup, Add(Setup)),
        (false, true, Asker::Setup, Ask),
        // You allowed it on the card that asked.
        (false, false, Asker::Approved, Add(Approved)),
        (false, true, Asker::Approved, Add(Approved)),
    ];
    for (trusted, trusts_any, asker, want) in rows {
        assert_eq!(
            decide(trusted, trusts_any, asker),
            want,
            "trusted {trusted}, trusts any {trusts_any}, asked by {asker:?}"
        );
    }
}

#[test]
fn the_file_is_private_and_holds_public_keys_with_how_they_came() {
    let temp = TempDir::new();
    let keys = store(&temp);
    let first = public_key_of(&[1; 32]);
    assert_eq!(
        keys.enroll(&first, "MaxHQ on studio-mac", Asker::Setup)
            .unwrap(),
        Decision::Add(AppKeyOrigin::Setup)
    );
    let folder = temp.path().join("phones");
    assert_eq!(mode(&folder), 0o700);
    assert_eq!(mode(&folder.join(FILE)), 0o600);
    let text = fs::read_to_string(folder.join(FILE)).unwrap();
    assert!(text.contains(&first));
    assert!(text.contains("\"setup\""));
    // Nothing but the file and its lock, so no half-written copy stays.
    let mut names: Vec<String> = fs::read_dir(&folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, [FILE, LOCK]);

    let [key] = keys.keys().unwrap().try_into().unwrap();
    assert_eq!(key.public_key, first);
    assert_eq!(key.label, "MaxHQ on studio-mac");
    assert_eq!(key.device, DeviceId::derive("devbox"));
    assert_eq!(key.id, AppKey::id_for(DeviceId::derive("devbox"), &first));
    assert_eq!(
        key.proved_at, None,
        "setup hands a key over without a proof"
    );
}

#[test]
fn a_proof_with_a_trusted_key_is_dated() {
    let temp = TempDir::new();
    let keys = store(&temp);
    let first = public_key_of(&[1; 32]);
    assert!(!keys.proved(&first).unwrap());
    assert!(!temp.path().join("phones").join(FILE).exists());
    keys.enroll(&first, "", Asker::Setup).unwrap();
    assert!(keys.proved(&first).unwrap());
    let [key] = keys.keys().unwrap().try_into().unwrap();
    assert!(key.proved_at.is_some());
    assert_eq!(key.label, "MaxHQ", "a key without a label gets one");
}

#[test]
fn only_the_first_key_counts_without_a_signature() {
    let temp = TempDir::new();
    let keys = store(&temp);
    let first = public_key_of(&[1; 32]);
    let second = public_key_of(&[2; 32]);
    let off = Asker::Connection(Signature::Off);
    assert_eq!(
        keys.enroll(&first, "MaxHQ", off).unwrap(),
        Decision::Add(AppKeyOrigin::First)
    );
    assert_eq!(
        keys.enroll(&first, "MaxHQ", off).unwrap(),
        Decision::Trusted
    );
    assert_eq!(keys.enroll(&second, "MaxHQ", off).unwrap(), Decision::Ask);
    assert_eq!(
        keys.enroll(&second, "MaxHQ", Asker::Setup).unwrap(),
        Decision::Ask
    );
    assert_eq!(
        keys.enroll(&second, "MaxHQ", Asker::App).unwrap(),
        Decision::Add(AppKeyOrigin::App)
    );
    let origins: Vec<_> = keys.keys().unwrap().iter().map(|k| k.origin).collect();
    assert_eq!(origins, [AppKeyOrigin::First, AppKeyOrigin::App]);
    assert!(keys.keys().unwrap().iter().all(|k| k.proved_at.is_some()));
}

#[test]
fn many_askers_at_once_get_one_first_key() {
    let temp = TempDir::new();
    let keys = store(&temp);
    let decisions: Vec<Decision> = std::thread::scope(|scope| {
        let asks: Vec<_> = (1..=8u8)
            .map(|n| {
                // Each its own handle, as `maxd authorize-key` and `maxd
                // run` are.
                let keys = store(&temp);
                scope.spawn(move || {
                    keys.enroll(
                        &public_key_of(&[n; 32]),
                        "MaxHQ",
                        Asker::Connection(Signature::Off),
                    )
                    .unwrap()
                })
            })
            .collect();
        asks.into_iter().map(|ask| ask.join().unwrap()).collect()
    });
    let firsts = decisions
        .iter()
        .filter(|d| **d == Decision::Add(AppKeyOrigin::First))
        .count();
    assert_eq!(firsts, 1, "{decisions:?}");
    assert_eq!(keys.keys().unwrap().len(), 1);
}

#[test]
fn removing_a_key_tells_its_connections_and_keeps_the_last() {
    let temp = TempDir::new();
    let keys = store(&temp);
    let first = public_key_of(&[1; 32]);
    let second = public_key_of(&[2; 32]);
    keys.enroll(&first, "MaxHQ", Asker::Setup).unwrap();
    keys.enroll(&second, "MaxHQ", Asker::App).unwrap();
    let mut removed = keys.removed();

    assert_eq!(
        keys.remove(&public_key_of(&[3; 32])).unwrap(),
        Removal::Absent
    );
    assert_eq!(keys.remove("not a key").unwrap(), Removal::Absent);
    assert_eq!(keys.remove(&first).unwrap(), Removal::Removed);
    assert_eq!(removed.try_recv().unwrap(), first);
    assert_eq!(keys.remove(&second).unwrap(), Removal::Last);
    assert!(removed.try_recv().is_err());
    let left: Vec<_> = keys
        .keys()
        .unwrap()
        .into_iter()
        .map(|k| k.public_key)
        .collect();
    assert_eq!(left, [second]);
}

#[test]
fn a_file_that_doesnt_parse_trusts_nothing_and_takes_nothing() {
    let temp = TempDir::new();
    let keys = store(&temp);
    let folder = temp.path().join("phones");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join(FILE), "not json").unwrap();
    let first = public_key_of(&[1; 32]);
    assert!(keys.proved(&first).is_err());
    // Read as empty, it would take the next key anyone offers.
    assert!(keys
        .enroll(&first, "MaxHQ", Asker::Connection(Signature::Off))
        .is_err());
    assert_eq!(fs::read_to_string(folder.join(FILE)).unwrap(), "not json");
}

#[test]
fn labels_are_one_short_line() {
    assert_eq!(
        clean_label("  MaxHQ on\nstudio-mac\t"),
        "MaxHQ on studio-mac"
    );
    assert_eq!(clean_label("\u{7}\n "), "MaxHQ");
    assert_eq!(clean_label(&"x".repeat(500)).chars().count(), LABEL_MAX);
}

#[test]
fn keys_have_one_form() {
    let key = public_key_of(&[1; 32]);
    assert_eq!(canonical(&key), Some(key.clone()));
    assert_eq!(canonical(&format!("{key}=")), None);
    assert_eq!(canonical("c2hvcnQ"), None);
    assert_eq!(canonical("not base64!"), None);
}

#[test]
fn the_hub_shows_the_keys_as_each_read_or_change_finds_them() {
    let temp = TempDir::new();
    let first = public_key_of(&[1; 32]);
    let second = public_key_of(&[2; 32]);
    // `maxd authorize-key` trusts a key before `maxd run` starts.
    store(&temp).enroll(&first, "MaxHQ", Asker::Setup).unwrap();
    let hub = crate::secrets::testing::hub_for("devbox");
    let keys = store(&temp);
    let shown = |public_key: &str| {
        let id = AppKey::id_for(DeviceId::derive("devbox"), public_key);
        match hub.get(&EntityKey::AppKey(id)) {
            Some(Entity::AppKey(key)) => Some(key),
            Some(other) => panic!("expected an app key, got {other:?}"),
            None => None,
        }
    };
    assert!(shown(&first).is_none(), "nothing shows until it's asked to");
    keys.publish_to(hub.clone());
    assert_eq!(shown(&first).unwrap().proved_at, None);

    assert!(keys.proved(&first).unwrap());
    assert!(shown(&first).unwrap().proved_at.is_some());
    keys.enroll(&second, "MaxHQ on studio-mac", Asker::App)
        .unwrap();
    assert_eq!(shown(&second).unwrap().label, "MaxHQ on studio-mac");
    assert_eq!(keys.remove(&first).unwrap(), Removal::Removed);
    assert!(shown(&first).is_none());

    // Another process changes the file, and the next read shows it.
    store(&temp).enroll(&first, "MaxHQ", Asker::App).unwrap();
    assert!(shown(&first).is_none());
    assert!(!keys.proved(&public_key_of(&[3; 32])).unwrap());
    assert!(shown(&first).is_some());
    assert!(shown(&second).is_some());
}
