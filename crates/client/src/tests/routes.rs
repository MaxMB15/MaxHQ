// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A device with several routes: which goes first, handing over to the
//! next, and refusing one that reaches a different device.

use super::*;
use crate::link::try_order;

impl Rig {
    /// The same rig, with links on network `key`.
    fn on_network(self, key: Option<&str>) -> Self {
        Self {
            client: self.client.on_network(key),
            ..self
        }
    }
}

/// A device reached over the LAN first, then over the tailnet.
fn two_routes() -> LinkSpec {
    LinkSpec {
        fallbacks: vec![ssh("devbox-tail")],
        ..spec(ssh("devbox-lan"))
    }
}

/// Reads events until the link is connected, and returns it.
async fn until_connected(events: &mut mpsc::Receiver<LinkEvent>) -> DeviceLink {
    loop {
        if let LinkEvent::Link(link) = next(events).await
            && is_connected(&link.state)
        {
            return *link;
        }
    }
}

/// Reads events until a route is recorded as working.
async fn until_worked(events: &mut mpsc::Receiver<LinkEvent>) -> (String, Route) {
    loop {
        if let LinkEvent::RouteWorked { net, route } = next(events).await {
            return (net, route);
        }
    }
}

#[test]
fn the_route_that_worked_here_goes_first_then_yours_in_order() {
    let yours = [ssh("a"), ssh("b"), ssh("c")];
    assert_eq!(try_order(&yours, None), yours);
    assert_eq!(
        try_order(&yours, Some(&ssh("b"))),
        [ssh("b"), ssh("a"), ssh("c")]
    );
    // A route you've since taken away isn't tried.
    assert_eq!(try_order(&yours, Some(&ssh("gone"))), yours);
}

#[tokio::test]
async fn a_route_that_fails_hands_over_to_the_next_and_is_remembered() {
    let mut rig = Rig::new(NO_RETRY, Tailnet::default(), &[]).on_network(Some("home"));
    rig.fake.set_host_down("devbox-lan");
    let (_handle, mut events) = rig.client.link(two_routes());
    let first = next_link(&mut events).await;
    assert_eq!(
        first
            .routes
            .iter()
            .map(|choice| choice.route.clone())
            .collect::<Vec<_>>(),
        [ssh("devbox-lan"), ssh("devbox-tail")]
    );

    let mut maxd = rig.next_maxd().await;
    maxd.accept(InstanceId::generate(), "boot-1").await;
    assert_eq!(
        until_worked(&mut events).await,
        ("home".to_owned(), ssh("devbox-tail"))
    );
    let link = until_connected(&mut events).await;
    assert_eq!(link.route, ssh("devbox-tail"));
    let here: Vec<bool> = link.routes.iter().map(|c| c.last_good_here).collect();
    assert_eq!(here, [false, true]);
    assert_eq!(rig.fake.dialed(), [ssh("devbox-lan"), ssh("devbox-tail")]);
}

#[tokio::test]
async fn the_route_that_last_worked_on_this_network_is_dialed_first() {
    let mut rig = Rig::new(NO_RETRY, Tailnet::default(), &[]).on_network(Some("cafe"));
    let mut spec = two_routes();
    spec.last_good.insert("cafe".into(), ssh("devbox-tail"));
    spec.last_good.insert("home".into(), ssh("devbox-lan"));
    let (_handle, mut events) = rig.client.link(spec);
    let mut maxd = rig.next_maxd().await;
    maxd.accept(InstanceId::generate(), "boot-1").await;
    let link = until_connected(&mut events).await;
    assert_eq!(link.route, ssh("devbox-tail"));
    assert_eq!(rig.fake.dialed(), [ssh("devbox-tail")]);
}

