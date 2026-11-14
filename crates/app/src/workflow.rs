// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The workflow channel. Each window gets one, which carries the updates of
//! every workflow run the window has open. Each open is one open on the
//! run's link, and every open asks `maxd` for a fresh reset, so a window
//! that joins late gets the whole run without the app holding it. Every
//! window watching a run gets every update, a reset included, and a page
//! whose revisions skip one opens the run again to start over.
//!
//! Updates go out as they come. `maxd` sends at most one patch a frame for
//! each run, so they need no batching here.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use maxhq_protocol::ids::LinkId;
use maxhq_protocol::workflow::{RunKey, WorkflowUpdate};
use serde::Serialize;
use specta::Type;

use crate::timeline::Sink;

/// One workflow update, as a window's channel carries it.
#[derive(Clone, Debug, PartialEq, Serialize, Type)]
pub struct WorkflowMessage {
    /// The run that changed.
    pub run: RunKey,
    /// What changed.
    pub update: WorkflowUpdate,
}

/// One open of a run that no window holds any more, which its link should
/// close.
pub type Close = (LinkId, RunKey);

/// What a window opening a run asks of the links.
#[derive(Debug, PartialEq)]
pub struct Plan {
    /// The link to open the run on: the one it comes through already, or
    /// the one given when no window watched it.
    pub open: LinkId,
    /// Opens the window held through the channel it had before it
    /// reloaded. Close these first.
    pub close: Vec<Close>,
}

/// Every window's workflow channel. Clones share it.
#[derive(Clone, Default)]
pub struct Workflows(Arc<Mutex<Hub>>);

#[derive(Default)]
struct Hub {
    windows: BTreeMap<String, Window>,
    /// The link each watched run comes through.
    runs: HashMap<RunKey, LinkId>,
    /// Opens no window holds since a window's channel failed, until their
    /// link closes them.
    orphans: Vec<Close>,
}

/// One window's channel.
struct Window {
    sink: Arc<dyn Sink<WorkflowMessage>>,
    /// How many times the window opened each run, less the times it closed
    /// it.
    runs: HashMap<RunKey, usize>,
}

impl Workflows {
    /// Has a window watch a run, which comes through `link` if no window
    /// watches it yet. Each call needs its open on the plan's link.
    pub fn subscribe(
        &self,
        window: &str,
        sink: Arc<dyn Sink<WorkflowMessage>>,
        run: RunKey,
        link: LinkId,
    ) -> Plan {
        let mut hub = self.hub();
        let mut close = Vec::new();
        // A window that reloaded without letting go has a new channel, and
        // what it watched before is gone.
        if hub
            .windows
            .get(window)
            .is_some_and(|held| held.sink.id() != sink.id())
        {
            close = hub.release(window);
        }
        let held = hub
            .windows
            .entry(window.to_owned())
            .or_insert_with(|| Window {
                sink,
                runs: HashMap::new(),
            });
        *held.runs.entry(run.clone()).or_default() += 1;
        let open = *hub.runs.entry(run).or_insert(link);
        Plan { open, close }
    }

    /// Takes one of a window's opens of a run back. Returns the open to
    /// close, unless the window doesn't hold one.
    pub fn unsubscribe(&self, window: &str, run: &RunKey) -> Option<Close> {
        self.hub().unsubscribe(window, run)
    }

    /// Forgets a window that closed. Returns the opens it held.
    pub fn release(&self, window: &str) -> Vec<Close> {
        self.hub().release(window)
    }

    /// Passes an update a link reported to every window watching the run.
    /// A window whose channel fails is forgotten, and its opens wait in
    /// [`Workflows::orphans`].
    pub fn deliver(&self, link: LinkId, run: RunKey, update: WorkflowUpdate) {
        let mut hub = self.hub();
        if hub.runs.get(&run) != Some(&link) {
            return;
        }
        let message = WorkflowMessage { run, update };
        let failed: Vec<String> = hub
            .windows
            .iter()
            .filter(|(_, window)| window.runs.contains_key(&message.run))
            .filter(|(_, window)| !window.sink.send(vec![message.clone()]))
            .map(|(label, _)| label.clone())
            .collect();
        for label in failed {
            tracing::debug!(window = %label, "a window's workflow channel stopped taking updates");
            let closes = hub.release(&label);
            hub.orphans.extend(closes);
        }
    }

    /// Every watched run, with its link and how many opens it holds there.
    pub fn watched(&self) -> Vec<(RunKey, LinkId, usize)> {
        let hub = self.hub();
        let runs = hub.runs.iter();
        runs.map(|(run, link)| (run.clone(), *link, hub.opens(run)))
            .collect()
    }

    /// Has a run come through `to` instead of `from`, if it comes through
    /// `from` now, and says whether it moved.
    pub fn relink(&self, run: &RunKey, from: LinkId, to: LinkId) -> bool {
        let mut hub = self.hub();
        match hub.runs.get_mut(run) {
            Some(link) if *link == from => {
                *link = to;
                true
            }
            _ => false,
        }
    }

    /// Takes the opens on `link` that no window holds since a window's
    /// channel stopped taking updates.
    pub fn orphans(&self, link: LinkId) -> Vec<RunKey> {
        let mut hub = self.hub();
        let (theirs, rest) = std::mem::take(&mut hub.orphans)
            .into_iter()
            .partition(|(on, _)| *on == link);
        hub.orphans = rest;
        theirs.into_iter().map(|(_, run)| run).collect()
    }

