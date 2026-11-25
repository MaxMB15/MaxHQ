// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The dispatcher: the tools it gets and the ones it's refused, the plan
//! it submits, the confirm only a client sends, and the changes it makes
//! at once, waits on or never makes.

use maxhq_core::tools::Tool;
use maxhq_protocol::agents::{
    Actor, LeaveContainer, Limits, PeerBook, PeerEntry, Permission, DEPTH,
};
use maxhq_protocol::dispatch::{AdminAction, Dispatcher, PlanOutcome, PlanStep, PlanTarget};
use maxhq_protocol::ids::{PermissionId, ProjectId, WorkspaceId};
use maxhq_protocol::local_models::LocalModelSettings;
use maxhq_protocol::model::{DeviceSettingChange, Project};
use maxhq_protocol::tasks::{FilterTest, TaskTarget, Trigger};
use serde_json::{json, Value};

use super::moves::devbox;
use super::*;
use crate::pulls::source::fake::FakePulls;
use crate::pulls::source::{Pull, User};
use crate::secrets::RepoName;

/// What the dispatcher hears for a change made at once that Undo puts
/// back.
const UNDOABLE: &str = "Done. The user can undo it from the card under your answer.";

/// What the dispatcher hears for a change that waits for the user.
pub(super) const ASKS_FIRST: &str =
    "It waits in the plan under the bar until the user confirms it.";

/// What the dispatcher hears for a change it never makes.
const NEVER: &str = "The user changes that in Settings, not you.";

/// A change the dispatcher made at once, as its card shows it.
#[derive(Debug, PartialEq)]
struct Card {
    device: DeviceId,
    action: AdminAction,
    target: String,
    undo: Option<IdempotencyKey>,
}

impl Harness {
    /// A harness whose manager has a dispatcher, as on the Mac.
    pub(super) async fn with_dispatcher() -> Self {
        let mut h = Harness::new();
        h.fakes.dispatcher = true;
        h.restart().await;
        h
    }

    /// Asks the dispatcher `text` as the Claude account. Gives its agent
    /// and holder once the request reached the CLI.
    pub(super) async fn dispatch(&mut self, text: &str) -> (AgentId, SessionId) {
        let account = self.claude;
        let asked = text.to_owned();
        let agent = started(
            self.act(|key| ClientMessage::AskDispatcher {
                key,
                text: asked,
                account,
                new: false,
                refs: Vec::new(),
            })
            .await,
        );
        let session = self.holders.last();
        let text = text.to_owned();
        eventually("the request", || self.sent(session).contains(&text)).await;
        (agent, session)
    }

    /// The dispatcher as clients see it.
    pub(super) fn dispatcher(&self) -> Dispatcher {
        match self.hub.get(&EntityKey::Dispatcher(self.host.id)) {
            Some(Entity::Dispatcher(dispatcher)) => dispatcher,
            other => panic!("the hub has no dispatcher: {other:?}"),
        }
    }

    /// The agents clients see.
    pub(super) fn agents_now(&self) -> Vec<AgentId> {
        self.hub.pick(|entity| match entity {
            Entity::Agent(agent) => Some(agent.id),
            _ => None,
        })
    }

    /// The test folder's project, linked to owner/repo.
    fn transit(&mut self) -> (ProjectId, WorkspaceId) {
        let workspace = self.hub.locate_folder(&self.folder);
        let Some(Entity::Workspace(found)) = self.hub.get(&EntityKey::Workspace(workspace)) else {
            panic!("the folder has no workspace");
        };
        let project = found.project.expect("a folder has a project");
        let Some(Entity::Project(found)) = self.hub.get(&EntityKey::Project(project)) else {
            panic!("no project");
        };
        self.keep(Entity::Project(Project {
            repo: Some("https://github.com/owner/repo.git".into()),
            ..found
        }));
        (project, workspace)
    }

    /// The cards on the dispatcher's timeline. Opens it, so call it once.
    async fn action_cards(&mut self, dispatcher: AgentId) -> Vec<Card> {
        let (_, items, _) = reset(self.open(dispatcher, None, 50).await);
        items
            .into_iter()
            .filter_map(|item| match item.body {
                ItemBody::DispatcherAction {
                    device,
                    action,
                    target,
                    undo,
                } => Some(Card {
                    device,
                    action: *action,
                    target,
                    undo,
                }),
                _ => None,
            })
            .collect()
    }

    /// Puts back the change on the card `action` names.
    pub(super) async fn undo(&mut self, action: IdempotencyKey) -> ActionOutcome {
        self.act(|key| ClientMessage::UndoDispatcherAction { key, action })
            .await
    }

