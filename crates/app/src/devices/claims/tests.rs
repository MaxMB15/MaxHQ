// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::time::{Duration, Instant};

use maxhq_client::{LinkEvent, LinkSpec};
use maxhq_protocol::ids::{AccountId, IdempotencyKey};
use maxhq_protocol::link::LinkState;
use maxhq_protocol::model::{Account, LoginKind, Vendor};
use maxhq_protocol::ClientMessage;

use super::*;
use crate::devices::tests::{harness, hello_as, Call, Harness};
use crate::merge::tests::{link_state, unreachable};
use crate::tests::block_on;

/// Two devices you added, neither yet known to reach a device, as a
/// registry from before links kept their device may open.
fn victim_and_hostile() -> (Harness, LinkSpec, LinkSpec) {
    let registry = Registry::in_memory().unwrap();
    let victim = registry.add("victim-host").unwrap();
    let hostile = registry.add("hostile-host").unwrap();
    (harness(registry, None), victim, hostile)
}

/// The view's line for `link`.
fn line(h: &Harness, link: LinkId) -> DeviceLink {
    match h
        .view
        .read(|merge| merge.store().get(&EntityKey::DeviceLink(link)).cloned())
    {
        Some(Entity::DeviceLink(line)) => line,
        other => panic!("the link isn't shown: {other:?}"),
    }
}

