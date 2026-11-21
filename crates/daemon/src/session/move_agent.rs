// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Moving an agent to another device. The device that runs the MaxHQ app
//! runs each move, since it keeps the tasks and every other device takes
//! its requests: it checks the device the agent goes to, has the device
//! the agent leaves stop it and sum up its conversation, has the other
//! start a new agent from that summary, then has the first mark the old
//! agent moved and points the tasks at the new one. A move that fails
//! after the stop leaves the old agent stopped, so a message resumes it,
//! and unmarked.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use maxhq_core::agents::{arrival_mode, runnable, task_permission};
use maxhq_core::handoff::{self, Usage};
use maxhq_core::tools::agents::MAX_LIST;
use maxhq_core::{inbox, names};
use maxhq_protocol::agents::{
    Actor, AgentAt, AgentMoveStep, Arrival, Departure, MoveCheck, MoveTo, Moved, Permission,
    ToolAnswer,
};
use maxhq_protocol::ids::{AgentId, DeviceId, IdempotencyKey, PermissionId};
use maxhq_protocol::message::{ActionOutcome, AgentPlace, AgentSpec, ClientMessage};
use maxhq_protocol::model::{InboxChange, WorkspaceLocation};
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::tasks::{Maker, Task, TaskTarget};
use maxhq_protocol::timeline::ItemId;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tracing::warn;
use uuid::Uuid;

use super::agent_tools::{failure, Later as Answered, Reply, NO_CONN};
use super::held::{internal_key, Handoff};
use super::idem::{Outcome, Seen};
use super::inbox::Busy;
use super::lookups::Take;
use super::move_tool::{Asking, Looked};
use super::requests::usable_folder;
use super::{refused, stopping, Input, Manager, Sessions};
use crate::agents::{PeerCalls, Unreachable};
use crate::drive::ToolResult;
use crate::moves::vendor_name;

/// Where a step's outcome goes.
pub(super) type Done = oneshot::Sender<ActionOutcome>;

/// What a device other than the one that runs the MaxHQ app answers a
/// move with.
pub(super) const NOT_HOME: &str = "Only the device that runs the MaxHQ app moves agents.";

/// What a move to the device the agent is on answers.
pub(super) const ALREADY_THERE: &str = "The agent is on that device already.";

/// The note on what a device lists for a move to it.
const TARGETS_THERE: &str = "An agent that moves to this device runs under one of these \
                             accounts, in one of these workspaces or in a folder here. The \
                             agents listed run here, newest first, and can move elsewhere.";

/// The moves in flight.
#[derive(Default)]
pub(super) struct Moves {
    /// The agents this device moves now.
    moving: HashSet<AgentId>,
    /// The steps here that wait for a stop or a start to finish.
    waiting: HashMap<IdempotencyKey, Waiting>,
    /// The moves tools asked for that wait for your answer.
    pub(super) asking: Vec<Asking>,
}

impl Moves {
    /// When the next card nobody answers runs out.
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.asking.iter().map(|asking| asking.until).min()
    }
}

/// A step that waits for a stop or a start.
enum Waiting {
    /// The agent stops before it leaves.
    Leave { agent: AgentId, done: Done },
    /// The new agent starts. `from` asked for it.
    Arrive {
        arrival: Box<Arrival>,
        from: DeviceId,
        done: Done,
    },
}

/// What comes back to the manager for a move.
pub(super) enum Later {
    /// `from` asked this device for a step.
    Step {
        from: DeviceId,
        step: AgentMoveStep,
        done: Done,
    },
    /// A check looked for the folder an agent goes to, off the loop.
    Located {
        to: MoveTo,
        folder: Result<PathBuf, String>,
        done: Done,
    },
    /// Both devices said what they could about a move a tool asked for.
    Looked(Box<Looked>),
    /// A move this device ran ended, with the new agent or why not.
    Ran {
        key: IdempotencyKey,
        agent: AgentId,
        from: DeviceId,
        to: DeviceId,
        outcome: ActionOutcome,
    },
}

impl Sessions {
    /// Runs a step of a move that `from`, the device that runs the MaxHQ
    /// app, asked for. Device links call it.
    pub async fn peer_move(&self, from: DeviceId, step: AgentMoveStep) -> ActionOutcome {
        let (done, answer) = oneshot::channel();
        let later = Later::Step { from, step, done };
        if self.tx.send(Input::Move(Box::new(later))).is_err() {
            return refused(stopping().text);
        }
        answer.await.unwrap_or_else(|_| refused(stopping().text))
    }
}

