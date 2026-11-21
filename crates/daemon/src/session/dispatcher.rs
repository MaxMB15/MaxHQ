// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The dispatcher: a Claude or Codex agent on the Mac, in a folder of its
//! own, behind the header's text field. It reads what MaxHQ knows, makes
//! the safe changes the user asked for at once (see [`super::dispatcher_admin`])
//! and puts the rest in a plan. Only a client's confirm runs the plan, item
//! by item, through the same keyed requests the app sends. Its CLI runs in
//! Auto and writes only in its own folder, and its calls are checked here
//! as well, whatever its CLI was given.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use maxhq_core::agents::user_permission;
use maxhq_core::pill_text::Carried;
use maxhq_core::tools::admin;
use maxhq_core::tools::dispatch::{Aim, Repo, Start, Step, Submit, MAX_ITEMS};
use maxhq_core::tools::tasks::NewAgent;
use maxhq_core::tools::Call;
use maxhq_core::{context, schedule};
use maxhq_protocol::dispatch::{Dispatcher, Plan, PlanItem, PlanOutcome, PlanStep, PlanTarget};
use maxhq_protocol::ids::WorkspaceId;
use maxhq_protocol::ids::{AccountId, AgentId, DeviceId, IdempotencyKey, SessionId, TaskId};
use maxhq_protocol::message::{
    ActionOutcome, AgentPlace, AgentSpec, ClientMessage, SendMode, UserInput,
};
use maxhq_protocol::model::{PermissionMode, Vendor, WorkspaceLocation};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::tasks::{Maker, Task, TaskTarget, Zone};
use serde_json::{json, Value};
use tracing::warn;

use super::agent_tools::{
    account_card, failure, no_account, workspace_card, Later, Reply, NO_CONN, NO_PLACE,
};
use super::dispatcher_admin::{succeeded, Admin, Then};
use super::dispatcher_guide;
use super::idem::{Outcome, Seen};
use super::move_agent::ALREADY_THERE;
use super::scheduled::system_zone;
use super::{refused, Input, Manager};
use crate::drive::{ToolCall, ToolResult};
use crate::hub::Hub;
use crate::pulls::source::{Pull, PullSource};
use crate::secrets::RepoName;

/// The dispatcher's folder, in `maxd`'s data directory.
pub const DIR: &str = "dispatcher";

/// Why an agent can't start in the dispatcher's folder.
pub(super) const OWN_FOLDER: &str = "An agent can't run in the dispatcher's own folder.";

/// Why a plan can't name another device yet.
const MAC_ONLY: &str = "This version of the dispatcher plans on this Mac only, except a move.";

/// How the dispatcher uses what `list_targets` shows of this Mac.
const TARGETS_HERE: &str = "Plans start agents and save tasks on this Mac; a move_agent item \
                            may move an agent on any device. A folder workspace's location is \
                            the folder to give start_agent; a container is named by its \
                            workspace id. Call list_targets with one of `other_devices` as \
                            `device` to see its accounts, folders and agents.";

/// What `submit_plan` answers.
const PLAN_SHOWN: &str =
    "The plan shows under the bar. Nothing in it runs until the user confirms it.";

/// Makes the dispatcher's folder and gives its workspace, or none if the
/// folder couldn't be made.
pub fn home(hub: &Hub, data: &Path) -> Option<WorkspaceId> {
    let folder = data.join(DIR);
    if let Err(err) = std::fs::create_dir_all(&folder) {
        warn!(%err, "couldn't make the dispatcher's folder");
        return None;
    }
    Some(hub.own_folder(&folder))
}

/// What the manager keeps for the dispatcher.
#[derive(Default)]
pub(super) struct Dispatch {
    /// The workspace of its folder. None on a device with no dispatcher.
    workspace: Option<WorkspaceId>,
    /// The agent of the conversation now.
    pub(super) agent: Option<AgentId>,
    /// The request that starts a new conversation, until it's done.
    pub(super) starting: Option<IdempotencyKey>,
    /// The plan shown, until it's confirmed or dismissed.
    pub(super) plan: Option<Plan>,
    /// The last plan's number.
    pub(super) plans: u32,
    /// The confirmed plan that runs now.
    pub(super) run: Option<Run>,
    /// The changes it makes at once.
    pub(super) admin: Admin,
    /// The pills in the conversation's requests, which its plans hand on.
    pub(super) carried: Carried,
    /// Where a project's PRs are read.
    pub(super) pulls: Option<Arc<dyn PullSource>>,
    /// Its conversations, each a chat.
    pub(super) chats: super::dispatcher_chats::Chats,
}

