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

/// A folder of its own under the system's temporary folder, removed after.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "maxhq-app-key-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn file(&self) -> std::path::PathBuf {
        self.0.join(store::FILE)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Keys opened as a computer whose keyring didn't answer, with the key
/// file to offer in `scratch`.
fn unkept(scratch: &Scratch, record: Option<Record>) -> AppKeys {
    let place = Place {
        store: Box::new(store::Nowhere),
        home: KeyHome::Memory,
        ask: true,
        file: Some(scratch.file()),
        read: None,
    };
    AppKeys::open_in(place, record, LABEL.into()).unwrap()
}

#[test]
fn the_key_the_keyring_gave_when_asked_is_the_one_that_opens() {
    let store = Shared::default();
    store.save(Key::App, &[7; 32]).unwrap();
    let read = store.load(Key::App);
    let place = Place {
        // A second read would fail, so the keys open only from the first.
        store: Box::new(Broken {
            read: true,
            write: false,
        }),
        home: KeyHome::Keyring,
        ask: false,
        file: None,
        read: Some(read),
    };
    let keys = AppKeys::open_in(place, None, LABEL.into()).unwrap();
    assert_eq!(keys.view().remade, None);
    assert_eq!(keys.public_key(), public_key(&[7; 32]));
}

#[test]
fn the_key_file_once_there_or_chosen_wins_then_the_keyring_then_memory() {
    let unasked = || -> bool { panic!("the keyring is asked only when it matters") };
    assert_eq!(store::start(None, true, unasked), (KeyHome::File, false));
    assert_eq!(
        store::start(Some(KeyHome::File), false, unasked),
        (KeyHome::File, false),
        "a chosen file that went missing is made again"
    );
    assert_eq!(
        store::start(Some(KeyHome::Memory), true, unasked),
        (KeyHome::File, false)
    );
    assert_eq!(
        store::start(None, false, || true),
        (KeyHome::Keyring, false)
    );
    assert_eq!(
        store::start(Some(KeyHome::Memory), false, || true),
        (KeyHome::Keyring, false),
        "Not now was about the file, so a keyring that answers takes the key"
    );
}

#[test]
fn with_no_keyring_the_app_asks_unless_you_said_not_now() {
    assert_eq!(store::start(None, false, || false), (KeyHome::Memory, true));
    assert_eq!(
        store::start(Some(KeyHome::Keyring), false, || false),
        (KeyHome::Memory, true),
        "a keyring you moved back to that stopped answering asks again"
    );
    assert_eq!(
        store::start(Some(KeyHome::Memory), false, || false),
        (KeyHome::Memory, false)
    );
}

// Windows has no mode bits, so the file's privacy is checked on Unix only.
#[cfg(unix)]
#[test]
fn the_key_file_is_private_and_comes_back_whole() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new();
    let file = store::KeyFile::new(scratch.file());
    assert_eq!(file.load(Key::App).unwrap(), None);
    file.save(Key::App, &[7; 64]).unwrap();
    let mode = std::fs::metadata(scratch.file())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    assert_eq!(file.load(Key::App).unwrap().unwrap().as_slice(), &[7; 64]);
    assert!(
        file.save(Key::Ssh, &[1; 32]).is_err(),
        "only the app key goes here"
    );
    file.forget(Key::App).unwrap();
    assert!(!scratch.file().exists());
    file.forget(Key::App).unwrap();
}

#[test]
fn a_key_with_nowhere_to_go_asks_and_lasts_until_quit() {
    let scratch = Scratch::new();
    let keys = unkept(&scratch, None);
    assert!(!keys.view().kept);
    assert_eq!(
        keys.home(),
        Some(KeyHomeState {
            home: KeyHome::Memory,
            ask: true,
            file: Some(scratch.file().display().to_string()),
        })
    );
    assert!(
        keys.rotate().is_err(),
        "a new key with nowhere to go would be lost"
    );
}

#[test]
fn choosing_the_file_moves_the_key_there_and_it_opens_from_there_after() {
    let scratch = Scratch::new();
    let keys = unkept(&scratch, None);
    let public = keys.public_key();
    keys.keep_in(KeyHome::File).unwrap();
    let home = keys.home().unwrap();
    assert_eq!((home.home, home.ask), (KeyHome::File, false));
    assert!(keys.view().kept);

    let file = store::KeyFile::new(scratch.file());
    assert_eq!(
        public_key(&seed_of(&file.load(Key::App).unwrap().unwrap())),
        public
    );
    let again = AppKeys::open_in(
        Place::only(Box::new(file), KeyHome::File),
        None,
        LABEL.into(),
    )
    .unwrap();
    assert_eq!(again.public_key(), public);
    assert_eq!(again.view().remade, None);
}

#[test]
fn not_now_stops_the_asking_and_moves_nothing() {
    let scratch = Scratch::new();
    let keys = unkept(&scratch, None);
    keys.keep_in(KeyHome::Memory).unwrap();
    let home = keys.home().unwrap();
    assert_eq!((home.home, home.ask), (KeyHome::Memory, false));
    assert!(!scratch.file().exists());

    keys.keep_in(KeyHome::File).unwrap();
    assert!(
        keys.keep_in(KeyHome::Memory).is_err(),
        "a kept key doesn't go back to memory"
    );
}

#[test]
fn moving_back_takes_both_keys_of_a_rotation_and_empties_the_file() {
    let scratch = Scratch::new();
    let keys = unkept(&scratch, None);
    keys.keep_in(KeyHome::File).unwrap();
    keys.keep(Box::new(Kept::knowing(&[devbox()])));
    keys.rotate().unwrap();

    let ring = Shared::default();
    keys.move_to(Box::new(ring.clone()), KeyHome::Keyring)
        .unwrap();
    assert_eq!(
        ring.item().len(),
        64,
        "the new key and the one devices move off"
    );
    assert_eq!(public_key(&seed_of(&ring.item())), keys.public_key());
    assert!(!scratch.file().exists());
    assert_eq!(keys.home().unwrap().home, KeyHome::Keyring);
}

#[test]
fn a_new_key_names_where_the_last_one_went() {
    let store = Shared::default();
    let (first, kept) = kept(&store, &[devbox()]);
    store.forget(Key::App).unwrap();
    let keys = open(store.clone(), Some(kept.record()));
    assert_eq!(
        keys.view().remade.as_deref(),
        Some("MaxHQ made a new app key, since the Keychain lost the one before"),
        "the Mac's reasons read as they did"
    );
    assert_eq!(keys.home(), None, "a Mac has one place for its key");

    let scratch = Scratch::new();
    let keys = unkept(&scratch, Some(kept.record()));
    assert_eq!(
        keys.view().remade.as_deref(),
        Some("MaxHQ made a new app key, since no keyring answered to keep the one before")
    );
    assert_eq!(keys.view().previous, Some(first.public_key()));
}
