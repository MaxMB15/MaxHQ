// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The state the windows show: every device's entities merged with the
//! app's own links and networks, under one sequence of numbered deltas.
//!
//! A window loads [`Snapshot`] once with the `view_snapshot` request and
//! then applies each delta in order. A gap in the numbers means it missed
//! one, and it loads the snapshot again. Deltas go out in batches, at most
//! one a frame, so a page streaming many agents renders once a frame.

use std::collections::BTreeMap;
use std::pin::pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use maxhq_protocol::ids::AgentId;
use maxhq_protocol::state::{Delta, Snapshot};
use serde::Deserialize;
use specta::Type;
use tokio::sync::Notify;

use crate::merge::Merge;
use crate::notify::{NotificationSender, Notifier, Panes};
use crate::pace::Paced;
use crate::registry::Queued;

/// What one window's panes show, as its page reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Type)]
pub struct Shown {
    /// The agent whose pane has focus. Only a window with focus has one.
    pub focused: Option<AgentId>,
    /// Every agent whose pane shows: the active pane of each group, docked
    /// or floating.
    pub visible: Vec<AgentId>,
}

/// Where each batch of deltas goes, oldest first. The desktop app emits it
/// to its windows.
pub type Emit = Box<dyn Fn(Vec<Delta>) + Send + Sync>;

/// The merged state. Clones share it.
#[derive(Clone)]
pub struct View {
    shared: Arc<Shared>,
}

/// What every clone of the view shares.
struct Shared {
    state: Mutex<State>,
    /// What goes out to `emit`, a batch at a time.
    outbox: Paced<Delta>,
    /// Wakes whoever waits on the state after each change.
    changed: Notify,
}

/// The state behind the view's lock.
struct State {
    merge: Merge,
    notifier: Notifier,
    /// What each window last said it shows, by its label.
    windows: BTreeMap<String, Shown>,
    /// What every window shows together, which the notifier goes by.
    panes: Panes,
}