impl Dispatch {
    /// The dispatcher of a device whose folder is `workspace`.
    pub(super) fn new(workspace: Option<WorkspaceId>) -> Self {
        Self {
            workspace,
            ..Self::default()
        }
    }

    /// The workspace of its folder.
    pub(super) fn home(&self) -> Option<WorkspaceId> {
        self.workspace
    }
}

/// A confirmed plan as it runs.
pub(super) struct Run {
    /// The confirm's key, which hears how the plan went.
    key: IdempotencyKey,
    /// The items still to run.
    items: std::vec::IntoIter<PlanStep>,
    /// How each item that ran went.
    results: Vec<PlanOutcome>,
    /// The request of the item that runs now, and what it was.
    waiting: Option<(IdempotencyKey, Waited)>,
}

/// What a running item asked for.
#[derive(Clone, Copy)]
pub(super) enum Waited {
    Start,
    Send(AgentId),
    Save(TaskId),
    Move,
    Admin,
}

/// The request that runs a plan item on `device`, and what it asks for.
/// `results` are how the plan's earlier items went, so a task aimed at one
/// reaches the agent it started. A change to MaxHQ itself has none, since
/// [`Manager::admin_start`] runs it.
pub(super) fn plan_message(
    device: DeviceId,
    key: IdempotencyKey,
    step: PlanStep,
    results: &[PlanOutcome],
) -> Result<(ClientMessage, Waited), String> {
    Ok(match step {
        PlanStep::StartAgent { spec, .. } => {
            let message = ClientMessage::StartAgent { key, spec: *spec };
            (message, Waited::Start)
        }
        PlanStep::SendMessage { agent, text, .. } => {
            // It reads as the dispatcher's, sent for you, so the agent
            // doesn't take what it says as your own words.
            let input = UserInput {
                text: super::agent_tools::mark(super::agent_tools::FOR_YOU, &text),
                attachments: Vec::new(),
                refs: Vec::new(),
            };
            let message = ClientMessage::SendInput {
                key,
                agent,
                input,
                mode: SendMode::Queue,
            };
            (message, Waited::Send(agent))
        }
        PlanStep::ScheduleTask {
            name,
            prompt,
            trigger,
            target,
            ..
        } => {
            let target = match target {
                PlanTarget::Task { target } => target,
                PlanTarget::Item { item } => match results.get(item as usize) {
                    Some(PlanOutcome::Started { agent }) => TaskTarget::Agent {
                        device,
                        agent: *agent,
                    },
                    _ => return Err(format!("Item {item} didn't start an agent.")),
                },
            };
            let id = TaskId::derive(&format!("dispatch:{key}"));
            let task = Task {
                id,
                name,
                prompt,
                trigger: *trigger,
                target,
                enabled: true,
                permission: user_permission(id),
                made_by: Maker::User,
                next_run: None,
                last_run: None,
                approval_asked: None,
                by_dispatcher: false,
            };
            let message = ClientMessage::SaveTask {
                key,
                task: Box::new(task),
            };
            (message, Waited::Save(id))
        }
        PlanStep::MoveAgent { device, agent, to } => {
            let message = ClientMessage::MoveAgent {
                key,
                agent,
                from: device,
                to,
                stop: false,
            };
            (message, Waited::Move)
        }
        PlanStep::Admin { .. } => return Err("MaxHQ runs that change itself.".into()),
    })
}

impl Manager {
    /// Whether an agent is a conversation with the dispatcher: the one now,
    /// or one of its chats, that runs in its folder. Only a request of
    /// yours starts one, so any other agent in that folder isn't one.
    pub(super) fn is_dispatcher(&self, agent: AgentId) -> bool {
        let lead = self.lead_of(agent);
        (self.dispatcher.agent == Some(lead) || self.is_chat(lead)) && self.in_home(lead)
    }