    /// This device's settings as clients see them.
    fn settings_now(&self) -> DeviceSettings {
        match self.hub.get(&EntityKey::DeviceSettings(self.host.id)) {
            Some(Entity::DeviceSettings(settings)) => settings,
            _ => hub::default_settings(&self.host),
        }
    }
}

/// The test request's plan: an idle Cursor agent in `folder`, and a task
/// on octocat's PR events, waiting 15 minutes for quiet, aimed at it. Each
/// event in the wait starts its 15 minutes again.
fn review_plan(folder: &Path) -> Value {
    json!({"items": [
        {"type": "start_agent", "reason": "Reviews octocat's PRs", "vendor": "cursor",
         "folder": folder.display().to_string(), "title": "PR reviews"},
        {"type": "schedule_task", "reason": "Sends it each batch of octocat's PR changes",
         "title": "Review octocat's PRs", "prompt": "Review the PR changes in this batch.",
         "schedule": {"type": "github_events", "repo": "owner/repo",
                      "events": ["pull_request"],
                      "filters": [{"path": "body.pull_request.user.login", "op": "equals", "value": "octocat"}],
                      "dedupe": {"coalesceKey": "octocat", "coalesceWindowSeconds": 900,
                                 "restartWindowOnEachRequest": true}},
         "target": {"type": "item", "item": 0}},
    ]})
}

#[tokio::test]
async fn the_dispatcher_gets_its_tools_and_no_others() {
    let mut h = Harness::with_dispatcher().await;
    let (dispatcher, session) = h.dispatch("What runs now?").await;
    assert_eq!(h.dispatcher().agent, Some(dispatcher));

    // Its CLI is served the dispatcher's tools, which read, plan and act,
    // and no lease tools.
    let place = h.vendors.claude_places().pop().expect("a Claude session");
    assert!(place.dispatcher);
    assert_eq!(place.tools(), &Tool::DISPATCHER);
    assert!(!place.tools().contains(&Tool::LeaseOpen));

    // A call to another tool anyway is refused where it's called, with no
    // permission asked for.
    let answer = h
        .call(
            session,
            dispatcher,
            "s1",
            "send_attachments",
            json!({"paths": ["a.txt"]}),
        )
        .await;
    assert_eq!(answer, Err("The dispatcher doesn't get that tool.".into()));
    // It doesn't change itself either.
    let itself =
        json!({"action": "rename", "agent": dispatcher.to_string(), "title": "Me", "asked": true});
    let answer = h
        .call(session, dispatcher, "s2", "manage_agent", itself)
        .await;
    assert_eq!(
        answer,
        Err(format!("There's no agent here with id {dispatcher}."))
    );

    // Its mode stays as it is.
    let outcome = h
        .act(|key| ClientMessage::SetPermission {
            key,
            agent: dispatcher,
            mode: PermissionMode::FullAccess,
        })
        .await;
    assert_eq!(reason(&outcome), "The dispatcher's mode stays as it is.");

    // Any other agent is refused the dispatcher's own tools.
    let (other, held) = h.start_claude(None).await;
    let answer = h
        .call(held, other, "p1", "submit_plan", review_plan(&h.folder))
        .await;
    assert!(answer.is_err(), "{answer:?}");
    let rename =
        json!({"action": "rename", "agent": other.to_string(), "title": "Mine", "asked": true});
    let answer = h.call(held, other, "m1", "manage_agent", rename).await;
    assert!(answer.is_err(), "{answer:?}");
    assert_ne!(h.agent(other).title, "Mine");
    assert_eq!(h.dispatcher().plan, None);
}

