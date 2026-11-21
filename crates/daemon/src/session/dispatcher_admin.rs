// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The dispatcher acting directly. A change the user asked for that the
//! risk table in [`maxhq_core::tools::admin`] calls safe runs at once,
//! through the same keyed request the app sends, and leaves a card on the
//! dispatcher's timeline with an Undo button. Any other change joins the
//! plan under the bar, and the few it never makes are refused. With the
//! device's "Act directly" switch off, every change joins the plan. A
//! change on another device goes there as a peer call, which only that
//! device's home may send.

use std::collections::{HashMap, VecDeque};
use std::future::Future;

use maxhq_core::agents::NOT_ALLOWED;
use maxhq_core::tools::admin::{self, Risk, Wants};
use maxhq_core::tools::dispatch::MAX_ITEMS;
use maxhq_core::tools::Call;
use maxhq_protocol::agents::{
    self, Actor, LeaveContainer, Limits, PeerCall, Permission, ToolAnswer,
};
use maxhq_protocol::dispatch::{AdminAction, Plan, PlanItem, PlanStep};
use maxhq_protocol::ids::{AgentId, DeviceId, IdempotencyKey, PermissionId};
use maxhq_protocol::message::{ActionOutcome, ClientMessage};
use maxhq_protocol::model::{InboxChange, Vendor, WorkspaceLocation};
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::timeline::{ItemBody, ItemId, ItemStatus, TimelineEvent, TimelineItem};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::warn;
use uuid::Uuid;

use super::agent_tools::{failure, Reply, NO_CONN};
use super::dispatcher::Waited;
use super::idem::Outcome;
use super::{refused, Input, Manager};

/// How many changes Undo may still put back, newest kept.
const KEEP_UNDO: usize = 50;

/// The peer tool a home's dispatcher changes another device with.
pub(super) const ACT_THERE: &str = "dispatcher_act";

/// Why Undo can't put back a change something else changed since.
const CHANGED: &str = "It changed since, so it can't be undone.";

/// Why Undo can't find a change, such as after a restart.
const GONE: &str = "That change can't be undone any more.";

/// What the dispatcher hears for a change it never makes.
const NEVER: &str = "The user changes that in Settings, not you.";

/// What the dispatcher hears for a change that waits for the user.
const ASKS_FIRST: &str = "It waits in the plan under the bar until the user confirms it.";

/// What the dispatcher hears for a change made at once that Undo can put
/// back.
const UNDOABLE: &str = "Done. The user can undo it from the card under your answer.";

/// What the dispatcher keeps of the changes it makes at once.
#[derive(Default)]
pub(super) struct Admin {
    /// Each change that runs now, by its request's key.
    running: HashMap<IdempotencyKey, Running>,
    /// Each start, message or task made at once, by its request's key, and
    /// who hears how it went.
    steps: HashMap<IdempotencyKey, (Waited, Reply)>,
    /// What Undo may still put back, oldest first.
    undo: VecDeque<Undo>,
    /// How many changes the request now made at once.
    pub(super) made: usize,
    /// The changes the request now put in the plan, which a plan it
    /// submits keeps.
    pub(super) added: Vec<PlanItem>,
    /// The last plan's number when the request now came. A plan numbered
    /// after it grows with each change that waits.
    pub(super) since: u32,
}

/// A change that runs now.
struct Running {
    /// The device it changes.
    device: DeviceId,
    /// What it does.
    action: AdminAction,
    /// What puts it back, read before it ran, or none.
    inverse: Option<AdminAction>,
    /// What it left, as another device said.
    after: Option<AdminAction>,
    /// What it changed, by name as it was.
    target: String,
    /// Who hears how it went.
    then: Then,
}

/// Who hears how a change went.
pub(super) enum Then {
    /// The dispatcher's call, which `lead` made.
    Tool { reply: Reply, lead: AgentId },
    /// A home's dispatcher on another device.
    Peer(Reply),
    /// The client that asked for an undo, under its key, and what the undo
    /// puts back.
    Undo {
        key: IdempotencyKey,
        record: Box<Undo>,
    },
    /// A confirmed plan, which hears through its item's key.
    Plan,
}