    /// Whether an agent this device holds runs in the dispatcher's folder.
    pub(super) fn in_home(&self, agent: AgentId) -> bool {
        let home = self.dispatcher.home();
        home.is_some()
            && self
                .held
                .get(&agent)
                .is_some_and(|held| held.guest.is_none() && Some(held.agent.workspace) == home)
    }

    /// Whether a start in `workspace` and `cwd` runs in the dispatcher's
    /// folder.
    pub(super) fn in_dispatcher_folder(
        &self,
        workspace: WorkspaceId,
        guest: Option<WorkspaceId>,
        cwd: &Path,
    ) -> bool {
        guest.is_none()
            && (Some(workspace) == self.dispatcher.home()
                || self
                    .dispatcher_folder()
                    .is_some_and(|own| cwd.starts_with(own)))
    }

    /// Shows the dispatcher as it is now.
    pub(super) fn publish_dispatcher(&self) {
        let Some(workspace) = self.dispatcher.home() else {
            return;
        };
        self.hub.apply([Change::Upsert {
            value: Entity::Dispatcher(Dispatcher {
                device: self.device,
                workspace,
                agent: self.dispatcher.agent,
                plan: self.dispatcher.plan.clone(),
                undo: self.undoable(),
            }),
        }]);
    }

    /// Handles the dispatcher's messages from a client.
    pub(super) fn dispatcher_message(&mut self, conn: u64, message: ClientMessage) {
        let key = match &message {
            ClientMessage::AskDispatcher { key, .. }
            | ClientMessage::ConfirmPlan { key, .. }
            | ClientMessage::DismissPlan { key, .. }
            | ClientMessage::UndoDispatcherAction { key, .. } => *key,
            _ => return,
        };
        match self.keys.begin(key, conn) {
            Seen::New => {}
            Seen::Running => return,
            Seen::Done(outcome) => {
                self.tell(conn, outcome.message(key));
                return;
            }
        }
        let outcome = match message {
            ClientMessage::AskDispatcher {
                text,
                account,
                new,
                refs,
                ..
            } => {
                if self.clients.get(&conn).is_some_and(|viewer| viewer.person) {
                    self.owner_words.sent(key, &text);
                }
                let input = UserInput {
                    text,
                    attachments: Vec::new(),
                    refs,
                };
                self.ask_dispatcher(key, input, account, new)
            }
            ClientMessage::ConfirmPlan { plan, .. } => self.confirm_plan(conn, key, plan),
            ClientMessage::DismissPlan { plan, .. } => Some(self.dismiss_plan(plan)),
            ClientMessage::UndoDispatcherAction { action, .. } => {
                self.undo_action(conn, key, action)
            }
            _ => None,
        };
        if let Some(outcome) = outcome {
            self.finish(key, Outcome::Action(outcome));
        }
    }

    /// Sends the conversation a request, or starts a new one under
    /// `account` with it: when asked, or when the account changed. The
    /// conversation keeps the request's pills for its plans.
    pub(super) fn ask_dispatcher(
        &mut self,
        key: IdempotencyKey,
        mut input: UserInput,
        account: AccountId,
        new: bool,
    ) -> Option<ActionOutcome> {
        let Some(home) = self.dispatcher.home() else {
            return Some(refused("The dispatcher runs on the Mac."));
        };
        // The plans it makes hand these pills on, so a session pill keeps
        // only what its card shares from here.
        self.keep_shared(&mut input.refs);
        if input.text.trim().is_empty() {
            return Some(refused("Type a request first."));
        }
        if let Err(reason) = context::check(&input.text, &input.refs, &input.attachments) {
            return Some(refused(reason));
        }
        if self.dispatcher.starting.is_some() {
            return Some(refused("The dispatcher is still starting."));
        }
        // A new request starts its own count of changes made at once, and
        // its own plan.
        self.dispatcher.admin.made = 0;
        self.dispatcher.admin.added.clear();
        self.dispatcher.admin.since = self.dispatcher.plans;
        let vendor = match self.hub.get(&EntityKey::Account(account)) {
            Some(Entity::Account(found))
                if found.device == self.device
                    && matches!(found.vendor, Vendor::Claude | Vendor::Codex) =>
            {
                found.vendor
            }
            _ => {
                return Some(refused(
                    "The dispatcher runs as a Claude or Codex account on this Mac.",
                ))
            }
        };
        let current = self.dispatcher.agent.filter(|agent| {
            self.held
                .get(agent)
                .is_some_and(|held| held.account == account)
        });
        let new = self.route_chat(key, &input, current, new);
        if let Some(agent) = current.filter(|_| !new) {
            self.dispatcher.carried.keep(&input.refs);
            let send = ClientMessage::SendInput {
                key,
                agent,
                input,
                mode: SendMode::Queue,
            };
            return self.action(key, send);
        }
        if let Some(agent) = self.dispatcher.agent.take() {
            let stop = IdempotencyKey::derive(&format!("dispatcher-stop:{key}"));
            self.message(NO_CONN, ClientMessage::StopAgent { key: stop, agent });
        }
        self.dispatcher.plan = None;
        self.dispatcher.carried = Carried::default();
        self.dispatcher.carried.keep(&input.refs);
        self.dispatcher.starting = Some(key);
        self.publish_dispatcher();
        let spec = AgentSpec {
            effort: None,
            vendor,
            account,
            place: AgentPlace::Workspace { workspace: home },
            model: None,
            permission: Some(PermissionMode::Auto),
            title: Some("Dispatcher".into()),
            first: Some(input),
            branch: None,
            origin: None,
        };
        self.action(key, ClientMessage::StartAgent { key, spec })
    }

