// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use image::{GenericImageView, ImageFormat, Rgb, RgbImage};
use maxhq_core::leases::LeaseCall;
use maxhq_protocol::agents::{Actor, PeerBook, PeerEntry};
use maxhq_protocol::ids::{AgentId, LeaseId, PermissionId, TestHostId, WorkspaceId};
use maxhq_protocol::leases::{
    host_id, LeaseEnd, LeaseState, TestHost, TestHostBackend, TestHostState, TestKind, TestLease,
};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{Activity, Agent, AgentLink, Control, Inbox, Vendor};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::time::Timestamp;

use super::backend::TestBackend;
use super::fake::FakeBackend;
use super::{refused, LeaseCaller, Leases, NoLeases, TestHosts, NOT_YET};
use crate::db::Db;
use crate::drive::ToolResult;
use crate::hub::Hub;
use crate::secrets::testing::hub;
use crate::session::clock::FakeClock;

const MINUTE: Duration = Duration::from_secs(60);

struct Fixture {
    hub: Hub,
    db: Db,
    clock: Arc<FakeClock>,
    store: TestHosts,
}

/// A store over `backends`, with `kinds` picked and the timers running.
fn fixture(backends: &[&Arc<FakeBackend>], kinds: &[TestKind]) -> Fixture {
    let hub = hub();
    let db = Db::open_in_memory().expect("a database");
    pick(&hub, kinds, |_| {});
    started(hub, db, backends)
}

/// Picks `kinds` in the device's settings, then lets `edit` change them.
fn pick(
    hub: &Hub,
    kinds: &[TestKind],
    edit: impl FnOnce(&mut maxhq_protocol::leases::TestHostSettings),
) {
    let mut settings = crate::hub::default_settings(hub.device());
    settings.test_hosts.kinds = kinds.to_vec();
    edit(&mut settings.test_hosts);
    hub.apply([Change::Upsert {
        value: Entity::DeviceSettings(settings),
    }]);
}

fn started(hub: Hub, db: Db, backends: &[&Arc<FakeBackend>]) -> Fixture {
    let clock = Arc::new(FakeClock::new());
    let backends = backends
        .iter()
        .map(|backend| Arc::clone(*backend) as Arc<dyn TestBackend>)
        .collect();
    let store = TestHosts::open(hub.clone(), db.clone(), clock.clone(), backends);
    tokio::spawn(store.clone().watch());
    Fixture {
        hub,
        db,
        clock,
        store,
    }
}

fn linux() -> Arc<FakeBackend> {
    Arc::new(FakeBackend::new(
        TestKind::Linux,
        TestHostBackend::IncusTest,
    ))
}

fn caller(name: &str) -> LeaseCaller {
    LeaseCaller {
        actor: Actor::Agent {
            agent: AgentId::derive(name),
        },
        permission: PermissionId::derive("testing"),
    }
}

fn open(kind: TestKind, wait_seconds: u16) -> LeaseCall {
    LeaseCall::Open {
        kind,
        minutes: None,
        wait_seconds,
    }
}

/// Waits, in real time, until `done` holds.
async fn until(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..500 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    panic!("never saw {what}");
}

impl Fixture {
    fn host(&self, backend: &FakeBackend) -> TestHost {
        let id = self.id(backend);
        match self.hub.get(&EntityKey::TestHost(id)) {
            Some(Entity::TestHost(host)) => host,
            other => panic!("no published host: {other:?}"),
        }
    }

    fn id(&self, backend: &FakeBackend) -> TestHostId {
        host_id(self.hub.device().id, backend.kind(), backend.backend())
    }

    fn lease(&self, id: LeaseId) -> Option<TestLease> {
        match self.hub.get(&EntityKey::TestLease(id)) {
            Some(Entity::TestLease(lease)) => Some(lease),
            _ => None,
        }
    }