    fn hub(&self) -> MutexGuard<'_, Hub> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Hub {
    fn unsubscribe(&mut self, label: &str, run: &RunKey) -> Option<Close> {
        let window = self.windows.get_mut(label)?;
        let opens = window.runs.get_mut(run)?;
        *opens -= 1;
        if *opens == 0 {
            window.runs.remove(run);
        }
        let link = self.runs.get(run).copied()?;
        if self.opens(run) == 0 {
            self.runs.remove(run);
        }
        Some((link, run.clone()))
    }

    fn release(&mut self, label: &str) -> Vec<Close> {
        let Some(window) = self.windows.remove(label) else {
            return Vec::new();
        };
        let mut closes = Vec::new();
        for (run, opens) in window.runs {
            let Some(link) = self.runs.get(&run).copied() else {
                continue;
            };
            closes.extend(std::iter::repeat_n((link, run.clone()), opens));
            if self.opens(&run) == 0 {
                self.runs.remove(&run);
            }
        }
        closes
    }

    /// How many opens every window holds of a run.
    fn opens(&self, run: &RunKey) -> usize {
        let windows = self.windows.values();
        windows.filter_map(|window| window.runs.get(run)).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    use maxhq_protocol::ids::{DeviceId, ProjectId};
    use maxhq_protocol::workflow::RunPatch;

    use crate::timeline::tests::Recorder;

    fn run(name: &str) -> RunKey {
        RunKey {
            device: DeviceId::derive("devbox"),
            project: ProjectId::derive("git:github.com/owner/repo"),
            workflow: "build".to_owned(),
            run: name.to_owned(),
        }
    }

    fn link(name: &str) -> LinkId {
        LinkId::derive(name)
    }

    fn patch(name: &str, rev: u64) -> WorkflowUpdate {
        WorkflowUpdate::Patch(RunPatch {
            key: run(name),
            rev,
            nodes: Vec::new(),
            loops: None,
            header: None,
        })
    }

    #[test]
    fn windows_share_a_run_on_its_first_link_and_each_open_closes() {
        let hub = Workflows::default();
        let (main, other) = (Recorder::new(1), Recorder::new(2));
        let plan = hub.subscribe("main", main.clone(), run("a"), link("devbox"));
        assert_eq!(
            plan,
            Plan {
                open: link("devbox"),
                close: Vec::new()
            }
        );
        // A later window opens on the link the run comes through.
        let plan = hub.subscribe("other", other.clone(), run("a"), link("peer"));
        assert_eq!(plan.open, link("devbox"));

        hub.deliver(link("devbox"), run("a"), patch("a", 2));
        // Nothing comes from a link the run doesn't come through.
        hub.deliver(link("peer"), run("a"), patch("a", 3));
        let message = WorkflowMessage {
            run: run("a"),
            update: patch("a", 2),
        };
        assert_eq!(main.messages(), vec![message.clone()]);
        assert_eq!(other.messages(), vec![message]);

        assert_eq!(
            hub.unsubscribe("main", &run("a")),
            Some((link("devbox"), run("a")))
        );
        assert_eq!(hub.unsubscribe("main", &run("a")), None);
        assert_eq!(hub.watched(), vec![(run("a"), link("devbox"), 1)]);
        assert_eq!(hub.release("other"), vec![(link("devbox"), run("a"))]);
        assert!(hub.watched().is_empty());
    }

    #[test]
    fn a_reloaded_window_closes_what_its_old_channel_held() {
        let hub = Workflows::default();
        hub.subscribe("main", Recorder::new(1), run("a"), link("devbox"));
        hub.subscribe("main", Recorder::new(1), run("a"), link("devbox"));
        let plan = hub.subscribe("main", Recorder::new(2), run("b"), link("devbox"));
        assert_eq!(plan.close, vec![(link("devbox"), run("a")); 2]);
        assert_eq!(hub.watched(), vec![(run("b"), link("devbox"), 1)]);
    }

    #[test]
    fn a_failed_channel_leaves_its_opens_for_the_link_to_close() {
        let hub = Workflows::default();
        let main = Recorder::new(1);
        hub.subscribe("main", main.clone(), run("a"), link("devbox"));
        main.broken.store(true, Ordering::SeqCst);
        hub.deliver(link("devbox"), run("a"), patch("a", 2));
        assert!(hub.watched().is_empty());
        assert!(hub.orphans(link("peer")).is_empty());
        assert_eq!(hub.orphans(link("devbox")), vec![run("a")]);
        assert!(hub.orphans(link("devbox")).is_empty());
    }

    #[test]
    fn a_run_moves_only_from_the_link_it_comes_through() {
        let hub = Workflows::default();
        let main = Recorder::new(1);
        hub.subscribe("main", main.clone(), run("a"), link("devbox"));
        assert!(!hub.relink(&run("a"), link("peer"), link("other")));
        assert!(hub.relink(&run("a"), link("devbox"), link("peer")));
        hub.deliver(link("devbox"), run("a"), patch("a", 2));
        hub.deliver(link("peer"), run("a"), patch("a", 3));
        assert_eq!(
            main.messages(),
            vec![WorkflowMessage {
                run: run("a"),
                update: patch("a", 3)
            }]
        );
    }
}