impl Manager {
    /// Handles a client's move.
    pub(super) fn move_message(&mut self, conn: u64, message: ClientMessage) {
        let ClientMessage::MoveAgent {
            key,
            agent,
            from,
            to,
            stop,
        } = message
        else {
            return;
        };
        match self.keys.begin(key, conn) {
            Seen::New => {}
            Seen::Running => return,
            Seen::Done(outcome) => {
                self.tell(conn, outcome.message(key));
                return;
            }
        }
        if let Some(outcome) = self.move_agent(key, agent, from, to, stop) {
            self.finish(key, Outcome::Action(outcome));
        }
    }

    /// Moves `agent` from `from` to `to`, and answers `key` with the new
    /// agent once it runs, or with why it doesn't. `stop` stops a turn
    /// that runs or a prompt that waits; without it, such an agent stays.
    pub(super) fn move_agent(
        &mut self,
        key: IdempotencyKey,
        agent: AgentId,
        from: DeviceId,
        to: MoveTo,
        stop: bool,
    ) -> Option<ActionOutcome> {
        let home = self.device;
        if self.agent_tools.peers.home().unwrap_or(home) != home {
            return Some(refused(NOT_HOME));
        }
        if from == to.device {
            return Some(refused(ALREADY_THERE));
        }
        if !self.moves.moving.insert(agent) {
            return Some(refused("That agent is moving already."));
        }
        // The tasks that follow it keep their permissions here. The device
        // it goes to gets a copy, and the one it leaves gives its copy back.
        let follow: Vec<Task> = self
            .following(from, agent)
            .into_iter()
            .filter(|task| task.made_by == Maker::User)
            .collect();
        let grants = if to.device == home {
            Vec::new()
        } else {
            // A task's permission names only its target's device, so the
            // old agent stands in for the new one, whose id isn't known yet.
            let target = TaskTarget::Agent {
                device: to.device,
                agent,
            };
            follow
                .iter()
                .map(|task| {
                    let moved = Task {
                        target: target.clone(),
                        ..task.clone()
                    };
                    task_permission(&moved, home)
                })
                .collect()
        };
        let revoke = if from == home {
            Vec::new()
        } else {
            follow.iter().map(|task| task.permission).collect()
        };
        let mover = self.mover();
        let tx = self.tx.clone();
        let device = to.device;
        tokio::spawn(async move {
            let outcome = mover.run(agent, from, to, stop, grants, revoke).await;
            let later = Later::Ran {
                key,
                agent,
                from,
                to: device,
                outcome,
            };
            let _ = tx.send(Input::Move(Box::new(later)));
        });
        None
    }

    /// The tasks kept here that target `agent` on `from`, which follow it
    /// when it moves.
    pub(super) fn following(&self, from: DeviceId, agent: AgentId) -> Vec<Task> {
        self.tasks_targeting(&TaskTarget::Agent {
            device: from,
            agent,
        })
    }

    /// Runs the steps of a move from this device.
    pub(super) fn mover(&self) -> Mover {
        Mover {
            here: self.device,
            tx: self.tx.clone(),
            peers: self.agent_tools.peers.clone(),
        }
    }