    /// The only lease `caller` holds or held.
    fn lease_of(&self, caller: &LeaseCaller) -> TestLease {
        let leases: Vec<TestLease> = self
            .hub
            .snapshot()
            .entities
            .into_iter()
            .filter_map(|entity| match entity {
                Entity::TestLease(lease) if lease.holder == caller.actor => Some(lease),
                _ => None,
            })
            .collect();
        assert_eq!(leases.len(), 1, "{leases:?}");
        leases[0].clone()
    }

    async fn state_of(&self, backend: &FakeBackend, state: TestHostState) {
        until(&format!("{state:?}"), || self.host(backend).state == state).await;
    }

    async fn built(&self, backend: &FakeBackend) {
        self.state_of(backend, TestHostState::Off).await;
        self.store.build(self.id(backend)).expect("a build");
        self.state_of(backend, TestHostState::Warm).await;
    }

    async fn ended(&self, id: LeaseId) -> LeaseEnd {
        until("the lease end", || {
            self.lease(id)
                .is_some_and(|lease| lease.state == LeaseState::Ended)
        })
        .await;
        self.lease(id)
            .and_then(|lease| lease.ended)
            .expect("why it ended")
    }

    /// Moves the clock once the timers sleep.
    async fn advance(&self, by: Duration) {
        until("the timers asleep", || self.clock.sleepers() > 0).await;
        self.clock.advance(by);
    }
}

fn text(answer: Result<ToolResult, String>) -> String {
    match answer {
        Ok(ToolResult::Text(text)) => text,
        other => panic!("not text: {other:?}"),
    }
}

#[tokio::test]
async fn an_open_lease_is_ready_with_how_to_drive_it() {
    let host = linux();
    let fx = fixture(&[&host], &[TestKind::Linux]);
    fx.built(&host).await;
    let me = caller("me");

    let answer = text(fx.store.run(me, open(TestKind::Linux, 0)).await);

    let lease = fx.lease_of(&me);
    assert_eq!(lease.state, LeaseState::Ready);
    assert_eq!(lease.target, Some(format!("fake-{}", lease.id)));
    assert_eq!(lease.host, fx.id(&host));
    assert!(
        answer.contains(&format!("Lease {} on the Linux", lease.id)),
        "{answer}"
    );
    assert!(answer.contains("lease_run"), "{answer}");
    assert_eq!(fx.host(&host).state, TestHostState::Leased);
    assert_eq!(host.log(), ["build", &format!("start {}", lease.id)]);
    assert_eq!(
        fx.db.leases().expect("leases"),
        std::slice::from_ref(&lease)
    );

    // Asking again answers the same lease.
    text(fx.store.run(me, open(TestKind::Linux, 0)).await);
    assert_eq!(fx.lease_of(&me).id, lease.id);
}

#[tokio::test]
async fn a_kind_this_device_doesnt_offer_names_the_devices_that_do() {
    let host = linux();
    let fx = fixture(&[&host], &[]);
    let me = caller("me");

    let alone = fx.store.run(me, open(TestKind::Linux, 0)).await;
    assert_eq!(
        alone,
        Err("This device doesn't offer a Linux host, and no linked device does.".into())
    );

    fx.db
        .set_peer_book(&PeerBook {
            ssh: true,
            peers: vec![PeerEntry {
                device: maxhq_protocol::ids::DeviceId::derive("devbox"),
                name: "devbox".into(),
                routes: Vec::new(),
                host_key: String::new(),
                test_kinds: vec![TestKind::Linux],
                home: false,
            }],
        })
        .expect("a peer book");
    let elsewhere = fx
        .store
        .run(me, open(TestKind::Linux, 0))
        .await
        .unwrap_err();
    assert!(
        elsewhere.contains("devbox does; use start_agent"),
        "{elsewhere}"
    );
    let list = text(fx.store.run(me, LeaseCall::ListTestHosts).await);
    assert!(list.contains("This device offers no test hosts."), "{list}");
    assert!(list.contains("devbox offers Linux"), "{list}");
}