    /// Runs the plan shown, unless an agent or the dispatcher itself asks.
    fn confirm_plan(&mut self, conn: u64, key: IdempotencyKey, plan: u32) -> Option<ActionOutcome> {
        if conn == NO_CONN || self.agent_tools.signed_in(conn) {
            return Some(refused("Only you can confirm a plan, in MaxHQ."));
        }
        if self.dispatcher.run.is_some() {
            return Some(refused("A plan is still running."));
        }
        let Some(plan) = self.dispatcher.plan.take_if(|shown| shown.id == plan) else {
            return Some(refused("That plan isn't the one shown any more."));
        };
        self.dispatcher.admin.added.clear();
        self.publish_dispatcher();
        let steps: Vec<PlanStep> = plan.items.into_iter().map(|item| item.step).collect();
        self.dispatcher.run = Some(Run {
            key,
            items: steps.into_iter(),
            results: Vec::new(),
            waiting: None,
        });
        self.next_item();
        None
    }

    /// Drops the plan shown.
    fn dismiss_plan(&mut self, plan: u32) -> ActionOutcome {
        if self
            .dispatcher
            .plan
            .take_if(|shown| shown.id == plan)
            .is_none()
        {
            return refused("That plan isn't the one shown any more.");
        }
        self.dispatcher.admin.added.clear();
        self.publish_dispatcher();
        ActionOutcome::Done
    }

    /// Sends the running plan's next item, or ends the run once none is
    /// left. A task aimed at an earlier item reaches the agent it started.
    fn next_item(&mut self) {
        let Some(run) = &mut self.dispatcher.run else {
            return;
        };
        let Some(step) = run.items.next() else {
            return self.end_plan(None);
        };
        let key = IdempotencyKey::derive(&format!("dispatch:{}:{}", run.key, run.results.len()));
        if let PlanStep::Admin { device, action } = step {
            run.waiting = Some((key, Waited::Admin));
            return self.admin_start(key, device, action, None, Then::Plan);
        }
        let (message, waited) = match plan_message(self.device, key, step, &run.results) {
            Ok(planned) => planned,
            Err(reason) => return self.end_plan(Some(reason)),
        };
        // Set first, since a request may finish before it returns.
        run.waiting = Some((key, waited));
        self.message(NO_CONN, message);
    }

    /// Ends the running plan, with why its last item failed.
    fn end_plan(&mut self, failed: Option<String>) {
        let Some(mut run) = self.dispatcher.run.take() else {
            return;
        };
        run.results
            .extend(failed.map(|reason| PlanOutcome::Failed { reason }));
        let results = run.results;
        self.finish(run.key, Outcome::Action(ActionOutcome::PlanRan { results }));
    }

