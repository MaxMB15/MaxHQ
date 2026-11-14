// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::time::Instant;

use futures::FutureExt;
use maxhq_client::LinkSpec;
use maxhq_core::notes::heads as heads_of;
use maxhq_core::notes::memory::{MemoryNotes, Save};
use maxhq_protocol::ids::LinkId;
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::notes::{NoteSummary, ProjectNotes};
use maxhq_protocol::time::Timestamp;
use tokio::sync::mpsc::Sender;
use tokio::sync::Semaphore;

use super::*;
use crate::devices::tests::{harness, on, Call, Harness, Record};
use crate::merge::tests::{connected, link_state, workspace};
use crate::registry::Registry;

/// The notes a fake link's device holds.
#[derive(Clone)]
pub(crate) struct FakeNotes {
    device: DeviceId,
    store: Arc<MemoryNotes>,
    /// Whether the device reports its notes after a put changes them, as
    /// `maxd` does.
    reports: bool,
}

/// A note sync request a fake link answers from its device's notes.
pub(crate) struct Syncing {
    message: ClientMessage,
    notes: Option<FakeNotes>,
    gate: Option<Arc<Semaphore>>,
    events: Option<Sender<LinkEvent>>,
}

/// The note sync request `action` is, ready to answer on `link`.
pub(crate) fn syncing(record: &Record, link: LinkId, action: &ClientMessage) -> Option<Syncing> {
    matches!(
        action,
        ClientMessage::NoteHashes { .. }
            | ClientMessage::NoteVersions { .. }
            | ClientMessage::PutNoteVersions { .. }
    )
    .then(|| Syncing {
        message: action.clone(),
        notes: record.notes.get(&link).cloned(),
        gate: record.notes_gate.clone(),
        events: record.events.get(&link).cloned(),
    })
}

impl Syncing {
    /// Answers the way `maxd` does, once the gate lets it.
    pub(crate) async fn answer(self) -> ActionOutcome {
        if let Some(gate) = self.gate {
            gate.acquire().await.unwrap().forget();
        }
        let Some(notes) = self.notes else {
            return ActionOutcome::Refused {
                reason: "This device keeps no notes.".to_owned(),
            };
        };
        let refused = |reason| ActionOutcome::Refused { reason };
        match self.message {
            ClientMessage::NoteHashes { project, .. } => ActionOutcome::NoteHashes {
                hashes: notes.store.hashes_now(project),
            },
            ClientMessage::NoteVersions {
                project, hashes, ..
            } => match notes.store.versions(project, hashes).await {
                Ok(versions) => ActionOutcome::NoteVersions { versions },
                Err(reason) => refused(reason),
            },
            ClientMessage::PutNoteVersions {
                project, versions, ..
            } => match notes.store.put(project, versions).await {
                Ok(stored) => {
                    if stored > 0
                        && notes.reports
                        && let Some(events) = self.events
                    {
                        let change = Change::Upsert {
                            value: reported(&notes, project),
                        };
                        let _ = events.send(LinkEvent::Changed(vec![change])).await;
                    }
                    ActionOutcome::NotesPut { stored }
                }
                Err(reason) => refused(reason),
            },
            _ => unreachable!("only note sync requests come here"),
        }
    }
}

/// The notes entity a device reports for a project, from what it holds.
fn reported(notes: &FakeNotes, project: ProjectId) -> Entity {
    let all = notes
        .store
        .versions(project, notes.store.hashes_now(project))
        .now_or_never()
        .unwrap()
        .unwrap();
    let mut by_note: BTreeMap<NoteId, Vec<NoteVersion>> = BTreeMap::new();
    for version in all {
        by_note.entry(version.note).or_default().push(version);
    }
    let summaries = by_note
        .values()
        .map(|versions| {
            let tops = heads_of(versions);
            assert_eq!(tops.len(), 1, "a store keeps one head per note");
            let head = tops[0];
            NoteSummary {
                note: head.note,
                title: head.title.clone(),
                head: head.hash.clone(),
                at: head.at,
                by: None,
                created_on: notes.device,
                deleted: head.deleted,
                size: u32::try_from(head.body.len()).unwrap(),
            }
        })
        .collect();
    Entity::ProjectNotes(ProjectNotes {
        device: notes.device,
        project,
        notes: summaries,
    })
}

fn project() -> ProjectId {
    ProjectId::derive("p")
}

fn note() -> NoteId {
    NoteId::derive("plan")
}

