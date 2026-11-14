// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Pushes, as the phone asks for them. The phone keeps its push token and
//! the kinds it wants, and gives them to every device it links to, again
//! each time a link comes back. While the app is open it says which agent
//! it shows, every [`PRESENCE_EVERY`], so no device pushes about that
//! agent. Going to the background clears it at once.

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use maxhq_protocol::ids::{AgentId, IdempotencyKey, LinkId};
use maxhq_protocol::link::LinkState;
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::phones::PushToken;
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::ClientMessage;
use tokio::sync::Notify;

use super::{lock, Connector, Core, Devices, LinkControl};
use crate::merge::Merge;

/// How often presence goes out while an agent is on screen. A device
/// counts it for 45 seconds.
const PRESENCE_EVERY: Duration = Duration::from_millis(if cfg!(test) { 40 } else { 25_000 });

/// What the phone said about pushes.
#[derive(Default)]
pub(super) struct Push {
    state: Mutex<State>,
    /// Wakes the presence loop when what it sends changes.
    changed: Arc<Notify>,
    /// Lets one presence out at a time, so the last a device gets is the
    /// newest.
    turn: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct State {
    token: Option<PushToken>,
    /// Whether the app is open.
    foreground: bool,
    /// The agent the app shows, open or not.
    agent: Option<AgentId>,
    /// What presence last went out as.
    sent: Option<AgentId>,
    /// Whether the presence loop runs.
    looping: bool,
}

impl State {
    /// The agent presence should name now.
    fn shown(&self) -> Option<AgentId> {
        self.agent.filter(|_| self.foreground)
    }
}

impl<C: Connector> Devices<C> {
    /// Keeps the phone's push token and kinds, and gives them to every
    /// connected device that takes phones. The rest get them when their
    /// links come up.
    pub fn set_push_token(&self, push: PushToken) {
        lock(&self.core.push.state).token = Some(push);
        for id in self.core.phone_links() {
            self.core.give_token(id);
        }
    }

    /// Says which agent the app shows, if any.
    pub fn show_agent(&self, agent: Option<AgentId>) {
        self.core.set_presence(|state| state.agent = agent, true);
    }

    /// Says whether the app is open. Closing it clears presence on every
    /// connected device before this returns, so the links can go after.
    pub async fn set_foreground(&self, open: bool) {
        // Closing sends here, not from the loop, so it's out before the
        // links go.
        self.core
            .set_presence(|state| state.foreground = open, open);
        if !open {
            self.core.send_presence(None).await;
        }
    }
}

impl<L: LinkControl> Core<L> {
    /// The links up now to devices that take phones.
    fn phone_links(&self) -> Vec<LinkId> {
        let ids: Vec<_> = lock(&self.links).keys().copied().collect();
        self.view.read(|merge| {
            ids.into_iter()
                .filter(|id| takes_phones(merge, *id))
                .collect()
        })
    }

    /// Changes what presence says, and starts the loop that sends it the
    /// first time. The loop sends the change when `wake` says so.
    fn set_presence(self: &Arc<Self>, change: impl FnOnce(&mut State), wake: bool) {
        let start = {
            let mut state = lock(&self.push.state);
            change(&mut state);
            !std::mem::replace(&mut state.looping, true)
        };
        if start {
            let changed = Arc::clone(&self.push.changed);
            self.runtime
                .spawn(presence_loop(Arc::downgrade(self), changed));
        }
        if wake {
            self.push.changed.notify_one();
        }
    }

    /// Sends presence as it stands to every device that takes phones, or
    /// just to link `only`'s. Each send reads it once it has its turn, so
    /// one that waited never lands after a newer one.
    async fn send_presence(&self, only: Option<LinkId>) {
        let _turn = self.push.turn.lock().await;
        let shown = {
            let mut state = lock(&self.push.state);
            if only.is_none() {
                state.sent = state.shown();
            }
            state.shown()
        };
        let links = match only {
            Some(id) => vec![id],
            None => self.phone_links(),
        };
        for id in links {
            let Some(link) = self.link(id) else { continue };
            if let Err(error) = link.send(ClientMessage::Presence { agent: shown }).await {
                tracing::debug!(%id, %error, "a device didn't get the phone's presence");
            }
        }
    }