/// A change Undo may put back.
pub(super) struct Undo {
    /// The key on its card.
    key: IdempotencyKey,
    /// The device it changed.
    device: DeviceId,
    /// What puts it back.
    inverse: AdminAction,
    /// What it left. Something else changed it since once that's no longer
    /// so.
    after: AdminAction,
}

/// How a change that ran off the loop went.
pub(super) enum Acted {
    /// Here.
    Here(ActionOutcome),
    /// On another device.
    There(Result<ToolAnswer, String>),
}

/// What another device answers for a change it made.
#[derive(Default, Serialize, Deserialize)]
struct Made {
    #[serde(default)]
    inverse: Option<AdminAction>,
    #[serde(default)]
    after: Option<AdminAction>,
    #[serde(default)]
    target: String,
}

/// What a home's dispatcher sends another device.
#[derive(Deserialize)]
struct There {
    action: AdminAction,
    /// What the field must hold now, for an undo.
    #[serde(default)]
    expect: Option<AdminAction>,
}

/// Whether a request went.
pub(super) fn succeeded(outcome: &ActionOutcome) -> bool {
    !matches!(
        outcome,
        ActionOutcome::Refused { .. } | ActionOutcome::UnknownAgent
    )
}

impl Manager {
    /// Runs a dispatcher call that changes something: at once when it's
    /// safe, `asked` says the user asked for it and the switch lets it,
    /// into the plan when the user should confirm it, or not at all.
    pub(super) fn dispatcher_act(&mut self, lead: AgentId, reply: Reply, asked: bool, call: Call) {
        let checked = admin::wants(call).and_then(|wants| match wants {
            Wants::Step(step) => Ok((self.plan_step(step)?, false)),
            Wants::Act(act) => {
                let device = act.device.unwrap_or(self.device);
                let settings = self.device_settings();
                let here = (device == self.device).then_some(&*settings.local_models);
                let (action, busy) = self.check_act(device, act.filled(here)?)?;
                Ok((PlanStep::Admin { device, action }, busy))
            }
        });
        let (step, busy) = match checked {
            Ok(checked) => checked,
            Err(reason) => return self.reply_now(reply, Err(reason)),
        };
        let risk = admin::risk(&step, busy);
        let acts = self.device_settings().dispatcher_acts;
        let made = self.dispatcher.admin.made;
        match admin::decide(risk, asked, acts, made) {
            Risk::Never => self.reply_now(reply, Err(NEVER.into())),
            Risk::Confirm => {
                let why = if !acts {
                    "Act directly is off in the dispatcher's settings."
                } else if !asked {
                    "The dispatcher suggests it."
                } else if made >= admin::MAX_AT_ONCE {
                    "One request made enough changes at once."
                } else {
                    "It asks first."
                };
                let answer = self.add_to_plan(lead, step, why);
                self.reply_now(reply, answer);
            }
            Risk::Direct => {
                self.dispatcher.admin.made += 1;
                let key = IdempotencyKey::generate();
                match step {
                    PlanStep::Admin { device, action } => {
                        self.admin_start(key, device, action, None, Then::Tool { reply, lead });
                    }
                    step => match super::dispatcher::plan_message(self.device, key, step, &[]) {
                        Ok((message, waited)) => {
                            self.dispatcher.admin.steps.insert(key, (waited, reply));
                            self.message(NO_CONN, message);
                        }
                        Err(reason) => self.reply_now(reply, Err(reason)),
                    },
                }
            }
        }
    }

    /// Adds a change to the plan under the bar: to the one this request
    /// made, or a new one. Each change gives the plan a new number, so a
    /// confirm of what the user saw before refuses.
    fn add_to_plan(&mut self, lead: AgentId, step: PlanStep, why: &str) -> Result<String, String> {
        if self.dispatcher.run.is_some() {
            return Err("The last plan is still running. Try again once it's done.".into());
        }
        let since = self.dispatcher.admin.since;
        let shown = match &self.dispatcher.plan {
            Some(plan) if plan.id > since => plan.items.len(),
            _ => 0,
        };
        if shown >= MAX_ITEMS {
            return Err(format!(
                "The plan already has {MAX_ITEMS} items, the most it holds."
            ));
        }
        let item = PlanItem {
            reason: why.into(),
            step,
        };
        self.dispatcher.admin.added.push(item.clone());
        let mut items = match self.dispatcher.plan.take() {
            Some(plan) if plan.id > since => plan.items,
            _ => Vec::new(),
        };
        items.push(item);
        self.dispatcher.plans += 1;
        self.dispatcher.plan = Some(Plan {
            id: self.dispatcher.plans,
            items,
        });
        self.dispatcher.agent = Some(lead);
        self.publish_dispatcher();
        Ok(ASKS_FIRST.into())
    }

