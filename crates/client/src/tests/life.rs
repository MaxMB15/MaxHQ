// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! An app going to the background and coming back.

use super::*;

/// The link's states until one matches `wanted`.
async fn until_state(
    events: &mut mpsc::Receiver<LinkEvent>,
    wanted: impl Fn(&LinkState) -> bool,
) -> LinkState {
    loop {
        let state = next_state(events).await;
        if wanted(&state) {
            return state;
        }
    }
}

#[tokio::test]
async fn waking_dials_at_once_past_the_backoff() {
    let mut rig = Rig::new(NO_RETRY, Tailnet::default(), &[]);
    rig.fake
        .set_down(Some(DialError::down("no route to the host")));
    let (_handle, mut events) = rig.client.link(spec(ssh("devbox")));
    until_state(&mut events, |state| {
        matches!(state, LinkState::Unreachable { .. })
    })
    .await;
    assert_eq!(rig.fake.dials(), 1);

    rig.fake.set_down(None);
    rig.client.wake();
    let mut maxd = rig.next_maxd().await;
    maxd.accept(InstanceId::generate(), "boot-1").await;
    until_state(&mut events, is_connected).await;
    assert_eq!(rig.fake.dials(), 2);
}

#[tokio::test]
async fn suspending_closes_the_connection_until_waking() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    let (_handle, mut events, mut maxd) = connected(&mut rig, InstanceId::generate()).await;

    rig.client.suspend();
    assert_eq!(maxd.recv().await, None, "the connection closes");
    until_state(&mut events, |state| *state == LinkState::Unknown).await;
    // FAST retries in tens of milliseconds, so this would have redialed.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(rig.fake.dials(), 1, "nothing dials in the background");

    rig.client.wake();
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Dialing, 1)
    );
    let mut maxd = rig.next_maxd().await;
    maxd.accept(InstanceId::generate(), "boot-1").await;
    until_state(&mut events, is_connected).await;
}