#[tokio::test]
async fn a_safe_change_the_user_asked_for_runs_at_once_and_undo_puts_it_back() {
    let mut h = Harness::with_dispatcher().await;
    let (agent, _) = h.start_claude(None).await;
    let before = h.agent(agent).title;
    let (dispatcher, session) = h
        .dispatch("Call my agent Auth fix, settle after 3 days")
        .await;

    let rename =
        json!({"action": "rename", "agent": agent.to_string(), "title": "Auth fix", "asked": true});
    let answer = h
        .call(session, dispatcher, "a1", "manage_agent", rename)
        .await;
    assert_eq!(answer, Ok(UNDOABLE.into()));
    assert_eq!(h.agent(agent).title, "Auth fix");
    let settle = json!({"action": "settings", "changes": [{"field": "auto_settle_days", "value": 3}], "asked": true});
    let was = h.settings_now().auto_settle_days;
    let answer = h
        .call(session, dispatcher, "a2", "manage_device", settle)
        .await;
    assert_eq!(answer, Ok(UNDOABLE.into()));
    assert_eq!(h.settings_now().auto_settle_days, 3);
    assert_eq!(h.dispatcher().plan, None, "nothing waits");

    // Each shows as a card under the answer, with an Undo clients list.
    let cards = h.action_cards(dispatcher).await;
    let [renamed, settled] = &cards[..] else {
        panic!("not two cards: {cards:?}");
    };
    assert_eq!(
        (renamed.device, &renamed.action, renamed.target.as_str()),
        (
            h.host.id,
            &AdminAction::RenameAgent {
                agent,
                title: "Auth fix".into()
            },
            before.as_str()
        )
    );
    let (renamed, settled) = (
        renamed.undo.expect("an undo"),
        settled.undo.expect("an undo"),
    );
    assert_eq!(h.dispatcher().undo, vec![renamed, settled]);

    // Undo puts each back, once.
    assert_eq!(h.undo(renamed).await, ActionOutcome::Done);
    assert_eq!(h.agent(agent).title, before);
    assert_eq!(h.undo(settled).await, ActionOutcome::Done);
    assert_eq!(h.settings_now().auto_settle_days, was);
    assert_eq!(h.dispatcher().undo, Vec::new());
    assert_eq!(
        reason(&h.undo(renamed).await),
        "That change can't be undone any more."
    );
}

#[tokio::test]
async fn undo_refuses_once_something_else_changed_it() {
    let mut h = Harness::with_dispatcher().await;
    let (agent, _) = h.start_claude(None).await;
    let (dispatcher, session) = h.dispatch("Call my agent Auth fix").await;
    let rename =
        json!({"action": "rename", "agent": agent.to_string(), "title": "Auth fix", "asked": true});
    h.call(session, dispatcher, "a1", "manage_agent", rename)
        .await
        .expect("renamed");
    let [undo] = h.dispatcher().undo[..] else {
        panic!("no undo");
    };

    let title = "Mine".to_owned();
    let outcome = h
        .act(|key| ClientMessage::RenameAgent { key, agent, title })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    assert_eq!(
        reason(&h.undo(undo).await),
        "It changed since, so it can't be undone."
    );
    assert_eq!(h.agent(agent).title, "Mine");
    assert_eq!(h.dispatcher().undo, Vec::new());
}

#[tokio::test]
async fn a_risky_change_or_one_the_user_did_not_ask_for_waits_in_the_plan() {
    let mut h = Harness::with_dispatcher().await;
    let (agent, _) = h.start_claude(None).await;
    let before = h.agent(agent).title;
    let (dispatcher, session) = h.dispatch("Clean up my agents").await;

    // A safe change it only suggests waits for the user.
    let rename =
        json!({"action": "rename", "agent": agent.to_string(), "title": "Old", "asked": false});
    let answer = h
        .call(session, dispatcher, "a1", "manage_agent", rename)
        .await;
    assert_eq!(answer, Ok(ASKS_FIRST.into()));
    assert_eq!(h.agent(agent).title, before);
    let first = h.dispatcher().plan.expect("the plan");
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].reason, "The dispatcher suggests it.");

    // Deleting asks first even when the user asked for it, in the same
    // plan, which a new number shows changed.
    let delete = json!({"action": "delete", "agent": agent.to_string(), "asked": true});
    let answer = h
        .call(session, dispatcher, "a2", "manage_agent", delete)
        .await;
    assert_eq!(answer, Ok(ASKS_FIRST.into()));
    let plan = h.dispatcher().plan.expect("the plan");
    assert!(plan.id > first.id);
    assert_eq!(plan.items.len(), 2);
    assert_eq!(plan.items[1].reason, "It asks first.");
    assert_eq!(
        plan.items[1].step,
        PlanStep::Admin {
            device: h.host.id,
            action: AdminAction::DeleteAgent { agent }
        }
    );
    assert!(h.agents_now().contains(&agent));
    assert_eq!(h.dispatcher().undo, Vec::new());
    let stale = h
        .act(|key| ClientMessage::ConfirmPlan {
            key,
            plan: first.id,
        })
        .await;
    assert_eq!(reason(&stale), "That plan isn't the one shown any more.");

    // Confirmed, each runs.
    let outcome = h
        .act(|key| ClientMessage::ConfirmPlan { key, plan: plan.id })
        .await;
    assert_eq!(
        outcome,
        ActionOutcome::PlanRan {
            results: vec![PlanOutcome::Done, PlanOutcome::Done]
        }
    );
    eventually("the agent is gone", || !h.agents_now().contains(&agent)).await;
}