    /// Checks a change on `device` and fills in what it leaves out, with
    /// whether the agent it names is busy. One on another device counts as
    /// busy, since this one can't tell, and that device checks it again.
    fn check_act(
        &self,
        device: DeviceId,
        action: AdminAction,
    ) -> Result<(AdminAction, bool), String> {
        use AdminAction as A;
        let action = match action {
            A::GrantPermission {
                agent,
                devices,
                depth,
            } if devices.is_empty() => A::GrantPermission {
                agent,
                devices: vec![device],
                depth,
            },
            action => action,
        };
        let waits_on_turn = matches!(
            action,
            A::SetMode { .. }
                | A::SetModel { .. }
                | A::SetEffort { .. }
                | A::Interrupt { .. }
                | A::StopAgent { .. }
        );
        if device != self.device {
            let book = self.db.peer_book().ok().flatten().unwrap_or_default();
            if !book.peers.iter().any(|peer| peer.device == device) {
                return Err(format!("There's no device with id {device}."));
            }
            return Ok((action, waits_on_turn));
        }
        let mut busy = false;
        match &action {
            A::RenameAgent { agent, .. }
            | A::SetInbox { agent, .. }
            | A::MarkRead { agent }
            | A::SetMode { agent, .. }
            | A::SetModel { agent, .. }
            | A::SetEffort { agent, .. }
            | A::Interrupt { agent }
            | A::StopAgent { agent }
            | A::DeleteAgent { agent }
            | A::GrantPermission { agent, .. }
            | A::RevokePermission { agent } => {
                let found = self
                    .hub
                    .reported(*agent)
                    .filter(|_| !self.is_dispatcher(*agent))
                    .ok_or_else(|| format!("There's no agent here with id {agent}."))?;
                busy = waits_on_turn && self.busy(&found).is_some();
                let holds = self.permission_of(*agent).is_some();
                if matches!(action, A::GrantPermission { .. }) && holds {
                    return Err("That agent can already run agents. Revoke that first.".into());
                }
                if matches!(action, A::RevokePermission { .. }) && !holds {
                    return Err("That agent can't run agents, so there's nothing to revoke.".into());
                }
            }
            A::HideProject { project, .. } | A::SetProject { project, .. } => {
                if self.hub.get(&EntityKey::Project(*project)).is_none() {
                    return Err(format!("There's no project with id {project}."));
                }
            }
            A::StartWorkspace { workspace } | A::StopWorkspace { workspace } => {
                let here = matches!(
                    self.hub.get(&EntityKey::Workspace(*workspace)),
                    Some(Entity::Workspace(found)) if found.device == device
                );
                if !here || Some(*workspace) == self.dispatcher.home() {
                    return Err(format!("There's no workspace here with id {workspace}."));
                }
            }
            A::SetTask { task, .. } | A::RunTask { task } | A::DeleteTask { task } => {
                if self.task_name(*task).is_none() {
                    return Err(format!("There's no task here with id {task}."));
                }
            }
            A::ChangeSettings { .. } | A::UpdateTools { .. } => {}
        }
        Ok((action, busy))
    }

    /// The permission an agent holds, if any.
    fn permission_of(&self, agent: AgentId) -> Option<PermissionId> {
        let holder = Actor::Agent { agent };
        self.agent_tools
            .permissions
            .values()
            .find(|permission| permission.holder == holder)
            .map(|permission| permission.id)
    }

    /// Starts a change under `key`, here or on `device`, and tells `then`
    /// how it went. `expect` is what the field must hold there now, for an
    /// undo on another device.
    pub(super) fn admin_start(
        &mut self,
        key: IdempotencyKey,
        device: DeviceId,
        action: AdminAction,
        expect: Option<AdminAction>,
        then: Then,
    ) {
        let here = device == self.device;
        let running = Running {
            device,
            inverse: if here { self.now(&action) } else { None },
            target: if here {
                self.target_of(&action)
            } else {
                String::new()
            },
            action: action.clone(),
            after: None,
            then,
        };
        self.dispatcher.admin.running.insert(key, running);
        if here {
            self.act_here(key, action);
        } else {
            self.act_there(key, device, action, expect);
        }
    }