#[tokio::test]
async fn a_host_that_isnt_built_says_so() {
    let host = linux();
    let fx = fixture(&[&host], &[TestKind::Linux]);
    fx.state_of(&host, TestHostState::Off).await;

    let answer = fx.store.run(caller("me"), open(TestKind::Linux, 0)).await;

    let answer = answer.unwrap_err();
    assert!(
        answer.starts_with("No Linux host here is ready to lease"),
        "{answer}"
    );
    assert!(answer.contains("not built yet"), "{answer}");
}

#[tokio::test]
async fn a_busy_host_answers_the_place_in_line_once_the_wait_is_over() {
    let host = linux();
    let fx = fixture(&[&host], &[TestKind::Linux]);
    fx.built(&host).await;
    text(
        fx.store
            .run(caller("first"), open(TestKind::Linux, 0))
            .await,
    );

    let store = fx.store.clone();
    let second =
        tokio::spawn(async move { store.run(caller("second"), open(TestKind::Linux, 60)).await });
    until("the second in line", || fx.host(&host).queue == 1).await;
    let queued = fx.lease_of(&caller("second"));
    assert_eq!(queued.state, LeaseState::Queued);
    until("the wait asleep", || fx.clock.sleepers() > 1).await;
    fx.clock.advance(MINUTE);

    let answer = second.await.expect("an answer").unwrap_err();
    assert!(
        answer.contains("You were number 1 in line after 60 seconds"),
        "{answer}"
    );
    assert_eq!(fx.lease(queued.id), None);
    assert_eq!(fx.host(&host).queue, 0);
    assert_eq!(fx.db.leases().expect("leases").len(), 1);
}

#[tokio::test]
async fn a_closed_lease_hands_the_host_to_the_next_in_line() {
    let host = linux();
    let fx = fixture(&[&host], &[TestKind::Linux]);
    fx.built(&host).await;
    let first = caller("first");
    text(fx.store.run(first, open(TestKind::Linux, 0)).await);
    let held = fx.lease_of(&first);

    let store = fx.store.clone();
    let second = tokio::spawn(async move {
        store
            .run(caller("second"), open(TestKind::Linux, 600))
            .await
    });
    until("the second in line", || fx.host(&host).queue == 1).await;
    let closed = text(
        fx.store
            .run(first, LeaseCall::Close { lease: held.id })
            .await,
    );

    assert_eq!(closed, format!("Lease {} is closed.", held.id));
    assert_eq!(
        fx.lease(held.id).and_then(|lease| lease.ended),
        Some(LeaseEnd::Closed)
    );
    text(second.await.expect("an answer"));
    let next = fx.lease_of(&caller("second"));
    assert_eq!(next.state, LeaseState::Ready);
    let log = host.log();
    assert_eq!(
        log[1..],
        [
            format!("start {}", held.id),
            format!("close fake-{}", held.id),
            format!("start {}", next.id),
        ]
    );
    // Closing it again says it ended.
    let again = fx
        .store
        .run(first, LeaseCall::Close { lease: held.id })
        .await;
    assert!(again.unwrap_err().contains("has ended. It was closed."));
}

#[tokio::test]
async fn a_lease_ends_at_its_time_limit() {
    let host = linux();
    let fx = fixture(&[&host], &[TestKind::Linux]);
    fx.built(&host).await;
    let me = caller("me");
    let call = LeaseCall::Open {
        kind: TestKind::Linux,
        minutes: Some(5),
        wait_seconds: 0,
    };
    let answer = text(fx.store.run(me, call).await);
    assert!(answer.contains("It lasts 5 minutes"), "{answer}");
    let lease = fx.lease_of(&me);
    assert_eq!(
        lease.ends_at,
        Timestamp(1_800_000_000_000).after(5 * MINUTE)
    );

    fx.advance(5 * MINUTE).await;

    assert_eq!(fx.ended(lease.id).await, LeaseEnd::TimeUp);
    fx.state_of(&host, TestHostState::Warm).await;
    assert!(host.log().contains(&format!("close fake-{}", lease.id)));
}