#[tokio::test]
async fn the_dispatcher_never_changes_sign_ins_the_network_or_its_own_switch() {
    let mut h = Harness::with_dispatcher().await;
    let (dispatcher, session) = h.dispatch("Let yourself ask less").await;
    for (key, field) in [
        ("n1", "dispatcher_acts"),
        ("n2", "cursor_usage"),
        ("n3", "claude_setup_token"),
    ] {
        let change = json!({"action": "settings", "changes": [{"field": field, "value": true}], "asked": true});
        let answer = h
            .call(session, dispatcher, key, "manage_device", change)
            .await;
        assert_eq!(answer, Err(NEVER.into()), "{field}");
    }
    // Not with a safe change beside it either.
    let change = json!({"action": "settings", "changes": [
        {"field": "auto_settle_days", "value": 3},
        {"field": "dispatcher_acts", "value": false},
    ], "asked": true});
    let answer = h
        .call(session, dispatcher, "n4", "manage_device", change)
        .await;
    assert_eq!(answer, Err(NEVER.into()));
    let settings = h.settings_now();
    assert!(settings.dispatcher_acts);
    assert_ne!(settings.auto_settle_days, 3);
    assert_eq!(h.dispatcher().plan, None);
}

#[tokio::test]
async fn with_act_directly_off_every_change_waits_in_the_plan() {
    let mut h = Harness::with_dispatcher().await;
    let (agent, _) = h.start_claude(None).await;
    let before = h.agent(agent).title;
    let device = h.host.id;
    let outcome = h
        .act(|key| ClientMessage::UpdateDeviceSettings {
            key,
            device,
            changes: vec![DeviceSettingChange::DispatcherActs { value: false }],
        })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    let (dispatcher, session) = h.dispatch("Call my agent Auth fix and start another").await;

    let rename =
        json!({"action": "rename", "agent": agent.to_string(), "title": "Auth fix", "asked": true});
    let answer = h
        .call(session, dispatcher, "a1", "manage_agent", rename)
        .await;
    assert_eq!(answer, Ok(ASKS_FIRST.into()));
    let start = json!({"request_id": "a2", "task": "Fix the login page", "vendor": "claude",
        "folder": h.folder.display().to_string(), "asked": true});
    let answer = h
        .call(session, dispatcher, "a2", "start_agent", start)
        .await;
    assert_eq!(answer, Ok(ASKS_FIRST.into()));

    let plan = h.dispatcher().plan.expect("the plan");
    assert_eq!(plan.items.len(), 2);
    for item in &plan.items {
        assert_eq!(
            item.reason,
            "Act directly is off in the dispatcher's settings."
        );
    }
    assert!(matches!(plan.items[1].step, PlanStep::StartAgent { .. }));
    assert_eq!(h.agent(agent).title, before);
    assert_eq!(h.agents_now().len(), 2, "nothing started");
    assert_eq!(h.dispatcher().undo, Vec::new());
}

#[tokio::test]
async fn a_local_models_change_keeps_the_fields_it_leaves_out() {
    let mut h = Harness::with_dispatcher().await;
    let device = h.host.id;
    let off = LocalModelSettings {
        linked_devices: false,
        ..Default::default()
    };
    let changes = vec![DeviceSettingChange::LocalModels {
        value: Box::new(off.clone()),
    }];
    let outcome = h
        .act(|key| ClientMessage::UpdateDeviceSettings {
            key,
            device,
            changes,
        })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    let (dispatcher, session) = h.dispatch("Give local models a 32k context").await;

    let change = json!({"action": "settings",
        "changes": [{"field": "local_models", "value": {"context": 32768}}], "asked": true});
    let answer = h
        .call(session, dispatcher, "l1", "manage_device", change)
        .await;
    assert_eq!(answer, Ok(ASKS_FIRST.into()));

    // The plan shows what the change leaves, linked devices still off, and
    // once confirmed it leaves just that.
    let after = LocalModelSettings {
        context: 32_768,
        ..off
    };
    let plan = h.dispatcher().plan.expect("the plan");
    assert_eq!(
        plan.items[0].step,
        PlanStep::Admin {
            device,
            action: AdminAction::ChangeSettings {
                changes: vec![DeviceSettingChange::LocalModels {
                    value: Box::new(after.clone()),
                }],
            },
        }
    );
    let outcome = h
        .act(|key| ClientMessage::ConfirmPlan { key, plan: plan.id })
        .await;
    assert_eq!(
        outcome,
        ActionOutcome::PlanRan {
            results: vec![PlanOutcome::Done]
        }
    );
    assert_eq!(*h.settings_now().local_models, after);
}