    /// Answers a call with what `device` lists for a move to it: its
    /// accounts, workspaces and agents. Asked off the loop.
    pub(super) fn targets_there(&self, reply: Reply, device: DeviceId) {
        let mover = self.mover();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = match mover.step(device, AgentMoveStep::Targets).await {
                ActionOutcome::Tool { answer } if !answer.failed => {
                    Ok(ToolResult::Text(answer.text))
                }
                ActionOutcome::Tool { answer } => Err(answer.text),
                other => Err(failure(&other)),
            };
            let later = Answered::Answer {
                reply,
                result,
                release: None,
            };
            let _ = tx.send(Input::Agents(Box::new(later)));
        });
    }

    /// Handles what came back for a move.
    pub(super) fn move_later(&mut self, later: Later) {
        match later {
            Later::Step { from, step, done } => self.move_step(from, step, done),
            Later::Looked(looked) => self.move_looked(*looked),
            Later::Located { to, folder, done } => {
                let outcome = match folder {
                    Ok(path) => self.move_checked(&to, Some(path)),
                    Err(reason) => refused(reason),
                };
                let _ = done.send(outcome);
            }
            Later::Ran {
                key,
                agent,
                from,
                to,
                outcome,
            } => {
                self.moves.moving.remove(&agent);
                if let ActionOutcome::Started { agent: new } = outcome {
                    self.retarget(
                        AgentAt {
                            device: from,
                            agent,
                        },
                        AgentAt {
                            device: to,
                            agent: new,
                        },
                    );
                }
                self.finish(key, Outcome::Action(outcome));
            }
        }
    }

    /// Takes a stop or a start a step waited for.
    pub(super) fn move_done(&mut self, key: IdempotencyKey, outcome: &Outcome) {
        let Some(waiting) = self.moves.waiting.remove(&key) else {
            return;
        };
        let outcome = match outcome {
            Outcome::Action(outcome) => outcome.clone(),
            Outcome::Answer(_) => refused("MaxHQ couldn't do that."),
        };
        match (waiting, outcome) {
            (Waiting::Leave { agent, done }, ActionOutcome::Done) => {
                self.depart_device(agent, done)
            }
            (
                Waiting::Arrive {
                    arrival,
                    from,
                    done,
                },
                ActionOutcome::Started { agent },
            ) => {
                self.arrived_here(agent, &arrival, from);
                let _ = done.send(ActionOutcome::Started { agent });
            }
            (Waiting::Leave { done, .. } | Waiting::Arrive { done, .. }, outcome) => {
                let _ = done.send(outcome);
            }
        }
    }

    /// Runs a step `from` asked for here.
    fn move_step(&mut self, from: DeviceId, step: AgentMoveStep, done: Done) {
        match step {
            AgentMoveStep::Targets => {
                let mut targets = self.device_targets(TARGETS_THERE);
                // An agent that moved away already can't move again.
                let agents: Vec<Value> = self
                    .newest_leads(|agent| {
                        !self.is_dispatcher(agent.id)
                            && agent.moved.as_ref().is_none_or(|moved| moved.to.is_none())
                    })
                    .iter()
                    .take(MAX_LIST)
                    .map(|agent| self.card(agent))
                    .collect();
                targets["agents"] = Value::Array(agents);
                let answer = ToolAnswer {
                    text: targets.to_string(),
                    failed: false,
                    images: Vec::new(),
                };
                let _ = done.send(ActionOutcome::Tool { answer });
            }
            AgentMoveStep::Check { to } => self.check_move(to, done),
            AgentMoveStep::Look { agent } => {
                let outcome = self.kept_from_leaving(agent, false).unwrap_or_else(|| {
                    self.departure(agent)
                        .map_or(ActionOutcome::UnknownAgent, |departure| {
                            ActionOutcome::Departed {
                                departure: Box::new(departure),
                            }
                        })
                });
                let _ = done.send(outcome);
            }
            AgentMoveStep::Leave { agent, stop } => self.leave_device(agent, stop, done),
            AgentMoveStep::Arrive { arrival } => self.arrive_here(from, arrival, done),
            AgentMoveStep::Settle { agent, to, revoke } => {
                let _ = done.send(self.settle_moved(from, agent, to, &revoke));
            }
        }
    }

    /// Checks that an agent can start here as `to` says, without starting
    /// it. A folder is looked for off the loop.
    fn check_move(&mut self, to: MoveTo, done: Done) {
        let path = match &to.place {
            AgentPlace::Folder { path } => PathBuf::from(path),
            AgentPlace::Workspace { .. } => {
                let _ = done.send(self.move_checked(&to, None));
                return;
            }
        };
        let hub = self.hub.clone();
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let folder = usable_folder(&hub, &path).map(|()| path);
            let later = Later::Located { to, folder, done };
            let _ = tx.send(Input::Move(Box::new(later)));
        });
    }

    /// What the agent will run as here, or why it can't. `folder` is the
    /// folder place, once found.
    fn move_checked(&self, to: &MoveTo, folder: Option<PathBuf>) -> ActionOutcome {
        let spec = match self.spec_here(to) {
            Ok(spec) => spec,
            Err(reason) => return refused(reason),
        };
        let folder = folder.map(|path| {
            let location = WorkspaceLocation::Folder {
                path: path.to_string_lossy().into_owned(),
            };
            (path, names::workspace(self.device, &location))
        });
        let info = match self.prepare(&spec, folder) {
            Ok((info, _)) => info,
            Err(reason) => return refused(reason),
        };
        if Some(info.workspace) == self.dispatcher.home() {
            return refused("An agent can't run in the dispatcher's own folder.");
        }
        let account = match self.hub.get(&EntityKey::Account(to.account)) {
            Some(Entity::Account(account)) => account,
            _ => return refused(NO_ACCOUNT),
        };
        let model = to
            .model
            .clone()
            .or_else(|| {
                account
                    .models
                    .iter()
                    .find(|choice| choice.default)
                    .map(|choice| choice.label.clone())
            })
            .unwrap_or_else(|| "its default model".to_owned());
        let place = match info.guest {
            Some(_) => "a new worktree in its container".to_owned(),
            None => info.cwd.display().to_string(),
        };
        let check = MoveCheck {
            device: self.hub.device().name.clone(),
            vendor: spec.vendor,
            account: account.label,
            model,
            place,
            workspace: info.workspace,
            mode: Some(runnable(spec.vendor, self.default_mode())),
            container: info.guest.map(|guest| self.workspace_name(guest)),
        };
        ActionOutcome::Checked {
            check: Box::new(check),
        }
    }

    /// The start of an agent that moves here as `to` says, with no name,
    /// mode or effort yet.
    fn spec_here(&self, to: &MoveTo) -> Result<AgentSpec, String> {
        if to.device != self.device {
            return Err("That move is for another device.".to_owned());
        }
        let account = match self.hub.get(&EntityKey::Account(to.account)) {
            Some(Entity::Account(account)) if account.device == self.device => account,
            _ => return Err(NO_ACCOUNT.to_owned()),
        };
        if let Some(model) = &to.model
            && !account.models.is_empty()
            && !account.models.iter().any(|choice| choice.id == *model)
        {
            return Err(format!(
                "{} doesn't offer {model} under that account.",
                vendor_name(account.vendor)
            ));
        }
        Ok(AgentSpec {
            vendor: account.vendor,
            account: to.account,
            place: to.place.clone(),
            model: to.model.clone(),
            permission: None,
            title: None,
            first: None,
            branch: None,
            origin: None,
            effort: None,
        })
    }

    /// Stops an agent that leaves, unless it's busy and `stop` isn't set,
    /// then sums its conversation up.
    fn leave_device(&mut self, agent: AgentId, stop: bool, done: Done) {
        if let Some(outcome) = self.kept_from_leaving(agent, stop) {
            let _ = done.send(outcome);
            return;
        }
        let key = internal_key();
        // The stop may finish at once, so the step waits for it first.
        self.moves
            .waiting
            .insert(key, Waiting::Leave { agent, done });
        self.message(NO_CONN, ClientMessage::StopAgent { key, agent });
    }

    /// Why an agent can't leave now, if it can't.
    fn kept_from_leaving(&self, agent: AgentId, stop: bool) -> Option<ActionOutcome> {
        let Some(found) = self.hub.reported(agent) else {
            return Some(ActionOutcome::UnknownAgent);
        };
        let reason = match self.held.get(&agent) {
            None => "Only an agent MaxHQ runs moves, and a sub-agent moves with its lead.",
            Some(_) if self.is_dispatcher(agent) => "MaxHQ's dispatcher stays on its device.",
            Some(held) if held.moved.as_ref().is_some_and(|moved| moved.to.is_some()) => {
                "That agent moved already."
            }
            Some(_) if stop => return None,
            Some(_) => match self.busy(&found)? {
                Busy::NeedsInput => {
                    "The agent waits for your answer. Answer it, or stop it and move it."
                }
                Busy::Works => {
                    "The agent is working. Wait for its turn to end, or stop it and move it."
                }
                Busy::Queued => {
                    "A message waits for the agent. Take it back or let it go, or stop the \
                     agent and move it."
                }
            },
        };
        Some(refused(reason))
    }

    /// What an agent would take along if it left, less the summary.
    fn departure(&self, agent: AgentId) -> Option<Departure> {
        let held = self.held.get(&agent)?;
        Some(Departure {
            device: self.hub.device().name.clone(),
            title: held
                .title
                .clone()
                .unwrap_or_else(|| held.agent.title.clone()),
            mode: held.mode,
            effort: held.effort.clone(),
            brief: None,
            container: held.guest.map(|guest| self.workspace_name(guest)),
        })
    }

    /// Sums up the conversation of an agent that stopped to leave, off the
    /// loop, and answers with what it takes along.
    fn depart_device(&mut self, agent: AgentId, done: Done) {
        let Some(departure) = self.departure(agent) else {
            let _ = done.send(ActionOutcome::UnknownAgent);
            return;
        };
        let read = self.reading(agent, &Take::All).map(|(_, from)| from);
        let here = self.device;
        tokio::spawn(async move {
            let items = match read {
                Ok(from) => from.items(agent).await,
                Err(reason) => Err(reason),
            };
            let outcome = match items {
                Ok(items) => {
                    let budget = handoff::budget("", &[], Usage::default());
                    let brief = handoff::deliver_moved(agent, here, &items, budget);
                    ActionOutcome::Departed {
                        departure: Box::new(Departure { brief, ..departure }),
                    }
                }
                Err(reason) => refused(format!(
                    "The agent stopped, but its conversation couldn't be read to carry over: {} \
                     It stays here, and a message resumes it.",
                    sentence(&reason)
                )),
            };
            let _ = done.send(outcome);
        });
    }

    /// Starts the agent that moves here, from what it took along.
    fn arrive_here(&mut self, from: DeviceId, arrival: Box<Arrival>, done: Done) {
        let mut spec = match self.spec_here(&arrival.to) {
            Ok(spec) => spec,
            Err(reason) => {
                let _ = done.send(refused(reason));
                return;
            }
        };
        let departure = &arrival.departure;
        // This device's default decides, so the mode the device it left
        // reports can't raise the agent above it unless you chose that.
        spec.permission = Some(arrival_mode(
            departure.mode,
            runnable(spec.vendor, self.default_mode()),
            arrival.to.mode,
            spec.vendor,
        ));
        spec.effort.clone_from(&departure.effort);
        spec.title = Some(departure.title.clone());
        let key = internal_key();
        // The start may finish at once, so the step waits for it first.
        self.moves.waiting.insert(
            key,
            Waiting::Arrive {
                arrival,
                from,
                done,
            },
        );
        self.message(NO_CONN, ClientMessage::StartAgent { key, spec });
    }

    /// Marks an agent that started here as moved from where it was, has
    /// its first message start from the summary, and keeps the copies of
    /// the permissions of the tasks that follow it.
    fn arrived_here(&mut self, agent: AgentId, arrival: &Arrival, from: DeviceId) {
        if let Some(held) = self.held.get_mut(&agent) {
            held.moved = Some(Box::new(Moved {
                to: None,
                from: Some(arrival.from),
            }));
            // The lines around the summary are written here, so the device
            // it left only fills in what goes between them.
            held.handoff = arrival.departure.brief.as_deref().map(|brief| Handoff {
                marker: ItemId(format!("maxhq-moved-{}", Uuid::now_v7())),
                since: None,
                context: None,
                brief: Some(handoff::moved(
                    arrival.from.agent,
                    arrival.from.device,
                    brief,
                )),
            });
        }
        self.save(agent);
        self.publish(agent);
        for permission in &arrival.grants {
            if !is_task_copy(permission, from) {
                continue;
            }
            if let ActionOutcome::Refused { reason } = self.grant(permission.clone()) {
                warn!(%reason, permission = %permission.id, "couldn't keep a moved task's permission");
            }
        }
    }

    /// Marks an agent that left moved to `to`, settles it, and takes back
    /// the permissions of the tasks that left with it.
    fn settle_moved(
        &mut self,
        from: DeviceId,
        agent: AgentId,
        to: AgentAt,
        revoke: &[PermissionId],
    ) -> ActionOutcome {
        let Some(held) = self.held.get_mut(&agent) else {
            return ActionOutcome::UnknownAgent;
        };
        held.moved.get_or_insert_with(Box::default).to = Some(to);
        self.save(agent);
        self.publish(agent);
        // It stopped when it left, so nothing it does keeps it from
        // settling.
        let settled = inbox::apply(
            &self.hub.inbox(agent),
            &InboxChange::Settle,
            self.clock.wall(),
        );
        self.write_inbox(agent, settled);
        self.stop_settled(agent);
        for id in revoke {
            let ours = self
                .agent_tools
                .permissions
                .get(id)
                .is_some_and(|permission| is_task_copy(permission, from));
            if ours && let ActionOutcome::Refused { reason } = self.revoke(*id) {
                warn!(%reason, permission = %id, "couldn't take back a moved task's permission");
            }
        }
        ActionOutcome::Done
    }

    /// Points the tasks kept here that target `from` at `to`.
    fn retarget(&mut self, from: AgentAt, to: AgentAt) {
        let old = TaskTarget::Agent {
            device: from.device,
            agent: from.agent,
        };
        for task in self.tasks_targeting(&old) {
            let id = task.id;
            let moved = Task {
                target: TaskTarget::Agent {
                    device: to.device,
                    agent: to.agent,
                },
                ..task
            };
            if let Err(reason) = self.keep_task(moved) {
                warn!(%reason, task = %id, "couldn't point a task at the moved agent");
            }
        }
    }
}