    /// Sends a change to the request the app sends for it.
    fn act_here(&mut self, key: IdempotencyKey, action: AdminAction) {
        use AdminAction as A;
        let device = self.device;
        let message = match action {
            A::RenameAgent { agent, title } => ClientMessage::RenameAgent { key, agent, title },
            A::SetInbox { agent, change } => ClientMessage::SetInbox { key, agent, change },
            A::MarkRead { agent } => {
                let at = self.hub.mark_seen(agent);
                if let Err(err) = self.db.mark_seen(agent, at) {
                    warn!(%err, %agent, "couldn't store when the agent was seen");
                }
                return self.act_now(key, ActionOutcome::Done);
            }
            A::SetMode { agent, mode } => ClientMessage::SetPermission { key, agent, mode },
            A::SetModel { agent, model } => ClientMessage::SetModel {
                key,
                agent,
                model,
                account: None,
            },
            A::SetEffort { agent, effort } => ClientMessage::SetEffort { key, agent, effort },
            A::Interrupt { agent } => ClientMessage::Interrupt { key, agent },
            A::StopAgent { agent } => ClientMessage::StopAgent { key, agent },
            A::DeleteAgent { agent } => ClientMessage::DeleteAgent { key, agent },
            A::HideProject { project, hidden } => ClientMessage::SetProjectHidden {
                key,
                device,
                project,
                hidden,
            },
            A::SetTask { task, enabled } => ClientMessage::SetTaskEnabled { key, task, enabled },
            A::RunTask { task } => ClientMessage::RunTaskNow { key, task },
            A::DeleteTask { task } => ClientMessage::DeleteTask { key, task },
            A::ChangeSettings { changes } => ClientMessage::UpdateDeviceSettings {
                key,
                device,
                changes,
            },
            A::GrantPermission {
                agent,
                devices,
                depth,
            } => ClientMessage::GrantPermission {
                key,
                permission: Box::new(Permission {
                    id: PermissionId::derive(&format!("dispatcher-grant:{agent}")),
                    holder: Actor::Agent { agent },
                    home: device,
                    devices,
                    vendors: vec![Vendor::Claude, Vendor::Codex],
                    accounts: None,
                    places: None,
                    depth,
                    limits: Limits::default(),
                    test_kinds: Vec::new(),
                    leave_container: LeaveContainer::default(),
                }),
            },
            A::RevokePermission { agent } => match self.permission_of(agent) {
                Some(permission) => ClientMessage::RevokePermission { key, permission },
                None => return self.act_now(key, refused("That agent can't run agents.")),
            },
            A::SetProject { project, changes } => {
                return self.act_off(
                    key,
                    crate::workspace::change_project_settings(project, changes),
                );
            }
            A::StartWorkspace { workspace } => {
                return self.act_off(key, crate::workspace::start(workspace));
            }
            A::StopWorkspace { workspace } => {
                return self.act_off(key, crate::workspace::stop(workspace));
            }
            A::UpdateTools { tools } => {
                let message = ClientMessage::UpdateTools { key, tools };
                return self.act_off(key, crate::fleet::handle(message));
            }
        };
        self.message(NO_CONN, message);
    }

    /// Ends a change that went, or didn't, at once.
    fn act_now(&mut self, key: IdempotencyKey, outcome: ActionOutcome) {
        self.keys.begin(key, NO_CONN);
        self.finish(key, Outcome::Action(outcome));
    }