    /// Gives the device on link `id` the token, apart, so the reports
    /// keep coming.
    fn give_token(&self, id: LinkId) {
        let token = lock(&self.push.state).token.clone();
        let (Some(push), Some(link)) = (token, self.link(id)) else {
            return;
        };
        let action = ClientMessage::SetPushToken {
            key: IdempotencyKey::generate(),
            push,
        };
        self.runtime.spawn(async move {
            match link.act(action).await {
                Ok(ActionOutcome::Done) => {}
                Ok(outcome) => tracing::warn!(%id, ?outcome, "a device refused the push token"),
                Err(error) => tracing::debug!(%id, %error, "a device didn't get the push token"),
            }
        });
    }

    /// Catches a device that came up on link `id` up with the token and
    /// presence, if it takes phones.
    pub(super) fn phone_came_up(self: &Arc<Self>, id: LinkId) {
        if !self.view.read(|merge| takes_phones(merge, id)) {
            return;
        }
        self.give_token(id);
        if lock(&self.push.state).shown().is_none() {
            return;
        }
        let core = Arc::clone(self);
        self.runtime
            .spawn(async move { core.send_presence(Some(id)).await });
    }
}

/// Sends presence when it changes, and again every [`PRESENCE_EVERY`]
/// while it names an agent, until the devices go.
async fn presence_loop<L: LinkControl>(core: Weak<Core<L>>, changed: Arc<Notify>) {
    loop {
        let Some(strong) = core.upgrade() else { return };
        let (shown, due) = {
            let state = lock(&strong.push.state);
            (state.shown(), state.shown() != state.sent)
        };
        if due || shown.is_some() {
            strong.send_presence(None).await;
        }
        drop(strong);
        if shown.is_some() {
            let _ = tokio::time::timeout(PRESENCE_EVERY, changed.notified()).await;
        } else {
            changed.notified().await;
        }
    }
}

/// Whether link `id` is up to a device that takes phones, from its last
/// hello.
fn takes_phones(merge: &Merge, id: LinkId) -> bool {
    matches!(
        merge.store().get(&EntityKey::DeviceLink(id)),
        Some(Entity::DeviceLink(link))
            if matches!(link.state, LinkState::Connected { .. })
                && link.capabilities.is_some_and(|does| does.phones)
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use maxhq_client::{LinkEvent, LinkSpec};
    use maxhq_protocol::ids::AgentId;
    use maxhq_protocol::link::LinkState;
    use maxhq_protocol::message::Capabilities;
    use maxhq_protocol::phones::{PushEnvironment, PushKind, PushToken};
    use maxhq_protocol::secret::Secret;
    use maxhq_protocol::ClientMessage;

    use super::PRESENCE_EVERY;
    use crate::devices::tests::{harness, wait_until, Call, Harness};
    use crate::merge::tests::{connected, link_state, unreachable};
    use crate::registry::Registry;
    use crate::tests::block_on;

    fn token(kinds: Vec<PushKind>) -> PushToken {
        PushToken {
            token: Secret::new("test-not-a-real-token".into()),
            topic: "com.example.maxhq".into(),
            environment: PushEnvironment::Sandbox,
            kinds,
        }
    }

    /// Has `spec`'s link report `state`, from a `maxd` that takes phones
    /// when `phones` says so.
    fn report(h: &Harness, spec: &LinkSpec, state: LinkState, phones: bool) {
        let LinkEvent::Link(mut link) = link_state(spec, state) else {
            unreachable!("link_state reports a link");
        };
        link.capabilities = Some(Capabilities {
            phones,
            ..Capabilities::default()
        });
        h.report(spec.id, LinkEvent::Link(link));
    }

    /// The tokens and presences sent since the last call, by link.
    fn sent(h: &Harness) -> Vec<(maxhq_protocol::ids::LinkId, ClientMessage)> {
        h.fake
            .calls()
            .into_iter()
            .filter_map(|(id, call)| match call {
                Call::Send(
                    message @ (ClientMessage::SetPushToken { .. } | ClientMessage::Presence { .. }),
                ) => Some((id, message)),
                _ => None,
            })
            .collect()
    }

    fn kinds_of(message: &ClientMessage) -> Option<Vec<PushKind>> {
        match message {
            ClientMessage::SetPushToken { push, .. } => Some(push.kinds.clone()),
            _ => None,
        }
    }

    #[test]
    fn the_token_goes_to_every_device_that_takes_phones_and_again_when_one_comes_back() {
        let registry = Registry::in_memory().unwrap();
        let a = registry.add("dev@devbox").unwrap();
        let b = registry.add("dev@ws-alpha").unwrap();
        let c = registry.add("dev@ws-bravo").unwrap();
        let h = harness(registry, None);
        for spec in [&a, &b, &c] {
            h.fake.record().up.insert(spec.id);
        }
        report(&h, &a, connected(), true);
        report(&h, &c, connected(), false);

        // One device is up and takes phones, one is down, one doesn't.
        h.devices.set_push_token(token(vec![PushKind::NeedsInput]));
        wait_until("the token never went", || !h.fake.record().calls.is_empty());
        let given = sent(&h);
        assert_eq!(given.len(), 1);
        assert_eq!(given[0].0, a.id);
        assert_eq!(kinds_of(&given[0].1), Some(vec![PushKind::NeedsInput]));

        // The one that was down gets it once it's up.
        report(&h, &b, connected(), true);
        wait_until("the token never reached the second", || {
            !h.fake.record().calls.is_empty()
        });
        let given = sent(&h);
        assert_eq!(given.len(), 1);
        assert_eq!(given[0].0, b.id);

        // New kinds go to both, and a link that drops and comes back gets
        // the newest.
        let kinds = vec![PushKind::Error, PushKind::Finished];
        h.devices.set_push_token(token(kinds.clone()));
        wait_until("the kinds never went", || h.fake.record().calls.len() == 2);
        assert!(sent(&h)
            .iter()
            .all(|(_, message)| kinds_of(message) == Some(kinds.clone())));
        report(&h, &a, unreachable(), true);
        report(&h, &a, connected(), true);
        wait_until("the token never came back", || {
            !h.fake.record().calls.is_empty()
        });
        let given = sent(&h);
        assert_eq!(given.len(), 1);
        assert_eq!((given[0].0, kinds_of(&given[0].1)), (a.id, Some(kinds)));
    }

    #[test]
    fn presence_names_the_agent_on_screen_while_the_app_is_open() {
        let registry = Registry::in_memory().unwrap();
        let a = registry.add("dev@devbox").unwrap();
        let h = harness(registry, None);
        h.fake.record().up.insert(a.id);
        report(&h, &a, connected(), true);
        let agent = AgentId::derive("fix-the-merge");
        let presences = |h: &Harness| {
            sent(h)
                .into_iter()
                .filter_map(|(_, message)| match message {
                    ClientMessage::Presence { agent } => Some(agent),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

        // An agent on screen in the background says nothing.
        h.devices.show_agent(Some(agent));
        std::thread::sleep(PRESENCE_EVERY * 2);
        assert!(presences(&h).is_empty());

        // Open, it goes at once and again on the timer.
        block_on(h.devices.set_foreground(true));
        wait_until("presence never repeated", || {
            h.fake.record().calls.len() >= 3
        });
        let seen = presences(&h);
        assert!(seen.len() >= 3);
        assert!(seen.iter().all(|shown| *shown == Some(agent)));

        // Leaving the agent clears it, once.
        h.devices.show_agent(None);
        wait_until("presence never cleared", || {
            h.fake
                .record()
                .calls
                .iter()
                .any(|(_, call)| *call == Call::Send(ClientMessage::Presence { agent: None }))
        });
        std::thread::sleep(PRESENCE_EVERY * 2);
        assert_eq!(presences(&h).last(), Some(&None));
        std::thread::sleep(PRESENCE_EVERY * 2);
        assert!(presences(&h).is_empty());

        // Going to the background clears it before it returns.
        h.devices.show_agent(Some(agent));
        wait_until("presence never came back", || {
            !h.fake.record().calls.is_empty()
        });
        block_on(h.devices.set_foreground(false));
        assert_eq!(presences(&h).last(), Some(&None));
        std::thread::sleep(Duration::from_millis(100));
        assert!(presences(&h).iter().all(Option::is_none));
    }
}