#[tokio::test]
async fn a_route_that_reaches_a_different_device_is_refused() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]).on_network(Some("home"));
    let (_handle, mut events) = rig.client.link(two_routes());
    let mut maxd = rig.next_maxd().await;
    maxd.accept(InstanceId::generate(), "boot-1").await;
    let link = until_connected(&mut events).await;
    assert_eq!(link.device, Some(device()));
    drop(maxd);

    // The LAN address now belongs to another machine running maxd.
    let mut stranger = rig.next_maxd().await;
    assert!(matches!(
        stranger.recv().await,
        Some(ClientMessage::Hello(_))
    ));
    stranger
        .send(&ServerMessage::Hello(ServerHello {
            protocol: ProtocolRange::OURS,
            maxd_version: "0.1.0".into(),
            device: DeviceId::derive("another-device"),
            boot_id: BootId("boot-9".into()),
            instance: InstanceId::generate(),
            capabilities: Capabilities::default(),
            build: None,
            target: None,
            challenge: None,
        }))
        .await;
    assert_eq!(stranger.recv().await, None, "the client hangs up on it");
    let mut maxd = rig.next_maxd().await;
    maxd.accept(InstanceId::generate(), "boot-1").await;
    let link = until_connected(&mut events).await;
    assert_eq!(link.route, ssh("devbox-tail"));
    assert_eq!(link.device, Some(device()));
    assert_eq!(
        rig.fake.dialed(),
        [ssh("devbox-lan"), ssh("devbox-lan"), ssh("devbox-tail")]
    );
}

/// A hello from a device other than the one the link is for.
async fn hello_from_another(maxd: &mut FakeMaxd) {
    assert!(matches!(maxd.recv().await, Some(ClientMessage::Hello(_))));
    maxd.send(&ServerMessage::Hello(ServerHello {
        protocol: ProtocolRange::OURS,
        maxd_version: "0.1.0".into(),
        device: DeviceId::derive("another-device"),
        boot_id: BootId("boot-9".into()),
        instance: InstanceId::generate(),
        capabilities: Capabilities::default(),
        build: None,
        target: None,
        challenge: None,
    }))
    .await;
}

#[tokio::test]
async fn a_link_to_a_device_refuses_another_from_its_first_hello() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    let (_handle, mut events) = rig.client.link_to(spec(ssh("devbox")), device());

    let mut stranger = rig.next_maxd().await;
    hello_from_another(&mut stranger).await;
    assert_eq!(stranger.recv().await, None, "the client hangs up on it");

    let reason = loop {
        match events.recv().await {
            Some(LinkEvent::Link(link)) => {
                assert_eq!(link.device, Some(device()), "it keeps the ID it was given");
                if let LinkState::Unreachable { reason, .. } = link.state {
                    break reason;
                }
            }
            Some(_) => {}
            None => panic!("the link closed"),
        }
    };
    assert_eq!(
        reason,
        "devbox reports a different device ID. If this device was reinstalled, \
         remove it and add it again to trust the new ID."
    );

    // The real device can still come back.
    let mut maxd = rig.next_maxd().await;
    maxd.accept(InstanceId::generate(), "boot-1").await;
    let link = until_connected(&mut events).await;
    assert_eq!(link.device, Some(device()));
}

#[tokio::test]
async fn a_link_to_a_device_reconnects_as_usual() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    let (_handle, mut events) = rig.client.link_to(spec(ssh("devbox")), device());
    let mut maxd = rig.next_maxd().await;
    maxd.accept(InstanceId::generate(), "boot-1").await;
    assert_eq!(until_connected(&mut events).await.device, Some(device()));
    drop(maxd);

    let mut maxd = rig.next_maxd().await;
    maxd.accept(InstanceId::generate(), "boot-1").await;
    assert_eq!(until_connected(&mut events).await.device, Some(device()));
}

#[tokio::test]
async fn a_device_with_one_route_never_reads_the_network() {
    let mut rig = Rig::new(NO_RETRY, Tailnet::default(), &[]).on_network(Some("home"));
    let (_handle, mut events) = rig.client.link(spec(ssh("devbox")));
    let mut maxd = rig.next_maxd().await;
    maxd.accept(InstanceId::generate(), "boot-1").await;
    let link = until_connected(&mut events).await;
    assert!(link.routes.is_empty());
    maxd.send(&snapshot(1, &[1])).await;
    // No route is recorded: the next event is the sync.
    assert!(matches!(next(&mut events).await, LinkEvent::Synced(_)));
}