#[tokio::test]
async fn a_change_on_another_device_goes_there_as_a_peer_call() {
    let mut mac = Harness::with_dispatcher().await;
    let mut devbox = Harness::on(devbox());
    mac.fakes.peers.answer_with(&devbox.sessions, mac.host.id);
    devbox.fakes.peers.set_home(mac.host.id);
    let book = PeerBook {
        ssh: true,
        peers: vec![PeerEntry {
            device: devbox.host.id,
            name: "devbox".into(),
            routes: Vec::new(),
            host_key: String::new(),
            test_kinds: Vec::new(),
            home: false,
            model_server: None,
            models_from: Vec::new(),
        }],
    };
    mac.db.set_peer_book(&book).expect("the book is kept");
    let (agent, _) = devbox.start_claude(None).await;
    let before = devbox.agent(agent).title;
    let (dispatcher, session) = mac.dispatch("Call devbox's agent Auth fix").await;

    let rename = json!({"action": "rename", "agent": agent.to_string(), "title": "Auth fix",
        "device": devbox.host.id.to_string(), "asked": true});
    let answer = mac
        .call(session, dispatcher, "a1", "manage_agent", rename.clone())
        .await;
    assert_eq!(answer, Ok(UNDOABLE.into()));
    assert_eq!(devbox.agent(agent).title, "Auth fix");
    let calls = mac.fakes.peers.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        (calls[0].0, calls[0].1.call.tool.as_str()),
        (devbox.host.id, "dispatcher_act")
    );

    // Its card names the agent as devbox had it, and Undo goes there too.
    let cards = mac.action_cards(dispatcher).await;
    let [card] = &cards[..] else {
        panic!("not one card: {cards:?}");
    };
    assert_eq!(
        (card.device, card.target.as_str()),
        (devbox.host.id, before.as_str())
    );
    let undo = card.undo.expect("an undo");
    assert_eq!(mac.undo(undo).await, ActionOutcome::Done);
    assert_eq!(devbox.agent(agent).title, before);
    assert_eq!(mac.fakes.peers.calls().len(), 2);

    // devbox's local model settings aren't known here, so a change there
    // that leaves some out stays here.
    let models = json!({"action": "settings",
        "changes": [{"field": "local_models", "value": {"context": 32768}}],
        "device": devbox.host.id.to_string(), "asked": true});
    let answer = mac
        .call(session, dispatcher, "m1", "manage_device", models)
        .await;
    assert!(
        matches!(&answer, Err(refused) if refused.contains("every field")),
        "{answer:?}"
    );
    assert_eq!(mac.fakes.peers.calls().len(), 2);
    assert_eq!(mac.dispatcher().plan, None);

    // devbox takes changes only from its home.
    devbox.fakes.peers.set_home(DeviceId::derive("elsewhere"));
    let answer = mac
        .call(session, dispatcher, "a2", "manage_agent", rename)
        .await;
    assert!(answer.is_err(), "{answer:?}");
    assert_eq!(devbox.agent(agent).title, before);
}

