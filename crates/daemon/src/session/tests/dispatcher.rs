// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The dispatcher: the tools it gets and the ones it's refused, the plan
//! it submits, and the confirm only a client sends.

use maxhq_core::tools::Tool;
use maxhq_protocol::agents::{Actor, LeaveContainer, Limits, Permission, DEPTH};
use maxhq_protocol::dispatch::{Dispatcher, PlanOutcome, PlanStep, PlanTarget};
use maxhq_protocol::ids::{PermissionId, ProjectId, WorkspaceId};
use maxhq_protocol::model::Project;
use maxhq_protocol::tasks::{FilterTest, TaskTarget, Trigger};
use serde_json::{json, Value};

use super::*;
use crate::pulls::source::fake::FakePulls;
use crate::pulls::source::{Pull, User};
use crate::secrets::RepoName;

/// What the dispatcher answers a tool it doesn't get.
const ONLY_READS: &str = "The dispatcher only reads and proposes plans.";

impl Harness {
    /// A harness whose manager has a dispatcher, as on the Mac.
    async fn with_dispatcher() -> Self {
        let mut h = Harness::new();
        h.fakes.dispatcher = true;
        h.restart().await;
        h
    }

    /// Asks the dispatcher `text` as the Claude account. Gives its agent
    /// and holder once the request reached the CLI.
    async fn dispatch(&mut self, text: &str) -> (AgentId, SessionId) {
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
    fn dispatcher(&self) -> Dispatcher {
        match self.hub.get(&EntityKey::Dispatcher(self.host.id)) {
            Some(Entity::Dispatcher(dispatcher)) => dispatcher,
            other => panic!("the hub has no dispatcher: {other:?}"),
        }
    }

    /// The agents clients see.
    fn agents_now(&self) -> Vec<AgentId> {
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
async fn the_dispatcher_gets_only_tools_that_read_or_propose() {
    let mut h = Harness::with_dispatcher().await;
    let (dispatcher, session) = h.dispatch("What runs now?").await;
    assert_eq!(h.dispatcher().agent, Some(dispatcher));

    // Its CLI is served the dispatcher's tools, none of which acts.
    let place = h.vendors.claude_places().pop().expect("a Claude session");
    assert!(place.dispatcher);
    assert_eq!(place.tools(), &Tool::DISPATCHER);
    for tool in [
        Tool::StartAgent,
        Tool::SendAgent,
        Tool::InterruptAgent,
        Tool::ScheduleTask,
        Tool::DeleteTask,
        Tool::RunTaskNow,
        Tool::LeaseOpen,
    ] {
        assert!(!place.tools().contains(&tool), "{tool:?}");
    }

    // A call to one anyway is refused where it's called, with no
    // permission asked for.
    let folder = h.folder.display().to_string();
    for (key, tool, arguments) in [
        (
            "s1",
            "start_agent",
            json!({"request_id": "s1", "task": "t", "vendor": "claude", "folder": folder}),
        ),
        (
            "s2",
            "send_agent",
            json!({"request_id": "s2", "agent": dispatcher.to_string(), "text": "hi"}),
        ),
        (
            "s3",
            "schedule_task",
            json!({"request_id": "s3", "prompt": "p", "schedule": {"type": "interval", "everyMs": 3600000}}),
        ),
        ("s4", "send_attachments", json!({"paths": ["a.txt"]})),
    ] {
        let answer = h.call(session, dispatcher, key, tool, arguments).await;
        assert_eq!(answer, Err(ONLY_READS.into()), "{tool}");
    }
    assert!(h.agents_now().len() == 1, "nothing started");

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
    assert_eq!(h.dispatcher().plan, None);
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
            agent
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
