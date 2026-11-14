// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What a device says about itself: what it does, and that it's about to
//! restart.

use std::time::Instant;

use maxhq_protocol::model::Device;

use super::*;

/// Retries after a wind-down come quickly and stop within a test. Any
/// other timed retry never comes.
const WIND_DOWN: Timing = Timing {
    wind_down_retry: Duration::from_millis(20),
    wind_down_window: Duration::from_secs(1),
    ..NO_RETRY
};

fn this_device(winding_down_until: Option<Timestamp>) -> ServerMessage {
    ServerMessage::Snapshot(Snapshot {
        seq: Seq(1),
        entities: vec![Entity::Device(Device {
            id: device(),
            name: "Test box".into(),
            os: "linux".into(),
            boot_id: BootId("boot-1".into()),
            maxd_version: "0.1.0".into(),
            winding_down_until,
            workspace_host: None,
            build: None,
            target: None,
        })],
    })
}

/// Connects a link to a device whose state is `snapshot`, then drops the
/// connection with dials failing from then on. Events are read in the
/// background so the link never waits on them.
async fn dropped(timing: Timing, snapshot: ServerMessage) -> (Rig, LinkHandle, Instant) {
    let mut rig = Rig::new(timing, Tailnet::default(), &[]);
    let (handle, mut events, mut maxd) = connected(&mut rig, InstanceId::generate()).await;
    maxd.send(&snapshot).await;
    assert!(matches!(next(&mut events).await, LinkEvent::Synced(_)));
    tokio::spawn(async move { while events.recv().await.is_some() {} });

    rig.fake
        .set_down(Some(DialError::down("no route to the host")));
    drop(maxd);
    (rig, handle, Instant::now())
}

async fn wait_for_dials(rig: &Rig, count: usize) {
    let dialed = async {
        while rig.fake.dials() < count {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    tokio::time::timeout(PATIENCE, dialed)
        .await
        .expect("the dials in time");
}

#[test]
fn a_wound_down_device_is_retried_every_2_seconds_for_10_minutes() {
    assert_eq!(Timing::DEFAULT.wind_down_retry, Duration::from_secs(2));
    assert_eq!(
        Timing::DEFAULT.wind_down_window,
        Duration::from_secs(10 * 60)
    );
}

#[tokio::test]
async fn a_device_that_drops_after_winding_down_is_retried_on_the_short_delay() {
    let until = Timestamp::now().after(Duration::from_secs(60));
    let (rig, _handle, dropped_at) = dropped(WIND_DOWN, this_device(Some(until))).await;

    // The first connect, the redial right after the drop, then retries
    // that the backoff alone would never make in a test.
    wait_for_dials(&rig, 6).await;
    assert!(
        dropped_at.elapsed() < WIND_DOWN.wind_down_window,
        "the retries came on the short delay"
    );

    // Once the window is over, the backoff takes over again.
    let after = WIND_DOWN.wind_down_window + Duration::from_millis(300);
    tokio::time::sleep(after.saturating_sub(dropped_at.elapsed())).await;
    let settled = rig.fake.dials();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(rig.fake.dials(), settled);
}

#[tokio::test]
async fn a_device_that_drops_without_winding_down_waits_for_the_backoff() {
    let (rig, _handle, _) = dropped(WIND_DOWN, this_device(None)).await;
    wait_for_dials(&rig, 2).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(rig.fake.dials(), 2);
}

#[tokio::test]
async fn a_device_that_drops_after_winding_down_shows_as_restarting() {
    let mut rig = Rig::new(WIND_DOWN, Tailnet::default(), &[]);
    let (_handle, mut events, mut maxd) = connected(&mut rig, InstanceId::generate()).await;
    let until = Timestamp::now().after(Duration::from_secs(60));
    maxd.send(&this_device(Some(until))).await;
    assert!(matches!(next(&mut events).await, LinkEvent::Synced(_)));

    // It shows as restarting from the drop, with no wait for a dial to
    // fail first.
    rig.fake
        .set_down(Some(DialError::down("no route to the host")));
    drop(maxd);
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::WaitingForReboot, 1)
    );

    rig.fake.set_down(None);
    let mut maxd = rig.next_maxd().await;
    assert_eq!(maxd.accept(InstanceId::generate(), "boot-2").await, None);
    loop {
        match next_state(&mut events).await {
            LinkState::Connecting {
                step: ConnectStep::WaitingForReboot,
                ..
            } => {}
            state => {
                assert!(is_connected(&state), "expected connected, got {state:?}");
                break;
            }
        }
    }
}

#[tokio::test]
async fn the_handle_reports_what_the_device_does() {
    let mut rig = Rig::new(NO_RETRY, Tailnet::default(), &[]);
    let (handle, mut events) = rig.client.link(spec(Route::Local));
    assert_eq!(handle.capabilities(), Capabilities::default());
    let mut maxd = rig.next_maxd().await;
    assert!(matches!(maxd.recv().await, Some(ClientMessage::Hello(_))));
    let does = Capabilities {
        transfers: true,
        tunnels: true,
        workspaces: false,
        github_checks: true,
        inbox: false,
        ..Capabilities::default()
    };
    maxd.send(&ServerMessage::Hello(ServerHello {
        protocol: ProtocolRange::OURS,
        maxd_version: "0.1.0".into(),
        device: device(),
        boot_id: BootId("boot-1".into()),
        instance: InstanceId::generate(),
        capabilities: does,
        build: None,
        target: None,
        challenge: None,
    }))
    .await;
    assert!(matches!(
        maxd.recv().await,
        Some(ClientMessage::Sync { since: None })
    ));
    let link = loop {
        let link = next_link(&mut events).await;
        if is_connected(&link.state) {
            break link;
        }
        assert_eq!(link.capabilities, None, "known before the hello");
        assert_eq!(link.protocol, None, "known before the hello");
    };
    assert_eq!(handle.capabilities(), does);
    assert_eq!(link.capabilities, Some(does));
    assert_eq!(link.protocol, Some(ProtocolRange::OURS.current));

    // A drop keeps what the device said until the next hello.
    drop(maxd);
    let link = next_link(&mut events).await;
    assert_eq!(link.state, connecting(ConnectStep::Dialing, 1));
    assert_eq!(handle.capabilities(), does);
    assert_eq!(link.capabilities, Some(does));
    assert_eq!(link.protocol, Some(ProtocolRange::OURS.current));
}