impl View {
    /// An empty view that sends its changes to `emit` and its alerts to
    /// `sender`.
    pub fn new(emit: Emit, sender: Box<dyn NotificationSender>) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    merge: Merge::new(),
                    notifier: Notifier::new(sender),
                    windows: BTreeMap::new(),
                    panes: Panes::default(),
                }),
                outbox: Paced::start("view", emit),
                changed: Notify::new(),
            }),
        }
    }

    /// The whole view now.
    pub fn snapshot(&self) -> Snapshot {
        self.lock().merge.store().snapshot()
    }

    /// Records what a window's panes show. A prompt stays quiet for the
    /// agent whose pane has focus, and the other alerts for any agent a
    /// window shows.
    pub fn set_shown(&self, window: &str, shown: Shown) {
        let mut state = self.lock();
        // Only one window has focus at a time, so the newest to say it has
        // one wins over a report that hasn't caught up yet.
        if shown.focused.is_some() {
            for other in state.windows.values_mut() {
                other.focused = None;
            }
        }
        state.windows.insert(window.to_owned(), shown);
        state.combine();
    }

    /// Forgets what a window showed, once it closes.
    pub fn forget_window(&self, window: &str) {
        let mut state = self.lock();
        if state.windows.remove(window).is_some() {
            state.combine();
        }
    }

    /// Reads the merged state.
    pub fn read<R>(&self, f: impl FnOnce(&Merge) -> R) -> R {
        f(&self.lock().merge)
    }

    /// Changes the merged state. The delta it makes, if any, goes to the
    /// notifier and then to the windows' outbox. Both happen under the lock,
    /// so deltas go out in the order they were made.
    pub fn update(&self, f: impl FnOnce(&mut Merge) -> Option<Delta>) {
        let mut state = self.lock();
        let State {
            merge,
            notifier,
            panes,
            ..
        } = &mut *state;
        let Some(delta) = f(merge) else {
            return;
        };
        notifier.observe(&delta, merge.store(), panes);
        self.shared.outbox.push(delta);
        self.shared.changed.notify_waiters();
    }

    /// Waits until `check` finds what it looks for, reading the merged
    /// state now and again after each change.
    pub async fn wait_for<R>(&self, mut check: impl FnMut(&Merge) -> Option<R>) -> R {
        loop {
            // Listening before the read means a change in between still
            // wakes it.
            let mut changed = pin!(self.shared.changed.notified());
            changed.as_mut().enable();
            if let Some(found) = self.read(&mut check) {
                return found;
            }
            changed.await;
        }
    }

    /// Says that `device` refused a change the outbox held for it, and why.
    pub fn refused(&self, queued: &Queued, device: &str, reason: &str) {
        let state = self.lock();
        crate::notify::refused(&state.notifier, queued, device, reason, state.merge.store());
    }

    /// Locks the state. Every update is applied whole, so a panic elsewhere
    /// can't leave it half-written.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl State {
    /// Puts every window's panes together and tells the notifier.
    fn combine(&mut self) {
        self.panes = Panes {
            focused: self.windows.values().find_map(|shown| shown.focused),
            visible: self
                .windows
                .values()
                .flat_map(|shown| shown.visible.iter().copied())
                .collect(),
        };
        self.notifier.look(&self.panes);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::merge::tests::{agent, connected, link_state, spec, unreachable};
    use crate::notify::tests::FakeSender;
    use maxhq_client::LinkEvent;
    use maxhq_protocol::ids::AttentionId;
    use maxhq_protocol::model::Activity;
    use maxhq_protocol::state::Seq;

    /// A view that records each delta it emits and what it notifies.
    pub(crate) fn recording() -> (View, Arc<Mutex<Vec<Delta>>>, FakeSender) {
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let sink = emitted.clone();
        let sender = FakeSender::default();
        let view = View::new(
            Box::new(move |deltas| sink.lock().unwrap().extend(deltas)),
            Box::new(sender.clone()),
        );
        sender.take();
        (view, emitted, sender)
    }

    /// Waits until `count` deltas went out, and returns their numbers.
    fn sent(emitted: &Mutex<Vec<Delta>>, count: usize) -> Vec<Seq> {
        let start = std::time::Instant::now();
        loop {
            let seqs: Vec<Seq> = emitted.lock().unwrap().iter().map(|d| d.seq).collect();
            if seqs.len() >= count {
                return seqs;
            }
            assert!(
                start.elapsed() < std::time::Duration::from_secs(5),
                "only {seqs:?} went out"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn every_change_goes_out_once_in_order() {
        let (view, emitted, _) = recording();
        let nl = spec("nl");
        view.update(|merge| merge.add_link(&nl));
        view.update(|merge| merge.link_event(nl.id, link_state(&nl, connected())));
        view.update(|merge| merge.link_event(nl.id, link_state(&nl, connected())));
        assert_eq!(view.snapshot().seq, Seq(2), "a repeat makes no delta");
        view.update(|merge| merge.link_event(nl.id, link_state(&nl, unreachable())));
        assert_eq!(sent(&emitted, 3), [Seq(1), Seq(2), Seq(3)]);
    }

    #[test]
    fn a_link_that_drops_doesnt_alert_and_its_return_does() {
        let (view, _, sender) = recording();
        let nl = spec("nl");
        view.update(|merge| merge.add_link(&nl));
        view.update(|merge| merge.link_event(nl.id, link_state(&nl, connected())));
        view.update(|merge| {
            merge.link_event(
                nl.id,
                LinkEvent::Synced(vec![agent("a", "w", Activity::Busy)]),
            )
        });
        view.update(|merge| merge.link_event(nl.id, link_state(&nl, unreachable())));
        assert!(sender.take().is_empty());

        view.update(|merge| merge.link_event(nl.id, link_state(&nl, connected())));
        view.update(|merge| {
            merge.link_event(
                nl.id,
                LinkEvent::Synced(vec![agent("a", "w", Activity::DoneUnread)]),
            )
        });
        assert_eq!(
            sender.delivered().len(),
            1,
            "the turn ended while it was down"
        );

        view.set_shown("main", shown(Some("a"), &["a"]));
        assert_eq!(sender.take().len(), 1, "focusing the pane clears it");
    }

    /// Sets the agents on a test's one device.
    type SetAgents = Box<dyn Fn(&[(&str, Activity)])>;

    /// A view with one connected device, a call that sets its agents, and
    /// what it notifies.
    fn with_agents() -> (View, SetAgents, FakeSender) {
        let (view, _, sender) = recording();
        let nl = spec("nl");
        view.update(|merge| merge.add_link(&nl));
        view.update(|merge| merge.link_event(nl.id, link_state(&nl, connected())));
        let set: SetAgents = {
            let view = view.clone();
            Box::new(move |agents: &[(&str, Activity)]| {
                let agents = agents
                    .iter()
                    .map(|(name, activity)| agent(name, "w", activity.clone()))
                    .collect();
                view.update(|merge| merge.link_event(nl.id, LinkEvent::Synced(agents)));
            })
        };
        (view, set, sender)
    }

    fn shown(focused: Option<&str>, visible: &[&str]) -> Shown {
        Shown {
            focused: focused.map(AgentId::derive),
            visible: visible.iter().copied().map(AgentId::derive).collect(),
        }
    }

    #[test]
    fn what_two_windows_show_adds_up() {
        let (view, set, sender) = with_agents();
        set(&[("a", Activity::Busy), ("b", Activity::Busy)]);
        view.set_shown("main", shown(None, &["a"]));
        view.set_shown("other", shown(None, &["b"]));
        set(&[("a", Activity::DoneUnread), ("b", Activity::DoneUnread)]);
        assert!(sender.take().is_empty(), "each window shows one of them");

        // Once a window stops showing its agent, that agent alerts again.
        view.set_shown("main", shown(None, &[]));
        set(&[("a", Activity::Busy), ("b", Activity::Busy)]);
        set(&[("a", Activity::DoneUnread), ("b", Activity::DoneUnread)]);
        assert_eq!(
            sender.delivered(),
            [format!("done:{}", AgentId::derive("a"))]
        );

        // And so does one whose window closed.
        view.forget_window("other");
        set(&[("a", Activity::Busy), ("b", Activity::Busy)]);
        set(&[("a", Activity::Idle), ("b", Activity::DoneUnread)]);
        assert_eq!(
            sender.delivered(),
            [format!("done:{}", AgentId::derive("b"))]
        );
    }

    #[test]
    fn the_window_that_last_took_focus_has_the_focused_pane() {
        let (view, set, sender) = with_agents();
        set(&[("a", Activity::Busy), ("b", Activity::Busy)]);
        view.set_shown("main", shown(Some("a"), &["a"]));
        view.set_shown("other", shown(Some("b"), &["b"]));

        // The main window's blur hasn't come in, but the other window has
        // focus now, so a prompt in the main window's pane alerts.
        let (first, second) = (AttentionId::derive("1"), AttentionId::derive("2"));
        set(&[
            ("a", Activity::NeedsInput { attention: first }),
            ("b", Activity::NeedsInput { attention: second }),
        ]);
        assert_eq!(sender.delivered(), [format!("needs-input:{first}")]);
    }
}