#[tokio::test]
async fn calls_keep_a_lease_from_going_idle() {
    let host = linux();
    let fx = fixture(&[&host], &[TestKind::Linux]);
    fx.built(&host).await;
    let me = caller("me");
    text(fx.store.run(me, open(TestKind::Linux, 0)).await);
    let lease = fx.lease_of(&me).id;

    fx.advance(9 * MINUTE).await;
    let ran = text(
        fx.store
            .run(
                me,
                LeaseCall::Run {
                    lease,
                    command: vec!["uname".into(), "-a".into()],
                    timeout_seconds: 30,
                },
            )
            .await,
    );
    assert_eq!(ran, "Exit code 0.\nstdout:\nran uname -a");
    let recent = fx.lease(lease).expect("the lease").recent;
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].command, ["uname", "-a"]);
    assert_eq!(recent[0].exit_code, Some(0));
    fx.advance(9 * MINUTE).await;
    tokio::task::yield_now().await;
    assert_eq!(fx.lease(lease).expect("the lease").state, LeaseState::Ready);

    fx.advance(MINUTE).await;

    assert_eq!(fx.ended(lease).await, LeaseEnd::Idle);
}

#[tokio::test]
async fn a_lease_ends_when_its_agent_goes() {
    let host = linux();
    let fx = fixture(&[&host], &[TestKind::Linux]);
    fx.built(&host).await;
    let me = caller("me");
    let Actor::Agent { agent } = me.actor else {
        unreachable!()
    };
    fx.hub.apply([Change::Upsert {
        value: Entity::Agent(agent_named(agent)),
    }]);
    text(fx.store.run(me, open(TestKind::Linux, 0)).await);
    let lease = fx.lease_of(&me).id;

    fx.hub.apply([Change::Remove {
        key: EntityKey::Agent(agent),
    }]);

    assert_eq!(fx.ended(lease).await, LeaseEnd::HolderGone);
}

fn agent_named(id: AgentId) -> Agent {
    Agent {
        id,
        workspace: WorkspaceId::derive("workspace"),
        branch: None,
        parent: None,
        vendor: Vendor::Claude,
        account: None,
        model: None,
        permission: None,
        session: None,
        vendor_session_id: None,
        control: Control::Held,
        pid: None,
        link: AgentLink::Live,
        title: "test the app".to_owned(),
        activity: Activity::Idle,
        context: None,
        started_at: Timestamp(1),
        last_activity_at: Timestamp(1),
        inbox: Inbox::default(),
        origin: None,
        pulls: Vec::new(),
        taking_over: false,
        asleep: false,
        effort: None,
        running: None,
        moved: None,
    }
}

#[tokio::test]
async fn you_end_a_lease_from_the_app() {
    let host = linux();
    let fx = fixture(&[&host], &[TestKind::Linux]);
    fx.built(&host).await;
    let me = caller("me");
    text(fx.store.run(me, open(TestKind::Linux, 0)).await);
    let lease = fx.lease_of(&me).id;

    assert_eq!(fx.store.end_by_you(lease).await, ActionOutcome::Done);

    assert_eq!(
        fx.lease(lease).and_then(|lease| lease.ended),
        Some(LeaseEnd::EndedByYou)
    );
    assert_eq!(fx.host(&host).state, TestHostState::Warm);
    assert!(matches!(
        fx.store.end_by_you(lease).await,
        ActionOutcome::Refused { .. }
    ));
    let run = fx
        .store
        .run(
            me,
            LeaseCall::Run {
                lease,
                command: vec!["true".into()],
                timeout_seconds: 5,
            },
        )
        .await;
    assert_eq!(
        run,
        Err(format!(
            "Lease {lease} has ended. It was ended in the MaxHQ app. Call lease_open for a new one."
        ))
    );
}

#[tokio::test]
async fn only_the_holder_uses_a_lease() {
    let host = linux();
    let fx = fixture(&[&host], &[TestKind::Linux]);
    fx.built(&host).await;
    let me = caller("me");
    text(fx.store.run(me, open(TestKind::Linux, 0)).await);
    let lease = fx.lease_of(&me).id;

    let theirs = fx
        .store
        .run(caller("other"), LeaseCall::Close { lease })
        .await;
    assert_eq!(theirs, Err(format!("Lease {lease} isn't yours.")));
    let missing = LeaseId::derive("missing");
    let gone = fx
        .store
        .run(me, LeaseCall::Screenshot { lease: missing })
        .await;
    assert_eq!(
        gone,
        Err(format!(
            "There's no lease {missing}. Call lease_open for one."
        ))
    );
}

