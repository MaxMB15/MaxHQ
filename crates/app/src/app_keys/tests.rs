// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The app key's store, its proofs and its rotation, against a store in
//! memory and a device that answers from a script. The proofs over a real
//! connection are in the client's tests, and against a real `maxd` in
//! maxd's.

use std::collections::VecDeque;
use std::sync::Arc;

use super::*;
use crate::keys::MemoryStore;

const LABEL: &str = "MaxHQ on studio-mac";

fn devbox() -> DeviceId {
    DeviceId::derive("devbox")
}

fn mac() -> DeviceId {
    DeviceId::derive("studio-mac")
}

/// A memory store the test can still read after the keys take it.
#[derive(Clone, Default)]
struct Shared(Arc<MemoryStore>);

impl KeyStore for Shared {
    fn load(&self, key: Key) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        self.0.load(key)
    }

    fn save(&self, key: Key, bytes: &[u8]) -> Result<(), String> {
        self.0.save(key, bytes)
    }

    fn forget(&self, key: Key) -> Result<(), String> {
        self.0.forget(key)
    }
}

impl Shared {
    fn item(&self) -> Vec<u8> {
        self.0.load(Key::App).unwrap().unwrap().to_vec()
    }
}

/// A store whose reads or writes fail, as a locked or tampered Keychain's
/// would.
struct Broken {
    read: bool,
    write: bool,
}

impl KeyStore for Broken {
    fn load(&self, _key: Key) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        if self.read {
            Err("the Keychain said no".into())
        } else {
            Ok(None)
        }
    }

    fn save(&self, _key: Key, _bytes: &[u8]) -> Result<(), String> {
        if self.write {
            Err("the Keychain said no".into())
        } else {
            Ok(())
        }
    }

    fn forget(&self, _key: Key) -> Result<(), String> {
        Ok(())
    }
}

/// Keeps the last record saved, and knows a fixed list of devices.
#[derive(Clone, Default)]
struct Kept {
    record: Arc<Mutex<Option<Record>>>,
    devices: Arc<Mutex<Vec<DeviceId>>>,
}

impl Keeper for Kept {
    fn save(&self, record: &Record) {
        *self.record.lock().unwrap() = Some(record.clone());
    }

    fn devices(&self) -> Vec<DeviceId> {
        self.devices.lock().unwrap().clone()
    }
}

impl Kept {
    fn knowing(devices: &[DeviceId]) -> Self {
        let kept = Self::default();
        *kept.devices.lock().unwrap() = devices.to_vec();
        kept
    }

    fn record(&self) -> Record {
        self.record.lock().unwrap().clone().unwrap()
    }
}

/// What the keys sent the device.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Sent {
    Prove { public_key: String, then_sync: bool },
    Enroll { label: String },
    Remove { public_key: String },
}

/// A device that answers each message from a script, in order.
struct Device {
    device: DeviceId,
    answers: Mutex<VecDeque<ActionOutcome>>,
    sent: Mutex<Vec<Sent>>,
}

impl Device {
    fn answering(device: DeviceId, answers: &[ActionOutcome]) -> Self {
        Self {
            device,
            answers: Mutex::new(answers.iter().cloned().collect()),
            sent: Mutex::new(Vec::new()),
        }
    }

    fn answer(&self, sent: Sent) -> Result<ActionOutcome, LinkError> {
        self.sent.lock().unwrap().push(sent);
        self.answers
            .lock()
            .unwrap()
            .pop_front()
            .ok_or(LinkError::NotConnected)
    }

    fn sent(&self) -> Vec<Sent> {
        self.sent.lock().unwrap().clone()
    }
}

impl Steps for Device {
    async fn prove(&self, seed: &[u8; 32], then_sync: bool) -> Result<ActionOutcome, LinkError> {
        self.answer(Sent::Prove {
            public_key: public_key(seed),
            then_sync,
        })
    }

    async fn enroll(&self, label: &str) -> Result<ActionOutcome, LinkError> {
        self.answer(Sent::Enroll {
            label: label.to_owned(),
        })
    }

    async fn remove(&self, public_key: &str) -> Result<ActionOutcome, LinkError> {
        self.answer(Sent::Remove {
            public_key: public_key.to_owned(),
        })
    }

    fn device(&self) -> DeviceId {
        self.device
    }

    fn name(&self) -> &str {
        "devbox"
    }
}

fn refused(reason: &str) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.into(),
    }
}

/// How `maxd` turns down a good proof with a key it doesn't trust.
fn unknown() -> ActionOutcome {
    refused("Unknown key: devbox doesn't trust this app key yet.")
}

fn prove(public_key: &str, then_sync: bool) -> Sent {
    Sent::Prove {
        public_key: public_key.into(),
        then_sync,
    }
}

fn enroll() -> Sent {
    Sent::Enroll {
        label: LABEL.into(),
    }
}

fn remove(public_key: &str) -> Sent {
    Sent::Remove {
        public_key: public_key.into(),
    }
}

fn open(store: impl KeyStore + 'static, record: Option<Record>) -> AppKeys {
    AppKeys::open(Box::new(store), record, LABEL.into()).unwrap()
}

