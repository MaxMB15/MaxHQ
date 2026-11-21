// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::path::PathBuf;

use maxhq_client::setup::host_key_alias;
use maxhq_client::LinkEvent;

use super::super::{SetupPhase, SetupProgress, SetupStep};
use super::*;
use crate::devices::tests::{harness, Fake, Harness};
use crate::merge::tests::{connected, link_state};
use crate::registry::Registry;
use crate::tests::{block_on, FakeShell};

#[test]
fn a_fingerprint_reads_as_ssh_keygen_shows_it() {
    // An ed25519 key whose blob is the type name and 32 zero bytes.
    let mut blob = Vec::new();
    blob.extend_from_slice(&11u32.to_be_bytes());
    blob.extend_from_slice(b"ssh-ed25519");
    blob.extend_from_slice(&32u32.to_be_bytes());
    blob.extend_from_slice(&[0; 32]);
    let key = format!("ssh-ed25519 {}", STANDARD.encode(&blob));
    let expected = format!(
        "ED25519 SHA256:{}",
        STANDARD_NO_PAD.encode(Sha256::digest(&blob))
    );
    assert_eq!(fingerprint(&key).as_deref(), Some(expected.as_str()));
    assert_eq!(fingerprint("ssh-ed25519 not*base64"), None);
    assert_eq!(fingerprint("ssh-ed25519"), None);
}

#[test]
fn setup_asks_only_before_adding_a_key_under_a_name_that_has_some() {
    let (old, new) = ("ssh-ed25519 OLD".to_owned(), "ssh-ed25519 NEW".to_owned());
    assert!(
        !adds_keys(&[], std::slice::from_ref(&new)),
        "a new device has none"
    );
    assert!(!adds_keys(
        std::slice::from_ref(&old),
        std::slice::from_ref(&old)
    ));
    assert!(adds_keys(std::slice::from_ref(&old), &[old.clone(), new]));
}

#[test]
fn a_links_addresses_are_its_hosts_in_order() {
    let spec = LinkSpec {
        id: LinkId::generate(),
        name: "devbox".into(),
        route: Route::Ssh {
            host: "devbox".into(),
        },
        fallbacks: vec![Route::Ssh {
            host: "devbox.example.ts.net".into(),
        }],
        last_good: BTreeMap::new(),
    };
    assert_eq!(addresses(&spec), ["devbox", "devbox.example.ts.net"]);
}

/// The app with devbox, which said hello once and so reaches its device,
/// a second link not yet heard from, and MaxHQ's own `known_hosts`
/// keeping `kept` under the device's name.
struct Setup {
    app: App<Fake>,
    fake: Fake,
    devbox: LinkSpec,
    second: LinkSpec,
    device: DeviceId,
    dir: PathBuf,
    known_hosts: PathBuf,
}

impl Setup {
    fn new(kept: &str) -> Self {
        let registry = Registry::in_memory().unwrap();
        let devbox = registry.add("devbox").unwrap();
        let second = registry.add("devbox.example.ts.net").unwrap();
        let device = DeviceId::derive("devbox");
        let Harness {
            devices,
            view,
            fake,
            ..
        } = harness(registry, None);
        let app = App::new(view, devices, FakeShell::default());
        let dir = std::env::temp_dir().join(format!("maxhq-setup-claim-{}", LinkId::generate()));
        std::fs::create_dir_all(&dir).unwrap();
        let known_hosts = dir.join("known_hosts");
        let alias = host_key_alias(&device.to_string());
        std::fs::write(&known_hosts, format!("{alias} {kept}\n")).unwrap();
        let setup = Self {
            app,
            fake,
            devbox,
            second,
            device,
            dir,
            known_hosts,
        };
        block_on(setup.hello(setup.devbox.id));
        let pinned = setup
            .app
            .devices()
            .registry()
            .known_device(setup.devbox.id)
            .unwrap();
        assert_eq!(pinned, Some(device));
        setup
    }

    /// Has `link` say hello as devbox's device, and waits until the view
    /// shows it reaching the device or held.
    async fn hello(&self, link: LinkId) {
        let spec = if link == self.devbox.id {
            &self.devbox
        } else {
            &self.second
        };
        let LinkEvent::Link(mut line) = link_state(spec, connected()) else {
            unreachable!("link_state reports a link");
        };
        line.device = Some(self.device);
        let events = self.fake.record().events[&link].clone();
        self.fake.record().up.insert(link);
        events.send(LinkEvent::Link(line)).await.unwrap();
        until(|| {
            self.app.view.read(|merge| {
                connected_device(merge, link).is_some() || claim(merge, link).is_some()
            })
        })
        .await;
    }

    /// How many times links opened.
    fn opened(&self) -> usize {
        self.fake.record().opened.len()
    }