/// Waits until the view shows `link` as `check` wants it, as it does once
/// a reopened link has said hello.
fn until(h: &Harness, link: LinkId, check: impl Fn(&DeviceLink) -> bool) {
    let start = Instant::now();
    while !h.view.read(
        |merge| match merge.store().get(&EntityKey::DeviceLink(link)) {
            Some(Entity::DeviceLink(line)) => check(line),
            _ => false,
        },
    ) {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "the link never got there"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// The links an action for `device` went through.
fn went_through(h: &Harness, device: &str) -> Vec<LinkId> {
    let action = ClientMessage::WindDown {
        key: IdempotencyKey::generate(),
        window_secs: 300,
    };
    let _ = block_on(h.devices.act_on(DeviceId::derive(device), action));
    h.fake
        .calls()
        .into_iter()
        .filter(|(_, call)| matches!(call, Call::Send(ClientMessage::WindDown { .. })))
        .map(|(id, _)| id)
        .collect()
}

/// Has `spec` drop, still naming `device`.
fn drop_as(h: &Harness, spec: &LinkSpec, device: &str) {
    let LinkEvent::Link(mut link) = link_state(spec, unreachable()) else {
        unreachable!("link_state reports a link");
    };
    link.device = Some(DeviceId::derive(device));
    h.fake.record().up.remove(&spec.id);
    h.report(spec.id, LinkEvent::Link(link));
}

/// An account on `device`.
fn account_on(device: &str) -> Entity {
    Entity::Account(Account {
        id: AccountId::derive(device),
        device: DeviceId::derive(device),
        vendor: Vendor::Claude,
        label: "Personal".into(),
        config_dir: "~/.claude".into(),
        email: None,
        organization: None,
        plan: None,
        models: Vec::new(),
        usage: None,
        usage_failure: None,
        acp: None,
        login: LoginKind::ConfigDir,
        origin: None,
    })
}

#[test]
fn a_link_that_names_a_device_another_knows_gets_nothing_until_you_answer() {
    let (h, victim, hostile) = victim_and_hostile();
    let device = DeviceId::derive("victim");
    hello_as(&h, &victim, "victim", None);
    hello_as(&h, &hostile, "victim", None);

    let held = line(&h, hostile.id);
    assert_eq!(held.device, None, "it reaches no device meanwhile");
    let claim = held.claim.expect("it waits for your answer");
    assert_eq!(claim.device, device);
    assert_eq!(claim.known_by, Some(victim.id));
    assert_eq!(claim.state, LinkClaimState::Asking);
    let registry = h.devices.registry();
    assert_eq!(registry.known_device(victim.id).unwrap(), Some(device));
    assert_eq!(registry.known_device(hostile.id).unwrap(), None);
    drop(registry);

    // What it reports isn't merged as the device.
    h.report_quiet(hostile.id, LinkEvent::Synced(vec![account_on("victim")]));
    let account = EntityKey::Account(AccountId::derive("victim"));
    assert_eq!(
        h.view.read(|merge| merge.store().get(&account).cloned()),
        None
    );

    // Actions go to the real device, and with it off, to none.
    assert_eq!(went_through(&h, "victim"), [victim.id]);
    drop_as(&h, &victim, "victim");
    assert_eq!(
        went_through(&h, "victim"),
        [victim.id],
        "it tries the real one only"
    );
    let store = |key| ClientMessage::StoreClaudeToken {
        key,
        origin: "0199a7c2-0000-7000-8000-000000000005".parse().unwrap(),
        label: "Personal".into(),
        token: maxhq_protocol::secret::Secret::new("not-a-real-token".into()),
    };
    let outcomes = block_on(h.devices.act_on_each(&[device], store));
    assert!(outcomes[0].is_err(), "the victim is off: {outcomes:?}");
    let sent = h
        .fake
        .calls()
        .into_iter()
        .any(|(id, call)| id == hostile.id && matches!(call, Call::Send(_)));
    assert!(!sent, "nothing goes out on the held link");

    // It stays held when it drops and says hello again, and nothing pins
    // it.
    drop_as(&h, &hostile, "victim");
    hello_as(&h, &hostile, "victim", None);
    assert!(line(&h, hostile.id).claim.is_some());
    assert_eq!(h.devices.registry().known_device(hostile.id).unwrap(), None);
}

#[test]
fn a_second_route_you_say_is_the_same_device_reaches_it_too() {
    let (h, real, second) = victim_and_hostile();
    let device = DeviceId::derive("victim");
    hello_as(&h, &real, "victim", None);
    hello_as(&h, &second, "victim", None);
    assert!(line(&h, second.id).claim.is_some());

    h.devices.answer_claim(second.id, true).unwrap();
    assert_eq!(
        h.fake.record().opened_to[&second.id],
        Some(device),
        "it opens again for the device"
    );
    hello_as(&h, &second, "victim", None);
    until(&h, second.id, |line| line.device == Some(device));
    assert_eq!(line(&h, second.id).claim, None);

    // Either link reaches it.
    drop_as(&h, &real, "victim");
    assert_eq!(went_through(&h, "victim"), [second.id]);
    // There's nothing left to answer.
    assert!(h.devices.answer_claim(second.id, true).is_err());
}

#[test]
fn a_link_you_say_is_a_different_device_stays_held_until_you_ask_again() {
    let (h, victim, hostile) = victim_and_hostile();
    hello_as(&h, &victim, "victim", None);
    hello_as(&h, &hostile, "victim", None);
    until(&h, hostile.id, |line| line.claim.is_some());

    h.devices.answer_claim(hostile.id, false).unwrap();
    assert_eq!(h.fake.record().opened_to[&hostile.id], None);
    hello_as(&h, &hostile, "victim", None);
    until(&h, hostile.id, |line| {
        line.claim
            .as_ref()
            .is_some_and(|claim| claim.state == LinkClaimState::Different)
    });
    assert_eq!(line(&h, hostile.id).device, None);
    assert_eq!(went_through(&h, "victim"), [victim.id]);

    // Even naming a device nobody knows, it stays held.
    drop_as(&h, &hostile, "hostile");
    assert!(line(&h, hostile.id).claim.is_some());
    assert_eq!(line(&h, hostile.id).device, None);
    assert_eq!(h.devices.registry().known_device(hostile.id).unwrap(), None);

    h.devices.ask_claim_again(hostile.id).unwrap();
    hello_as(&h, &hostile, "victim", None);
    until(&h, hostile.id, |line| {
        line.claim
            .as_ref()
            .is_some_and(|claim| claim.state == LinkClaimState::Asking)
    });
}

#[test]
fn forgetting_a_links_identity_asks_again_at_its_next_hello() {
    let (h, real, second) = victim_and_hostile();
    let device = DeviceId::derive("victim");
    hello_as(&h, &real, "victim", None);
    hello_as(&h, &second, "victim", None);
    h.devices.answer_claim(second.id, true).unwrap();
    hello_as(&h, &second, "victim", None);
    until(&h, second.id, |line| line.device == Some(device));

    h.devices.forget_identity(second.id).unwrap();
    assert_eq!(h.fake.record().opened_to[&second.id], None);
    assert_eq!(h.devices.registry().known_device(second.id).unwrap(), None);
    hello_as(&h, &second, "victim", None);
    until(&h, second.id, |line| line.claim.is_some());
    assert_eq!(went_through(&h, "victim"), [real.id]);

    // A link alone with its device reaches it again without asking.
    h.devices.forget_identity(second.id).unwrap();
    h.devices.forget_identity(real.id).unwrap();
    hello_as(&h, &real, "victim", None);
    until(&h, real.id, |line| line.device == Some(device));
    assert_eq!(
        h.devices.registry().known_device(real.id).unwrap(),
        Some(device)
    );
    assert!(h.devices.forget_identity(local_link()).is_err());
}

#[test]
fn a_link_you_set_up_again_waits_before_its_device_takes_new_host_keys() {
    let (h, real, _) = victim_and_hostile();
    let device = DeviceId::derive("victim");
    hello_as(&h, &real, "victim", None);
    until(&h, real.id, |line| line.device == Some(device));

    h.devices.ask_about(real.id, device).unwrap();
    assert_eq!(h.fake.record().opened_to[&real.id], None);
    hello_as(&h, &real, "victim", None);
    until(&h, real.id, |line| line.claim.is_some());
    let claim = line(&h, real.id).claim.unwrap();
    assert_eq!((claim.device, claim.known_by), (device, None));
    assert_eq!(went_through(&h, "victim"), Vec::<LinkId>::new());

    h.devices.answer_claim(real.id, true).unwrap();
    hello_as(&h, &real, "victim", None);
    until(&h, real.id, |line| line.device == Some(device));
}

#[test]
fn this_macs_own_link_is_never_held() {
    let (h, victim, _) = victim_and_hostile();
    hello_as(&h, &victim, "victim", None);
    let local = h.local();
    let LinkEvent::Link(mut link) = link_state(
        &local,
        LinkState::Connected {
            since: Timestamp(1),
        },
    ) else {
        unreachable!("link_state reports a link");
    };
    link.device = Some(DeviceId::derive("victim"));
    h.report(local.id, LinkEvent::Link(link));
    let local_line = line(&h, local.id);
    assert_eq!(local_line.claim, None);
    assert!(matches!(local_line.state, LinkState::Connected { .. }));
}