#[tokio::test]
async fn a_screenshot_fits_the_largest_edge_and_installs_reach_the_machine() {
    let host = linux();
    let mut png = Vec::new();
    RgbImage::from_fn(2560, 1600, |x, _| Rgb([(x % 256) as u8, 0, 0]))
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .expect("a PNG");
    host.script().screen = png;
    let fx = fixture(&[&host], &[TestKind::Linux]);
    fx.built(&host).await;
    let me = caller("me");
    text(fx.store.run(me, open(TestKind::Linux, 0)).await);
    let lease = fx.lease_of(&me).id;

    let Ok(ToolResult::Mixed { text: said, images }) =
        fx.store.run(me, LeaseCall::Screenshot { lease }).await
    else {
        panic!("no screenshot");
    };
    assert_eq!(said, format!("The screen of lease {lease}."));
    assert_eq!(images[0].mime, "image/png");
    let shown = STANDARD.decode(&images[0].data).expect("base64");
    let shown = image::load_from_memory_with_format(&shown, ImageFormat::Png).expect("a PNG");
    assert_eq!(shown.dimensions(), (1280, 800));

    let ActionOutcome::LeaseScreen { image, at } = fx.store.screen(lease).await else {
        panic!("no screen for the app");
    };
    assert_eq!(image, images[0]);
    assert_eq!(at, Timestamp(1_800_000_000_000));

    let source = PathBuf::from("/work/app.apk");
    let installed = fx.store.run(me, LeaseCall::Install { lease, source }).await;
    assert_eq!(text(installed), "Installed /work/app.apk.");
    assert!(host
        .log()
        .contains(&format!("install fake-{lease} /work/app.apk")));
}

#[tokio::test]
async fn a_lease_closed_while_starting_ends_once_it_started() {
    let host = linux();
    let fx = fixture(&[&host], &[TestKind::Linux]);
    fx.built(&host).await;
    host.hold_starts();
    let me = caller("me");
    let store = fx.store.clone();
    let opening = tokio::spawn(async move { store.run(me, open(TestKind::Linux, 0)).await });
    until("the start", || {
        fx.hub.snapshot().entities.iter().any(|entity| {
            matches!(entity, Entity::TestLease(lease) if lease.state == LeaseState::Starting)
        })
    })
    .await;
    let lease = fx.lease_of(&me).id;

    let store = fx.store.clone();
    let closing = tokio::spawn(async move { store.run(me, LeaseCall::Close { lease }).await });
    until("the end", || {
        fx.lease(lease)
            .is_some_and(|lease| lease.state == LeaseState::Ending)
    })
    .await;
    host.release_starts();

    text(closing.await.expect("an answer"));
    let opened = opening.await.expect("an answer").unwrap_err();
    assert!(opened.contains("has ended. It was closed."), "{opened}");
    assert!(host.log().contains(&format!("close fake-{lease}")));
    fx.state_of(&host, TestHostState::Warm).await;
}

#[tokio::test]
async fn a_start_that_fails_ends_the_lease_and_fails_the_host() {
    let host = linux();
    host.script().start_fails = Some("The container didn't start.".into());
    let fx = fixture(&[&host], &[TestKind::Linux]);
    fx.built(&host).await;
    let me = caller("me");

    let answer = fx
        .store
        .run(me, open(TestKind::Linux, 0))
        .await
        .unwrap_err();

    assert!(answer.contains("Its test host failed"), "{answer}");
    assert_eq!(fx.lease_of(&me).ended, Some(LeaseEnd::HostFailed));
    let failed = fx.host(&host);
    assert_eq!(failed.state, TestHostState::Failed);
    assert_eq!(failed.error.as_deref(), Some("The container didn't start."));
}