/// A device in a test: its link and its notes.
struct Device {
    link: LinkSpec,
    id: DeviceId,
    store: Arc<MemoryNotes>,
}

/// This machine as "mac", and a device for each of `others`, named with
/// the protocol version its `maxd` speaks. Each has its notes in memory,
/// reported after each put that changes them when `reports` says so.
/// Every device has a workspace in the project but the Mac, whose `maxd`
/// keeps notes for any project.
fn mac_and(others: &[(&str, u32)], reports: bool) -> (Harness, Device, Vec<Device>) {
    let registry = Registry::in_memory().unwrap();
    let specs: Vec<LinkSpec> = others
        .iter()
        .map(|(name, _)| registry.add(&format!("{name}.example.com")).unwrap())
        .collect();
    let h = harness(registry, None);
    let device = |link: LinkSpec, name: &str, protocol: u32| {
        let id = DeviceId::derive(name);
        let store = Arc::new(MemoryNotes::default());
        h.fake.record().notes.insert(
            link.id,
            FakeNotes {
                device: id,
                store: store.clone(),
                reports,
            },
        );
        let LinkEvent::Link(mut line) = link_state(&link, connected()) else {
            unreachable!("link_state reports a link");
        };
        line.device = Some(id);
        line.protocol = Some(protocol);
        h.fake.record().up.insert(link.id);
        h.report(link.id, LinkEvent::Link(line));
        Device { link, id, store }
    };
    let mac = device(h.local(), "mac", 27);
    let devices: Vec<Device> = specs
        .into_iter()
        .zip(others)
        .map(|(link, &(name, protocol))| {
            let device = device(link, name, protocol);
            let space = format!("w-{name}");
            h.report(
                device.link.id,
                LinkEvent::Synced(vec![on(name, workspace(&space, "p"))]),
            );
            device
        })
        .collect();
    (h, mac, devices)
}

/// Saves the note on a device, after `base`.
fn save(device: &Device, base: Option<&NoteVersion>, body: &str, at: u64) -> NoteVersion {
    device
        .store
        .save(Save {
            project: project(),
            note: note(),
            base: base.map(|base| base.hash.clone()),
            title: "Plan".into(),
            body: body.into(),
            deleted: false,
            at: Timestamp(at),
            by: None,
        })
        .unwrap()
        .0
}

/// Has a device report the notes it holds.
fn tell(h: &Harness, device: &Device) {
    let notes = h.fake.record().notes[&device.link.id].clone();
    let change = Change::Upsert {
        value: reported(&notes, project()),
    };
    h.report(device.link.id, LinkEvent::Changed(vec![change]));
}