    /// Runs a change off the loop.
    fn act_off(
        &mut self,
        key: IdempotencyKey,
        work: impl Future<Output = ActionOutcome> + Send + 'static,
    ) {
        self.keys.begin(key, NO_CONN);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let done = Acted::Here(work.await);
            let _ = tx.send(Input::Acted { key, done });
        });
    }

    /// Sends a change to another device, whose `maxd` makes it.
    fn act_there(
        &mut self,
        key: IdempotencyKey,
        device: DeviceId,
        action: AdminAction,
        expect: Option<AdminAction>,
    ) {
        self.keys.begin(key, NO_CONN);
        let call = PeerCall {
            by: Actor::Agent {
                agent: self
                    .dispatcher
                    .agent
                    .unwrap_or_else(|| AgentId::derive("dispatcher")),
            },
            permission: PermissionId::derive("dispatcher"),
            level: 0,
            call: agents::ToolCall {
                tool: ACT_THERE.into(),
                arguments: json!({"action": action, "expect": expect}),
            },
        };
        let peers = self.agent_tools.peers.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let answer = peers.call(device, call).await;
            let done = Acted::There(answer.map_err(|unreachable| unreachable.0));
            let _ = tx.send(Input::Acted { key, done });
        });
    }

    /// Hears how a change that ran off the loop went.
    pub(super) fn acted(&mut self, key: IdempotencyKey, done: Acted) {
        let outcome = match done {
            Acted::Here(outcome) => outcome,
            Acted::There(Err(reason)) => refused(reason),
            Acted::There(Ok(answer)) if answer.failed => refused(answer.text),
            Acted::There(Ok(answer)) => {
                let made: Made = serde_json::from_str(&answer.text).unwrap_or_default();
                if let Some(running) = self.dispatcher.admin.running.get_mut(&key) {
                    running.inverse = made.inverse;
                    running.after = made.after;
                    running.target = made.target;
                }
                ActionOutcome::Done
            }
        };
        self.finish(key, Outcome::Action(outcome));
    }

    /// Hears that a request finished, and tells whoever waits on it if it
    /// was a change the dispatcher made.
    pub(super) fn admin_done(&mut self, key: IdempotencyKey, outcome: &Outcome) {
        let outcome = match outcome {
            Outcome::Action(outcome) => outcome,
            _ => &refused("MaxHQ couldn't do that."),
        };
        if let Some((waited, reply)) = self.dispatcher.admin.steps.remove(&key) {
            let text = match (waited, outcome) {
                (Waited::Start, ActionOutcome::Started { agent }) => {
                    Ok(format!("Started agent {agent}."))
                }
                (Waited::Send(_), ActionOutcome::Done) => {
                    Ok("Sent. It reaches the agent once its turn ends.".into())
                }
                (Waited::Save(task), ActionOutcome::TaskSaved { .. }) => {
                    Ok(format!("Saved task {task}."))
                }
                (_, outcome) => Err(failure(outcome)),
            };
            return self.reply_now(reply, text);
        }
        let Some(running) = self.dispatcher.admin.running.remove(&key) else {
            return;
        };
        let Running {
            device,
            action,
            inverse,
            after,
            target,
            then,
        } = running;
        let failed = (!succeeded(outcome)).then(|| failure(outcome));
        let after = match failed {
            Some(_) => None,
            None if device == self.device => self.now(&action),
            None => after,
        };
        match then {
            Then::Plan => {}
            Then::Tool { reply, lead } => {
                let text = match failed {
                    Some(reason) => Err(reason),
                    None => {
                        let undo = self.remember(key, device, inverse, after);
                        self.action_card(lead, device, action, target, undo);
                        Ok(if undo.is_some() { UNDOABLE } else { "Done." }.into())
                    }
                };
                self.reply_now(reply, text);
            }
            Then::Peer(reply) => {
                let text = match failed {
                    Some(reason) => Err(reason),
                    None => Ok(json!(Made {
                        inverse,
                        after,
                        target
                    })
                    .to_string()),
                };
                self.reply_now(reply, text);
            }
            Then::Undo { key: asked, record } => {
                let outcome = match failed {
                    None => ActionOutcome::Done,
                    Some(reason) => {
                        // A try that failed for another reason may go
                        // again.
                        if reason != CHANGED && self.still(&record) {
                            self.dispatcher.admin.undo.push_back(*record);
                        }
                        refused(reason)
                    }
                };
                self.publish_dispatcher();
                self.finish(asked, Outcome::Action(outcome));
            }
        }
    }

    /// Keeps what puts a change back, and gives the key Undo names, or
    /// none when nothing does.
    fn remember(
        &mut self,
        key: IdempotencyKey,
        device: DeviceId,
        inverse: Option<AdminAction>,
        after: Option<AdminAction>,
    ) -> Option<IdempotencyKey> {
        let (inverse, after) = (inverse?, after?);
        if inverse == after {
            return None;
        }
        let undo = &mut self.dispatcher.admin.undo;
        if undo.len() >= KEEP_UNDO {
            undo.pop_front();
        }
        undo.push_back(Undo {
            key,
            device,
            inverse,
            after,
        });
        self.publish_dispatcher();
        Some(key)
    }

    /// Shows a change the dispatcher made at once as a card on its
    /// timeline.
    fn action_card(
        &mut self,
        lead: AgentId,
        device: DeviceId,
        action: AdminAction,
        target: String,
        undo: Option<IdempotencyKey>,
    ) {
        let now = self.clock.wall();
        let item = TimelineItem {
            id: ItemId(format!("maxhq-dispatcher-action-{}", Uuid::now_v7())),
            agent: lead,
            turn: None,
            parent: None,
            ordinal: 0,
            status: ItemStatus::Completed,
            started_at: Some(now),
            completed_at: Some(now),
            body: ItemBody::DispatcherAction {
                device,
                action: Box::new(action),
                target,
                undo,
            },
        };
        self.timeline_events(vec![(lead, TimelineEvent::Upsert { item })]);
    }

    /// The keys of the changes Undo may still put back. A change here that
    /// something else changed since isn't one. Another device checks its
    /// own when Undo reaches it.
    pub(super) fn undoable(&self) -> Vec<IdempotencyKey> {
        let undo = &self.dispatcher.admin.undo;
        undo.iter()
            .filter(|record| self.still(record))
            .map(|record| record.key)
            .collect()
    }

    /// Whether nothing here changed what a change left since.
    fn still(&self, record: &Undo) -> bool {
        record.device != self.device || self.now(&record.after).as_ref() == Some(&record.after)
    }

    /// Answers `UndoDispatcherAction`: puts back the change on the card
    /// `action` names, once, unless something else changed it since. Only
    /// a person in MaxHQ may.
    pub(super) fn undo_action(
        &mut self,
        conn: u64,
        key: IdempotencyKey,
        action: IdempotencyKey,
    ) -> Option<ActionOutcome> {
        if conn == NO_CONN || self.agent_tools.signed_in(conn) {
            return Some(refused(
                "Only you can undo the dispatcher's changes, in MaxHQ.",
            ));
        }
        let undo = &mut self.dispatcher.admin.undo;
        let Some(record) = undo
            .iter()
            .position(|record| record.key == action)
            .and_then(|at| undo.remove(at))
        else {
            return Some(refused(GONE));
        };
        if !self.still(&record) {
            self.publish_dispatcher();
            return Some(refused(CHANGED));
        }
        let inner = IdempotencyKey::derive(&format!("dispatcher-undo:{key}"));
        let expect = (record.device != self.device).then(|| record.after.clone());
        let (device, inverse) = (record.device, record.inverse.clone());
        self.publish_dispatcher();
        let record = Box::new(record);
        self.admin_start(inner, device, inverse, expect, Then::Undo { key, record });
        None
    }

    /// Runs a change a home's dispatcher sent for this device. Only the
    /// device's home may send one, and the checks here run again, so a
    /// change it never makes is refused here too.
    pub(super) fn peer_act(&mut self, from: DeviceId, arguments: Value, reply: Reply) {
        if self.agent_tools.peers.home() != Some(from) {
            return self.reply_now(reply, Err(NOT_ALLOWED.into()));
        }
        let Ok(There { action, expect }) = serde_json::from_value(arguments) else {
            let reason = "This device's maxd doesn't know that change. Update it first.";
            return self.reply_now(reply, Err(reason.into()));
        };
        let action = match self.check_act(self.device, action) {
            Ok((action, _)) => action,
            Err(reason) => return self.reply_now(reply, Err(reason)),
        };
        let step = PlanStep::Admin {
            device: self.device,
            action,
        };
        if admin::risk(&step, false) == Risk::Never {
            return self.reply_now(reply, Err(NEVER.into()));
        }
        let PlanStep::Admin { action, .. } = step else {
            return;
        };
        if let Some(expect) = expect
            && self.now(&action) != Some(expect)
        {
            return self.reply_now(reply, Err(CHANGED.into()));
        }
        let key = IdempotencyKey::generate();
        self.admin_start(key, self.device, action, None, Then::Peer(reply));
    }

    /// What the fields a change names hold now, as the change that puts
    /// them back, or none when nothing does.
    fn now(&self, action: &AdminAction) -> Option<AdminAction> {
        use AdminAction as A;
        let held = |agent: &AgentId| self.held.get(&self.lead_of(*agent));
        Some(match action {
            A::RenameAgent { agent, .. } => {
                self.hub.reported(*agent)?;
                A::RenameAgent {
                    agent: *agent,
                    title: self.hub.given_name(*agent).unwrap_or_default(),
                }
            }
            A::SetInbox { agent, .. } => {
                self.hub.reported(*agent)?;
                A::SetInbox {
                    agent: *agent,
                    change: InboxChange::Restore {
                        inbox: self.hub.inbox(*agent),
                    },
                }
            }
            A::SetMode { agent, .. } => A::SetMode {
                agent: *agent,
                mode: held(agent)?.mode,
            },
            A::SetModel { agent, .. } => A::SetModel {
                agent: *agent,
                model: held(agent)?.model.clone()?,
            },
            A::SetEffort { agent, .. } => A::SetEffort {
                agent: *agent,
                effort: held(agent)?.effort.clone(),
            },
            A::HideProject { project, .. } => A::HideProject {
                project: *project,
                hidden: matches!(
                    self.hub.get(&EntityKey::HiddenProjects(self.device)),
                    Some(Entity::HiddenProjects(hidden)) if hidden.projects.contains(project)
                ),
            },
            A::SetProject { project, changes } => {
                let settings = crate::workspace::manager()?.project_settings(*project);
                A::SetProject {
                    project: *project,
                    changes: admin::project_now(&settings, changes),
                }
            }
            A::SetTask { task, .. } => A::SetTask {
                task: *task,
                enabled: self.task_enabled(*task)?,
            },
            A::ChangeSettings { changes } => A::ChangeSettings {
                changes: admin::settings_now(&self.device_settings(), changes),
            },
            A::MarkRead { .. }
            | A::Interrupt { .. }
            | A::StopAgent { .. }
            | A::DeleteAgent { .. }
            | A::StartWorkspace { .. }
            | A::StopWorkspace { .. }
            | A::RunTask { .. }
            | A::DeleteTask { .. }
            | A::UpdateTools { .. }
            | A::GrantPermission { .. }
            | A::RevokePermission { .. } => return None,
        })
    }

    /// What a change changes, by name: an agent's title, a project's name,
    /// a workspace's folder or container, a task's name, or the device's
    /// name.
    fn target_of(&self, action: &AdminAction) -> String {
        use AdminAction as A;
        match action {
            A::RenameAgent { agent, .. }
            | A::SetInbox { agent, .. }
            | A::MarkRead { agent }
            | A::SetMode { agent, .. }
            | A::SetModel { agent, .. }
            | A::SetEffort { agent, .. }
            | A::Interrupt { agent }
            | A::StopAgent { agent }
            | A::DeleteAgent { agent }
            | A::GrantPermission { agent, .. }
            | A::RevokePermission { agent } => self
                .hub
                .reported(*agent)
                .map(|agent| agent.title)
                .unwrap_or_default(),
            A::HideProject { project, .. } | A::SetProject { project, .. } => {
                match self.hub.get(&EntityKey::Project(*project)) {
                    Some(Entity::Project(project)) => project.name,
                    _ => String::new(),
                }
            }
            A::StartWorkspace { workspace } | A::StopWorkspace { workspace } => {
                match self.hub.get(&EntityKey::Workspace(*workspace)) {
                    Some(Entity::Workspace(workspace)) => match workspace.location {
                        WorkspaceLocation::Folder { path } => path,
                        WorkspaceLocation::Container { instance, .. } => instance,
                    },
                    _ => String::new(),
                }
            }
            A::SetTask { task, .. } | A::RunTask { task } | A::DeleteTask { task } => {
                self.task_name(*task).unwrap_or_default()
            }
            A::ChangeSettings { .. } | A::UpdateTools { .. } => self.hub.device().name.clone(),
        }
    }
}