#[tokio::test]
async fn a_build_shows_how_it_went_and_a_missing_tool_blocks_it() {
    let host = linux();
    host.script().build_fails = Some("No image.".into());
    let fx = fixture(&[&host], &[TestKind::Linux]);
    fx.state_of(&host, TestHostState::Off).await;

    fx.store.build(fx.id(&host)).expect("a build");
    fx.state_of(&host, TestHostState::Failed).await;
    assert_eq!(fx.host(&host).error.as_deref(), Some("No image."));

    host.script().build_fails = None;
    fx.store.build(fx.id(&host)).expect("a build");
    fx.state_of(&host, TestHostState::Warm).await;
    let warm = fx.host(&host);
    assert_eq!(warm.error, None);
    assert_eq!(warm.agent_device.as_deref(), Some("0.0.0-fake"));
    assert_eq!(warm.last_reset, Some(Timestamp(1_800_000_000_000)));
    assert_eq!(
        fx.store.build(fx.id(&host)),
        Err("It's built already.".into())
    );

    // Detection that finds something missing makes an idle host
    // unavailable, and finding it again lets it be built.
    host.script().setup = Some("Install Incus.".into());
    fx.store.check().await;
    let missing = fx.host(&host);
    assert_eq!(missing.state, TestHostState::Unavailable);
    assert_eq!(missing.setup.as_deref(), Some("Install Incus."));
    assert_eq!(fx.store.build(fx.id(&host)), Err("Install Incus.".into()));
    host.script().setup = None;
    fx.store.check().await;
    assert_eq!(fx.host(&host).state, TestHostState::Off);
}

#[tokio::test]
async fn a_reset_ends_the_leases_on_the_host_first() {
    let host = linux();
    let fx = fixture(&[&host], &[TestKind::Linux]);
    fx.built(&host).await;
    let me = caller("me");
    text(fx.store.run(me, open(TestKind::Linux, 0)).await);
    let lease = fx.lease_of(&me).id;

    fx.store.reset(fx.id(&host)).await.expect("a reset");

    assert_eq!(
        fx.lease(lease).and_then(|lease| lease.ended),
        Some(LeaseEnd::EndedByYou)
    );
    assert_eq!(fx.host(&host).state, TestHostState::Warm);
    assert_eq!(
        host.log()[2..],
        [format!("close fake-{lease}"), "reset".to_owned()]
    );

    host.script().reset_fails = Some("The snapshot is gone.".into());
    let failed = fx.store.reset(fx.id(&host)).await;
    assert_eq!(failed, Err("The snapshot is gone.".into()));
    assert_eq!(fx.host(&host).state, TestHostState::Failed);
}

#[tokio::test]
async fn a_restart_ends_the_leases_it_finds_started_and_cleans_their_hosts() {
    let host = linux();
    let fx = fixture(&[&host], &[TestKind::Linux]);
    fx.built(&host).await;
    let me = caller("me");
    text(fx.store.run(me, open(TestKind::Linux, 0)).await);
    let ready = fx.lease_of(&me);
    let mut queued = ready.clone();
    queued.id = LeaseId::derive("queued");
    queued.state = LeaseState::Queued;
    fx.db.put_lease(&queued).expect("a queued lease");
    let gone = TestHost {
        id: TestHostId::derive("gone"),
        ..fx.host(&host)
    };
    fx.db.put_test_host(&gone).expect("a host no backend runs");

    let hub = hub();
    pick(&hub, &[TestKind::Linux], |_| {});
    let again = started(hub, fx.db.clone(), &[&host]);

    assert_eq!(again.ended(ready.id).await, LeaseEnd::HostFailed);
    again.state_of(&host, TestHostState::Warm).await;
    assert_eq!(host.log().last().map(String::as_str), Some("reset"));
    assert_eq!(again.lease(queued.id), None);
    let stored: Vec<LeaseId> = again
        .db
        .leases()
        .expect("leases")
        .iter()
        .map(|lease| lease.id)
        .collect();
    assert_eq!(stored, [ready.id]);
    let hosts: Vec<TestHostId> = again
        .db
        .test_hosts()
        .expect("hosts")
        .iter()
        .map(|host| host.id)
        .collect();
    assert_eq!(hosts, [again.id(&host)]);
}