/// Waits until `done` holds.
fn wait_until(what: &str, done: impl Fn() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{what} never happened"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Lets the relay settle, for a test that checks nothing more happens.
fn quiet() {
    std::thread::sleep(SETTLE * 3);
}

/// What the relay did so far.
fn log(h: &Harness) -> Vec<Logged> {
    lock(&h.devices.core.notes.log).clone()
}

/// The devices the relay swapped notes with, in order.
fn swapped(h: &Harness) -> Vec<DeviceId> {
    log(h)
        .into_iter()
        .filter_map(|logged| match logged {
            Logged::Start(of, device, _) if of == project() => Some(device),
            _ => None,
        })
        .collect()
}

/// How many passes the relay ran.
fn passes(h: &Harness) -> usize {
    log(h)
        .iter()
        .filter(|logged| matches!(logged, Logged::Pass(of) if *of == project()))
        .count()
}

/// Whether every device holds the same versions of the note, ending at
/// the same head.
fn in_step(devices: &[&Device]) -> bool {
    let first = devices[0];
    devices.iter().all(|device| {
        device.store.hashes_now(project()) == first.store.hashes_now(project())
            && device.store.head(project(), note()) == first.store.head(project(), note())
    })
}

#[test]
fn notes_since_matches_the_sync_requests() {
    let key = IdempotencyKey::generate;
    for message in [
        ClientMessage::NoteHashes {
            key: key(),
            project: project(),
        },
        ClientMessage::NoteVersions {
            key: key(),
            project: project(),
            hashes: Vec::new(),
        },
        ClientMessage::PutNoteVersions {
            key: key(),
            project: project(),
            versions: Vec::new(),
        },
    ] {
        assert_eq!(message.since(), NOTES_SINCE);
    }
}

#[test]
fn edits_on_each_device_reach_every_other() {
    let (h, mac, devices) = mac_and(&[("nl", 27), ("ws-alpha", 27)], true);
    let [nl, alpha] = &devices[..] else {
        unreachable!("two devices");
    };
    // A note made on one device reaches the Mac, and then the other device,
    // which has a workspace in the project but no notes yet.
    let first = save(nl, None, "one\ntwo\nthree\n", 1);
    tell(&h, nl);
    wait_until("the note reaching every device", || {
        in_step(&[&mac, nl, alpha]) && alpha.store.head(project(), note()).is_some()
    });
    assert_eq!(alpha.store.head(project(), note()).unwrap(), first);

    // Edits on two devices at once merge, and the merge reaches all three.
    save(&mac, Some(&first), "ONE\ntwo\nthree\n", 2);
    save(alpha, Some(&first), "one\ntwo\nTHREE\n", 3);
    tell(&h, &mac);
    tell(&h, alpha);
    wait_until("the edits merging everywhere", || {
        in_step(&[&mac, nl, alpha])
            && nl.store.head(project(), note()).unwrap().body == "ONE\ntwo\nTHREE\n"
    });
}

#[test]
fn a_device_too_old_for_notes_is_left_alone() {
    let (h, mac, devices) = mac_and(&[("nl", 26), ("ws-alpha", 27)], true);
    let [nl, alpha] = &devices[..] else {
        unreachable!("two devices");
    };
    save(nl, None, "old\n", 1);
    tell(&h, nl);
    let made = save(alpha, None, "new\n", 2);
    tell(&h, alpha);
    wait_until("the newer device's note reaching the Mac", || {
        mac.store.head(project(), note()).as_ref() == Some(&made)
    });
    quiet();
    assert!(swapped(&h).iter().all(|device| *device == alpha.id));
    assert!(
        !h.fake.calls().iter().any(|(link, call)| *link == nl.link.id
            && matches!(call, Call::Send(message) if message.since() == NOTES_SINCE))
    );
    assert_eq!(nl.store.hashes_now(project()).len(), 1);
}

#[test]
fn changes_in_quick_succession_bring_one_exchange_once_they_settle() {
    let (h, mac, devices) = mac_and(&[("nl", 27)], false);
    let nl = &devices[0];
    let mut last = save(nl, None, "0\n", 0);
    tell(&h, nl);
    let mut told = Instant::now();
    for at in 1..4 {
        last = save(nl, Some(&last), &format!("{at}\n"), at);
        told = Instant::now();
        tell(&h, nl);
    }
    wait_until("the exchange", || {
        mac.store.head(project(), note()).as_ref() == Some(&last)
    });
    quiet();
    let log = log(&h);
    let starts: Vec<Instant> = log
        .iter()
        .filter_map(|logged| match logged {
            Logged::Start(_, _, at) => Some(*at),
            _ => None,
        })
        .collect();
    assert_eq!(starts.len(), 1, "{log:?}");
    assert!(starts[0] >= told + SETTLE);
    assert_eq!(passes(&h), 1);
}

#[test]
fn changes_during_an_exchange_bring_one_more_after_it() {
    let (h, mac, devices) = mac_and(&[("nl", 27)], false);
    let nl = &devices[0];
    let gate = Arc::new(Semaphore::new(0));
    h.fake.record().notes_gate = Some(gate.clone());
    let first = save(nl, None, "one\n", 1);
    tell(&h, nl);
    wait_until("the first exchange", || !swapped(&h).is_empty());

    // While it waits on the devices, more changes come in.
    let mut last = first;
    for at in 2..5 {
        last = save(nl, Some(&last), &format!("{at}\n"), at);
        tell(&h, nl);
    }
    quiet();
    assert_eq!(swapped(&h).len(), 1, "one exchange at a time");
    assert_eq!(passes(&h), 1);

    gate.add_permits(10_000);
    wait_until("the second exchange", || swapped(&h).len() == 2);
    wait_until("the second exchange ending", || {
        log(&h)
            .iter()
            .filter(|logged| matches!(logged, Logged::End(of, device) if *of == project() && *device == nl.id))
            .count()
            == 2
    });
    quiet();
    assert_eq!(passes(&h), 2);
    let order: Vec<bool> = log(&h)
        .iter()
        .filter_map(|logged| match logged {
            Logged::Start(..) => Some(true),
            Logged::End(..) => Some(false),
            Logged::Pass(..) => None,
        })
        .collect();
    assert_eq!(order, [true, false, true, false]);
    assert_eq!(mac.store.head(project(), note()).unwrap(), last);
}