    /// Hears that a request finished: a new conversation's start, a change
    /// made at once, or a running plan's item.
    pub(super) fn dispatch_done(&mut self, key: IdempotencyKey, outcome: &Outcome) {
        self.admin_done(key, outcome);
        if self.dispatcher.starting == Some(key) {
            self.dispatcher.starting = None;
            if let Outcome::Action(ActionOutcome::Started { agent }) = outcome {
                self.dispatcher.agent = Some(*agent);
                self.owner_words.reached(key, *agent);
            }
            return self.publish_dispatcher();
        }
        let Some(run) = &mut self.dispatcher.run else {
            return;
        };
        let Some((_, waited)) = run.waiting.take_if(|(waiting, _)| *waiting == key) else {
            return;
        };
        let Outcome::Action(outcome) = outcome else {
            return;
        };
        let result = match (waited, outcome) {
            (Waited::Start | Waited::Move, ActionOutcome::Started { agent }) => {
                PlanOutcome::Started { agent: *agent }
            }
            (Waited::Send(agent), ActionOutcome::Done) => PlanOutcome::Sent { agent },
            (Waited::Save(task), ActionOutcome::TaskSaved { .. }) => {
                PlanOutcome::TaskSaved { task }
            }
            (Waited::Admin, outcome) if succeeded(outcome) => PlanOutcome::Done,
            (_, outcome) => return self.end_plan(Some(failure(outcome))),
        };
        run.results.push(result);
        self.next_item();
    }

    /// Runs the dispatcher's call to MaxHQ's tools: a read, a plan, or a
    /// change. Anything else is refused here, whatever its CLI was given.
    pub(super) fn dispatcher_tool(&mut self, session: SessionId, call: ToolCall) {
        let lead = self.lead_of(call.agent);
        let reply = Reply::Driver {
            session,
            key: call.key,
        };
        let asked = (
            admin::asked(&call.arguments),
            admin::user_words(&call.arguments),
        );
        let text = match call.call {
            Call::ListTargets(Some(device)) if device != self.device => {
                return self.targets_there(reply, device);
            }
            Call::ListTargets(_) => {
                let mut targets = self.device_targets(TARGETS_HERE);
                targets["other_devices"] = self.other_devices();
                Ok(targets.to_string())
            }
            Call::ListAgents(list) => Ok(self.list(self.agent_caller(lead), None, &list)),
            Call::ReadAgent(read) if read.device.is_none_or(|device| device == self.device) => {
                return self.read_agent(reply, Some(lead), read, true);
            }
            Call::ReadAgent(_) => Err(MAC_ONLY.to_owned()),
            Call::ListTasks => Ok(self.task_list(|_| true)),
            Call::ListRepoPullRequests(repo) => return self.repo_pulls(reply, repo),
            Call::SubmitPlan(submit) => self.submit_plan(lead, submit),
            Call::SearchGuide(query) => Ok(dispatcher_guide::search(&query)),
            Call::ReadGuide(id) => dispatcher_guide::read(&id),
            acting if admin::acts(acting.tool()) => {
                return self.dispatcher_act(lead, reply, asked, acting);
            }
            _ => Err("The dispatcher doesn't get that tool.".to_owned()),
        };
        self.reply_now(reply, text);
    }

    /// What the dispatcher's `list_targets` shows of this device, with
    /// `note` for how to use it: its accounts of every vendor, and its
    /// projects with their folders and workspaces.
    pub(super) fn device_targets(&self, note: &str) -> Value {
        let home = self.dispatcher.home();
        let mut accounts = Vec::new();
        let mut projects = Vec::new();
        let mut workspaces = Vec::new();
        for entity in self.hub.pick(|entity| match entity {
            Entity::Account(_) | Entity::Project(_) | Entity::Workspace(_) => Some(entity.clone()),
            _ => None,
        }) {
            match entity {
                Entity::Account(account) if account.device == self.device => {
                    accounts.push(account_card(&account));
                }
                Entity::Project(project) => projects.push(project),
                Entity::Workspace(workspace)
                    if workspace.device == self.device && Some(workspace.id) != home =>
                {
                    workspaces.push(workspace);
                }
                _ => {}
            }
        }
        let projects: Vec<Value> = projects
            .iter()
            .map(|project| {
                let checkouts: Vec<Value> = workspaces
                    .iter()
                    .filter(|workspace| workspace.project == Some(project.id))
                    .map(workspace_card)
                    .collect();
                json!({
                    "project": project.id,
                    "name": project.name,
                    "repo": project.repo,
                    "workspaces": checkouts,
                })
            })
            .collect();
        let others: Vec<Value> = workspaces
            .iter()
            .filter(|workspace| workspace.project.is_none())
            .map(workspace_card)
            .collect();
        json!({
            "device": {"device": self.device, "name": self.hub.device().name},
            "accounts": accounts,
            "projects": projects,
            "other_workspaces": others,
            "note": note,
        })
    }