#[tokio::test]
async fn a_restart_while_building_fails_the_host() {
    let host = linux();
    let db = Db::open_in_memory().expect("a database");
    let hub = hub();
    let id = host_id(hub.device().id, TestKind::Linux, TestHostBackend::IncusTest);
    db.put_test_host(&TestHost {
        id,
        device: hub.device().id,
        kind: TestKind::Linux,
        backend: TestHostBackend::IncusTest,
        state: TestHostState::Building,
        setup: None,
        error: None,
        agent_device: None,
        last_reset: None,
        queue: 0,
        vm: None,
    })
    .expect("a host");

    let fx = started(hub, db, &[&host]);

    let failed = fx.host(&host);
    assert_eq!(failed.state, TestHostState::Failed);
    assert_eq!(
        failed.error.as_deref(),
        Some("MaxHQ restarted while it was being built.")
    );
}

#[tokio::test]
async fn ios_prefers_a_vm_and_the_mac_itself_needs_your_yes() {
    let vm = Arc::new(FakeBackend::new(
        TestKind::IosSimulator,
        TestHostBackend::MacVm,
    ));
    let mac = Arc::new(FakeBackend::new(
        TestKind::IosSimulator,
        TestHostBackend::MacHost,
    ));
    let hub = hub();
    let db = Db::open_in_memory().expect("a database");
    pick(&hub, &[TestKind::IosSimulator], |settings| {
        settings.max_vms = 2
    });
    let fx = started(hub.clone(), db, &[&vm, &mac]);
    fx.built(&vm).await;
    let off = fx.host(&mac);
    assert_eq!(off.state, TestHostState::Unavailable);
    assert!(off
        .setup
        .is_some_and(|setup| setup.starts_with("Allow agents to lease")));

    // The VM takes two leases when you allow a second VM.
    text(
        fx.store
            .run(caller("one"), open(TestKind::IosSimulator, 0))
            .await,
    );
    text(
        fx.store
            .run(caller("two"), open(TestKind::IosSimulator, 0))
            .await,
    );
    assert_eq!(fx.lease_of(&caller("one")).host, fx.id(&vm));
    assert_eq!(fx.lease_of(&caller("two")).host, fx.id(&vm));
    // The Mac itself isn't leased while you haven't allowed it.
    let busy = fx
        .store
        .run(caller("three"), open(TestKind::IosSimulator, 0))
        .await;
    assert!(busy
        .unwrap_err()
        .contains("Every iOS Simulator host is busy"));
    let list = text(
        fx.store
            .run(caller("three"), LeaseCall::ListTestHosts)
            .await,
    );
    assert_eq!(
        list,
        "Test hosts on this device:\n- iOS Simulator on the Mac VM: leased."
    );

    pick(&hub, &[TestKind::IosSimulator], |settings| {
        settings.max_vms = 2;
        settings.mac_host = true;
    });
    fx.built(&mac).await;
    let list = text(
        fx.store
            .run(caller("three"), LeaseCall::ListTestHosts)
            .await,
    );
    assert!(
        list.contains("\n- iOS Simulator on the Mac itself: ready"),
        "{list}"
    );
    text(
        fx.store
            .run(caller("three"), open(TestKind::IosSimulator, 0))
            .await,
    );
    assert_eq!(fx.lease_of(&caller("three")).host, fx.id(&mac));
}

#[tokio::test]
async fn without_test_hosts_every_call_is_refused_in_words() {
    let answer = NoLeases.run(caller("me"), LeaseCall::ListTestHosts).await;
    assert_eq!(answer, Err(NOT_YET.to_owned()));
    assert_eq!(
        refused(),
        ActionOutcome::Refused {
            reason: NOT_YET.into()
        }
    );
}