#[tokio::test]
async fn the_dispatcher_reads_projects_accounts_and_pr_authors() {
    let mut h = Harness::with_dispatcher().await;
    let cursor = h.cursor_account(LoginKind::ConfigDir);
    let (project, workspace) = h.transit();
    let pulls = Arc::new(FakePulls::default());
    let open = |number: u64, login: &str| Pull {
        number,
        title: format!("Change {number}"),
        state: "open".into(),
        user: Some(User {
            login: login.into(),
            ..User::default()
        }),
        html_url: format!("https://github.com/owner/repo/pull/{number}"),
        ..Pull::default()
    };
    let repo = RepoName::parse("owner/repo").unwrap();
    pulls.state().pulls.insert(
        repo.key(),
        vec![open(1, "octocat"), open(2, "hubot"), open(3, "octocat")],
    );
    h.sessions.watch_pulls(pulls.clone());
    let (dispatcher, session) = h.dispatch("Who opens PRs?").await;

    let targets = h
        .call(session, dispatcher, "t1", "list_targets", json!({}))
        .await
        .expect("the targets");
    let targets: Value = serde_json::from_str(&targets).unwrap();
    let vendors: Vec<&str> = targets["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|account| account["vendor"].as_str())
        .collect();
    assert!(vendors.contains(&"cursor"), "{vendors:?}");
    assert!(targets["accounts"]
        .to_string()
        .contains(&cursor.to_string()));
    let projects = targets["projects"].as_array().unwrap();
    let listed = projects
        .iter()
        .find(|listed| listed["project"] == json!(project))
        .expect("the project is listed");
    assert_eq!(listed["repo"], "https://github.com/owner/repo.git");
    assert_eq!(listed["workspaces"][0]["workspace"], json!(workspace));
    assert_eq!(
        listed["workspaces"][0]["location"]["path"],
        h.folder.display().to_string()
    );
    // Its own folder isn't somewhere to plan for.
    let home = h.dispatcher().workspace;
    assert!(!targets.to_string().contains(&home.to_string()));

    let read = h
        .call(
            session,
            dispatcher,
            "r1",
            "list_repo_pull_requests",
            json!({"project": project.to_string()}),
        )
        .await
        .expect("the PRs");
    let read: Value = serde_json::from_str(&read).unwrap();
    assert_eq!(read["authors"], json!(["hubot", "octocat"]));
    assert_eq!(read["pull_requests"][0]["author"], "octocat");
}

#[tokio::test]
async fn a_confirmed_plan_starts_an_idle_cursor_agent_and_a_task_aimed_at_it() {
    let mut h = Harness::with_dispatcher().await;
    let cursor = h.cursor_account(LoginKind::ConfigDir);
    let (dispatcher, session) = h.dispatch("Review octocat's PRs on owner/repo").await;

    let answer = h
        .call(
            session,
            dispatcher,
            "p1",
            "submit_plan",
            review_plan(&h.folder),
        )
        .await;
    assert_eq!(
        answer,
        Ok("The plan shows under the bar. Nothing in it runs until the user confirms it.".into())
    );

    // The plan reaches clients, and nothing runs yet.
    let plan = h.dispatcher().plan.expect("the plan is shown");
    assert_eq!(plan.items.len(), 2);
    let PlanStep::StartAgent { spec, .. } = &plan.items[0].step else {
        panic!("a start first");
    };
    assert_eq!((spec.vendor, spec.account), (Vendor::Cursor, cursor));
    assert_eq!(spec.first, None);
    let PlanStep::ScheduleTask {
        target, trigger, ..
    } = &plan.items[1].step
    else {
        panic!("a task second");
    };
    assert_eq!(target, &PlanTarget::Item { item: 0 });
    let Trigger::GithubEvents { repo, rules, .. } = &**trigger else {
        panic!("a GitHub events trigger");
    };
    assert_eq!(
        (repo.as_str(), rules.coalesce_secs, rules.coalesce_restart),
        ("owner/repo", Some(900), true)
    );
    assert_eq!(h.agents_now().len(), 1);
    assert!(h.published().tasks.is_empty());

    let outcome = h
        .act(|key| ClientMessage::ConfirmPlan { key, plan: plan.id })
        .await;
    let ActionOutcome::PlanRan { results } = outcome else {
        panic!("the plan didn't run: {outcome:?}");
    };
    let [PlanOutcome::Started { agent }, PlanOutcome::TaskSaved { task }] = results[..] else {
        panic!("not a start and a task: {results:?}");
    };

    // The agent is a Cursor agent in the folder, and nothing was sent.
    let started = h.agent(agent);
    assert_eq!(started.vendor, Vendor::Cursor);
    assert_eq!(started.workspace, h.hub.locate_folder(&h.folder));
    let held = h.holders.last();
    assert!(h.sent(held).is_empty(), "{:?}", h.sent(held));

    // The task waits on octocat's PR events, and reaches that agent.
    let saved = h.published().tasks;
    let saved = saved.iter().find(|saved| saved.id == task).expect("saved");
    assert_eq!(
        saved.target,
        TaskTarget::Agent {
            device: h.host.id,
            agent,
            fresh: false
        }
    );
    let Trigger::GithubEvents {
        repo,
        events,
        rules,
    } = &saved.trigger
    else {
        panic!("a GitHub events trigger");
    };
    assert_eq!(
        (repo.as_str(), events.as_slice()),
        ("owner/repo", &["pull_request".to_owned()][..])
    );
    assert_eq!(rules.filters[0].path, "body.pull_request.user.login");
    assert_eq!(
        rules.filters[0].test,
        FilterTest::Equals {
            value: "octocat".into()
        }
    );
    assert_eq!(
        (rules.coalesce_secs, rules.coalesce_restart),
        (Some(900), true)
    );
    // Like a task an agent makes, it lets no free text into the prompt.
    assert_eq!(rules.fields, Some(Vec::new()));
    assert_eq!(h.dispatcher().plan, None);

    // The plan ran once: a second confirm of it is refused.
    let again = h
        .act(|key| ClientMessage::ConfirmPlan { key, plan: plan.id })
        .await;
    assert_eq!(reason(&again), "That plan isn't the one shown any more.");
}

#[tokio::test]
async fn the_pills_in_a_request_reach_the_first_message_the_plan_writes() {
    let mut h = Harness::with_dispatcher().await;
    let cursor = h.cursor_account(LoginKind::ConfigDir);
    let account = h.claude;
    let pill = InlineRef::Agent {
        agent: AgentId::derive("cache"),
        title: "cache fix".into(),
        vendor: Vendor::Claude,
        device: h.host.id,
        excerpt: None,
    };
    let refs = vec![pill.clone()];
    let dispatcher = started(
        h.act(|key| ClientMessage::AskDispatcher {
            key,
            text: "Finish \u{FFFC} in a new agent".into(),
            account,
            new: false,
            refs,
        })
        .await,
    );
    let session = h.holders.last();
    eventually("the request", || {
        h.sent(session) == ["Finish \u{FFFC} in a new agent".to_owned()]
    })
    .await;

    let plan = json!({"items": [
        {"type": "start_agent", "reason": "Finishes the cache fix", "vendor": "cursor",
         "folder": h.folder.display().to_string(), "title": "Cache",
         "first_message": "Finish what [Agent: cache fix] started, as in #12"},
    ]});
    h.call(session, dispatcher, "p1", "submit_plan", plan)
        .await
        .expect("the plan is shown");
    let plan = h.dispatcher().plan.expect("the plan");
    let PlanStep::StartAgent { spec, .. } = &plan.items[0].step else {
        panic!("a start");
    };
    assert_eq!(spec.account, cursor);
    // The marker is the pill again. #12 names nothing the request had.
    assert_eq!(
        spec.first,
        Some(UserInput {
            text: "Finish what \u{FFFC} started, as in #12".into(),
            attachments: Vec::new(),
            refs: vec![pill],
        })
    );

    // So does a pill you answer its question with.
    let refs = spec.first.clone().expect("a first message").refs;
    let release = InlineRef::Agent {
        agent: AgentId::derive("release"),
        title: "release".into(),
        vendor: Vendor::Claude,
        device: h.host.id,
        excerpt: None,
    };
    let asked = h.prompt(dispatcher, "Which agent?");
    h.ask(session, &asked).await;
    let key = IdempotencyKey::generate();
    h.phone.send(ClientMessage::Answer {
        idempotency_key: key,
        event: asked.id,
        answer: AttentionAnswer::Text {
            text: "Like \u{FFFC}".into(),
        },
        refs: vec![release.clone()],
    });
    assert_eq!(h.phone.answered(key).await, AnswerOutcome::Accepted);
    let plan = json!({"items": [
        {"type": "start_agent", "reason": "Ships it", "vendor": "cursor",
         "folder": h.folder.display().to_string(), "title": "Ship",
         "first_message": "Ship it like [Agent: release]"},
    ]});
    h.call(session, dispatcher, "p2", "submit_plan", plan)
        .await
        .expect("the plan is shown");
    let plan = h.dispatcher().plan.expect("the plan");
    let PlanStep::StartAgent { spec, .. } = &plan.items[0].step else {
        panic!("a start");
    };
    assert_eq!(
        spec.first.clone().map(|first| first.refs),
        Some(vec![release])
    );

    // A pill the request has can't go with text that doesn't fit it.
    let wrong = h
        .act(|key| ClientMessage::AskDispatcher {
            key,
            text: "No pill here".into(),
            account,
            new: false,
            refs,
        })
        .await;
    assert_eq!(
        reason(&wrong),
        "the pills in this message don't match its text"
    );
}

#[tokio::test]
async fn a_failed_item_stops_the_rest_and_says_why() {
    let mut h = Harness::with_dispatcher().await;
    h.cursor_account(LoginKind::ConfigDir);
    let (dispatcher, session) = h.dispatch("Review octocat's PRs").await;
    let gone = h.dir.path().join("gone");
    h.call(session, dispatcher, "p1", "submit_plan", review_plan(&gone))
        .await
        .expect("the plan is shown");
    let plan = h.dispatcher().plan.expect("the plan").id;

    let outcome = h.act(|key| ClientMessage::ConfirmPlan { key, plan }).await;
    assert_eq!(
        outcome,
        ActionOutcome::PlanRan {
            results: vec![PlanOutcome::Failed {
                reason: "that folder doesn't exist".into()
            }]
        }
    );
    assert!(h.published().tasks.is_empty());
}

#[tokio::test]
async fn only_the_app_confirms_a_plan_and_dismiss_drops_it() {
    let mut h = Harness::with_dispatcher().await;
    h.cursor_account(LoginKind::ConfigDir);
    let (dispatcher, session) = h.dispatch("Review octocat's PRs").await;
    h.call(
        session,
        dispatcher,
        "p1",
        "submit_plan",
        review_plan(&h.folder),
    )
    .await
    .expect("the plan is shown");
    let plan = h.dispatcher().plan.expect("the plan").id;

    // A connection signed in as the dispatcher can't confirm it.
    let given = Permission {
        id: PermissionId::derive("dispatcher"),
        holder: Actor::Agent { agent: dispatcher },
        home: h.host.id,
        devices: vec![h.host.id],
        vendors: vec![Vendor::Claude, Vendor::Codex, Vendor::Cursor],
        accounts: None,
        places: None,
        depth: DEPTH,
        limits: Limits::default(),
        test_kinds: Vec::new(),
        leave_container: LeaveContainer::Ask,
    };
    let minted = h
        .act(|key| ClientMessage::MintKey {
            key,
            name: "dispatcher".into(),
            permission: Box::new(given),
        })
        .await;
    let ActionOutcome::KeyMinted { path, .. } = minted else {
        panic!("no key: {minted:?}");
    };
    let secret = crate::agents::read_key(Path::new(&path)).expect("the key file");
    let mut signed = Inbox::new(&h.sessions, ClientId::derive("mcp"));
    let key = IdempotencyKey::generate();
    signed.send(ClientMessage::AuthKey { key, secret });
    assert_eq!(signed.result(key).await, ActionOutcome::Done);
    let key = IdempotencyKey::generate();
    signed.send(ClientMessage::ConfirmPlan { key, plan });
    assert_eq!(
        reason(&signed.result(key).await),
        "Only you can confirm a plan, in MaxHQ."
    );
    assert_eq!(h.dispatcher().plan.map(|shown| shown.id), Some(plan));
    assert_eq!(h.agents_now().len(), 1);

    // Dismissed, it's gone, and can't be confirmed after.
    let outcome = h.act(|key| ClientMessage::DismissPlan { key, plan }).await;
    assert_eq!(outcome, ActionOutcome::Done);
    assert_eq!(h.dispatcher().plan, None);
    let outcome = h.act(|key| ClientMessage::ConfirmPlan { key, plan }).await;
    assert_eq!(reason(&outcome), "That plan isn't the one shown any more.");
    assert_eq!(h.agents_now().len(), 1);
}

#[tokio::test]
async fn a_new_conversation_stops_the_last_and_drops_its_plan() {
    let mut h = Harness::with_dispatcher().await;
    h.cursor_account(LoginKind::ConfigDir);
    let (first, session) = h.dispatch("Review octocat's PRs").await;
    h.call(session, first, "p1", "submit_plan", review_plan(&h.folder))
        .await
        .expect("the plan is shown");

    // A second request goes to the same conversation.
    let account = h.claude;
    let outcome = h
        .act(|key| ClientMessage::AskDispatcher {
            key,
            text: "And hubot's too".into(),
            account,
            new: false,
            refs: Vec::new(),
        })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    eventually("the second request", || {
        h.sent(session).contains(&"And hubot's too".to_owned())
    })
    .await;

    let outcome = h
        .act(|key| ClientMessage::AskDispatcher {
            key,
            text: "Start over".into(),
            account,
            new: true,
            refs: Vec::new(),
        })
        .await;
    let second = started(outcome);
    assert_ne!(second, first);
    let shown = h.dispatcher();
    assert_eq!((shown.agent, shown.plan), (Some(second), None));
}

#[tokio::test]
async fn the_dispatcher_stays_on_claude_or_codex() {
    let mut h = Harness::with_dispatcher().await;
    let (agent, _) = h.dispatch("what's running?").await;
    let cursor = h.cursor_account(LoginKind::ConfigDir);

    let outcome = h
        .act(|key| ClientMessage::SetModel {
            key,
            agent,
            model: String::new(),
            account: Some(cursor),
        })
        .await;

    assert!(reason(&outcome).contains("Claude or Codex"), "{outcome:?}");
    assert_eq!(h.agent(agent).vendor, Vendor::Claude);
}