/// What a device answers when the account a move names isn't there.
const NO_ACCOUNT: &str = "That account isn't on that device.";

/// Whether `permission` is a copy of a task's permission that `home`
/// keeps, the only kind a move hands over.
fn is_task_copy(permission: &Permission, home: DeviceId) -> bool {
    matches!(permission.holder, Actor::Task { .. }) && permission.home == home
}

/// Runs a move's steps in order, off the loop, asking each device for its
/// step: this one through the manager, others over their links.
pub(super) struct Mover {
    here: DeviceId,
    tx: mpsc::UnboundedSender<Input>,
    peers: Arc<dyn PeerCalls>,
}

impl Mover {
    /// What `device` answers a step with.
    pub(super) async fn step(&self, device: DeviceId, step: AgentMoveStep) -> ActionOutcome {
        if device != self.here {
            return self
                .peers
                .move_step(device, step)
                .await
                .unwrap_or_else(|Unreachable(reason)| refused(reason));
        }
        let (done, answer) = oneshot::channel();
        let later = Later::Step {
            from: self.here,
            step,
            done,
        };
        if self.tx.send(Input::Move(Box::new(later))).is_err() {
            return refused(stopping().text);
        }
        answer.await.unwrap_or_else(|_| refused(stopping().text))
    }

    /// Checks the device the agent goes to, stops it where it is, starts
    /// the new one, and marks the old one moved. Answers with the new
    /// agent.
    async fn run(
        self,
        agent: AgentId,
        from: DeviceId,
        to: MoveTo,
        stop: bool,
        grants: Vec<Permission>,
        revoke: Vec<PermissionId>,
    ) -> ActionOutcome {
        let check = AgentMoveStep::Check { to: to.clone() };
        match self.step(to.device, check).await {
            ActionOutcome::Checked { .. } => {}
            other => return refused(failure(&other)),
        }
        let departure = match self.step(from, AgentMoveStep::Leave { agent, stop }).await {
            ActionOutcome::Departed { departure } => departure,
            other => return refused(failure(&other)),
        };
        let arrival = Arrival {
            to: to.clone(),
            from: AgentAt {
                device: from,
                agent,
            },
            departure: *departure,
            grants,
        };
        let arrive = AgentMoveStep::Arrive {
            arrival: Box::new(arrival),
        };
        let new = match self.step(to.device, arrive).await {
            ActionOutcome::Started { agent } => agent,
            other => {
                return refused(format!(
                    "The agent stopped, but the new one didn't start there: {} It stays where \
                     it was, and a message resumes it.",
                    sentence(&failure(&other))
                ));
            }
        };
        let settle = AgentMoveStep::Settle {
            agent,
            to: AgentAt {
                device: to.device,
                agent: new,
            },
            revoke,
        };
        let settled = self.step(from, settle).await;
        if !matches!(settled, ActionOutcome::Done) {
            warn!(%agent, reason = failure(&settled), "the moved agent couldn't be marked moved");
        }
        ActionOutcome::Started { agent: new }
    }
}

/// `reason` as a sentence, ending in a full stop.
fn sentence(reason: &str) -> String {
    let reason = reason.trim_end();
    if reason.ends_with(['.', '!', '?']) {
        reason.to_owned()
    } else {
        format!("{reason}.")
    }
}
