// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Workflow runs: each window's opens go to a link to the run's device, and
//! a run whose link drops moves to another link to the same device, which
//! starts it over with a reset.

use std::sync::Arc;

use maxhq_protocol::ids::IdempotencyKey;
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::workflow::{NodeDetail, RunKey};
use maxhq_protocol::ClientMessage;

use super::{
    connected, device_targets, failed, gone, sentence, Connector, Core, Devices, LinkControl,
};
use crate::fleet::{check_can, Needs};
use crate::request::CommandError;
use crate::timeline::Sink;
use crate::workflow::{Close, WorkflowMessage};

impl<C: Connector> Devices<C> {
    /// Has a window watch a workflow run through its channel. Every open
    /// starts with a reset from the run's device.
    pub async fn open_workflow(
        &self,
        window: &str,
        sink: Arc<dyn Sink<WorkflowMessage>>,
        run: RunKey,
    ) -> Result<(), CommandError> {
        self.core.open_workflow(window, sink, run).await
    }

    /// Takes back one of a window's opens of a run. The run closes on its
    /// link once no window watches it.
    pub async fn close_workflow(&self, window: &str, run: &RunKey) {
        if let Some(close) = self.core.workflows.unsubscribe(window, run) {
            let _order = self.core.order.lock().await;
            self.core.close_runs(vec![close]).await;
        }
    }

    /// Asks the run's device for more about one of its nodes.
    pub async fn node_detail(&self, run: RunKey, node: String) -> Result<NodeDetail, CommandError> {
        let device = run.device;
        self.core
            .view
            .read(|merge| check_can(merge, device, Needs::Workflows))?;
        let action = ClientMessage::NodeDetail {
            key: IdempotencyKey::generate(),
            run,
            node,
        };
        match self.act_on(device, action).await? {
            ActionOutcome::NodeDetail { detail } => Ok(detail),
            ActionOutcome::Refused { reason } => Err(failed(reason)),
            outcome => Err(failed(format!("An unexpected answer: {outcome:?}"))),
        }
    }
}

impl<L: LinkControl> Core<L> {
    /// Subscribes a window to a run and opens it on the run's link, all in
    /// turn with the other changes.
    async fn open_workflow(
        &self,
        window: &str,
        sink: Arc<dyn Sink<WorkflowMessage>>,
        run: RunKey,
    ) -> Result<(), CommandError> {
        let _order = self.order.lock().await;
        let targets = self.view.read(|merge| {
            check_can(merge, run.device, Needs::Workflows)?;
            device_targets(merge, run.device)
        })?;
        let Some(first) = targets.iter().find(|(id, _)| self.link(*id).is_some()) else {
            return Err(gone("This device"));
        };
        let plan = self.workflows.subscribe(window, sink, run.clone(), first.0);
        self.close_runs(plan.close).await;
        let Some(link) = self.link(plan.open) else {
            self.workflows.unsubscribe(window, &run);
            return Err(gone("This device"));
        };
        if let Err(error) = link.open_workflow(run.clone()).await {
            // Only a link that closed fails, and nothing opened.
            self.workflows.unsubscribe(window, &run);
            return Err(failed(sentence(error.to_string())));
        }
        Ok(())
    }

    /// Closes opens no window holds any more. Call it with the order lock
    /// held.
    pub(super) async fn close_runs(&self, closes: Vec<Close>) {
        for (id, run) in closes {
            let Some(link) = self.link(id) else {
                continue;
            };
            if let Err(error) = link.close_workflow(run).await {
                tracing::debug!(%id, %error, "a link didn't close a workflow run");
            }
        }
    }