/// Keys that know `devices`, with what they save.
fn kept(store: &Shared, devices: &[DeviceId]) -> (AppKeys, Kept) {
    let keys = open(store.clone(), None);
    let kept = Kept::knowing(devices);
    keys.keep(Box::new(kept.clone()));
    (keys, kept)
}

#[test]
fn a_missing_key_is_made_once_and_read_back_after() {
    let store = Shared::default();
    let keys = open(store.clone(), None);
    let item = store.item();
    assert_eq!(item.len(), 32);
    assert_eq!(keys.public_key(), public_key(&seed_of(&item)));
    let view = keys.view();
    assert_eq!(view.remade, None, "a first key needs no notice");
    assert!(view.kept);
    assert_eq!(view.previous, None);

    let again = open(store.clone(), None);
    assert_eq!(again.public_key(), keys.public_key());
    assert_eq!(store.item(), item);
}

#[test]
fn a_key_that_isnt_one_seed_or_two_counts_as_tampered_and_retires_the_last() {
    let store = Shared::default();
    let (first, kept) = kept(&store, &[devbox()]);
    let old = first.public_key();
    store.save(Key::App, &[1, 2, 3]).unwrap();

    let keys = open(store.clone(), Some(kept.record()));
    assert_ne!(keys.public_key(), old);
    assert_eq!(store.item().len(), 32);
    keys.keep(Box::new(kept.clone()));
    let view = keys.view();
    assert!(view.remade.is_some());
    assert_eq!(view.previous.as_deref(), Some(old.as_str()));
    assert_eq!(view.waiting, vec![devbox()]);
    assert!(keys.is_own(&old), "the old key comes out, not in");
}

#[test]
fn an_unreadable_key_counts_as_tampered_and_one_that_cant_be_kept_lasts_until_quit() {
    let keys = open(
        Broken {
            read: true,
            write: false,
        },
        None,
    );
    let view = keys.view();
    assert!(view.remade.is_some());
    assert!(view.kept);

    let keys = open(
        Broken {
            read: false,
            write: true,
        },
        None,
    );
    let view = keys.view();
    assert_eq!(view.remade, None);
    assert!(!view.kept);
}

#[test]
fn a_lost_key_retires_the_one_the_record_names() {
    let store = Shared::default();
    let (first, kept) = kept(&store, &[devbox()]);
    store.forget(Key::App).unwrap();

    let keys = open(store.clone(), Some(kept.record()));
    assert!(keys.view().remade.is_some());
    assert_eq!(keys.view().previous, Some(first.public_key()));
}

#[tokio::test]
async fn a_trusted_key_proves_once_and_asks_for_the_sync_behind_it_after() {
    let store = Shared::default();
    let (keys, kept) = kept(&store, &[devbox()]);
    let public = keys.public_key();

    let device = Device::answering(devbox(), &[ActionOutcome::Done]);
    assert_eq!(keys.run(&device).await, Ok(Verdict::Proven));
    // The first time, the device may not trust it, so the Sync waits.
    assert_eq!(device.sent(), vec![prove(&public, false)]);
    assert!(kept.record().trusted.contains(&devbox()));

    let device = Device::answering(devbox(), &[ActionOutcome::Done]);
    assert_eq!(keys.run(&device).await, Ok(Verdict::Proven));
    assert_eq!(device.sent(), vec![prove(&public, true)]);
}

#[tokio::test]
async fn an_unknown_key_enrolls_under_the_macs_name() {
    let store = Shared::default();
    let (keys, _) = kept(&store, &[devbox()]);
    let public = keys.public_key();
    let device = Device::answering(devbox(), &[unknown(), ActionOutcome::Done]);
    assert_eq!(keys.run(&device).await, Ok(Verdict::Proven));
    assert_eq!(device.sent(), vec![prove(&public, false), enroll()]);
    assert!(keys.view().refusals.is_empty());
}

#[tokio::test]
async fn a_refusal_says_whether_the_device_trusted_the_key_before() {
    let store = Shared::default();
    let (keys, _) = kept(&store, &[devbox()]);
    let device = Device::answering(devbox(), &[unknown(), refused("you said no")]);
    assert_eq!(
        keys.run(&device).await,
        Ok(Verdict::Refused(
            "devbox doesn't trust this Mac's app key: you said no".into()
        ))
    );
    let view = keys.view();
    assert_eq!(
        view.refusals,
        vec![KeyRefusal {
            device: devbox(),
            reason: "you said no".into(),
            trusted_before: false,
            kept_previous: false,
        }]
    );

    let device = Device::answering(devbox(), &[ActionOutcome::Done]);
    assert_eq!(keys.run(&device).await, Ok(Verdict::Proven));
    assert!(keys.view().refusals.is_empty(), "trusted again");

    let device = Device::answering(devbox(), &[unknown(), refused("removed")]);
    assert_eq!(
        keys.run(&device).await,
        Ok(Verdict::Refused(
            "devbox no longer trusts this Mac's app key: removed".into()
        ))
    );
    assert!(keys.view().refusals[0].trusted_before);
}