    /// What the view says `link` waits on.
    fn held(&self, link: LinkId) -> Option<LinkClaim> {
        self.app.view.read(|merge| claim(merge, link))
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn until(check: impl Fn() -> bool) {
    let start = Instant::now();
    while !check() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "it never got there"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// The steps a setup run reported.
#[derive(Default)]
struct Steps(Mutex<Vec<SetupPhase>>);

impl Steps {
    fn report(&self, progress: SetupProgress) {
        if let SetupProgress::Step { phase, .. } = progress {
            self.0.lock().unwrap().push(phase);
        }
    }

    /// Whether the run said it waits for your answer.
    fn waited(&self) -> bool {
        self.0
            .lock()
            .unwrap()
            .contains(&SetupPhase::WaitingForAnswer)
    }
}

fn run(report: &(dyn Fn(SetupProgress) + Sync)) -> Run<'_> {
    Run {
        setup: "setup-0".into(),
        host: "devbox".into(),
        at: Mutex::new(SetupStep::Handshake),
        report,
    }
}

const OLD: &str = "ssh-ed25519 OLD";
const NEW: &str = "ssh-ed25519 NEW";

#[test]
fn setup_asks_before_new_host_keys_join_a_name_that_has_some() {
    let s = Setup::new(OLD);
    let steps = Steps::default();
    let report = |progress: SetupProgress| steps.report(progress);
    let run = run(&report);
    let keys = [NEW.to_owned()];
    // Setup reopens the link to ask before the answer below first runs.
    let opened = s.opened();
    let vouched = block_on(async {
        let vouch = s
            .app
            .vouched_device(&run, &s.known_hosts, s.devbox.id, &keys);
        let answer = async {
            until(|| s.opened() > opened).await;
            s.hello(s.devbox.id).await;
            let held = s.held(s.devbox.id).expect("devbox waits for your answer");
            assert_eq!(
                (held.device, held.state),
                (s.device, LinkClaimState::Asking)
            );
            let pinned = s.app.devices().registry().known_device(s.devbox.id);
            assert_eq!(pinned.unwrap(), None);
            until(|| steps.waited()).await;
            s.app.devices().answer_claim(s.devbox.id, true).unwrap();
            s.hello(s.devbox.id).await;
        };
        tokio::join!(vouch, answer).0
    });
    assert_eq!(vouched, Ok(s.device));
    assert_eq!(s.held(s.devbox.id), None);
}

#[test]
fn setup_stops_when_you_say_the_host_is_another_device() {
    let s = Setup::new(OLD);
    let steps = Steps::default();
    let report = |progress: SetupProgress| steps.report(progress);
    let run = run(&report);
    let keys = [NEW.to_owned()];
    // Setup reopens the link to ask before the answer below first runs.
    let opened = s.opened();
    let vouched = block_on(async {
        let vouch = s
            .app
            .vouched_device(&run, &s.known_hosts, s.devbox.id, &keys);
        let answer = async {
            until(|| s.opened() > opened).await;
            s.hello(s.devbox.id).await;
            until(|| steps.waited()).await;
            s.app.devices().answer_claim(s.devbox.id, false).unwrap();
            s.hello(s.devbox.id).await;
        };
        tokio::join!(vouch, answer).0
    });
    let error = vouched.unwrap_err();
    assert!(error.contains("isn't the device it names"), "{error}");
    assert!(error.contains(WHERE_TO_CHANGE), "{error}");
    let held = s.held(s.devbox.id).expect("devbox stays cut off");
    assert_eq!(held.state, LinkClaimState::Different);
}

#[test]
fn setup_goes_on_without_asking_when_the_keys_are_kept_already() {
    let s = Setup::new(OLD);
    let report = |_: SetupProgress| {};
    let run = run(&report);
    let opened = s.opened();
    let keys = [OLD.to_owned()];
    let vouched = block_on(
        s.app
            .vouched_device(&run, &s.known_hosts, s.devbox.id, &keys),
    );
    assert_eq!(vouched, Ok(s.device));
    assert_eq!(s.opened(), opened, "nothing reopened to ask");
}

#[test]
fn a_second_route_you_said_is_the_device_asks_only_once() {
    let s = Setup::new(OLD);
    let steps = Steps::default();
    let report = |progress: SetupProgress| steps.report(progress);
    let run = run(&report);
    let keys = [NEW.to_owned()];
    let opened = s.opened();
    let vouched = block_on(async {
        let vouch = s
            .app
            .vouched_device(&run, &s.known_hosts, s.second.id, &keys);
        let answer = async {
            s.hello(s.second.id).await;
            let held = s.held(s.second.id).expect("the second link waits");
            assert_eq!(held.known_by, Some(s.devbox.id));
            until(|| steps.waited()).await;
            s.app.devices().answer_claim(s.second.id, true).unwrap();
            s.hello(s.second.id).await;
        };
        tokio::join!(vouch, answer).0
    });
    assert_eq!(vouched, Ok(s.device));
    assert_eq!(s.opened(), opened + 1, "only your answer reopened it");
}