    /// Moves each run whose link isn't connected to a connected link to the
    /// same device, one open for each it held. Call it with the order lock
    /// held.
    pub(super) async fn relink_runs(&self) {
        for (run, from, opens) in self.workflows.watched() {
            let to = self.view.read(|merge| {
                if connected(merge, from) {
                    return None;
                }
                let targets = device_targets(merge, run.device).ok()?;
                let mut ids = targets.into_iter().map(|(id, _)| id);
                ids.find(|id| *id != from && connected(merge, *id))
            });
            let Some((to, link)) = to.and_then(|to| self.link(to).map(|link| (to, link))) else {
                continue;
            };
            if !self.workflows.relink(&run, from, to) {
                continue;
            }
            for _ in 0..opens {
                if let Err(error) = link.open_workflow(run.clone()).await {
                    tracing::debug!(%to, %error, "a link didn't open a workflow run");
                }
            }
            let closes = std::iter::repeat_n((from, run.clone()), opens);
            self.close_runs(closes.collect()).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use maxhq_client::LinkEvent;
    use maxhq_protocol::ids::{DeviceId, LinkId, ProjectId};
    use maxhq_protocol::message::Capabilities;
    use maxhq_protocol::workflow::{RunPatch, WorkflowUpdate};

    use super::*;
    use crate::devices::tests::{
        connect_doing, disconnect, harness, two_devices, wait_until, Call, Harness,
    };
    use crate::registry::Registry;
    use crate::tests::block_on;
    use crate::timeline::tests::Recorder;

    fn run(device: &str) -> RunKey {
        RunKey {
            device: DeviceId::derive(device),
            project: ProjectId::derive("p"),
            workflow: "build".to_owned(),
            run: "r1".to_owned(),
        }
    }

    fn open(device: &str) -> Call {
        Call::Send(ClientMessage::OpenWorkflow { run: run(device) })
    }

    fn close(device: &str) -> Call {
        Call::Send(ClientMessage::CloseWorkflow { run: run(device) })
    }

    fn patch(rev: u64) -> WorkflowUpdate {
        WorkflowUpdate::Patch(RunPatch {
            key: run("nl"),
            rev,
            nodes: Vec::new(),
            loops: None,
            header: None,
        })
    }

    fn tell(h: &Harness, link: LinkId, update: WorkflowUpdate) {
        let events = h.fake.record().events[&link].clone();
        let event = LinkEvent::Workflow {
            run: run("nl"),
            update: Box::new(update),
        };
        block_on(events.send(event)).unwrap();
    }

    #[test]
    fn each_open_asks_the_runs_device_and_each_close_takes_one_back() {
        let (h, _, nl) = two_devices();
        let (main, other) = (Recorder::new(1), Recorder::new(2));
        block_on(h.devices.open_workflow("main", main.clone(), run("nl"))).unwrap();
        block_on(h.devices.open_workflow("main", main.clone(), run("nl"))).unwrap();
        block_on(h.devices.open_workflow("other", other.clone(), run("nl"))).unwrap();
        // Every open asks for a reset, so a window that joins late gets the
        // whole run.
        assert_eq!(
            h.fake.calls(),
            [
                (nl.id, open("nl")),
                (nl.id, open("nl")),
                (nl.id, open("nl"))
            ]
        );
        assert_eq!(
            block_on(h.devices.open_workflow("main", main.clone(), run("gone"))),
            Err(gone("This device"))
        );

        tell(&h, nl.id, patch(2));
        let wanted = [WorkflowMessage {
            run: run("nl"),
            update: patch(2),
        }];
        wait_until("the patch never came", || {
            main.messages() == wanted && other.messages() == wanted
        });

        block_on(h.devices.close_workflow("main", &run("nl")));
        block_on(h.devices.release_window("main")).unwrap();
        block_on(h.devices.close_workflow("other", &run("nl")));
        block_on(h.devices.close_workflow("other", &run("nl")));
        assert_eq!(
            h.fake.calls(),
            [
                (nl.id, close("nl")),
                (nl.id, close("nl")),
                (nl.id, close("nl"))
            ]
        );
    }

    #[test]
    fn a_maxd_from_before_workflows_is_refused_in_words() {
        let (h, _, nl) = two_devices();
        let detail = block_on(h.devices.node_detail(run("nl"), "build".to_owned()));
        assert_eq!(detail, Ok(NodeDetail { lists: Vec::new() }));
        let asked = h.fake.calls();
        assert!(matches!(
            &asked[..],
            [(id, Call::Send(ClientMessage::NodeDetail { node, .. }))] if *id == nl.id && node == "build"
        ));

        connect_doing(&h, &nl, "nl", Capabilities::default());
        let refused =
            failed("devbox's maxd is too old to report its workflows. Update it first".to_owned());
        let window = Recorder::new(1);
        assert_eq!(
            block_on(h.devices.open_workflow("main", window, run("nl"))),
            Err(refused.clone())
        );
        assert_eq!(
            block_on(h.devices.node_detail(run("nl"), "build".to_owned())),
            Err(refused)
        );
        assert!(h.fake.calls().is_empty());
    }

    #[test]
    fn a_run_moves_to_another_link_when_its_own_drops() {
        let registry = Registry::in_memory().unwrap();
        let tailnet = registry.add("devbox").unwrap();
        let lan = registry.add("devbox.lan").unwrap();
        // Both routes are known to reach the device, as after you said so.
        for spec in [&tailnet, &lan] {
            registry
                .keep_device(spec.id, DeviceId::derive("nl"))
                .unwrap();
        }
        let h = harness(registry, None);
        let does = Capabilities {
            workflows: true,
            ..Capabilities::default()
        };
        connect_doing(&h, &tailnet, "nl", does);
        connect_doing(&h, &lan, "nl", does);
        let window = Recorder::new(1);
        block_on(h.devices.open_workflow("main", window.clone(), run("nl"))).unwrap();
        let calls = h.fake.calls();
        let [(first, _)] = calls[..] else {
            panic!("one open went out: {calls:?}");
        };
        let (dropped, second) = if first == lan.id {
            (&lan, tailnet.id)
        } else {
            (&tailnet, lan.id)
        };

        disconnect(&h, dropped, "nl");
        wait_until("the run never moved", || h.fake.record().calls.len() == 2);
        assert_eq!(h.fake.calls(), [(second, open("nl")), (first, close("nl"))]);
        tell(&h, first, patch(2));
        tell(&h, second, patch(3));
        let wanted = [WorkflowMessage {
            run: run("nl"),
            update: patch(3),
        }];
        wait_until("the patch never came", || window.messages() == wanted);
    }
}