    /// The other devices this one links to, by id and name.
    fn other_devices(&self) -> Value {
        let book = self.db.peer_book().ok().flatten().unwrap_or_default();
        book.peers
            .iter()
            .filter(|peer| peer.device != self.device)
            .map(|peer| json!({"device": peer.device, "name": peer.name}))
            .collect()
    }

    /// Reads a project's open PRs off the loop, for `list_repo_pull_requests`.
    fn repo_pulls(&mut self, reply: Reply, repo: Repo) {
        let named = match self.hub.get(&EntityKey::Project(repo.project)) {
            Some(Entity::Project(project)) => project.repo,
            _ => return self.reply_now(reply, Err("There's no project here with that id.".into())),
        };
        let Some(name) = named.as_deref().and_then(RepoName::parse) else {
            let reason = "That project names no GitHub repository.";
            return self.reply_now(reply, Err(reason.into()));
        };
        let Some(source) = self.dispatcher.pulls.clone() else {
            let reason = "MaxHQ can't read GitHub on this Mac yet.";
            return self.reply_now(reply, Err(reason.into()));
        };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = source
                .open_list(&name)
                .await
                .map(|pulls| ToolResult::Text(pull_list(&name, &pulls)))
                .map_err(|err| format!("couldn't read {name}'s pull requests: {err}"));
            let later = Later::Answer {
                reply,
                result,
                release: None,
            };
            let _ = tx.send(Input::Agents(Box::new(later)));
        });
    }

    /// Checks a plan and shows it under the bar. The conversation that
    /// submits it is the one the header shows.
    fn submit_plan(&mut self, lead: AgentId, submit: Submit) -> Result<String, String> {
        if self.dispatcher.run.is_some() {
            return Err("The last plan is still running. Submit again once it's done.".into());
        }
        let items = submit
            .items
            .into_iter()
            .enumerate()
            .map(|(n, proposed)| {
                let step = self
                    .plan_step(proposed.step)
                    .map_err(|reason| format!("Item {n}: {reason}"))?;
                Ok(PlanItem {
                    reason: proposed.reason,
                    step,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        // The changes this request put in the plan stay in it, after the
        // items it submits, so an item's number still names it.
        let added = &self.dispatcher.admin.added;
        if items.len() + added.len() > MAX_ITEMS {
            return Err(format!(
                "With the {} changes waiting, that's more than {MAX_ITEMS} items.",
                added.len()
            ));
        }
        let items = items.into_iter().chain(added.iter().cloned()).collect();
        self.dispatcher.plans += 1;
        self.dispatcher.plan = Some(Plan {
            id: self.dispatcher.plans,
            items,
        });
        self.adopt_chat(lead);
        self.publish_dispatcher();
        Ok(PLAN_SHOWN.into())
    }

    /// Checks one item of a plan and fills in what it leaves out.
    pub(super) fn plan_step(&self, step: Step) -> Result<PlanStep, String> {
        Ok(match step {
            Step::Start(Start {
                agent,
                title,
                first,
            }) => {
                let (device, spec) = self.planned_agent(agent, title, first)?;
                PlanStep::StartAgent {
                    device,
                    spec: Box::new(spec),
                }
            }
            Step::Send {
                device,
                agent,
                text,
            } => PlanStep::SendMessage {
                device: self.plan_device(device)?,
                agent: self.running_agent(agent)?,
                text,
            },
            Step::Move { device, agent, to } => {
                let device = device.unwrap_or(self.device);
                if device == to.device {
                    return Err(ALREADY_THERE.into());
                }
                if device == self.device {
                    self.running_agent(agent)?;
                }
                PlanStep::MoveAgent { device, agent, to }
            }
            Step::Schedule {
                device,
                name,
                prompt,
                schedule,
                target,
            } => PlanStep::ScheduleTask {
                device: self.plan_device(device)?,
                name,
                prompt,
                trigger: Box::new(schedule::parse(&schedule, &Zone(system_zone()))?),
                target: match target {
                    Aim::Agent { device, agent } => PlanTarget::Task {
                        target: TaskTarget::Agent {
                            device: self.plan_device(device)?,
                            agent: self.running_agent(agent)?,
                        },
                    },
                    Aim::Item(item) => PlanTarget::Item { item },
                    Aim::New(new) => {
                        let (device, spec) = self.planned_agent(new, None, None)?;
                        PlanTarget::Task {
                            target: TaskTarget::New {
                                device,
                                spec: Box::new(spec),
                            },
                        }
                    }
                },
            },
        })
    }

    /// The device a plan names, which must be this one.
    fn plan_device(&self, device: Option<DeviceId>) -> Result<DeviceId, String> {
        match device {
            None => Ok(self.device),
            Some(device) if device == self.device => Ok(device),
            Some(_) => Err(MAC_ONLY.into()),
        }
    }

    /// An agent this device runs that a plan may message or aim a task at.
    fn running_agent(&self, agent: AgentId) -> Result<AgentId, String> {
        if !self.held.contains_key(&agent) || self.is_dispatcher(agent) {
            return Err(format!("There's no agent on this Mac with id {agent}."));
        }
        Ok(agent)
    }

    /// The agent a plan starts, with the vendor's first account here when
    /// it names none. It can't run in the dispatcher's own folder.
    fn planned_agent(
        &self,
        new: NewAgent,
        title: Option<String>,
        first: Option<String>,
    ) -> Result<(DeviceId, AgentSpec), String> {
        let device = self.plan_device(new.device)?;
        let vendor = new.vendor;
        let place = new.place.ok_or(NO_PLACE)?;
        let accounts = self.accounts(vendor, device);
        let account = match new.account {
            Some(account) if accounts.contains(&account) => account,
            Some(account) => {
                return Err(format!(
                    "There's no {} account on this Mac with id {account}.",
                    vendor.name()
                ))
            }
            None => *self
                .default_accounts(vendor, device)
                .first()
                .ok_or_else(|| no_account(vendor))?,
        };
        match &place {
            AgentPlace::Workspace { workspace } => {
                let here = matches!(
                    self.hub.get(&EntityKey::Workspace(*workspace)),
                    Some(Entity::Workspace(found)) if found.device == device
                );
                if !here || Some(*workspace) == self.dispatcher.home() {
                    return Err(format!(
                        "There's no workspace on this Mac with id {workspace}."
                    ));
                }
            }
            AgentPlace::Folder { path } => {
                let path = Path::new(path);
                if !path.is_absolute() {
                    return Err("A folder must be an absolute path.".into());
                }
                if self
                    .dispatcher_folder()
                    .is_some_and(|own| path.starts_with(own))
                {
                    return Err(OWN_FOLDER.into());
                }
            }
        }
        let spec = AgentSpec {
            effort: None,
            vendor,
            account,
            place,
            model: new.model,
            permission: new.mode,
            title,
            first: first.map(|text| self.dispatcher.carried.input(&text)),
            branch: None,
            origin: None,
        };
        Ok((device, spec))
    }

    /// The dispatcher's own folder.
    pub(super) fn dispatcher_folder(&self) -> Option<PathBuf> {
        match self.hub.get(&EntityKey::Workspace(self.dispatcher.home()?)) {
            Some(Entity::Workspace(workspace)) => match workspace.location {
                WorkspaceLocation::Folder { path } => Some(PathBuf::from(path)),
                _ => None,
            },
            _ => None,
        }
    }
}

/// What `list_repo_pull_requests` answers: each open PR, then each author
/// once.
fn pull_list(repo: &RepoName, pulls: &[Pull]) -> String {
    let mut authors: Vec<&str> = pulls
        .iter()
        .filter_map(|pull| pull.user.as_ref())
        .map(|user| user.login.as_str())
        .collect();
    authors.sort_unstable();
    authors.dedup();
    let listed: Vec<Value> = pulls
        .iter()
        .map(|pull| {
            json!({
                "number": pull.number,
                "title": pull.title,
                "author": pull.user.as_ref().map(|user| &user.login),
                "draft": pull.draft,
                "updated_at": pull.updated_at,
                "url": pull.html_url,
            })
        })
        .collect();
    json!({
        "repo": repo.to_string(),
        "pull_requests": listed,
        "authors": authors,
    })
    .to_string()
}