#[tokio::test]
async fn only_an_unknown_key_asks_to_go_in_and_maxds_prefix_stays_out_of_the_reason() {
    let store = Shared::default();
    let (keys, _) = kept(&store, &[devbox()]);
    let public = keys.public_key();
    let answers = [
        (
            "Bad proof: the signature doesn't match.",
            "the signature doesn't match.",
        ),
        (
            "maxd couldn't read its app keys. Its log says why.",
            "maxd couldn't read its app keys. Its log says why.",
        ),
    ];
    for (answer, shown) in answers {
        let device = Device::answering(devbox(), &[refused(answer)]);
        assert_eq!(
            keys.run(&device).await,
            Ok(Verdict::Refused(format!(
                "devbox doesn't trust this Mac's app key: {shown}"
            )))
        );
        assert_eq!(device.sent(), vec![prove(&public, false)], "no enroll");
        assert_eq!(keys.view().refusals[0].reason, shown);
    }
}

#[tokio::test]
async fn a_connection_that_drops_mid_proof_is_lost() {
    let store = Shared::default();
    let (keys, _) = kept(&store, &[devbox()]);
    let device = Device::answering(devbox(), &[unknown()]);
    assert_eq!(keys.run(&device).await, Err(LinkError::NotConnected));
}

#[tokio::test]
async fn a_new_key_moves_in_on_each_device_and_the_old_one_goes_once_all_have() {
    let store = Shared::default();
    let (keys, kept) = kept(&store, &[devbox(), mac()]);
    let old = keys.public_key();
    for device in [devbox(), mac()] {
        let trusted = Device::answering(device, &[ActionOutcome::Done]);
        keys.run(&trusted).await.unwrap();
    }

    keys.rotate().unwrap();
    let new = keys.public_key();
    assert_ne!(new, old);
    assert_eq!(store.item().len(), 64, "both seeds wait in the item");
    assert_eq!(keys.view().waiting, vec![devbox(), mac()]);
    assert!(keys.rotate().is_err(), "one rotation at a time");

    // The old key proves the connection, the new one goes in over it
    // with no card, and the old one comes out.
    let device = Device::answering(
        devbox(),
        &[
            ActionOutcome::Done,
            unknown(),
            ActionOutcome::Done,
            ActionOutcome::Done,
            ActionOutcome::Done,
        ],
    );
    assert_eq!(keys.run(&device).await, Ok(Verdict::Proven));
    assert_eq!(
        device.sent(),
        vec![
            prove(&old, true),
            prove(&new, true),
            enroll(),
            prove(&new, false),
            remove(&old),
        ]
    );
    assert_eq!(keys.view().waiting, vec![mac()]);
    assert!(kept.record().moved.contains(&devbox()));

    // A moved device proves only the new key.
    let again = Device::answering(devbox(), &[ActionOutcome::Done]);
    assert_eq!(keys.run(&again).await, Ok(Verdict::Proven));
    assert_eq!(again.sent(), vec![prove(&new, true)]);

    // A relaunch picks the rotation up from the item and the record.
    let relaunched = open(store.clone(), Some(kept.record()));
    relaunched.keep(Box::new(kept.clone()));
    assert_eq!(relaunched.public_key(), new);
    assert_eq!(relaunched.view().waiting, vec![mac()]);

    let last = Device::answering(
        mac(),
        &[
            ActionOutcome::Done,
            ActionOutcome::Done,
            ActionOutcome::Done,
        ],
    );
    assert_eq!(relaunched.run(&last).await, Ok(Verdict::Proven));
    assert_eq!(
        last.sent(),
        vec![prove(&old, true), prove(&new, true), remove(&old)],
        "a device that already took the new key only loses the old"
    );
    let view = relaunched.view();
    assert_eq!(view.previous, None);
    assert!(view.waiting.is_empty());
    assert_eq!(store.item().len(), 32, "the old seed is gone");
    assert_eq!(public_key(&seed_of(&store.item())), new);
    assert!(!relaunched.is_own(&old));
    assert!(relaunched.rotate().is_ok(), "the next rotation may start");
}

#[tokio::test]
async fn a_device_that_turns_the_new_key_down_keeps_the_old_one() {
    let store = Shared::default();
    let (keys, _) = kept(&store, &[devbox()]);
    let device = Device::answering(devbox(), &[ActionOutcome::Done]);
    keys.run(&device).await.unwrap();
    keys.rotate().unwrap();

    let device = Device::answering(devbox(), &[ActionOutcome::Done, unknown(), refused("no")]);
    assert_eq!(keys.run(&device).await, Ok(Verdict::Proven));
    let view = keys.view();
    assert_eq!(view.waiting, vec![devbox()]);
    assert!(view.refusals[0].kept_previous);
}

#[test]
fn a_removed_device_stops_holding_a_rotation_up() {
    let store = Shared::default();
    let (keys, kept) = kept(&store, &[devbox()]);
    keys.rotate().unwrap();
    assert_eq!(keys.view().waiting, vec![devbox()]);
    kept.devices.lock().unwrap().clear();
    assert_eq!(keys.view().previous, None);
    assert_eq!(store.item().len(), 32);
}
